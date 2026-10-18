use crate::alpaca_stream::{AlpacaStream, AlpacaStreamConfig};
use crate::config::{MarketDataMode, ProviderConfig};
use crate::{
    ClockMode, ClockState, FeedItem, MarketDataProvider, ProviderError, ProviderInfo,
    ProviderSession, Speed,
};
use market_events::{MarketEvent, UnixNanos};
use market_replay::{Link, MarkClock};
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
}

impl Default for LiveOptions {
    fn default() -> Self {
        LiveOptions {
            idle_tick: Some(Duration::from_secs(1)),
            mark_step_ns: market_replay::marks::DEFAULT_MARK_STEP_NS,
            channel: 8192,
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
    /// `LIVE_SYMBOLS`, credentials), or "not configured" naming what is missing. Nothing is
    /// connected until [`MarketDataProvider::start`].
    pub fn from_config(cfg: &ProviderConfig) -> Self {
        let opts = LiveOptions::default();
        let feed = match (&cfg.mode, &cfg.live.credentials) {
            (MarketDataMode::Live, Some(creds)) => {
                let mut c = AlpacaStreamConfig::new(
                    cfg.live.stream_base.clone(),
                    cfg.live.feed,
                    cfg.live.symbols.clone(),
                    creds.clone(),
                );
                c.channel = opts.channel;
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

/// Stamps the receive time when the feed did not, and returns the instant the event counts as
/// delivered (same rule as replay: receive time, else event time).
fn stamp(e: &mut MarketEvent, now: UnixNanos) -> i64 {
    let r = match e {
        MarketEvent::Trade(t) => &mut t.receive_ts,
        MarketEvent::Quote(q) => &mut q.receive_ts,
        MarketEvent::Correction(c) => &mut c.receive_ts,
        MarketEvent::Cancel(c) => &mut c.receive_ts,
        MarketEvent::Status(s) => &mut s.receive_ts,
    };
    r.get_or_insert(now).nanos()
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
        let mut link = feed.link();
        let mut rx = feed.connect()?;
        let (out, items) = mpsc::channel(self.opts.channel.max(16));
        let state = watch::Sender::new(live_clock(wall_now(), 0));
        let clock = state.subscribe();
        let opts = self.opts;
        tokio::spawn(async move {
            let mut marks = MarkClock::new(opts.mark_step_ns);
            let mut events = 0u64;
            let mut last_d = i64::MIN;
            let mut idle = opts.idle_tick.map(tokio::time::interval);
            // The state at start (usually "connecting") is news to the consumer too.
            if let Some(l) = link.as_mut() {
                let first = l.borrow_and_update().clone();
                if out.send(FeedItem::Link(first)).await.is_err() {
                    return;
                }
            }
            loop {
                tokio::select! {
                    ev = rx.recv() => {
                        let Some(mut e) = ev else { break };
                        // Delivery is monotone, like replay's running maximum.
                        let d = stamp(&mut e, wall_now()).max(last_d);
                        last_d = d;
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
        })
    }
}
