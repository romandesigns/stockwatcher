//! The stream socket: one long-lived task per [`AlpacaStream`].
//!
//! # Session
//!
//! connect (`{base}/{feed path}`, TLS via rustls + ring + the platform verifier, JSON frames) →
//! `connected` → send `auth` → `authenticated` → send `subscribe` (`trades`, and `statuses` for
//! the same symbols; `*` is the whole market) → `subscription` → stream. Every stage must finish
//! within [`AlpacaStreamConfig::handshake_timeout`] of connecting. Once streaming, the client
//! pings every [`AlpacaStreamConfig::ping_every`]; a connection that delivers no frame at all
//! (not even the pong) for [`AlpacaStreamConfig::silence_timeout`] is presumed dead. A closed
//! market is quiet but still answers pings, so silence alone never means "no data".
//!
//! # Feed by session (owner, 2026-10-18)
//!
//! With [`AlpacaStreamConfig::night_feed`] set, the client streams `night_feed` (Blue Ocean,
//! `boats`) during the overnight session (20:00–04:00 ET before a trading day,
//! `market_events::session`) and `feed` (`sip`) the rest of the time. SIP carries nothing
//! overnight, so a SIP-only stream showed an empty market from Sunday 20:00 although stocks were
//! trading. The session is checked every [`FEED_CHECK`] while streaming; when it calls for the
//! other feed the socket closes and the next connects at once (no backoff: nothing failed). Alpaca
//! allows one stream connection per account on most plans, so the two feeds take turns on one
//! connection rather than running side by side. Prints are labelled with the feed they came from.
//!
//! # Reconnect
//!
//! Any end of a session (network loss, close, read error, an `error` message, a missed stage)
//! closes the socket and reconnects after an exponential backoff with equal jitter: the wait is
//! uniform in `[d/2, d]`, `d` doubling from [`Backoff::initial`] to [`Backoff::max`]. `d` resets
//! only after a session that reached `subscription` and stayed up [`Backoff::stable_after`], so
//! a server that accepts and immediately drops us still backs off. Errors that a retry cannot fix
//! soon (402 auth failed, 405 symbol limit, 406 connection limit, 409 insufficient subscription,
//! 410 invalid action) wait the full `max`: Alpaca allows one stream connection per account on
//! most plans, and hammering a 406 would only keep the other holder's slot contested. Each
//! reconnect re-authenticates and re-subscribes from the configuration. **Events published while
//! disconnected are lost**: Alpaca does not replay, and this client does not backfill.
//!
//! # Reader and parser (2026-11-27)
//!
//! The socket is read on a thread of its own (`alpaca-reader`, its own single-threaded tokio
//! runtime) that does nothing but the session: the handshake, pings, and reading frames, each
//! pushed as raw text with its read time into a bounded queue. Parsing, stamping and delivery to
//! the consumer happen in a separate task on the caller's runtime. The read loop used to parse
//! and deliver inline, and to wait for the consumer, so anything that slowed those (a busy
//! runtime, the 2 s grace below, a closing rush's JSON) stopped the socket being read: on
//! 2026-11-25 between 15:51 and 16:00 ET Alpaca dropped the SIP stream eight times with
//! `407 slow client`. Now nothing downstream can slow the read.
//!
//! The reader still has to recognize the few frames that steer the session (`success`,
//! `subscription`, `error`): a frame naming one of them is parsed on the reader too (they are
//! rare and small), and handed on like any other so the parser delivers whatever events it holds.
//!
//! The queue is bounded twice, by frames ([`AlpacaStreamConfig::reader_queue`]) and by bytes
//! ([`AlpacaStreamConfig::reader_queue_bytes`]); the defaults hold well over ten seconds of the
//! closing rush. When it is full the reader **keeps reading and drops the frame**, counted in
//! [`StreamStats::queue_dropped_frames`] and, as the events it carried, in
//! [`StreamStats::dropped`]; it never blocks on the queue. Both edges are logged at WARN and put on
//! the link. The depth is in [`StreamStats`] and in a once-a-minute log line whenever the queue
//! held more than a trickle.
//!
//! # Backpressure (the consumer lags)
//!
//! Events go into a bounded channel ([`AlpacaStreamConfig::channel`], `LiveOptions.channel` in
//! production). The parser never blocks on it for longer than
//! [`AlpacaStreamConfig::lag_grace`]: while it waits the reader's queue fills, and past the
//! queue the frames are lost. When the grace runs out the parser **sheds**: it drops events
//! (counted in [`StreamStats::dropped`]) without waiting until the channel accepts one again,
//! then reports how many were lost. Both edges are logged and published on the link (state stays
//! `Up`, the detail names the gap), so a chart built during the gap is known to be incomplete
//! rather than silently wrong. Dropping beats disconnecting: a reconnect loses the same events
//! plus the handshake time, and risks the 406 race.
//!
//! # Credentials
//!
//! Only the auth message carries them. They are never logged, never part of a URL or an error
//! string, and [`AlpacaCredentials`]' `Debug` redacts them.

use super::protocol::{Inbound, StreamFeed, Subscription, parse_frame};
use crate::ProviderError;
use crate::import::alpaca::AlpacaCredentials;
use crate::live::LiveFeed;
use futures_util::{SinkExt, StreamExt};
use market_events::session::{SessionKind, classify};
use market_events::{MarketEvent, UnixNanos};
use market_replay::{Link, LinkState};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
use tokio_tungstenite::Connector;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{self, Message};

/// What to subscribe to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiveSymbols {
    /// `*`: every symbol on the feed.
    All,
    List(Vec<String>),
}

