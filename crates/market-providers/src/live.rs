//! [`LiveProvider`]: a [`LiveFeed`] on replay's engine-clock rule, recorded into the archive.
//!
//! Each event is stamped with its receive time, which is then raised to its **delivery time**
//! (the running maximum over the stream, replay's rule), so the recorded event carries exactly
//! the instant the engine clock was moved to before it. That makes a symbol's recorded day
//! replayable on its own with the same marks (`floor_1s` of each event's `receive_ts`), which is
//! what the server's per-symbol history load relies on (`chart_engine::ChartEngine::adopt_symbol`).
//! The only change to an event is in the rare case the wall clock stepped back.
//!
//! With [`LiveOptions::record`] the event then goes to the [`Recorder`] (a non-blocking queue;
//! `crate::recorder`) before it is handed to the consumer; the session carries the recorder so
//! the server can ask it for barriers. The recorder also gets the facts its coverage records are
//! made of (`market_archive::coverage`): the subscription at start, every link change, and the
//! feed's drop counter. A feed without a link (a channel) is up from the start and down when it
//! ends.

use crate::alpaca_stream::{AlpacaStream, AlpacaStreamConfig, StreamFeed};
use crate::config::{MarketDataMode, ProviderConfig};
use crate::recorder::{DropCounter, Recorder, RecorderConfig};
use crate::{
    ClockMode, ClockState, FeedItem, MarketDataProvider, ProviderError, ProviderInfo,
    ProviderSession, Speed,
};
use market_archive::{LinkStatus, Subscription};
use market_events::{MarketEvent, UnixNanos};
use market_replay::{Link, LinkState, MarkClock};
use std::time::Duration;
use tokio::sync::{mpsc, watch};

/// A live event source, already normalized into the contract: [`AlpacaStream`] in production,
/// [`ChannelFeed`] in tests.
pub trait LiveFeed: Send {
    fn label(&self) -> String;
    fn symbols(&self) -> Vec<String>;
    /// The source's connection state, if it has one. Taken once, before [`LiveFeed::connect`];
    /// [`LiveProvider`] forwards every change as [`FeedItem::Link`]. A watch, not a queue: only
    /// the latest state matters, and publishing it can never block the socket reader.
    fn link(&mut self) -> Option<watch::Receiver<Link>> {
        None
    }
    /// The feed's cumulative count of events it dropped before delivering them (a lagging
    /// consumer), for the coverage records. Taken once, before [`LiveFeed::connect`].
    fn drop_counter(&self) -> Option<DropCounter> {
        None
    }
    fn connect(self: Box<Self>) -> Result<mpsc::Receiver<MarketEvent>, ProviderError>;
}

/// A feed backed by a channel: tests, and capture-replay parity checks.
pub struct ChannelFeed {
    pub label: String,
    pub symbols: Vec<String>,
    pub rx: mpsc::Receiver<MarketEvent>,
}

impl LiveFeed for ChannelFeed {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn symbols(&self) -> Vec<String> {
        self.symbols.clone()
    }

    fn connect(self: Box<Self>) -> Result<mpsc::Receiver<MarketEvent>, ProviderError> {
        Ok(self.rx)
    }
}

#[derive(Clone, Debug)]
pub struct LiveOptions {
    /// Engine-clock and display-clock tick while the feed is quiet (wall time). `None` disables it
    /// (the clock then moves only with events' receive times, as in the parity test).
    pub idle_tick: Option<Duration>,
    pub mark_step_ns: i64,
    pub channel: usize,
    /// Record every event into the archive (`crate::recorder`). `None`: not recorded (tests,
    /// `LIVE_RECORD=0`).
    pub record: Option<RecorderConfig>,
}

impl Default for LiveOptions {
    fn default() -> Self {
        LiveOptions {
            idle_tick: Some(Duration::from_secs(1)),
            mark_step_ns: market_replay::marks::DEFAULT_MARK_STEP_NS,
            channel: 8192,
            record: None,
        }
    }
}

