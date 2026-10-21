//! The news stream's session: what to send, what to count, and when to stop trying.
//!
//! A pure state machine over [`super::protocol::Inbound`], so the whole of it is testable
//! offline — which is the only way it is tested here, by agreement: no authenticated probe and no
//! live acquisition.
//!
//! # What a stream is for, and what it is not
//!
//! It is *intended* to reduce the delay before an edit is seen — an edit the REST searches would
//! find anyway. No delivery delay has been measured, here or anywhere: these are offline state
//! machine tests and they establish no performance result. It is not
//! coverage: receiving one message says one story changed and says nothing about what else did,
//! so the session never writes a coverage record and never advances the covered-through point.
//! The bounded repair search remains the only thing that can claim a window was read
//! (`crate::import::evidence_collect::news_repair_window`), and it keeps running whether the
//! stream is connected or not.
//!
//! Stories the session accepts go through the same durable path a REST page does — propose,
//! append, commit ([`crate::import::alpaca_news::NewsVersions`]) — so there is one owner of
//! version state rather than two dedup maps racing each other.
//!
//! # Off by default
//!
//! [`NewsStreamConfig::enabled`] is false unless `EVIDENCE_NEWS_STREAM=1`. Nothing about the
//! pillars changes when it is on: the same stories reach the same archive, by the same rules,
//! through the same durable path.

use super::protocol::{Inbound, Refusal, auth_frame, refusal_of, subscribe_frame};
use crate::import::alpaca::AlpacaCredentials;
use crate::import::alpaca_news::RawStory;
use serde::Serialize;
use std::time::Duration;

/// Stories the inbound queue holds before the session starts dropping them (and counting).
pub const QUEUE_CAPACITY: usize = 4_096;

/// Reconnect backoff: doubling from the first to the last, then held.
pub const BACKOFF_FIRST: Duration = Duration::from_secs(1);
pub const BACKOFF_MAX: Duration = Duration::from_secs(60);

/// Whether the news stream runs, and against what.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewsStreamConfig {
    /// `EVIDENCE_NEWS_STREAM=1`. **False** by default: the REST searches are the contract, and a
    /// socket that is off cannot be the reason a window went unsearched.
    pub enabled: bool,
    pub url: String,
    /// Symbols to subscribe to; empty means every symbol (`["*"]`).
    pub symbols: Vec<String>,
    pub queue_capacity: usize,
    /// How long a single connection attempt may take.
    ///
    /// Separate from the pump's reading budget and stated separately, because it is not part of
    /// it: a pump that has to dial costs the dial as well as the read. A dial is attempted at
    /// most once per backoff period, so the cost is bounded over time rather than per call.
    pub dial_timeout: Duration,
    /// How long a single send may take before the connection is given up on.
    pub send_timeout: Duration,
}

impl Default for NewsStreamConfig {
    fn default() -> Self {
        NewsStreamConfig {
            enabled: false,
            url: "wss://stream.data.alpaca.markets/v1beta1/news".into(),
            symbols: Vec::new(),
            queue_capacity: QUEUE_CAPACITY,
            dial_timeout: Duration::from_secs(10),
            send_timeout: Duration::from_secs(10),
        }
    }
}

impl NewsStreamConfig {
    /// Reads the configuration from a lookup (the process environment in production).
    ///
    /// Enabled only by an explicit `1`/`true`: a misspelled value leaves it off, because the
    /// failure that matters is a stream nobody meant to run, not one nobody meant to stop.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> NewsStreamConfig {
        let d = NewsStreamConfig::default();
        let on = get("EVIDENCE_NEWS_STREAM")
            .map(|v| matches!(v.trim(), "1" | "true" | "TRUE" | "yes"))
            .unwrap_or(false);
        NewsStreamConfig {
            enabled: on,
            url: get("ALPACA_NEWS_STREAM_URL").unwrap_or(d.url),
            symbols: get("EVIDENCE_NEWS_STREAM_SYMBOLS")
                .map(|v| {
                    v.split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_uppercase)
                        .collect()
                })
                .unwrap_or(d.symbols),
            queue_capacity: d.queue_capacity,
            dial_timeout: d.dial_timeout,
            send_timeout: d.send_timeout,
        }
    }
}

/// What the session wants sent, as a kind rather than a payload where credentials are involved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outbound {
    /// The authentication frame. Rendered by [`Outbound::frame`], which is the only place the
    /// secret appears; [`Outbound::describe`] is what a log line may use.
    Auth,
    Subscribe,
}