impl LiveSymbols {
    /// `*` or a comma list (`SMPL, abc` → `["SMPL", "ABC"]`). A list containing `*` is `All`.
    pub fn parse(s: &str) -> Result<LiveSymbols, String> {
        let mut out: Vec<String> = Vec::new();
        for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            if part == "*" {
                return Ok(LiveSymbols::All);
            }
            let sym = part.to_ascii_uppercase();
            // Alpaca symbols: letters, digits and the class separators (BRK.B, BRK/B).
            if !sym
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '/' | '-'))
            {
                return Err(format!("not a symbol: {part:?}"));
            }
            if !out.contains(&sym) {
                out.push(sym);
            }
        }
        if out.is_empty() {
            return Err("no symbols (use * for the whole market)".into());
        }
        Ok(LiveSymbols::List(out))
    }

    fn wire(&self) -> Vec<String> {
        match self {
            LiveSymbols::All => vec!["*".into()],
            LiveSymbols::List(l) => l.clone(),
        }
    }
}

/// How an event's `receive_ts` is set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiveTime {
    /// Wall time when the frame was read (production).
    Wall,
    /// `receive_ts = event_ts`, as if delivery were instant: the engine clock then follows the
    /// prints, not the machine, so a test's output does not depend on when it runs (parity).
    EventTime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Backoff {
    pub initial: Duration,
    pub max: Duration,
    pub stable_after: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Backoff {
            initial: Duration::from_secs(1),
            max: Duration::from_secs(60),
            stable_after: Duration::from_secs(30),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AlpacaStreamConfig {
    /// e.g. `wss://stream.data.alpaca.markets`, or provider-sim's `ws://127.0.0.1:8791`.
    pub base: String,
    /// The feed outside the overnight session (and always, without `night_feed`).
    pub feed: StreamFeed,
    /// The feed during the overnight session (see the module docs), e.g. `boats`. `None` streams
    /// `feed` around the clock.
    pub night_feed: Option<StreamFeed>,
    pub symbols: LiveSymbols,
    /// Also subscribe to trading statuses (halts) for the same symbols.
    pub statuses: bool,
    pub credentials: AlpacaCredentials,
    pub backoff: Backoff,
    pub handshake_timeout: Duration,
    pub ping_every: Duration,
    pub silence_timeout: Duration,
    pub lag_grace: Duration,
    pub channel: usize,
    /// Frames the reader's queue holds before it drops (module docs, "Reader and parser").
    pub reader_queue: usize,
    /// Bytes of frame text the reader's queue holds before it drops.
    pub reader_queue_bytes: usize,
    pub receive_time: ReceiveTime,
}

/// [`AlpacaStreamConfig::reader_queue`]'s default: 2^19 frames, ten seconds of single-message
/// frames at 50,000 a second (the open peaks near 45,000 events a second; Alpaca batches several
/// messages per frame, so in practice far more). Slots are allocated as the queue fills: an idle
/// queue costs nothing.
pub const READER_QUEUE: usize = 1 << 19;

/// [`AlpacaStreamConfig::reader_queue_bytes`]'s default: 128 MiB, about 20 s of the open's ~45,000
/// events a second at ~140 bytes of JSON each. The memory is only held while the parser is
/// behind.
pub const READER_QUEUE_BYTES: usize = 128 << 20;

impl AlpacaStreamConfig {
    pub const DEFAULT_BASE: &'static str = "wss://stream.data.alpaca.markets";

    pub fn new(
        base: impl Into<String>,
        feed: StreamFeed,
        symbols: LiveSymbols,
        credentials: AlpacaCredentials,
    ) -> Self {
        AlpacaStreamConfig {
            base: base.into(),
            feed,
            night_feed: None,
            symbols,
            statuses: true,
            credentials,
            backoff: Backoff::default(),
            handshake_timeout: Duration::from_secs(15),
            ping_every: Duration::from_secs(20),
            silence_timeout: Duration::from_secs(60),
            lag_grace: Duration::from_secs(2),
            channel: 8192,
            reader_queue: READER_QUEUE,
            reader_queue_bytes: READER_QUEUE_BYTES,
            receive_time: ReceiveTime::Wall,
        }
    }

    pub fn url(&self) -> String {
        self.url_for(self.feed)
    }

    pub fn url_for(&self, feed: StreamFeed) -> String {
        format!("{}/{}", self.base.trim_end_matches('/'), feed.path())
    }

    /// The feed to stream at `now`: `night_feed` inside an overnight session, else `feed`.
    pub fn feed_at(&self, now: UnixNanos) -> StreamFeed {
        match self.night_feed {
            Some(night) if classify(now).is_some_and(|s| s.kind == SessionKind::Overnight) => night,
            _ => self.feed,
        }
    }
}

/// How often a streaming session checks whether the session calls for the other feed.
pub const FEED_CHECK: Duration = Duration::from_secs(15);

/// Counters since the feed started (all sessions). Cheap to read from another task.
#[derive(Debug, Default)]
pub struct StreamStats {
    pub connects: AtomicU64,
    /// Sessions that reached `subscription`.
    pub subscribed: AtomicU64,
    pub frames: AtomicU64,
    pub events: AtomicU64,
    /// Events dropped: the consumer lagged past its grace, or the reader's queue was full (the
    /// events its dropped frames carried). See the module docs.
    pub dropped: AtomicU64,
    pub malformed: AtomicU64,
    pub ignored: AtomicU64,
    /// `error` messages received.
    pub errors: AtomicU64,
    /// Frames in the reader's queue now, and their bytes.
    pub queue_frames: AtomicU64,
    pub queue_bytes: AtomicU64,
    /// The most bytes the queue has held.
    pub queue_peak_bytes: AtomicU64,
    /// Frames the reader dropped because its queue was full.
    pub queue_dropped_frames: AtomicU64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatsSnapshot {
    pub connects: u64,
    pub subscribed: u64,
    pub frames: u64,
    pub events: u64,
    pub dropped: u64,
    pub malformed: u64,
    pub ignored: u64,
    pub errors: u64,
    pub queue_frames: u64,
    pub queue_bytes: u64,
    pub queue_peak_bytes: u64,
    pub queue_dropped_frames: u64,
}

impl StreamStats {
    pub fn snapshot(&self) -> StatsSnapshot {
        let g = |a: &AtomicU64| a.load(Ordering::Relaxed);
        StatsSnapshot {
            connects: g(&self.connects),
            subscribed: g(&self.subscribed),
            frames: g(&self.frames),
            events: g(&self.events),
            dropped: g(&self.dropped),
            malformed: g(&self.malformed),
            ignored: g(&self.ignored),
            errors: g(&self.errors),
            queue_frames: g(&self.queue_frames),
            queue_bytes: g(&self.queue_bytes),
            queue_peak_bytes: g(&self.queue_peak_bytes),
            queue_dropped_frames: g(&self.queue_dropped_frames),
        }
    }
}

fn bump(a: &AtomicU64) -> u64 {
    a.fetch_add(1, Ordering::Relaxed) + 1
}

/// The Alpaca stream as a [`LiveFeed`].
pub struct AlpacaStream {
    cfg: AlpacaStreamConfig,
    link: Arc<watch::Sender<Link>>,
    stats: Arc<StreamStats>,
}

impl AlpacaStream {
    pub fn new(cfg: AlpacaStreamConfig) -> Self {
        let link = Arc::new(watch::Sender::new(Link {
            state: LinkState::Connecting,
            detail: Some(format!(
                "connecting to {}",
                cfg.url_for(cfg.feed_at(wall_now()))
            )),
        }));
        AlpacaStream {
            cfg,
            link,
            stats: Arc::default(),
        }
    }

    pub fn stats(&self) -> Arc<StreamStats> {
        self.stats.clone()
    }
}

impl LiveFeed for AlpacaStream {
    fn label(&self) -> String {
        match (self.cfg.feed, self.cfg.night_feed) {
            (StreamFeed::Test, _) => "alpaca test stream (fake data, not market evidence)".into(),
            (f, Some(n)) => format!("alpaca {} stream ({} overnight)", f.as_str(), n.as_str()),
            (f, None) => format!("alpaca {} stream", f.as_str()),
        }
    }

    /// Empty for `*`: the whole market is not a list, and the symbols the server knows then come
    /// from the prints themselves.
    fn symbols(&self) -> Vec<String> {
        match &self.cfg.symbols {
            LiveSymbols::All => Vec::new(),
            LiveSymbols::List(l) => l.clone(),
        }
    }

    fn link(&mut self) -> Option<watch::Receiver<Link>> {
        Some(self.link.subscribe())
    }

    fn drop_counter(&self) -> Option<crate::recorder::DropCounter> {
        let stats = self.stats.clone();
        Some(std::sync::Arc::new(move || {
            stats.dropped.load(Ordering::Relaxed)
        }))
    }

    fn connect(self: Box<Self>) -> Result<mpsc::Receiver<MarketEvent>, ProviderError> {
        let rt = tokio::runtime::Handle::try_current()
            .map_err(|_| ProviderError::Failed("the live stream needs a tokio runtime".into()))?;
        let connector = tls_connector().map_err(ProviderError::Failed)?;
        let (tx, rx) = mpsc::channel(self.cfg.channel.max(16));
        let (frames_tx, frames_rx) = mpsc::channel(self.cfg.reader_queue.max(16));
        let AlpacaStream { cfg, link, stats } = *self;
        let cfg = Arc::new(cfg);
        // The reader first: if its thread cannot start there is nothing for the parser to do.
        let reader = {
            let (cfg, link, stats) = (cfg.clone(), link.clone(), stats.clone());
            std::thread::Builder::new()
                .name("alpaca-reader".into())
                .spawn(move || {
                    let rt = match tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        Ok(rt) => rt,
                        Err(e) => {
                            tracing::error!(error = %e, "alpaca stream: no runtime for the reader");
                            return;
                        }
                    };
                    rt.block_on(run(cfg, connector, frames_tx, link, stats));
                })
        };
        if let Err(e) = reader {
            return Err(ProviderError::Failed(format!(
                "the live stream's reader thread did not start: {e}"
            )));
        }
        rt.spawn(parse(cfg, frames_rx, tx, link, stats));
        Ok(rx)
    }
}

/// One frame as the reader read it, for the parser.
struct Frame {
    text: tungstenite::Utf8Bytes,
    /// Wall time when it was read: every event in it is stamped with this, however late it is
    /// parsed.
    at: UnixNanos,
    source: market_events::Source,
    /// The reader parsed it already (a `success`, `subscription` or `error` frame) and acted on
    /// its control messages; the parser takes only its events.
    control: bool,
}

/// Whether a frame may carry a message that steers the session. Alpaca's messages are
/// `{"T":"<type>",...}`; these three types are never part of a trade, status or correction, so
/// a data frame is never parsed twice.
fn names_control(text: &str) -> bool {
    text.contains("\"success\"") || text.contains("\"subscription\"") || text.contains("\"error\"")
}

/// The parser: frames from the reader's queue to events for the consumer, on the caller's
/// runtime. Waiting for the consumer (up to the grace) happens here, never on the reader.
async fn parse(
    cfg: Arc<AlpacaStreamConfig>,
    mut frames: mpsc::Receiver<Frame>,
    tx: mpsc::Sender<MarketEvent>,
    link: Arc<watch::Sender<Link>>,
    stats: Arc<StreamStats>,
) {
    let mut shed = Shed::default();
    loop {
        let f = tokio::select! {
            biased;
            _ = tx.closed() => return,
            f = frames.recv() => match f {
                Some(f) => f,
                // The reader stopped for good.
                None => return,
            },
        };
        stats.queue_frames.fetch_sub(1, Ordering::Relaxed);
        stats
            .queue_bytes
            .fetch_sub(f.text.len() as u64, Ordering::Relaxed);
        let items = match parse_frame(f.text.as_str(), f.source) {
            Ok(items) => items,
            Err(e) => {
                let n = bump(&stats.malformed);
                if n <= 10 || n.is_multiple_of(1000) {
                    tracing::warn!(error = %e, malformed = n, "alpaca stream: unreadable frame");
                }
                continue;
            }
        };
        for item in items {
            match item {
                Inbound::Event(mut e) => {
                    stamp(&mut e, cfg.receive_time, f.at);
                    bump(&stats.events);
                    if deliver(&tx, e, &cfg, &mut shed, &stats, &link)
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                // Counted (and acted on) by the reader.
                _ if f.control => {}
                Inbound::Malformed(m) => {
                    let n = bump(&stats.malformed);
                    if n <= 10 || n.is_multiple_of(1000) {
                        tracing::warn!(error = %m, malformed = n, "alpaca stream: message skipped");
                    }
                }
                Inbound::Error { code, msg } => {
                    // Only if a control message ever came spelled so the reader missed it.
                    bump(&stats.errors);
                    tracing::warn!(code, msg = %msg, "alpaca stream: error message the reader did not act on");
                }
                _ => {
                    bump(&stats.ignored);
                }
            }
        }
    }
}

/// The reader's queue-full state for one session (module docs, "Reader and parser").
#[derive(Default)]
struct Overflow {
    active: bool,
    frames: u64,
    events: u64,
}

/// Hands one frame to the parser without ever waiting. `Err` when the parser is gone.
fn enqueue(
    frames: &mpsc::Sender<Frame>,
    f: Frame,
    cfg: &AlpacaStreamConfig,
    over: &mut Overflow,
    stats: &StreamStats,
    link: &watch::Sender<Link>,
) -> Result<(), Gone> {
    let len = f.text.len() as u64;
    let held = stats.queue_bytes.load(Ordering::Relaxed);
    let room = held + len <= cfg.reader_queue_bytes as u64;
    let full = |f: Frame, over: &mut Overflow| {
        // Each Alpaca message has one "T" key: the events the frame carried, without parsing it.
        let events = f.text.as_str().matches("\"T\":").count() as u64;
        bump(&stats.queue_dropped_frames);
        stats.dropped.fetch_add(events, Ordering::Relaxed);
        if !over.active {
            over.active = true;
            tracing::warn!(
                queue_frames = stats.queue_frames.load(Ordering::Relaxed),
                queue_bytes = stats.queue_bytes.load(Ordering::Relaxed),
                "alpaca stream: reader queue full; dropping frames (still reading the socket)"
            );
            link.send_replace(Link {
                state: LinkState::Up,
                detail: Some("reader queue full: dropping live frames (data gap)".to_string()),
            });
        }
        over.frames += 1;
        over.events += events;
    };
    if !room {
        full(f, over);
        return Ok(());
    }
    // Counted before it is visible to the parser, which subtracts it.
    stats.queue_frames.fetch_add(1, Ordering::Relaxed);
    let now = stats.queue_bytes.fetch_add(len, Ordering::Relaxed) + len;
    match frames.try_send(f) {
        Ok(()) => {
            stats.queue_peak_bytes.fetch_max(now, Ordering::Relaxed);
            if over.active {
                over.active = false;
                let (n, e) = (
                    std::mem::take(&mut over.frames),
                    std::mem::take(&mut over.events),
                );
                tracing::warn!(
                    dropped_frames = n,
                    dropped_events = e,
                    "alpaca stream: reader queue accepting again; frames were dropped while it was full"
                );
                link.send_replace(Link {
                    state: LinkState::Up,
                    detail: Some(format!(
                        "recovered: {e} events in {n} frames were dropped while the reader's queue \
                         was full; charts over that window are incomplete"
                    )),
                });
            }
            Ok(())
        }
        Err(e) => {
            stats.queue_frames.fetch_sub(1, Ordering::Relaxed);
            stats.queue_bytes.fetch_sub(len, Ordering::Relaxed);
            match e {
                TrySendError::Closed(_) => Err(Gone::Consumer),
                TrySendError::Full(f) => {
                    full(f, over);
                    Ok(())
                }
            }
        }
    }
}

/// rustls with the ring provider (as the importer) and the OS trust store via the platform
/// verifier, so no process-wide default provider has to be installed first.
fn tls_connector() -> Result<Connector, String> {
    use rustls_platform_verifier::BuilderVerifierExt;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("tls: {e}"))?
        .with_platform_verifier()
        .map_err(|e| format!("tls verifier: {e}"))?
        .with_no_client_auth();
    Ok(Connector::Rustls(Arc::new(cfg)))
}

fn wall_now() -> UnixNanos {
    UnixNanos::from_utc(chrono::Utc::now()).unwrap_or_default()
}

/// Equal-jitter waits from a small xorshift generator (no crate needed for this).
struct Jitter(u64);

impl Jitter {
    fn seeded() -> Self {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        Jitter(t | 1)
    }

    fn unit(&mut self) -> f64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in `[d/2, d]`.
    fn wait(&mut self, d: Duration) -> Duration {
        d.mul_f64(0.5 + 0.5 * self.unit())
    }
}

enum SessionEnd {
    /// The consumer dropped its receiver: stop for good.
    ConsumerGone,
    /// The session calls for the other feed: reconnect at once.
    Switch { from: StreamFeed, to: StreamFeed },
    Ended {
        reason: String,
        subscribed: bool,
        /// Wait the full backoff maximum before retrying.
        wait_max: bool,
    },
}

/// The reader: sessions and reconnects, on the `alpaca-reader` thread. `tx` is the parser's
/// queue.
async fn run(
    cfg: Arc<AlpacaStreamConfig>,
    connector: Connector,
    tx: mpsc::Sender<Frame>,
    link: Arc<watch::Sender<Link>>,
    stats: Arc<StreamStats>,
) {
    let cfg = &*cfg;
    let link = &*link;
    let mut delay = cfg.backoff.initial;
    let mut jitter = Jitter::seeded();
    loop {
        let feed = cfg.feed_at(wall_now());
        let url = cfg.url_for(feed);
        link.send_replace(Link {
            state: LinkState::Connecting,
            detail: Some(format!("connecting to {url}")),
        });
        let began = Instant::now();
        let (reason, subscribed, wait_max) =
            match session(cfg, feed, &url, &connector, &tx, link, &stats).await {
                SessionEnd::ConsumerGone => {
                    tracing::info!(
                        feed = feed.as_str(),
                        "alpaca stream stopped (consumer gone)"
                    );
                    return;
                }
                SessionEnd::Switch { from, to } => {
                    tracing::info!(
                        from = from.as_str(),
                        to = to.as_str(),
                        "alpaca stream: session changed, switching feed"
                    );
                    delay = cfg.backoff.initial;
                    continue;
                }
                SessionEnd::Ended {
                    reason,
                    subscribed,
                    wait_max,
                } => (reason, subscribed, wait_max),
            };
        if subscribed && began.elapsed() >= cfg.backoff.stable_after {
            delay = cfg.backoff.initial;
        }
        let wait = jitter.wait(if wait_max { cfg.backoff.max } else { delay });
        delay = (delay * 2).min(cfg.backoff.max);
        tracing::warn!(
            feed = feed.as_str(),
            reason = %reason,
            retry_in_ms = wait.as_millis() as u64,
            "alpaca stream disconnected"
        );
        link.send_replace(Link {
            state: LinkState::Down,
            detail: Some(format!(
                "{reason}; reconnecting in {:.1} s",
                wait.as_secs_f64()
            )),
        });
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = tx.closed() => return,
        }
    }
}

/// Operator-facing meaning of Alpaca's stream error codes ("Streaming Market Data" docs), and
/// whether a quick retry is pointless.
fn explain(code: u16, msg: &str, feed: StreamFeed) -> (String, bool) {
    let (what, wait_max) = match code {
        402 => (
            "auth failed: check ALPACA_API_KEY / ALPACA_API_SECRET".to_string(),
            true,
        ),
        405 => (
            "symbol limit exceeded: subscribe to fewer symbols, or LIVE_SYMBOLS=*".into(),
            true,
        ),
        406 => (
            "connection limit exceeded: another client (a second server, a script) holds this \
             account's stream connection"
                .into(),
            true,
        ),
        409 => (
            format!(
                "insufficient subscription: the account is not entitled to the {} feed",
                feed.as_str()
            ),
            true,
        ),
        410 => ("invalid subscribe action for this feed".into(), true),
        407 => (
            "slow client: Alpaca dropped us for reading too slowly".into(),
            false,
        ),
        404 => ("auth timeout".into(), false),
        _ => (format!("error: {msg}"), false),
    };
    (format!("alpaca {code} {what} (\"{msg}\")"), wait_max)
}

fn describe_connect_error(e: &tungstenite::Error) -> String {
    match e {
        // The body is not shown: it is the provider's, and we only need the status.
        tungstenite::Error::Http(resp) => format!("upgrade refused: HTTP {}", resp.status()),
        other => format!("connect failed: {other}"),
    }
}

fn summarize(list: &[String]) -> String {
    if list.len() <= 5 {
        format!("[{}]", list.join(","))
    } else {
        format!("[{} symbols]", list.len())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Connected,
    Authenticated,
    Subscribed,
    Streaming,
}

impl Phase {
    fn awaiting(self) -> &'static str {
        match self {
            Phase::Connected => "\"connected\"",
            Phase::Authenticated => "\"authenticated\"",
            Phase::Subscribed => "\"subscription\"",
            Phase::Streaming => "frame",
        }
    }
}

/// Consumer-lag state for one session (see the module docs).
#[derive(Default)]
struct Shed {
    active: bool,
    dropped: u64,
}

enum Gone {
    Consumer,
}

async fn deliver(
    tx: &mpsc::Sender<MarketEvent>,
    e: MarketEvent,
    cfg: &AlpacaStreamConfig,
    shed: &mut Shed,
    stats: &StreamStats,
    link: &watch::Sender<Link>,
) -> Result<(), Gone> {
    match tx.try_send(e) {
        Ok(()) => {
            if shed.active {
                shed.active = false;
                let n = std::mem::take(&mut shed.dropped);
                tracing::warn!(
                    dropped = n,
                    "alpaca stream: consumer caught up; events were dropped while it lagged"
                );
                link.send_replace(Link {
                    state: LinkState::Up,
                    detail: Some(format!(
                        "recovered: {n} events were dropped while the consumer lagged; charts \
                         over that window are incomplete"
                    )),
                });
            }
            Ok(())
        }
        Err(TrySendError::Closed(_)) => Err(Gone::Consumer),
        Err(TrySendError::Full(e)) => {
            if shed.active {
                shed.dropped += 1;
                bump(&stats.dropped);
                return Ok(());
            }
            match tokio::time::timeout(cfg.lag_grace, tx.send(e)).await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(_)) => Err(Gone::Consumer),
                Err(_) => {
                    shed.active = true;
                    shed.dropped = 1;
                    bump(&stats.dropped);
                    tracing::warn!(
                        grace_ms = cfg.lag_grace.as_millis() as u64,
                        "alpaca stream: consumer is lagging; dropping events until it catches up"
                    );
                    link.send_replace(Link {
                        state: LinkState::Up,
                        detail: Some(
                            "consumer lagging: dropping live events (data gap)".to_string(),
                        ),
                    });
                    Ok(())
                }
            }
        }
    }
}