/// The live-provider boundary. See the crate docs: without a feed it is "not configured".
pub struct LiveProvider {
    /// `Err` says why there is no feed (shown to operators as `not_configured`).
    feed: Result<Box<dyn LiveFeed>, String>,
    opts: LiveOptions,
}

const NO_CREDENTIALS: &str = "live mode needs Alpaca credentials: set ALPACA_API_KEY and ALPACA_API_SECRET (or APCA_API_KEY_ID and APCA_API_SECRET_KEY) in the server's environment, or use MARKET_DATA_MODE=replay";

impl LiveProvider {
    /// The Alpaca stream from the configuration (`ALPACA_STREAM_BASE`, `ALPACA_STREAM_FEED`,
    /// `ALPACA_STREAM_NIGHT_FEED`, `LIVE_SYMBOLS`, credentials, `LIVE_RECORD*`), or "not
    /// configured" naming what is missing. Nothing is connected until
    /// [`MarketDataProvider::start`].
    pub fn from_config(cfg: &ProviderConfig) -> Self {
        let opts = LiveOptions {
            record: cfg.live.record.clone(),
            ..LiveOptions::default()
        };
        let feed = match (&cfg.mode, &cfg.live.credentials) {
            (MarketDataMode::Live, Some(creds)) => {
                let mut c = AlpacaStreamConfig::new(
                    cfg.live.stream_base.clone(),
                    cfg.live.feed,
                    cfg.live.symbols.clone(),
                    creds.clone(),
                );
                c.channel = opts.channel;
                // Blue Ocean overnight only pairs with SIP by day: an IEX or test day feed is a
                // deliberate choice, and switching it to BOATS at night would change what it is.
                if cfg.live.feed == StreamFeed::Sip {
                    c.night_feed = cfg.live.night_feed;
                }
                Ok(Box::new(AlpacaStream::new(c)) as Box<dyn LiveFeed>)
            }
            (MarketDataMode::Live, None) => Err(NO_CREDENTIALS.to_string()),
            (MarketDataMode::Replay, _) => Err("MARKET_DATA_MODE is replay".to_string()),
        };
        LiveProvider { feed, opts }
    }

    /// A live provider over an explicit feed.
    pub fn with_feed(feed: Box<dyn LiveFeed>, opts: LiveOptions) -> Self {
        LiveProvider {
            feed: Ok(feed),
            opts,
        }
    }
}

fn wall_now() -> UnixNanos {
    UnixNanos::from_utc(chrono::Utc::now()).unwrap_or_default()
}

/// Stamps the event's delivery time as its receive time: the stamp the feed gave it (or `now`),
/// raised to `last` so delivery is monotone like replay's running maximum. Returns it.
fn stamp(e: &mut MarketEvent, now: UnixNanos, last: i64) -> i64 {
    let r = match e {
        MarketEvent::Trade(t) => &mut t.receive_ts,
        MarketEvent::Quote(q) => &mut q.receive_ts,
        MarketEvent::Correction(c) => &mut c.receive_ts,
        MarketEvent::Cancel(c) => &mut c.receive_ts,
        MarketEvent::Status(s) => &mut s.receive_ts,
    };
    let d = r.get_or_insert(now).nanos().max(last);
    *r = Some(UnixNanos(d));
    d
}

/// A link change, as a coverage fact for the recorder.
fn note_link(rec: &Option<Recorder>, l: &Link) {
    if let Some(r) = rec {
        let state = match l.state {
            LinkState::Up => LinkStatus::Up,
            LinkState::Connecting => LinkStatus::Connecting,
            LinkState::Down => LinkStatus::Down,
        };
        r.note_link(state, l.detail.clone());
    }
}

fn live_clock(now: UnixNanos, events: u64) -> ClockState {
    ClockState {
        mode: ClockMode::Live,
        now,
        speed: Speed::Times(1),
        playing: true,
        day: None,
        start: None,
        end: None,
        finished: false,
        events,
    }
}