impl Outbound {
    /// The text to send.
    pub fn frame(&self, creds: &AlpacaCredentials, symbols: &[String]) -> String {
        match self {
            Outbound::Auth => auth_frame(&creds.key_id, &creds.secret),
            Outbound::Subscribe => subscribe_frame(symbols),
        }
    }

    /// What a log line says instead of the frame.
    pub fn describe(&self) -> &'static str {
        match self {
            Outbound::Auth => "auth",
            Outbound::Subscribe => "subscribe",
        }
    }
}

/// Where the session is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub enum Phase {
    /// Not connected, and due to try.
    #[default]
    Idle,
    /// Connected, waiting for `authenticated`.
    Authenticating,
    /// Authenticated, waiting for the subscription to be confirmed.
    Subscribing,
    /// Subscribed: stories are arriving.
    Streaming,
    /// Refused in a way retrying cannot fix. The session stays here and the REST searches carry
    /// the whole load, which they were already doing.
    Refused,
}

/// Everything the session has counted. No field can hold a credential.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct NewsStreamStats {
    pub connects: u64,
    pub reconnects: u64,
    /// Stories handed to the collector.
    pub stories: u64,
    /// Stories dropped because the inbound queue was full.
    pub dropped: u64,
    /// Messages that would not parse.
    pub malformed: u64,
    /// Messages of a kind never subscribed to.
    pub ignored: u64,
    /// Server errors, by what retrying can do about them.
    pub auth_refusals: u64,
    pub entitlement_refusals: u64,
    pub transient_errors: u64,
}

/// The session: a state machine and its counters.
#[derive(Debug, Default)]
pub struct Session {
    phase: Phase,
    stats: NewsStreamStats,
    attempts: u32,
}

/// What the caller should do with one inbound message.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// Send this, then carry on.
    Send(Outbound),
    /// Hand this story to the collector's durable path.
    Story(Box<RawStory>),
    /// Nothing to do.
    Nothing,
    /// Drop the connection and try again after the backoff.
    Reconnect,
    /// Stop trying: retrying cannot fix this.
    Stop(Refusal),
}

impl Session {
    pub fn new() -> Session {
        Session::default()
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn stats(&self) -> NewsStreamStats {
        self.stats
    }

    /// A socket has just opened.
    pub fn connected(&mut self) -> Step {
        if self.stats.connects > 0 {
            self.stats.reconnects += 1;
        }
        self.stats.connects += 1;
        self.phase = Phase::Authenticating;
        Step::Send(Outbound::Auth)
    }

    /// The socket closed, for whatever reason.
    pub fn disconnected(&mut self) {
        if self.phase != Phase::Refused {
            self.phase = Phase::Idle;
            self.attempts += 1;
        }
    }

    /// How long to wait before the next attempt. Doubling, capped, and held at the cap.
    pub fn backoff(&self) -> Duration {
        let shift = self.attempts.saturating_sub(1).min(16);
        let d = BACKOFF_FIRST.saturating_mul(1u32 << shift);
        d.min(BACKOFF_MAX)
    }

    /// Whether the session will try again at all.
    pub fn will_retry(&self) -> bool {
        self.phase != Phase::Refused
    }

    /// One inbound message.
    pub fn on(&mut self, m: Inbound) -> Step {
        match m {
            Inbound::Success(msg) if msg == "authenticated" => {
                self.phase = Phase::Subscribing;
                // Authenticated, so the connection is working: the backoff starts over.
                self.attempts = 0;
                Step::Send(Outbound::Subscribe)
            }
            Inbound::Success(_) => Step::Nothing,
            Inbound::Subscribed(news) => {
                // An empty confirmation is the server saying it subscribed us to nothing, which
                // is not a stream however healthy the socket is.
                self.phase = if news.is_empty() {
                    Phase::Subscribing
                } else {
                    Phase::Streaming
                };
                Step::Nothing
            }
            Inbound::Story(s) => {
                self.stats.stories += 1;
                Step::Story(Box::new(s))
            }
            Inbound::Ignored(_) => {
                self.stats.ignored += 1;
                Step::Nothing
            }
            Inbound::Malformed(_) => {
                self.stats.malformed += 1;
                Step::Nothing
            }
            Inbound::Error { code, .. } => match refusal_of(code) {
                Refusal::Auth => {
                    self.stats.auth_refusals += 1;
                    self.phase = Phase::Refused;
                    Step::Stop(Refusal::Auth)
                }
                Refusal::Entitlement => {
                    self.stats.entitlement_refusals += 1;
                    self.phase = Phase::Refused;
                    Step::Stop(Refusal::Entitlement)
                }
                Refusal::Transient => {
                    self.stats.transient_errors += 1;
                    Step::Reconnect
                }
            },
        }
    }

    /// A story the inbound queue had no room for.
    pub fn dropped(&mut self) {
        self.stats.dropped += 1;
    }

    /// One log-friendly line. Never includes a credential, because nothing it reads holds one.
    pub fn line(&self) -> String {
        let s = self.stats;
        format!(
            "news stream {:?}: {} connects ({} reconnects), {} stories, {} dropped, {} malformed, {} ignored, refusals {} auth / {} entitlement, {} transient",
            self.phase,
            s.connects,
            s.reconnects,
            s.stories,
            s.dropped,
            s.malformed,
            s.ignored,
            s.auth_refusals,
            s.entitlement_refusals,
            s.transient_errors
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::news_stream::protocol::parse_frame;

    fn story(id: u64) -> Inbound {
        let frame = format!(
            r#"[{{"T":"n","id":{id},"headline":"h","summary":"","author":"a",
            "created_at":"2026-10-15T13:00:00Z","updated_at":"2026-10-15T13:00:00Z",
            "symbols":["ABCD"],"source":"benzinga","url":"u"}}]"#
        );
        parse_frame(&frame).expect("a frame").remove(0)
    }