fn stamp(e: &mut MarketEvent, mode: ReceiveTime, now: UnixNanos) {
    let at = match mode {
        ReceiveTime::Wall => now,
        ReceiveTime::EventTime => e.event_ts(),
    };
    let r = match e {
        MarketEvent::Trade(t) => &mut t.receive_ts,
        MarketEvent::Quote(q) => &mut q.receive_ts,
        MarketEvent::Correction(c) => &mut c.receive_ts,
        MarketEvent::Cancel(c) => &mut c.receive_ts,
        MarketEvent::Status(s) => &mut s.receive_ts,
    };
    *r = Some(at);
}

/// How often a streaming session may log the reader queue's depth (only when it held more than
/// [`QUEUE_LOG_BYTES`] or dropped since the last line).
const QUEUE_LOG_EVERY: Duration = Duration::from_secs(60);
const QUEUE_LOG_BYTES: u64 = 1 << 20;

async fn session(
    cfg: &AlpacaStreamConfig,
    feed: StreamFeed,
    url: &str,
    connector: &Connector,
    tx: &mpsc::Sender<Frame>,
    link: &watch::Sender<Link>,
    stats: &StreamStats,
) -> SessionEnd {
    let mut subscribed = false;
    let end = |reason: String, subscribed: bool, wait_max: bool| SessionEnd::Ended {
        reason,
        subscribed,
        wait_max,
    };
    bump(&stats.connects);
    let mut req = match url.into_client_request() {
        Ok(r) => r,
        Err(e) => return end(format!("bad stream URL {url}: {e}"), false, true),
    };
    // Alpaca picks the frame encoding from this header; JSON means text frames.
    req.headers_mut().insert(
        "Content-Type",
        tungstenite::http::HeaderValue::from_static("application/json"),
    );
    let connecting =
        tokio_tungstenite::connect_async_tls_with_config(req, None, true, Some(connector.clone()));
    let ws = match tokio::time::timeout(cfg.handshake_timeout, connecting).await {
        Err(_) => return end("connect timed out".into(), false, false),
        Ok(Err(e)) => return end(describe_connect_error(&e), false, false),
        Ok(Ok((ws, _))) => ws,
    };
    let (mut sink, mut stream) = ws.split();
    let source = feed.source();
    let mut feed_check = tokio::time::interval_at(Instant::now() + FEED_CHECK, FEED_CHECK);
    let deadline = Instant::now() + cfg.handshake_timeout;
    let mut phase = Phase::Connected;
    let mut ping = tokio::time::interval_at(Instant::now() + cfg.ping_every, cfg.ping_every);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_frame = Instant::now();
    let mut over = Overflow::default();
    let mut queue_log = tokio::time::interval_at(Instant::now() + QUEUE_LOG_EVERY, QUEUE_LOG_EVERY);
    // The window's deepest queue and the drop count at its start.
    let (mut window_peak, mut window_dropped) =
        (0u64, stats.queue_dropped_frames.load(Ordering::Relaxed));
    let symbols = cfg.symbols.wire();

    macro_rules! send_text {
        ($v:expr) => {
            if let Err(e) = sink.send(Message::Text($v.to_string().into())).await {
                return end(format!("send failed: {e}"), subscribed, false);
            }
        };
    }

    loop {
        let silent_until = last_frame + cfg.silence_timeout;
        let msg = tokio::select! {
            biased;
            _ = tx.closed() => {
                let _ = sink.send(Message::Close(None)).await;
                return SessionEnd::ConsumerGone;
            }
            _ = tokio::time::sleep_until(deadline), if phase != Phase::Streaming => {
                let _ = sink.send(Message::Close(None)).await;
                return end(
                    format!("no {} within {:?} of connecting", phase.awaiting(), cfg.handshake_timeout),
                    false,
                    false,
                );
            }
            _ = tokio::time::sleep_until(silent_until), if phase == Phase::Streaming => {
                let _ = sink.send(Message::Close(None)).await;
                return end(
                    format!("no frame (not even a pong) for {:?}; connection presumed dead", cfg.silence_timeout),
                    subscribed,
                    false,
                );
            }
            _ = feed_check.tick(), if cfg.night_feed.is_some() => {
                let to = cfg.feed_at(wall_now());
                if to != feed {
                    let _ = sink.send(Message::Close(None)).await;
                    return SessionEnd::Switch { from: feed, to };
                }
                continue;
            }
            _ = ping.tick(), if phase == Phase::Streaming => {
                if let Err(e) = sink.send(Message::Ping(Default::default())).await {
                    return end(format!("ping failed: {e}"), subscribed, false);
                }
                continue;
            }
            _ = queue_log.tick() => {
                let dropped = stats.queue_dropped_frames.load(Ordering::Relaxed);
                if window_peak >= QUEUE_LOG_BYTES || dropped > window_dropped {
                    tracing::info!(
                        peak_bytes = window_peak,
                        queue_frames = stats.queue_frames.load(Ordering::Relaxed),
                        queue_bytes = stats.queue_bytes.load(Ordering::Relaxed),
                        dropped_frames = dropped - window_dropped,
                        dropped_frames_total = dropped,
                        frames_total = stats.frames.load(Ordering::Relaxed),
                        "alpaca stream reader queue (the parser fell behind the socket)"
                    );
                }
                window_peak = 0;
                window_dropped = dropped;
                continue;
            }
            m = stream.next() => m,
        };
        last_frame = Instant::now();
        let text = match msg {
            None => return end("connection lost".into(), subscribed, false),
            Some(Err(e)) => return end(format!("read error: {e}"), subscribed, false),
            Some(Ok(Message::Text(t))) => t,
            Some(Ok(Message::Close(f))) => {
                let why = f.map_or(String::new(), |f| format!(" ({} {})", f.code, f.reason));
                return end(
                    format!("server closed the connection{why}"),
                    subscribed,
                    false,
                );
            }
            Some(Ok(Message::Binary(_))) => {
                let _ = sink.send(Message::Close(None)).await;
                return end(
                    "binary frame (MessagePack?) although JSON was requested".into(),
                    subscribed,
                    false,
                );
            }
            // Ping (answered by tungstenite), pong, raw frames: liveness only.
            Some(Ok(_)) => continue,
        };
        bump(&stats.frames);
        let now = wall_now();
        // Only a frame that can steer the session is parsed here; every frame goes on to the
        // parser, which delivers its events.
        let control = phase != Phase::Streaming || names_control(text.as_str());
        // An unreadable one is left to the parser to count, as it would any.
        let items = if control {
            parse_frame(text.as_str(), source).unwrap_or_default()
        } else {
            Vec::new()
        };
        let frame = Frame {
            text,
            at: now,
            source,
            // An unreadable one is the parser's to count.
            control: control && !items.is_empty(),
        };
        if enqueue(tx, frame, cfg, &mut over, stats, link).is_err() {
            let _ = sink.send(Message::Close(None)).await;
            return SessionEnd::ConsumerGone;
        }
        window_peak = window_peak.max(stats.queue_bytes.load(Ordering::Relaxed));
        for item in items {
            match item {
                // The parser delivers it from the queued frame.
                Inbound::Event(_) => {}
                Inbound::Connected if phase == Phase::Connected => {
                    phase = Phase::Authenticated;
                    send_text!(json!({
                        "action": "auth",
                        "key": cfg.credentials.key_id,
                        "secret": cfg.credentials.secret,
                    }));
                }
                Inbound::Authenticated if phase == Phase::Authenticated => {
                    phase = Phase::Subscribed;
                    // corrections and cancelErrors come with trades; naming them is an error.
                    let mut sub = json!({"action": "subscribe", "trades": symbols});
                    if cfg.statuses {
                        sub["statuses"] = json!(symbols);
                    }
                    send_text!(sub);
                }
                Inbound::Subscription(s) => {
                    let first = phase == Phase::Subscribed;
                    phase = Phase::Streaming;
                    if first {
                        subscribed = true;
                        bump(&stats.subscribed);
                    }
                    let detail = describe_subscription(cfg, feed, &s);
                    tracing::info!(detail = %detail, "alpaca stream subscribed");
                    link.send_replace(Link {
                        state: LinkState::Up,
                        detail: Some(detail),
                    });
                }
                Inbound::Error { code, msg } => {
                    bump(&stats.errors);
                    let (reason, wait_max) = explain(code, &msg, feed);
                    let _ = sink.send(Message::Close(None)).await;
                    return end(reason, subscribed, wait_max);
                }
                Inbound::Malformed(m) => {
                    let n = bump(&stats.malformed);
                    if n <= 10 || n.is_multiple_of(1000) {
                        tracing::warn!(error = %m, malformed = n, "alpaca stream: message skipped");
                    }
                }
                Inbound::Ignored(_) | Inbound::Connected | Inbound::Authenticated => {
                    bump(&stats.ignored);
                }
            }
        }
    }
}