impl MarketDataProvider for LiveProvider {
    fn mode(&self) -> ClockMode {
        ClockMode::Live
    }

    fn start(self: Box<Self>) -> Result<ProviderSession, ProviderError> {
        let mut feed = self.feed.map_err(ProviderError::NotConfigured)?;
        let info = ProviderInfo {
            mode: ClockMode::Live,
            label: feed.label(),
            day: None,
            symbols: feed.symbols(),
            fixture: false,
        };
        let recorder = self.opts.record.as_ref().map(Recorder::start);
        let mut link = feed.link();
        if let Some(r) = &recorder {
            r.note_subscription(if info.symbols.is_empty() {
                Subscription::All
            } else {
                Subscription::Symbols(info.symbols.clone())
            });
            if let Some(c) = feed.drop_counter() {
                r.watch_stream_drops(c);
            }
            if link.is_none() {
                r.note_link(LinkStatus::Up, Some("channel feed".into()));
            }
        }
        let mut rx = feed.connect()?;
        let (out, items) = mpsc::channel(self.opts.channel.max(16));
        let state = watch::Sender::new(live_clock(wall_now(), 0));
        let clock = state.subscribe();
        let opts = self.opts;
        let rec = recorder.clone();
        tokio::spawn(async move {
            let mut marks = MarkClock::new(opts.mark_step_ns);
            let mut events = 0u64;
            let mut last_d = i64::MIN;
            let mut idle = opts.idle_tick.map(tokio::time::interval);
            // The state at start (usually "connecting") is news to the consumer too.
            if let Some(l) = link.as_mut() {
                let first = l.borrow_and_update().clone();
                note_link(&rec, &first);
                if out.send(FeedItem::Link(first)).await.is_err() {
                    return;
                }
            }
            loop {
                tokio::select! {
                    ev = rx.recv() => {
                        let Some(mut e) = ev else {
                            if let Some(r) = &rec {
                                r.note_link(LinkStatus::Down, Some("the feed ended".into()));
                            }
                            break;
                        };
                        let d = stamp(&mut e, wall_now(), last_d);
                        last_d = d;
                        // Before the consumer sees it: a barrier the server asks for later then
                        // covers every event it has applied.
                        if let Some(r) = &rec {
                            r.append(&e);
                        }
                        if let Some(m) = marks.mark(d)
                            && out.send(FeedItem::Advance(m)).await.is_err()
                        {
                            break;
                        }
                        events += 1;
                        if out.send(FeedItem::Event(e)).await.is_err() {
                            break;
                        }
                    }
                    changed = async { link.as_mut().expect("guarded").changed().await }, if link.is_some() => {
                        if changed.is_err() {
                            // The feed dropped its sender (it stopped); events may still drain.
                            link = None;
                            continue;
                        }
                        let l = link.as_mut().expect("guarded").borrow_and_update().clone();
                        note_link(&rec, &l);
                        if out.send(FeedItem::Link(l)).await.is_err() {
                            break;
                        }
                    }
                    _ = async { idle.as_mut().expect("guarded").tick().await }, if idle.is_some() => {
                        let now = wall_now();
                        if let Some(m) = marks.mark(now.nanos())
                            && out.send(FeedItem::Advance(m)).await.is_err()
                        {
                            break;
                        }
                        let c = live_clock(now, events);
                        state.send_replace(c.clone());
                        if out.send(FeedItem::Clock(c)).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Ok(ProviderSession {
            info,
            items,
            clock,
            control: None,
            recorder,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use market_archive::{
        Archive, ArchiveConfig, Collection, CoverageBasis, CoverageEvent, GapReason,
    };

    /// A feed with a connection state the test drives, and no events at all.
    struct LinkFeed {
        link: watch::Sender<Link>,
        rx: mpsc::Receiver<MarketEvent>,
    }

    impl LiveFeed for LinkFeed {
        fn label(&self) -> String {
            "test link feed".into()
        }
        fn symbols(&self) -> Vec<String> {
            Vec::new()
        }
        fn link(&mut self) -> Option<watch::Receiver<Link>> {
            Some(self.link.subscribe())
        }
        fn connect(self: Box<Self>) -> Result<mpsc::Receiver<MarketEvent>, ProviderError> {
            Ok(self.rx)
        }
    }

    async fn next_link(items: &mut mpsc::Receiver<FeedItem>) -> Link {
        loop {
            match tokio::time::timeout(Duration::from_secs(10), items.recv()).await {
                Ok(Some(FeedItem::Link(l))) => return l,
                Ok(Some(_)) => {}
                other => panic!("no link item: {other:?}"),
            }
        }
    }

    /// A disconnect becomes a durable gap record, and the quiet stretches around it, with no
    /// event at all for any symbol, read as complete: silence is not an outage.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_disconnect_is_recorded_as_a_gap_and_a_quiet_market_is_not() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = ArchiveConfig {
            root: tmp.path().join("archive"),
            retention_days: 0,
            min_free_bytes: 0,
        };
        let link = watch::Sender::new(Link {
            state: LinkState::Connecting,
            detail: None,
        });
        let (_events, rx) = mpsc::channel(16);
        let feed = LinkFeed {
            link: link.clone(),
            rx,
        };
        let opts = LiveOptions {
            idle_tick: None,
            record: Some(RecorderConfig::new(cfg.clone())),
            ..LiveOptions::default()
        };
        let mut session = Box::new(LiveProvider::with_feed(Box::new(feed), opts))
            .start()
            .unwrap();
        assert_eq!(
            next_link(&mut session.items).await.state,
            LinkState::Connecting
        );
        for (state, detail) in [
            (LinkState::Up, "subscribed"),
            (LinkState::Down, "network lost; reconnecting in 1.0 s"),
            (LinkState::Up, "subscribed"),
        ] {
            tokio::time::sleep(Duration::from_millis(30)).await;
            link.send_replace(Link {
                state,
                detail: Some(detail.into()),
            });
            assert_eq!(next_link(&mut session.items).await.state, state);
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
        let rec = session.recorder.take().unwrap();
        assert!(rec.close(Duration::from_secs(10)));

        let a = Archive::new(cfg);
        let days = a.coverage_days().unwrap();
        assert_eq!(days.len(), 1, "one trading date of wall time");
        let log = a.read_coverage(days[0]).unwrap().unwrap();
        assert_eq!(log.unreadable, 0);
        let kinds: Vec<&CoverageEvent> = log.records.iter().map(|r| &r.event).collect();
        assert!(matches!(kinds[0], CoverageEvent::ProcessStart { .. }));
        assert!(matches!(kinds.last().unwrap(), CoverageEvent::ProcessStop));
        let link_ts: Vec<(market_archive::LinkStatus, i64)> = log
            .records
            .iter()
            .filter_map(|r| match &r.event {
                CoverageEvent::Link { state, .. } => Some((*state, r.ts)),
                _ => None,
            })
            .collect();
        use market_archive::LinkStatus as L;
        assert_eq!(
            link_ts.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            [L::Connecting, L::Up, L::Down, L::Up]
        );
        let (up1, down, up2) = (link_ts[1].1, link_ts[2].1, link_ts[3].1);
        let ask = |from, to| {
            a.coverage(days[0], "QUIET", from, to, CoverageBasis::Stream)
                .unwrap()
        };
        assert_eq!(ask(up1, down).collection, Collection::Complete);
        let gap = ask(up1, up2);
        assert_eq!(gap.collection, Collection::Partial);
        assert_eq!(gap.gaps.len(), 1);
        assert_eq!(gap.gaps[0].reason, GapReason::LinkDown);
        assert_eq!((gap.gaps[0].from, gap.gaps[0].to), (down, up2));
        // After the clean stop: known not collected.
        let stop = log.records.last().unwrap().ts;
        assert_eq!(
            ask(stop, stop + 1_000).gaps[0].reason,
            GapReason::ProcessStopped
        );
    }
}