    /// The stream is off unless it is explicitly turned on.
    #[test]
    fn the_stream_is_off_by_default() {
        assert!(!NewsStreamConfig::default().enabled);
        assert!(!NewsStreamConfig::from_lookup(|_| None).enabled);
        // A value that is not plainly an affirmative leaves it off: the failure that matters is
        // a stream nobody meant to run.
        for v in ["0", "", "no", "maybe", "On"] {
            assert!(
                !NewsStreamConfig::from_lookup(
                    |k| (k == "EVIDENCE_NEWS_STREAM").then(|| v.to_string())
                )
                .enabled,
                "{v:?} does not enable it"
            );
        }
        for v in ["1", "true", "yes"] {
            assert!(
                NewsStreamConfig::from_lookup(
                    |k| (k == "EVIDENCE_NEWS_STREAM").then(|| v.to_string())
                )
                .enabled,
                "{v:?} does"
            );
        }
    }

    /// The handshake: connect, authenticate, subscribe, stream.
    #[test]
    fn the_handshake_reaches_streaming_and_then_carries_stories() {
        let mut s = Session::new();
        assert_eq!(s.phase(), Phase::Idle);
        assert_eq!(s.connected(), Step::Send(Outbound::Auth));
        assert_eq!(s.phase(), Phase::Authenticating);
        assert_eq!(
            s.on(Inbound::Success("connected".into())),
            Step::Nothing,
            "the greeting is not the authentication"
        );
        assert_eq!(
            s.on(Inbound::Success("authenticated".into())),
            Step::Send(Outbound::Subscribe)
        );
        assert_eq!(s.phase(), Phase::Subscribing);
        assert_eq!(s.on(Inbound::Subscribed(vec!["*".into()])), Step::Nothing);
        assert_eq!(s.phase(), Phase::Streaming);
        match s.on(story(1)) {
            Step::Story(st) => assert_eq!(st.id, 1),
            other => panic!("expected a story, got {other:?}"),
        }
        assert_eq!(s.stats().stories, 1);
        assert_eq!(s.stats().connects, 1);
        assert_eq!(s.stats().reconnects, 0);
    }

    /// A subscription confirmed as empty is not a stream, however healthy the socket is.
    #[test]
    fn an_empty_subscription_is_not_streaming() {
        let mut s = Session::new();
        s.connected();
        s.on(Inbound::Success("authenticated".into()));
        s.on(Inbound::Subscribed(Vec::new()));
        assert_eq!(s.phase(), Phase::Subscribing);
    }

