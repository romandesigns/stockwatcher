//! The news stream's runner: the socket seam, the bounded inbound queue, and one pump the
//! collector's own thread calls.
//!
//! # Why a pump rather than a loop
//!
//! The stream and the REST searches must share **one** owner of version state, or two dedup maps
//! race each other and the same version is written twice. The owner is the collector, and the
//! collector lives on the news worker's thread. So the runner does not own a loop of its own and
//! does not hold the collector behind a lock — putting provider I/O behind a shared lock is how
//! the two halves become coupled again. Instead the worker calls [`Runner::pump`] on its own
//! cadence, gets whatever arrived, and hands it to the collector directly.
//!
//! # The seam
//!
//! [`Dial`] opens a [`Transport`]. Production uses a WebSocket ([`super::socket`]); the tests
//! inject a fake that replays a script. Nothing here needs a network, a credential or a provider,
//! which is the only way this could be tested at all under the agreed boundaries.

use super::protocol::{Inbound, Refusal, parse_frame};
use super::session::{NewsStreamConfig, NewsStreamStats, Phase, Session, Step};
use crate::import::alpaca::AlpacaCredentials;
use crate::import::alpaca_news::RawStory;
use std::time::{Duration, Instant};

/// What a read produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Recv {
    /// A text frame.
    Frame(String),
    /// Nothing within the time allowed. The socket is still open.
    Idle,
    /// The socket is gone, with why.
    Closed(String),
}

/// One open socket, as the runner needs it.
pub trait Transport {
    /// Sends one frame, giving up after `timeout`. Bounded because a half-open socket accepts a
    /// write and never completes it, and the news worker's own cadence must not hang on that.
    fn send(&mut self, text: &str, timeout: Duration) -> Result<(), String>;
    /// The next frame, waiting at most `timeout`.
    fn recv(&mut self, timeout: Duration) -> Recv;
}

/// Opens a socket, giving up after `timeout`.
pub trait Dial {
    fn dial(&mut self, url: &str, timeout: Duration) -> Result<Box<dyn Transport>, String>;
}

/// The optional news stream, as the news worker drives it.
pub struct Runner {
    cfg: NewsStreamConfig,
    creds: AlpacaCredentials,
    session: Session,
    open: Option<Box<dyn Transport>>,
    /// When the next dial is due, after a backoff.
    retry_at: Option<Instant>,
}

impl Runner {
    /// `None` when the stream is not enabled, which is the default. A disabled stream is not a
    /// runner that does nothing: there is nothing to call.
    pub fn new(cfg: NewsStreamConfig, creds: AlpacaCredentials) -> Option<Runner> {
        if !cfg.enabled {
            return None;
        }
        Some(Runner {
            cfg,
            creds,
            session: Session::new(),
            open: None,
            retry_at: None,
        })
    }

    pub fn stats(&self) -> NewsStreamStats {
        self.session.stats()
    }

    pub fn phase(&self) -> Phase {
        self.session.phase()
    }

    /// Whether the runner has given up: a refusal retrying cannot fix. The REST searches carry
    /// the whole load then, as they do whenever the stream is off.
    pub fn stopped(&self) -> bool {
        !self.session.will_retry()
    }

    pub fn line(&self) -> String {
        self.session.line()
    }

    /// Connects if due, reads what is available for at most `budget`, and returns the stories to
    /// store, each stamped with the time it was observed.
    ///
    /// Bounded, and the bound is stated honestly: **reading** takes at most `budget`, and a pump
    /// that also has to dial costs at most [`NewsStreamConfig::dial_timeout`] on top of it. A
    /// dial happens at most once per backoff period, so over time the cost is the reading budget.
    /// Stories are bounded too, at [`NewsStreamConfig::queue_capacity`] per pump, past which the
    /// rest are dropped and counted — a stall must not grow memory, and the repair search covers
    /// what was dropped.
    pub fn pump(
        &mut self,
        dial: &mut dyn Dial,
        now: &mut dyn FnMut() -> i64,
        budget: Duration,
        log: &mut dyn FnMut(&str),
    ) -> Vec<(RawStory, i64)> {
        let mut out: Vec<(RawStory, i64)> = Vec::new();
        if self.stopped() {
            return out;
        }
        let deadline = Instant::now() + budget;
        if self.open.is_none() {
            if self.retry_at.is_some_and(|t| Instant::now() < t) {
                return out;
            }
            match dial.dial(&self.cfg.url, self.cfg.dial_timeout) {
                Ok(t) => {
                    self.open = Some(t);
                    self.retry_at = None;
                    let step = self.session.connected();
                    if !self.act(step, &mut out, now, log) {
                        return out;
                    }
                }
                Err(e) => {
                    // A dial failure is transient by nature: nothing was negotiated.
                    self.session.disconnected();
                    let wait = self.session.backoff();
                    self.retry_at = Some(Instant::now() + wait);
                    log(&format!(
                        "news stream: cannot connect ({e}); next attempt in {}s",
                        wait.as_secs()
                    ));
                    return out;
                }
            }
        }
        while Instant::now() < deadline {
            let left = deadline.saturating_duration_since(Instant::now());
            let Some(t) = self.open.as_mut() else { break };
            match t.recv(left.min(Duration::from_millis(250))) {
                Recv::Idle => break,
                Recv::Closed(why) => {
                    self.open = None;
                    self.session.disconnected();
                    let wait = self.session.backoff();
                    self.retry_at = Some(Instant::now() + wait);
                    log(&format!(
                        "news stream: disconnected ({why}); next attempt in {}s",
                        wait.as_secs()
                    ));
                    break;
                }
                Recv::Frame(text) => {
                    let messages = match parse_frame(&text) {
                        Ok(m) => m,
                        Err(e) => {
                            // The frame itself was not a frame: counted, never fatal.
                            vec![Inbound::Malformed(e)]
                        }
                    };
                    for m in messages {
                        let step = self.session.on(m);
                        if !self.act(step, &mut out, now, log) {
                            return out;
                        }
                    }
                }
            }
        }
        out
    }