fn describe_subscription(cfg: &AlpacaStreamConfig, feed: StreamFeed, s: &Subscription) -> String {
    let mut d = format!(
        "alpaca {}: subscribed trades={}",
        feed.as_str(),
        summarize(&s.trades)
    );
    if cfg.statuses {
        d.push_str(&format!(" statuses={}", summarize(&s.statuses)));
    }
    // The server echoes what it accepted; say so when it is less than we asked for.
    let wanted = cfg.symbols.wire();
    if !wanted.iter().all(|w| s.trades.contains(w)) {
        d.push_str(" (fewer trade symbols than requested)");
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbols_parse() {
        assert_eq!(LiveSymbols::parse("*").unwrap(), LiveSymbols::All);
        assert_eq!(LiveSymbols::parse("smpl, *").unwrap(), LiveSymbols::All);
        assert_eq!(
            LiveSymbols::parse(" smpl,BRK.B,smpl ").unwrap(),
            LiveSymbols::List(vec!["SMPL".into(), "BRK.B".into()])
        );
        assert!(LiveSymbols::parse(" , ").is_err());
        assert!(LiveSymbols::parse("A B").is_err());
    }

    fn frame(text: &str) -> Frame {
        Frame {
            text: text.to_string().into(),
            at: UnixNanos(1),
            source: market_events::Source::AlpacaSip,
            control: false,
        }
    }

    /// The reader's queue: full by frames or by bytes it drops (never waits), counts the frames
    /// and the events they carried, says so once on the link, and reports the gap once the
    /// parser has made room.
    #[test]
    fn a_full_reader_queue_drops_counts_and_recovers() {
        let creds = AlpacaCredentials {
            key_id: "k".into(),
            secret: "s".into(),
        };
        let mut cfg = AlpacaStreamConfig::new("ws://x", StreamFeed::Sip, LiveSymbols::All, creds);
        let two = r#"[{"T":"t","S":"A"},{"T":"t","S":"B"}]"#;
        cfg.reader_queue_bytes = 3 * two.len();
        let (tx, mut rx) = mpsc::channel::<Frame>(2);
        let link = watch::Sender::new(Link {
            state: LinkState::Up,
            detail: None,
        });
        let stats = StreamStats::default();
        let mut over = Overflow::default();
        let put = |over: &mut Overflow| enqueue(&tx, frame(two), &cfg, over, &stats, &link);
        // Two fit the channel; the third finds it full.
        assert!(put(&mut over).is_ok() && put(&mut over).is_ok());
        assert!(
            put(&mut over).is_ok(),
            "full is a drop, not an error or a wait"
        );
        let st = stats.snapshot();
        assert_eq!(
            (
                st.queue_frames,
                st.queue_bytes,
                st.queue_dropped_frames,
                st.dropped
            ),
            (2, 2 * two.len() as u64, 1, 2),
            "{st:?}"
        );
        assert!(
            link.borrow()
                .detail
                .as_deref()
                .unwrap()
                .contains("reader queue full")
        );
        // The parser takes one (as `parse` does); the next is queued and the gap reported.
        let f = rx.try_recv().unwrap();
        stats.queue_frames.fetch_sub(1, Ordering::Relaxed);
        stats
            .queue_bytes
            .fetch_sub(f.text.len() as u64, Ordering::Relaxed);
        assert!(put(&mut over).is_ok());
        let d = link.borrow().detail.clone().unwrap();
        assert!(d.contains("2 events in 1 frames were dropped"), "{d}");
        // By bytes: room for one more frame in the channel, not in the byte bound.
        let _ = rx.try_recv().unwrap();
        stats.queue_frames.fetch_sub(1, Ordering::Relaxed);
        stats
            .queue_bytes
            .fetch_sub(two.len() as u64, Ordering::Relaxed);
        cfg.reader_queue_bytes = two.len() + 1;
        let put = |over: &mut Overflow| enqueue(&tx, frame(two), &cfg, over, &stats, &link);
        assert!(put(&mut over).is_ok());
        assert_eq!(stats.snapshot().queue_dropped_frames, 2);
        assert_eq!(stats.snapshot().queue_peak_bytes, 2 * two.len() as u64);
        // The parser gone: the reader stops.
        drop(rx);
        cfg.reader_queue_bytes = usize::MAX;
        assert!(enqueue(&tx, frame(two), &cfg, &mut over, &stats, &link).is_err());
        assert_eq!(
            stats.snapshot().queue_frames,
            1,
            "nothing left counted for it"
        );
    }

    #[test]
    fn only_frames_that_can_steer_the_session_are_parsed_on_the_reader() {
        assert!(names_control(r#"[{"T":"success","msg":"connected"}]"#));
        assert!(names_control(r#"[{"T":"subscription","trades":["*"]}]"#));
        assert!(names_control(
            r#"[{"T":"error","code":407,"msg":"slow client"}]"#
        ));
        assert!(!names_control(
            r#"[{"T":"t","S":"AAPL","i":1,"x":"V","p":1.5,"s":100,"c":["@"],"z":"C","t":"2026-11-25T20:59:59Z"}]"#
        ));
        assert!(!names_control(
            r#"[{"T":"s","S":"AAPL","sc":"H","sm":"Trading Halt"}]"#
        ));
    }

    #[test]
    fn jitter_stays_in_the_upper_half() {
        let mut j = Jitter(12345);
        let d = Duration::from_secs(8);
        for _ in 0..1000 {
            let w = j.wait(d);
            assert!(w >= d / 2 && w <= d, "{w:?}");
        }
    }

    #[test]
    fn errors_that_a_retry_cannot_fix_wait_the_maximum() {
        for code in [402, 405, 406, 409, 410] {
            assert!(explain(code, "x", StreamFeed::Sip).1, "{code}");
        }
        for code in [400, 401, 403, 404, 407, 500] {
            assert!(!explain(code, "x", StreamFeed::Sip).1, "{code}");
        }
        let (m, _) = explain(402, "auth failed", StreamFeed::Sip);
        assert!(m.contains("ALPACA_API_KEY") && m.contains("402"), "{m}");
        let (m, _) = explain(409, "insufficient subscription", StreamFeed::Sip);
        assert!(m.contains("sip feed"), "{m}");
    }

    #[test]
    fn the_night_feed_follows_the_overnight_session() {
        use market_events::session::et;
        let creds = AlpacaCredentials {
            key_id: "k".into(),
            secret: "s".into(),
        };
        let mut c = AlpacaStreamConfig::new("wss://x", StreamFeed::Sip, LiveSymbols::All, creds);
        let d = |y, m, day| chrono::NaiveDate::from_ymd_opt(y, m, day).unwrap();
        // Sunday 2026-10-18 22:06 ET: Monday's overnight session (Blue Ocean trades, SIP is quiet).
        let sunday_night = et(d(2026, 10, 18), 22, 6, 0, 0).unwrap();
        assert_eq!(
            c.feed_at(sunday_night),
            StreamFeed::Sip,
            "no night feed: SIP around the clock"
        );
        c.night_feed = Some(StreamFeed::Boats);
        assert_eq!(c.feed_at(sunday_night), StreamFeed::Boats);
        assert_eq!(c.url_for(c.feed_at(sunday_night)), "wss://x/v1beta1/boats");
        // 19:59:59 Sunday is no session at all: the day feed.
        assert_eq!(
            c.feed_at(et(d(2026, 10, 18), 19, 59, 59, 0).unwrap()),
            StreamFeed::Sip
        );
        // Monday 03:59 is still overnight; 04:00 opens pre-market on SIP.
        assert_eq!(
            c.feed_at(et(d(2026, 10, 19), 3, 59, 0, 0).unwrap()),
            StreamFeed::Boats
        );
        assert_eq!(
            c.feed_at(et(d(2026, 10, 19), 4, 0, 0, 0).unwrap()),
            StreamFeed::Sip
        );
        assert_eq!(
            c.feed_at(et(d(2026, 10, 19), 10, 0, 0, 0).unwrap()),
            StreamFeed::Sip
        );
        // Friday 21:00: Saturday is not a trading day, so no overnight session.
        assert_eq!(
            c.feed_at(et(d(2026, 10, 16), 21, 0, 0, 0).unwrap()),
            StreamFeed::Sip
        );
        assert_eq!(
            AlpacaStream::new(c).label(),
            "alpaca sip stream (boats overnight)"
        );
    }

    #[test]
    fn url_and_label() {
        let creds = AlpacaCredentials {
            key_id: "key-id-7f3a".into(),
            secret: "secret-9c1e".into(),
        };
        let c = AlpacaStreamConfig::new(
            "ws://127.0.0.1:8791/",
            StreamFeed::Sip,
            LiveSymbols::All,
            creds.clone(),
        );
        assert_eq!(c.url(), "ws://127.0.0.1:8791/v2/sip");
        let s = AlpacaStream::new(c);
        assert_eq!(s.label(), "alpaca sip stream");
        assert!(s.symbols().is_empty(), "* is not a symbol list");
        let dbg = format!("{:?}", s.cfg);
        assert!(
            !dbg.contains("key-id-7f3a")
                && !dbg.contains("secret-9c1e")
                && dbg.contains("REDACTED"),
            "{dbg}"
        );
    }
}