    /// An entitlement refusal stops the session: asking again cannot change the answer, and the
    /// REST searches were already carrying the load.
    #[test]
    fn an_entitlement_refusal_stops_rather_than_retrying_forever() {
        let mut s = Session::new();
        s.connected();
        let step = s.on(Inbound::Error {
            code: Some(409),
            msg: "insufficient subscription".into(),
        });
        assert_eq!(step, Step::Stop(Refusal::Entitlement));
        assert_eq!(s.phase(), Phase::Refused);
        assert!(!s.will_retry());
        s.disconnected();
        assert_eq!(s.phase(), Phase::Refused, "and it stays refused");
        assert_eq!(s.stats().entitlement_refusals, 1);
    }

    /// An auth refusal likewise, and counted apart from an entitlement one: a wrong key and a
    /// missing subscription are different problems with different fixes.
    #[test]
    fn an_auth_refusal_is_counted_apart() {
        let mut s = Session::new();
        s.connected();
        assert_eq!(
            s.on(Inbound::Error {
                code: Some(402),
                msg: "auth failed".into()
            }),
            Step::Stop(Refusal::Auth)
        );
        assert_eq!(s.stats().auth_refusals, 1);
        assert_eq!(s.stats().entitlement_refusals, 0);
    }

    /// A transient error reconnects, with a doubling backoff that is capped and then held.
    #[test]
    fn a_transient_error_reconnects_with_a_capped_backoff() {
        let mut s = Session::new();
        s.connected();
        assert_eq!(
            s.on(Inbound::Error {
                code: Some(500),
                msg: "upstream".into()
            }),
            Step::Reconnect
        );
        assert!(s.will_retry());
        assert_eq!(s.stats().transient_errors, 1);

        let mut seen = Vec::new();
        for _ in 0..10 {
            s.disconnected();
            seen.push(s.backoff());
            s.connected();
        }
        assert_eq!(seen[0], BACKOFF_FIRST);
        assert_eq!(seen[1], BACKOFF_FIRST * 2);
        assert_eq!(
            *seen.last().expect("a backoff"),
            BACKOFF_MAX,
            "capped and held: {seen:?}"
        );
        assert_eq!(s.stats().reconnects, 10, "every reconnect is counted");
    }

    /// Authenticating again resets the backoff: the connection demonstrably works.
    #[test]
    fn a_working_connection_resets_the_backoff() {
        let mut s = Session::new();
        s.connected();
        s.disconnected();
        s.disconnected();
        assert!(s.backoff() > BACKOFF_FIRST);
        s.connected();
        s.on(Inbound::Success("authenticated".into()));
        s.disconnected();
        assert_eq!(s.backoff(), BACKOFF_FIRST);
    }

    /// Malformed and unsubscribed messages are counted and ignored, never fatal.
    #[test]
    fn noise_is_counted_and_ignored() {
        let mut s = Session::new();
        s.connected();
        assert_eq!(s.on(Inbound::Malformed("news: bad".into())), Step::Nothing);
        assert_eq!(s.on(Inbound::Ignored("q".into())), Step::Nothing);
        assert_eq!((s.stats().malformed, s.stats().ignored), (1, 1));
        assert!(s.will_retry());
    }

    /// A full inbound queue is a counted drop, and the log line says so without a credential in
    /// sight.
    #[test]
    fn a_dropped_story_is_counted_and_the_log_line_is_safe() {
        let mut s = Session::new();
        s.connected();
        s.on(Inbound::Success("authenticated".into()));
        s.on(Inbound::Subscribed(vec!["*".into()]));
        s.dropped();
        s.dropped();
        assert_eq!(s.stats().dropped, 2);
        let line = s.line();
        assert!(line.contains("2 dropped"), "{line}");
        for secret in ["key", "secret", "APCA", "Bearer"] {
            assert!(
                !line.contains(secret),
                "nothing a log line reads holds a credential: {line}"
            );
        }
    }

    /// The queue and the two socket timeouts are all bounded, so neither a stall nor a half-open
    /// socket can hold the news worker's thread or grow its memory.
    #[test]
    fn the_queue_and_the_socket_timeouts_are_bounded() {
        let d = NewsStreamConfig::default();
        assert_eq!(d.queue_capacity, QUEUE_CAPACITY);
        assert!(
            d.queue_capacity > 0,
            "a bound of zero would drop everything"
        );
        assert!(d.dial_timeout > Duration::ZERO && d.dial_timeout <= Duration::from_secs(30));
        assert!(d.send_timeout > Duration::ZERO && d.send_timeout <= Duration::from_secs(30));
    }
}