    /// Carries out one step. Returns false when the pump should stop.
    fn act(
        &mut self,
        step: Step,
        out: &mut Vec<(RawStory, i64)>,
        now: &mut dyn FnMut() -> i64,
        log: &mut dyn FnMut(&str),
    ) -> bool {
        match step {
            Step::Nothing => true,
            Step::Send(o) => {
                let frame = o.frame(&self.creds, &self.cfg.symbols);
                let Some(t) = self.open.as_mut() else {
                    return false;
                };
                match t.send(&frame, self.cfg.send_timeout) {
                    Ok(()) => true,
                    Err(e) => {
                        // The kind, never the frame: the auth frame carries the credentials.
                        log(&format!("news stream: cannot send {}: {e}", o.describe()));
                        self.open = None;
                        self.session.disconnected();
                        self.retry_at = Some(Instant::now() + self.session.backoff());
                        false
                    }
                }
            }
            Step::Story(s) => {
                if out.len() >= self.cfg.queue_capacity {
                    // The repair search covers the whole freshness horizon, so a dropped delivery
                    // is a delay rather than a loss — but it is counted either way.
                    self.session.dropped();
                    return true;
                }
                out.push((*s, now()));
                true
            }
            Step::Reconnect => {
                self.open = None;
                self.session.disconnected();
                self.retry_at = Some(Instant::now() + self.session.backoff());
                false
            }
            Step::Stop(r) => {
                self.open = None;
                log(&format!(
                    "news stream: {} refused; the stream stays off and the searches carry it",
                    match r {
                        Refusal::Auth => "authentication",
                        Refusal::Entitlement => "the subscription",
                        Refusal::Transient => "the connection",
                    }
                ));
                false
            }
        }
    }
}

#[cfg(test)]
impl Runner {
    fn will_retry_for_tests(&self) -> bool {
        self.session.will_retry()
    }

    fn clear_backoff_for_tests(&mut self) {
        self.retry_at = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A transport that replays a script and records what was sent.
    #[derive(Default)]
    struct Fake {
        frames: Vec<Recv>,
        sent: Arc<Mutex<Vec<String>>>,
        send_fails: bool,
    }

    impl Transport for Fake {
        fn send(&mut self, text: &str, _timeout: Duration) -> Result<(), String> {
            if self.send_fails {
                return Err("socket closed".into());
            }
            self.sent.lock().expect("sent lock").push(text.to_string());
            Ok(())
        }

        fn recv(&mut self, _timeout: Duration) -> Recv {
            if self.frames.is_empty() {
                Recv::Idle
            } else {
                self.frames.remove(0)
            }
        }
    }

    /// A dialer that hands out prepared transports, then refuses.
    struct Fakes {
        ready: Vec<Fake>,
        dials: usize,
        refuse: bool,
    }

    impl Dial for Fakes {
        fn dial(&mut self, _url: &str, _timeout: Duration) -> Result<Box<dyn Transport>, String> {
            self.dials += 1;
            if self.refuse || self.ready.is_empty() {
                return Err("connection refused".into());
            }
            Ok(Box::new(self.ready.remove(0)))
        }
    }

    fn creds() -> AlpacaCredentials {
        AlpacaCredentials {
            key_id: "dummy-key".into(),
            secret: "dummy-secret".into(),
        }
    }

    fn on() -> NewsStreamConfig {
        NewsStreamConfig {
            enabled: true,
            ..NewsStreamConfig::default()
        }
    }

    fn story_frame(id: u64) -> Recv {
        Recv::Frame(format!(
            r#"[{{"T":"n","id":{id},"headline":"ABCD Announces FDA Approval","summary":"",
            "author":"Staff","created_at":"2026-10-15T13:00:00Z",
            "updated_at":"2026-10-15T13:00:00Z","symbols":["ABCD"],"source":"benzinga",
            "url":"u"}}]"#
        ))
    }

    /// Disabled is the default, and a disabled stream is not a runner at all.
    #[test]
    fn a_disabled_stream_has_no_runner() {
        assert!(Runner::new(NewsStreamConfig::default(), creds()).is_none());
        assert!(Runner::new(on(), creds()).is_some());
    }

    /// The whole path with a fake socket: dial, authenticate, subscribe, then stories come out of
    /// the pump ready for the collector's durable append.
    #[test]
    fn the_runner_handshakes_and_yields_stories() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let mut dial = Fakes {
            ready: vec![Fake {
                frames: vec![
                    Recv::Frame(r#"[{"T":"success","msg":"connected"}]"#.into()),
                    Recv::Frame(r#"[{"T":"success","msg":"authenticated"}]"#.into()),
                    Recv::Frame(r#"[{"T":"subscription","news":["*"]}]"#.into()),
                    story_frame(1),
                    story_frame(2),
                ],
                sent: sent.clone(),
                send_fails: false,
            }],
            dials: 0,
            refuse: false,
        };
        let mut r = Runner::new(on(), creds()).expect("enabled");
        let mut log = Vec::new();
        let got = r.pump(
            &mut dial,
            &mut || 1_000,
            Duration::from_millis(500),
            &mut |l| log.push(l.to_string()),
        );
        assert_eq!(dial.dials, 1);
        assert_eq!(r.phase(), Phase::Streaming, "{:?}", r.phase());
        let ids: Vec<u64> = got.iter().map(|(s, _)| s.id).collect();
        assert_eq!(ids, vec![1, 2]);
        assert!(
            got.iter().all(|(_, at)| *at == 1_000),
            "stamped when observed"
        );
        assert_eq!(r.stats().stories, 2);

        // The frames it sent: the auth frame carries the credentials, so nothing logged them.
        let sent = sent.lock().expect("sent lock").clone();
        assert_eq!(sent.len(), 2, "{sent:?}");
        assert!(sent[0].contains("\"action\":\"auth\""));
        assert!(sent[1].contains("\"action\":\"subscribe\""));
        for line in &log {
            assert!(
                !line.contains("dummy-secret") && !line.contains("dummy-key"),
                "no credential reaches a log line: {line}"
            );
        }
    }

    /// A refused subscription stops the runner: there is nothing to retry, and the searches were
    /// already carrying the load.
    #[test]
    fn an_entitlement_refusal_stops_the_runner() {
        let mut dial = Fakes {
            ready: vec![Fake {
                frames: vec![Recv::Frame(
                    r#"[{"T":"error","code":409,"msg":"insufficient subscription"}]"#.into(),
                )],
                sent: Arc::new(Mutex::new(Vec::new())),
                send_fails: false,
            }],
            dials: 0,
            refuse: false,
        };
        let mut r = Runner::new(on(), creds()).expect("enabled");
        let mut log = Vec::new();
        r.pump(&mut dial, &mut || 0, Duration::from_millis(100), &mut |l| {
            log.push(l.to_string())
        });
        assert!(r.stopped());
        assert_eq!(r.stats().entitlement_refusals, 1);

        // And it does not dial again, however often it is pumped.
        let before = dial.dials;
        for _ in 0..5 {
            assert!(
                r.pump(&mut dial, &mut || 0, Duration::from_millis(10), &mut |_| {})
                    .is_empty()
            );
        }
        assert_eq!(dial.dials, before, "no further attempts");
        assert!(log.iter().any(|l| l.contains("subscription")), "{log:?}");
    }

    /// A dial failure backs off instead of spinning, and nothing is claimed about the window.
    #[test]
    fn a_dial_failure_backs_off_and_retries() {
        let mut dial = Fakes {
            ready: Vec::new(),
            dials: 0,
            refuse: true,
        };
        let mut r = Runner::new(on(), creds()).expect("enabled");
        let got = r.pump(&mut dial, &mut || 0, Duration::from_millis(50), &mut |_| {});
        assert!(got.is_empty());
        assert_eq!(dial.dials, 1);
        // The backoff holds the next attempt back rather than dialing on every pump.
        for _ in 0..5 {
            r.pump(&mut dial, &mut || 0, Duration::from_millis(5), &mut |_| {});
        }
        assert_eq!(dial.dials, 1, "the backoff is respected: {}", dial.dials);
        assert!(r.will_retry_for_tests());
    }

    /// A disconnect reconnects on the next pump once the backoff has passed, and the reconnect is
    /// counted.
    #[test]
    fn a_disconnect_is_counted_and_retried() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let fake = |frames: Vec<Recv>| Fake {
            frames,
            sent: sent.clone(),
            send_fails: false,
        };
        let mut dial = Fakes {
            ready: vec![
                fake(vec![
                    Recv::Frame(r#"[{"T":"success","msg":"authenticated"}]"#.into()),
                    Recv::Closed("eof".into()),
                ]),
                fake(vec![Recv::Frame(
                    r#"[{"T":"success","msg":"authenticated"}]"#.into(),
                )]),
            ],
            dials: 0,
            refuse: false,
        };
        let mut r = Runner::new(on(), creds()).expect("enabled");
        let mut log = Vec::new();
        r.pump(&mut dial, &mut || 0, Duration::from_millis(100), &mut |l| {
            log.push(l.to_string())
        });
        assert_eq!(r.stats().connects, 1);
        assert!(log.iter().any(|l| l.contains("disconnected")), "{log:?}");
        // Past the backoff, the next pump dials again.
        r.clear_backoff_for_tests();
        r.pump(
            &mut dial,
            &mut || 0,
            Duration::from_millis(100),
            &mut |_| {},
        );
        assert_eq!(dial.dials, 2);
        assert_eq!(r.stats().reconnects, 1);
    }

    /// A send failure drops the connection rather than pretending the handshake happened.
    #[test]
    fn a_send_failure_drops_the_connection() {
        let mut dial = Fakes {
            ready: vec![Fake {
                frames: Vec::new(),
                sent: Arc::new(Mutex::new(Vec::new())),
                send_fails: true,
            }],
            dials: 0,
            refuse: false,
        };
        let mut r = Runner::new(on(), creds()).expect("enabled");
        let mut log = Vec::new();
        let got = r.pump(&mut dial, &mut || 0, Duration::from_millis(50), &mut |l| {
            log.push(l.to_string())
        });
        assert!(got.is_empty());
        assert_ne!(r.phase(), Phase::Streaming);
        assert!(
            log.iter().any(|l| l.contains("cannot send auth")),
            "the kind, not the frame: {log:?}"
        );
        assert!(
            log.iter().all(|l| !l.contains("dummy-secret")),
            "and never the credential: {log:?}"
        );
    }

    /// More stories than the queue allows in one pump are dropped and counted, never buffered
    /// without limit.
    #[test]
    fn the_pump_is_bounded_and_drops_are_counted() {
        let frames: Vec<Recv> = std::iter::once(Recv::Frame(
            r#"[{"T":"success","msg":"authenticated"}]"#.into(),
        ))
        .chain((0..10u64).map(story_frame))
        .collect();
        let mut dial = Fakes {
            ready: vec![Fake {
                frames,
                sent: Arc::new(Mutex::new(Vec::new())),
                send_fails: false,
            }],
            dials: 0,
            refuse: false,
        };
        let cfg = NewsStreamConfig {
            enabled: true,
            queue_capacity: 4,
            ..NewsStreamConfig::default()
        };
        let mut r = Runner::new(cfg, creds()).expect("enabled");
        let got = r.pump(
            &mut dial,
            &mut || 0,
            Duration::from_millis(200),
            &mut |_| {},
        );
        assert_eq!(got.len(), 4, "bounded per pump");
        assert_eq!(r.stats().dropped, 6, "and the rest are counted");
        assert_eq!(r.stats().stories, 10, "all of them were seen");
    }

    /// A frame that is not a frame is counted and ignored, and its socket survives.
    #[test]
    fn a_bad_frame_is_counted_not_fatal() {
        let mut dial = Fakes {
            ready: vec![Fake {
                frames: vec![
                    Recv::Frame("not json".into()),
                    Recv::Frame(r#"[{"T":"success","msg":"authenticated"}]"#.into()),
                ],
                sent: Arc::new(Mutex::new(Vec::new())),
                send_fails: false,
            }],
            dials: 0,
            refuse: false,
        };
        let mut r = Runner::new(on(), creds()).expect("enabled");
        r.pump(
            &mut dial,
            &mut || 0,
            Duration::from_millis(100),
            &mut |_| {},
        );
        assert_eq!(r.stats().malformed, 1);
        assert_eq!(r.phase(), Phase::Subscribing, "the socket carried on");
    }
}
