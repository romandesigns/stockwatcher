//! Market data in the server: provider → one [`ChartEngine`] → broadcast to `/ws` clients.
//!
//! One ingest task owns the provider session. It applies every item to the engine under a short
//! lock, stamps a sequence number, and broadcasts the engine's increments **pre-serialized** (each
//! increment is turned into JSON once, not once per client). Each client connection filters the
//! broadcast by its subscription. The broadcast buffer is the bounded per-connection queue: a
//! client that falls behind gets `Lagged` and is resynchronized with a fresh snapshot instead of
//! growing memory.
//!
//! Snapshots are taken under the same lock as the sequence number, so a client skips every
//! broadcast increment with `seq ≤ snapshot seq` (already contained in the snapshot) and never
//! sees a candle move backwards.
//!
//! The paper broker (crates/paper-broker, docs/architecture/chart-trading-sim.md) rides on the
//! same path: every price-qualifying trade is fed to it after the engine, and its `paper.state` /
//! `paper.fill` frames go out on the same broadcast to every connection. A rebuild (`Reset`, i.e.
//! a seek or the initial catch-up) resets it, and the rebuild's re-read of the day is not fed to it:
//! orders cannot survive the tape being rewritten under them, nor fill against history replayed
//! only to rebuild candles.
//!
//! The quote board ([`crate::quotes`]) rides on it too: every trade updates its symbol's row under
//! the core lock, and one task per market ([`QUOTES_INTERVAL`], wall time) takes the rows that
//! changed and broadcasts ONE pre-serialized `quotes.update` that every opted-in connection
//! forwards (`Outbound::seq` = the engine seq at the take, so a connection skips frames already
//! contained in its `quotes.snapshot`: the chart's rule). Nothing is built while nobody is
//! subscribed or while a rebuild runs; after the rebuild every subscriber takes a fresh snapshot
//! (the `Resync` path). Names and exchanges come from the archive's optional `assets.json`, and
//! each row's previous close (the change-% reference brokers use) from its optional
//! `daily-closes.json` (market_archive::closes). The same previous close goes into every
//! `chart.snapshot` as `prevClose`, so the chart header and the "Prev close" level agree with the
//! search rows without the chart having to subscribe to the quote board.
//!
//! Switching the replay day (`replay.control {action: "day"}`, [`Command::Day`]) goes through the
//! same single provider session: the driver emits [`FeedItem::Source`] (the new day's label,
//! symbols and fixture flag, which replace [`ProviderInfo`] here, so `/api/symbols` and
//! `/api/replay/state` follow) and then the ordinary seek rebuild. Everything downstream (engine,
//! tape rings, last prices, quote board, paper account, the simulator's bar book) is therefore
//! reset by the `Reset` exactly as for a seek, and every subscriber gets fresh snapshots on the
//! `Resync` at its end. The previous day is not retained: the `Reset` replaces the whole [`Core`]
//! (the engine is where a day's candles live), the driver drops its old source and readers, and
//! the old `ProviderInfo` is released with its `Arc`. The days offered come from [`DayCatalog`].
//!
//! The live market is the archive's history plus the live tail ([`crate::history`]): the day's
//! recorded (or imported) events are loaded per symbol, on a chart's request or in the
//! background, into an engine of their own and moved into this one
//! (`ChartEngine::adopt_symbol`); until then the symbol's live events are held in `Core::unloaded`
//! rather than applied, a chart asking for it gets an empty snapshot, and `OutKind::Loaded` sends
//! the real one. History never goes through [`Market::apply`], so it never reaches the paper
//! broker.
//!
//! A rebuild (seek, day switch) does not serialize candle increments it will not send, only the
//! tape rows the history ring keeps: same state, about a third less time per rebuilt event.
//!
//! The Halt watcher's build-up board ([`crate::pressure`]) rides beside the quote board: every
//! trade (with the engine's verdict), every `chart.halt` and every `chart.luld` (from events and
//! from clock advances) update it under the core lock, and the quotes task ranks it once per
//! second and broadcasts ONE pre-serialized `pressure.update` (the whole board, at most
//! [`crate::pressure::BOARD_SIZE`] cards, often none) when it changed, to the
//! connections that sent `pressure.subscribe` (the quote board's opt-in pattern and `seq` rule).
//! The ranking runs whether or not anyone listens, so its hysteresis does not restart with the
//! first subscriber; only the frame is skipped.

use crate::context::ContextBoard;
use crate::days::{DayCatalog, DayEntry};
use crate::history::{History, Unloaded};
use crate::pressure::{Card, HaltInfo, PressureBoard};
use crate::quotes::{Counted, QuoteBoard, QuoteChanges, QuoteRow};
use chart_engine::wire::{ChartSnapshot, ChartUpdate, ClockInfo};
use chart_engine::{ChartEngine, DataQuality, Timeframe};
use chrono::{NaiveDate, Utc};
use chrono_tz::America::New_York;
use market_archive::fixture::{DEFAULT_SEED, DEFAULT_SYMBOLS, write_fixture_day};
use market_archive::{Archive, AssetMeta, PrevCloses};
use market_events::{MarketEvent, Price, PriceBand, SessionId, UnixNanos};
use market_providers::{
    ClockMode, ClockState, Command, CommandError as ReplayCommandError, FeedItem, LinkState,
    MarketDataMode, ProviderConfig, ProviderError, ProviderInfo, ReplayHandle, Speed,
};
use paper_broker::wire::{ModifyCommand, PaperFill, PlaceCommand};
use paper_broker::{Broker, CommandError, Fill};
use provider_sim::{Sim, SimConfig};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};

/// Recent tape rows kept per symbol and sent after a snapshot (same as the browser fixture).
/// Which market a client reads (owner, 2026-10-18: the dashboard consumes live market data). The
/// server runs both side by side, see [`Markets`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Replay,
    Live,
}

/// The live market and the replay market, running side by side. The main page (the dashboard)
/// reads live; replay is the practice feature (the chart page), with its own clock, day and paper
/// account. Each `/ws` connection picks one in its `hello` and each `/api` market route with
/// `?source=`; a client that names neither gets `default`, which is `MARKET_DATA_MODE`, so apps
/// built before the split keep seeing what they saw.
#[derive(Clone)]
pub struct Markets {
    pub replay: Market,
    pub live: Market,
    pub default: Source,
}

impl Markets {
    /// One market in the slot its mode says and an idle market in the other, defaulting to it
    /// (tests, and every caller from before the split).
    pub fn single(market: Market) -> Self {
        match market.mode() {
            ClockMode::Live => Markets {
                replay: Market::idle(ClockMode::Replay, "idle", None),
                live: market,
                default: Source::Live,
            },
            _ => Markets {
                live: Market::idle(ClockMode::Live, "idle", None),
                replay: market,
                default: Source::Replay,
            },
        }
    }

    pub fn get(&self, source: Option<Source>) -> &Market {
        match source.unwrap_or(self.default) {
            Source::Replay => &self.replay,
            Source::Live => &self.live,
        }
    }

    /// Starts both. The live market needs the Alpaca credentials (without them it is an idle
    /// "not_configured" market, as before); the replay market needs an archived day. Returns the
    /// replay's simulator when one is enabled.
    pub fn start(cfg: &MarketConfig) -> (Markets, Option<Sim>) {
        let with_mode = |mode| {
            let mut c = cfg.clone();
            c.provider.mode = mode;
            c
        };
        let (replay, sim) = Market::start(&with_mode(MarketDataMode::Replay));
        let (live, _) = Market::start(&with_mode(MarketDataMode::Live));
        let default = match cfg.provider.mode {
            MarketDataMode::Replay => Source::Replay,
            MarketDataMode::Live => Source::Live,
        };
        (
            Markets {
                replay,
                live,
                default,
            },
            sim,
        )
    }
}

pub const TAPE_HISTORY: usize = 40;
/// Broadcast capacity: how far one client may fall behind before it is resynchronized.
pub const BROADCAST_CAPACITY: usize = 16_384;
/// How often (wall time) changed quote rows are broadcast as one `quotes.update`. Wall time, not
/// replay time: at 10× a row may change many times a second, and a client wants a steady,
/// bounded frame rate rather than one that scales with the replay speed.
pub const QUOTES_INTERVAL: Duration = Duration::from_secs(1);

/// Market configuration (environment). Provider and simulator variables are documented on
/// [`ProviderConfig`] and [`SimConfig`]; here:
///
/// | Variable | Default | Meaning |
/// |---|---|---|
/// | `SIM_AUTOFIXTURE` | `0` | in replay mode, write a FIXTURE day when the archive is empty (dev) |
/// | `SIM_FIXTURE_DAY` | `2026-10-15` | that day |
/// | `SIM_FIXTURE_SYMBOLS` | `SMPL,FXLO,FXMD,FXHI` | its symbols |
/// | `PRICE_BAND_MIN` / `PRICE_BAND_MAX` | `0.10` / `25.00` | the candidate price band (inclusive) |
/// | `ARCHIVE_META_CHECK_SECS` | `60` | how often `assets.json` / `daily-closes.json` are checked for changes (0 = only on a day switch) |
/// | `REPLAY_FOLLOW_LATEST` | `0` | switch the running replay to a newly imported latest day (see [`Market::follow_latest`]) |
/// | `LIVE_HISTORY_PRELOAD` | `1` | live: load every symbol's history from the archive in the background, not only on a chart's request (`crate::history`) |
/// | `STOCK_CONTEXT` | `0` | five-pillar stock context and the Ross Panel (`crate::context`, `ross.*` / `context.get`) |
/// | `CONTEXT_EVIDENCE_CHECK_SECS` | `15` | with `STOCK_CONTEXT`: how often the archive's float, news and baseline evidence is checked for new records (`crate::evidence`; 0 = only at startup and on a replay day switch) |
#[derive(Clone, Debug)]
pub struct MarketConfig {
    pub provider: ProviderConfig,
    pub sim: SimConfig,
    pub autofixture: bool,
    pub fixture_day: NaiveDate,
    pub fixture_symbols: Vec<String>,
    /// Only stocks whose latest price-qualifying trade is inside this band are let through.
    pub band: PriceBand,
    /// Period of the archive metadata check ([`Market::refresh_archive_meta`]); zero = only on a
    /// day switch.
    pub meta_check: Duration,
    /// `REPLAY_FOLLOW_LATEST` (off by default: a switch resets the paper account).
    pub follow_latest: bool,
    /// `LIVE_HISTORY_PRELOAD` (default on): the live market loads every symbol's recorded history
    /// in the background, busiest first, not only the ones a chart asks for (`crate::history`).
    pub history_preload: bool,
    /// `STOCK_CONTEXT` (off by default): stock context and the Ross Panel (`crate::context`).
    pub stock_context: bool,
    /// `CONTEXT_EVIDENCE_CHECK_SECS`: period of the evidence check (`crate::evidence`); zero =
    /// only at startup and on a replay day switch.
    pub evidence_check: Duration,
}

impl Default for MarketConfig {
    fn default() -> Self {
        MarketConfig {
            provider: ProviderConfig::default(),
            sim: SimConfig::default(),
            autofixture: false,
            fixture_day: NaiveDate::from_ymd_opt(2026, 10, 15).expect("valid"),
            fixture_symbols: DEFAULT_SYMBOLS.iter().map(|s| s.to_string()).collect(),
            band: PriceBand::DEFAULT,
            meta_check: Duration::from_secs(60),
            follow_latest: false,
            history_preload: true,
            stock_context: false,
            evidence_check: Duration::from_secs(crate::evidence::DEFAULT_CHECK_SECS),
        }
    }
}

impl MarketConfig {
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let var = |k: &str| {
            get(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let mut c = MarketConfig {
            provider: ProviderConfig::from_lookup(&get)?,
            sim: SimConfig::from_lookup(&get)?,
            ..MarketConfig::default()
        };
        let on = |v: &str| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on");
        if let Some(v) = var("SIM_AUTOFIXTURE") {
            c.autofixture = on(&v);
        }
        if let Some(v) = var("REPLAY_FOLLOW_LATEST") {
            c.follow_latest = on(&v);
        }
        if let Some(v) = var("LIVE_HISTORY_PRELOAD") {
            c.history_preload = on(&v);
        }
        if let Some(v) = var("STOCK_CONTEXT") {
            c.stock_context = on(&v);
        }
        if let Some(v) = var("ARCHIVE_META_CHECK_SECS") {
            c.meta_check = Duration::from_secs(v.parse().map_err(|_| {
                format!("ARCHIVE_META_CHECK_SECS is not a whole number of seconds: {v:?}")
            })?);
        }
        if let Some(v) = var("CONTEXT_EVIDENCE_CHECK_SECS") {
            c.evidence_check = Duration::from_secs(v.parse().map_err(|_| {
                format!("CONTEXT_EVIDENCE_CHECK_SECS is not a whole number of seconds: {v:?}")
            })?);
        }
        if let Some(d) = var("SIM_FIXTURE_DAY") {
            c.fixture_day = NaiveDate::parse_from_str(&d, "%Y-%m-%d")
                .map_err(|_| format!("SIM_FIXTURE_DAY is not YYYY-MM-DD: {d:?}"))?;
        }
        if let Some(s) = var("SIM_FIXTURE_SYMBOLS") {
            c.fixture_symbols = s
                .split(',')
                .map(|x| x.trim().to_ascii_uppercase())
                .filter(|x| !x.is_empty())
                .collect();
        }
        let price = |k: &str, d: Price| -> Result<Price, String> {
            match var(k) {
                Some(v) => v
                    .parse::<Price>()
                    .map_err(|e| format!("{k} is not a price: {v:?} ({e})")),
                None => Ok(d),
            }
        };
        c.band = PriceBand::new(
            price("PRICE_BAND_MIN", PriceBand::DEFAULT.min)?,
            price("PRICE_BAND_MAX", PriceBand::DEFAULT.max)?,
        )?;
        Ok(c)
    }
}

/// One broadcast item.
#[derive(Debug)]
pub struct Outbound {
    /// Engine sequence after the change this item belongs to.
    pub seq: u64,
    pub kind: OutKind,
}

#[derive(Debug)]
pub enum OutKind {
    /// A chart increment or tape row (`timeframe` is `None` for tape rows).
    Chart {
        symbol: String,
        timeframe: Option<Timeframe>,
        update: Box<ChartUpdate>,
        json: String,
    },
    /// A `replay.clock` frame.
    Clock(String),
    /// A rebuild (seek) finished: every subscriber must take a fresh snapshot.
    Resync,
    /// A pre-serialized `paper.state` or `paper.fill` frame for every connection, whatever it
    /// subscribes to. Not filtered by `seq` (the frame carries the broker's own `seq`); its
    /// `Outbound::seq` is 0.
    Paper(String),
    /// A pre-serialized `quotes.update` for the connections that sent `quotes.subscribe`; every
    /// other connection drops it unread. `Outbound::seq` is the engine seq when it was taken.
    Quotes(String),
    /// Live: a symbol's history was just loaded into the engine (`crate::history`): every
    /// connection showing it takes a fresh snapshot (it had an empty or partial chart).
    Loaded(String),
    /// A pre-serialized `pressure.update` (the Halt watcher's whole board) for the connections
    /// that sent `pressure.subscribe`. `Outbound::seq` is the engine seq when it was ranked.
    Pressure(String),
    /// A pre-serialized `ross.update` (the Ross Panel, `crate::context`) for the connections that
    /// sent `ross.subscribe`. `Outbound::seq` is the engine seq when it was taken.
    Ross(String),
}

pub(crate) struct Core {
    pub(crate) engine: ChartEngine,
    pub(crate) seq: u64,
    pub(crate) tape: HashMap<String, VecDeque<String>>,
    /// Latest price-qualifying trade per symbol at the engine clock; drives the price band.
    pub(crate) last_price: HashMap<String, Price>,
    /// Last / open / volume per symbol for the ticker search (`quotes.*`).
    pub(crate) quotes: QuoteBoard,
    /// The Halt watcher's per-symbol pressure and halt risk and its board (`pressure.*`).
    pub(crate) pressure: PressureBoard,
    /// The cards of the last ranking: what `pressure.snapshot` sends, and what the next ranking
    /// is compared with (an unchanged board sends nothing).
    pressure_cards: Vec<PressureCardWire>,
    rebuilding: bool,
    /// Live: symbols whose recorded history is in the archive but not in the engine yet
    /// (`crate::history`). Their live events are counted or held here instead of applied.
    pub(crate) unloaded: HashMap<String, Unloaded>,
    /// The latest trading date of a price-qualifying print (labels the last session shown).
    pub(crate) latest_trade_day: Option<NaiveDate>,
    /// Stock context and the Ross Panel (`STOCK_CONTEXT=1`, `crate::context`); `None` when off.
    pub(crate) context: Option<ContextBoard>,
}

impl Core {
    fn new(closes: Arc<PrevCloses>) -> Self {
        Core {
            engine: ChartEngine::default(),
            seq: 0,
            tape: HashMap::new(),
            last_price: HashMap::new(),
            quotes: QuoteBoard::with_closes(closes),
            pressure: PressureBoard::new(),
            pressure_cards: Vec::new(),
            rebuilding: false,
            unloaded: HashMap::new(),
            latest_trade_day: None,
            context: None,
        }
    }

    /// Keeps the last [`TAPE_HISTORY`] tape rows of a symbol.
    pub(crate) fn record_tape(&mut self, symbol: &str, json: String) {
        let ring = match self.tape.get_mut(symbol) {
            Some(r) => r,
            None => self.tape.entry(symbol.to_string()).or_default(),
        };
        if ring.len() == TAPE_HISTORY {
            ring.pop_front();
        }
        ring.push_back(json);
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ProviderStatus {
    /// `starting`, `running`, `reconnecting` (live: link down, retrying), `no_data`,
    /// `not_configured`, `error`, `stopped`, `idle`.
    pub state: &'static str,
    pub detail: Option<String>,
}

pub(crate) struct Inner {
    mode: ClockMode,
    /// What is being replayed. Behind a lock because a day switch replaces it (on
    /// [`FeedItem::Source`]); readers clone the `Arc` out and never hold this lock while taking
    /// another one.
    info: Mutex<Option<Arc<ProviderInfo>>>,
    /// The archive's selectable days (replay mode with an archive; `None` for test sessions and
    /// live mode, which then offer no days).
    days: Option<DayCatalog>,
    status: Mutex<ProviderStatus>,
    pub(crate) core: Mutex<Core>,
    clock: Mutex<Option<ClockState>>,
    pub(crate) tx: broadcast::Sender<Arc<Outbound>>,
    control: Option<ReplayHandle>,
    bin: Price,
    band: PriceBand,
    /// The one paper account (single-user app). Never locked while `core` is held, or the other
    /// way round, so the two locks cannot deadlock.
    paper: Mutex<Broker>,
    /// `assets.json` (name, exchange); empty when the file is missing. Replaced whole when the
    /// file changes ([`Market::refresh_archive_meta`]); readers clone the `Arc` and let go.
    assets: Mutex<Arc<HashMap<String, AssetMeta>>>,
    /// `daily-closes.json` indexed per symbol; empty when the file is missing (change % then
    /// falls back to the open everywhere). Every rebuild's board starts from the current one, and
    /// a reload is handed to the running board too.
    closes: Mutex<Arc<PrevCloses>>,
    /// Where those two files are, and how they looked when last read (`None` in tests that pass
    /// them in directly).
    meta_files: Option<MetaFiles>,
    /// `REPLAY_FOLLOW_LATEST`, and the latest ready day already seen (so a day is followed once,
    /// and a user who switches back to an older day is not dragged forward again).
    follow_latest: bool,
    latest_seen: Mutex<Option<NaiveDate>>,
    /// Connections that sent `quotes.subscribe`. While 0, the quotes task builds nothing.
    quote_subs: AtomicUsize,
    /// Connections that sent `pressure.subscribe`. While 0, no `pressure.update` is serialized.
    pressure_subs: AtomicUsize,
    /// `STOCK_CONTEXT` (`MarketConfig::stock_context`): every `Core` this market makes gets a
    /// context board.
    context_on: AtomicBool,
    /// The last per-pillar readiness summary (`crate::context::Readiness`), taken on the evidence
    /// task and sent with every `ross` frame so an empty panel says why. A whole-market pass
    /// belongs nowhere near the publish path, so it is cached rather than computed per frame.
    pub(crate) readiness: Mutex<Option<crate::context::Readiness>>,
    /// Connections that sent `ross.subscribe`. While 0, no `ross.update` is serialized.
    ross_subs: AtomicUsize,
    /// Baseline observation/latency capture for the context panel (`crate::observe`): a bounded
    /// queue filled under the core lock and absorbed off it, with counted drops. Measurement
    /// scaffolding, not a latency claim.
    observations: crate::observe::ObservationSink,
    /// Live: the archive history this market loads from (`crate::history`); set once at start.
    pub(crate) history: std::sync::OnceLock<History>,
    /// With `STOCK_CONTEXT`: the archive evidence every context board is fed from
    /// (`crate::evidence`); set once at start.
    pub(crate) evidence: std::sync::OnceLock<crate::evidence::Evidence>,
    /// Live: the price alerts' input channel (`crate::alerts`), set once at start. Offered every
    /// eligible print, halt, resume and LULD change with `try_send`, never awaited.
    alerts: std::sync::OnceLock<Arc<crate::alerts::AlertSink>>,
}

/// What the market reads from the archive besides the replayed day itself: the root's optional
/// `assets.json` and `daily-closes.json`, and the catalog of days a replay can switch to. Grouped
/// because they share a lifecycle (read at startup from one archive root, the two files re-read
/// when they change; tests pass them empty).
#[derive(Default)]
struct ArchiveMeta {
    assets: HashMap<String, AssetMeta>,
    closes: Arc<PrevCloses>,
    days: Option<DayCatalog>,
    files: Option<MetaFiles>,
    follow_latest: bool,
}

/// A file's identity for change detection: a rewrite (the maintainer's temp file + rename)
/// changes the length or the mtime. A missing file is `(0, None)`.
type FileStamp = (u64, Option<std::time::SystemTime>);

fn file_stamp(path: &std::path::Path) -> FileStamp {
    std::fs::metadata(path)
        .map(|m| (m.len(), m.modified().ok()))
        .unwrap_or((0, None))
}

/// The archive root's `assets.json` and `daily-closes.json` as last read. The archive maintainer
/// (`swimport maintain`, a separate process) rewrites them after every session; checking costs
/// two `stat`s, and a file is re-read only when its stamp moved.
struct MetaFiles {
    archive: Archive,
    /// `[assets, closes]`.
    stamps: Mutex<[FileStamp; 2]>,
}

impl MetaFiles {
    /// Stamps taken BEFORE the first read: a rewrite between the stamp and the read is then seen
    /// as a change on the next check (one extra reload) rather than missed.
    fn new(archive: Archive) -> Self {
        let stamps = [
            file_stamp(&archive.assets_path()),
            file_stamp(&archive.daily_closes_path()),
        ];
        MetaFiles {
            archive,
            stamps: Mutex::new(stamps),
        }
    }

    /// Which files changed since the last call: `[assets, closes]`.
    fn changed(&self) -> [bool; 2] {
        let now = [
            file_stamp(&self.archive.assets_path()),
            file_stamp(&self.archive.daily_closes_path()),
        ];
        let mut last = self.stamps.lock().expect("meta stamps lock");
        let out = [now[0] != last[0], now[1] != last[1]];
        *last = now;
        out
    }
}

/// Shared market state; cheap to clone.
#[derive(Clone)]
pub struct Market(pub(crate) Arc<Inner>);

/// The `replay.clock` message.
#[derive(Serialize)]
struct ClockMessage<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    clock: &'a ClockInfo,
}

pub fn clock_info(c: &ClockState) -> ClockInfo {
    ClockInfo {
        mode: c.mode.as_str().to_string(),
        now: c.now.millis(),
        speed: c.speed.as_wire(),
        playing: c.playing,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum SnapshotError {
    UnknownSymbol,
    /// Known, but its latest price is outside the candidate band (or it has not traded yet).
    OutsidePriceBand,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ControlError {
    NotReplay,
    BadRequest(String),
    /// `day`: not one of the archive's replayable days ([`DayCatalog`]). Nothing changed.
    UnknownDay(String),
    /// The driver refused the command (e.g. the day's files could not be opened). Nothing changed.
    Rejected(String),
    Failed(String),
}

pub struct SnapshotOut {
    pub seq: u64,
    pub json: String,
    pub tape: Vec<String>,
}

fn mode_of(cfg: &ProviderConfig) -> ClockMode {
    match cfg.mode {
        MarketDataMode::Replay => ClockMode::Replay,
        MarketDataMode::Live => ClockMode::Live,
    }
}

impl Market {
    fn build(
        mode: ClockMode,
        info: Option<ProviderInfo>,
        status: ProviderStatus,
        control: Option<ReplayHandle>,
        band: PriceBand,
        meta: ArchiveMeta,
    ) -> Self {
        let ArchiveMeta {
            assets,
            closes,
            days,
            files,
            follow_latest,
        } = meta;
        let latest_seen = days
            .as_ref()
            .filter(|_| follow_latest)
            .and_then(|d| d.list().last().map(|e| e.day));
        let market = Market(Arc::new(Inner {
            mode,
            info: Mutex::new(info.map(Arc::new)),
            days,
            status: Mutex::new(status),
            core: Mutex::new(Core::new(closes.clone())),
            clock: Mutex::new(None),
            tx: broadcast::channel(BROADCAST_CAPACITY).0,
            control,
            bin: ChartEngine::default().config().profile_bin,
            band,
            paper: Mutex::new(Broker::new()),
            assets: Mutex::new(Arc::new(assets)),
            closes: Mutex::new(closes),
            meta_files: files,
            follow_latest,
            latest_seen: Mutex::new(latest_seen),
            quote_subs: AtomicUsize::new(0),
            pressure_subs: AtomicUsize::new(0),
            context_on: AtomicBool::new(false),
            readiness: Mutex::new(None),
            ross_subs: AtomicUsize::new(0),
            observations: crate::observe::ObservationSink::new(crate::observe::QUEUE_CAPACITY),
            history: std::sync::OnceLock::new(),
            evidence: std::sync::OnceLock::new(),
            alerts: std::sync::OnceLock::new(),
        }));
        let board = market.new_context_board();
        market.0.core.lock().expect("core lock").context = board;
        market
    }

    /// A fresh context board for this market's current day, or `None` with `STOCK_CONTEXT` off.
    /// Takes the `info` lock, then the evidence log's (nobody holding either takes another), so
    /// call it before locking `core`. The board starts with every evidence record already loaded
    /// for the day (`crate::evidence`): a seek's rebuild decides with the same evidence as the
    /// first pass, each record still joining only from its own `observed_at`.
    fn new_context_board(&self) -> Option<ContextBoard> {
        if !self.0.context_on.load(Ordering::Relaxed) {
            return None;
        }
        let day = self
            .0
            .info
            .lock()
            .expect("info lock")
            .as_ref()
            .and_then(|i| i.day);
        let mut board = ContextBoard::new(
            self.0.mode == ClockMode::Live,
            day,
            self.0.band.min,
            self.0.band.max,
        )?;
        if let Some(ev) = self.0.evidence.get()
            && let Some(batch) = ev.log.lock().expect("evidence log lock").since(None)
        {
            board.add_evidence(batch);
        }
        Some(board)
    }

    /// Turns stock context on or off for this market from now (tests; production reads
    /// `STOCK_CONTEXT` once at start). Rebuilds the board for the current day.
    pub fn set_stock_context(&self, on: bool) {
        self.0.context_on.store(on, Ordering::Relaxed);
        let board = self.new_context_board();
        self.0.core.lock().expect("core lock").context = board;
    }

    /// Connects the price alerts (crate::alerts) to this market. Only the live market gets one:
    /// alerts watch the live market only. A second call is ignored.
    pub fn attach_alerts(&self, sink: Arc<crate::alerts::AlertSink>) {
        let _ = self.0.alerts.set(sink);
    }

    pub fn alert_sink(&self) -> Option<&Arc<crate::alerts::AlertSink>> {
        self.0.alerts.get()
    }

    /// What the engine already holds for `symbol`, for an alert that starts watching it mid-day
    /// (crate::alerts::eval::Seed): the latest trading date's 15 s candles give the last price,
    /// the regular open, the high since 04:00 and the last 15 minutes' buckets; the snapshot's
    /// halts and modelled LULD bands give the rest. None while the symbol's history is not in the
    /// engine (live: not loaded yet). Called rarely (a newly watched symbol), off the ingest task.
    pub(crate) fn alert_seed(&self, symbol: &str) -> Option<crate::alerts::eval::Seed> {
        use market_events::SessionKind;
        let snap = {
            let core = self.0.core.lock().expect("core lock");
            core.engine
                .snapshot(symbol, Timeframe::S15, Some(self.0.bin))?
        };
        let last = snap.candles.last()?;
        let session = |t: i64| market_events::classify(UnixNanos::from_millis(t));
        let date = session(last.t).map(|s| s.date)?;
        let mut seed = crate::alerts::eval::Seed {
            date: Some(date),
            last: Some((last.t, last.c)),
            halted: snap.halts.last().is_some_and(|h| h.end.is_none()),
            luld: snap.luld.last().and_then(|p| p.up.zip(p.down)),
            ..Default::default()
        };
        for c in &snap.candles {
            let Some(s) = session(c.t).filter(|s| s.date == date) else {
                continue;
            };
            if matches!(
                s.kind,
                SessionKind::Pre | SessionKind::Regular | SessionKind::After
            ) {
                seed.high = Some(seed.high.map_or(c.h, |h: f64| h.max(c.h)));
            }
            if s.kind == SessionKind::Regular && seed.open.is_none() {
                seed.open = Some(c.o);
            }
            if c.t >= last.t - 15 * 60_000 {
                seed.buckets.push((c.t, c.l, c.h));
            }
        }
        Some(seed)
    }

    /// Hands this event to the price alerts, if attached (the live market). Eligible prints only,
    /// by the same rule as the candles and the last price (`Trade::effect().updates_ohlc`), so an
    /// alert can only fire on a price the chart shows. `ups` are the engine's increments for it
    /// (the modelled LULD band changes come from there), `revision` the engine's account of a
    /// correction (its replacement print, if eligible, is a late print).
    fn offer_alerts(
        &self,
        e: &MarketEvent,
        ups: &[ChartUpdate],
        revision: Option<&chart_engine::Revision>,
    ) {
        use crate::alerts::eval::Input;
        let Some(sink) = self.0.alerts.get() else {
            return;
        };
        let session = |ts: UnixNanos| market_events::classify(ts).map(|s| (s.date, s.kind));
        match e {
            MarketEvent::Trade(t) if t.effect().updates_ohlc => sink.offer(Input::Trade {
                symbol: t.symbol.clone(),
                price: t.price.to_f64(),
                ts: t.event_ts.millis(),
                session: t.session.map(|s| (s.date, s.kind)),
            }),
            MarketEvent::Status(s)
                if matches!(
                    s.kind,
                    market_events::StatusKind::Halt | market_events::StatusKind::Resume
                ) =>
            {
                if let Some(symbol) = &s.symbol {
                    let reason = if s.reason.trim().is_empty() {
                        s.code.clone()
                    } else {
                        s.reason.clone()
                    };
                    sink.offer(Input::Status {
                        symbol: symbol.clone(),
                        halted: s.kind == market_events::StatusKind::Halt,
                        ts: s.event_ts.millis(),
                        session: session(s.event_ts),
                        reason,
                    });
                }
            }
            MarketEvent::Correction(c) => {
                if let Some(added) = revision.and_then(|r| r.added.as_ref()).filter(|a| a.ohlc) {
                    sink.offer(Input::Corrected {
                        symbol: c.symbol.clone(),
                        price: added.price.to_f64(),
                        ts: c.event_ts.millis(),
                        session: revision.map(|r| (r.session.date, r.session.kind)),
                    });
                }
            }
            _ => {}
        }
        for u in ups {
            if let ChartUpdate::Luld(l) = u {
                sink.offer(Input::Luld {
                    symbol: l.symbol.clone(),
                    ts: l.point.t,
                    up: l.point.up,
                    down: l.point.down,
                });
            }
        }
    }

    /// No provider (tests, or a provider that could not start).
    pub fn idle(mode: ClockMode, state: &'static str, detail: Option<String>) -> Self {
        Market::build(
            mode,
            None,
            ProviderStatus { state, detail },
            None,
            PriceBand::DEFAULT,
            ArchiveMeta::default(),
        )
    }

    /// Starts the configured provider and the ingest task. Never fails: a provider that cannot
    /// start leaves an idle market whose status says why (the server, health and web app still
    /// run). Returns the simulator when replay mode has one enabled.
    pub fn start(cfg: &MarketConfig) -> (Market, Option<Sim>) {
        let mode = mode_of(&cfg.provider);
        let archive = Archive::new(cfg.provider.archive.clone());
        if mode == ClockMode::Replay {
            let today = Utc::now().with_timezone(&New_York).date_naive();
            match archive.prune(today) {
                Ok(d) if !d.is_empty() => {
                    tracing::info!(days = ?d, "archive retention pruned days")
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "archive prune failed"),
            }
            if cfg.autofixture && archive.days().map(|d| d.is_empty()).unwrap_or(false) {
                match write_fixture_day(
                    &archive,
                    cfg.fixture_day,
                    &cfg.fixture_symbols,
                    DEFAULT_SEED,
                ) {
                    Ok(m) => tracing::warn!(
                        day = %cfg.fixture_day, symbols = m.symbols.len(), events = m.total_events(),
                        root = %archive.root().display(),
                        "SIM_AUTOFIXTURE: wrote a FIXTURE day (synthetic, not market data)"
                    ),
                    Err(e) => tracing::error!(error = %e, "SIM_AUTOFIXTURE failed"),
                }
            }
        }
        let files = MetaFiles::new(archive.clone());
        let assets = load_assets(&archive);
        let closes = Arc::new(load_closes(&archive));
        let started = market_providers::from_config(&cfg.provider).and_then(|p| p.start());
        let session = match started {
            Ok(s) => s,
            Err(e) => {
                let state = match &e {
                    ProviderError::NotConfigured(_) => "not_configured",
                    ProviderError::NoData(_) => "no_data",
                    ProviderError::Failed(_) => "error",
                };
                tracing::warn!(mode = mode.as_str(), error = %e, "market data provider not running");
                return (Market::idle(mode, state, Some(e.to_string())), None);
            }
        };
        tracing::info!(mode = mode.as_str(), source = %session.info.label, symbols = session.info.symbols.len(), "market data provider started");
        let sim = (mode == ClockMode::Replay && cfg.sim.enabled)
            .then_some(session.info.day)
            .flatten()
            .map(|day| {
                Sim::new(
                    cfg.sim.clone(),
                    archive.clone(),
                    day,
                    session.info.symbols.clone(),
                    session.info.fixture,
                    session.clock.clone(),
                )
            });
        // Only a controllable replay can switch days, so only it gets the catalog.
        let days = session
            .control
            .is_some()
            .then(|| DayCatalog::new(archive.clone()));
        let market = Market::build(
            mode,
            Some(session.info.clone()),
            ProviderStatus {
                state: "running",
                detail: None,
            },
            session.control.clone(),
            cfg.band,
            ArchiveMeta {
                assets,
                closes,
                days,
                files: Some(files),
                follow_latest: cfg.follow_latest,
            },
        );
        if cfg.stock_context {
            market.set_stock_context(true);
            // Before the ingest starts: a replay's first load runs here (the feed waits on disk,
            // nothing is lost), a live one in the background (crate::evidence).
            market.init_evidence(archive.clone(), cfg.band.max, cfg.evidence_check);
        }
        if mode == ClockMode::Live {
            // Before the first event: the history decides which symbols the ingest holds back.
            market.init_history(
                archive.clone(),
                session.recorder.clone(),
                crate::history::wall_now(),
                cfg.history_preload,
            );
        }
        tokio::spawn(ingest(session.items, market.clone(), sim.clone()));
        market.spawn_quotes_task();
        if !cfg.meta_check.is_zero() {
            market.spawn_meta_task(cfg.meta_check);
        }
        (market, sim)
    }

    /// A live market on a running session that loads its history from `archive` as if started
    /// at `now` (tests; [`Market::start`] does the same with the wall clock).
    pub fn from_live_session(
        session: market_providers::ProviderSession,
        archive: Archive,
        now: UnixNanos,
        preload: bool,
    ) -> Market {
        let market = Market::build(
            ClockMode::Live,
            Some(session.info.clone()),
            ProviderStatus {
                state: "running",
                detail: None,
            },
            None,
            PriceBand::DEFAULT,
            ArchiveMeta::default(),
        );
        market.init_history(archive, session.recorder.clone(), now, preload);
        tokio::spawn(ingest(session.items, market.clone(), None));
        market.spawn_quotes_task();
        market
    }

    /// Starts on an already-running provider session (tests).
    pub fn from_session(session: market_providers::ProviderSession, sim: Option<Sim>) -> Market {
        Self::from_session_with_assets(session, sim, HashMap::new())
    }

    /// [`Market::from_session`] with asset metadata (tests of names and exchanges).
    pub fn from_session_with_assets(
        session: market_providers::ProviderSession,
        sim: Option<Sim>,
        assets: HashMap<String, AssetMeta>,
    ) -> Market {
        Self::from_session_with(session, sim, assets, PrevCloses::default())
    }

    /// [`Market::from_session`] with asset metadata and previous closes (tests).
    pub fn from_session_with(
        session: market_providers::ProviderSession,
        sim: Option<Sim>,
        assets: HashMap<String, AssetMeta>,
        closes: PrevCloses,
    ) -> Market {
        let market = Market::build(
            session.info.mode,
            Some(session.info.clone()),
            ProviderStatus {
                state: "running",
                detail: None,
            },
            session.control.clone(),
            PriceBand::DEFAULT,
            ArchiveMeta {
                assets,
                closes: Arc::new(closes),
                ..ArchiveMeta::default()
            },
        );
        tokio::spawn(ingest(session.items, market.clone(), sim));
        market.spawn_quotes_task();
        market
    }

    /// The once-per-[`QUOTES_INTERVAL`] broadcaster. Holds only a weak reference, so it ends with
    /// the market instead of keeping it alive.
    fn spawn_quotes_task(&self) {
        let weak: Weak<Inner> = Arc::downgrade(&self.0);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(QUOTES_INTERVAL);
            // Delay, not Skip: after a late tick (a busy runtime at the open) Skip fires the next
            // one on the original schedule, which can be well under a second later and breaks the
            // "at most once per wall second" promise in the ws protocol docs. Delay restarts the
            // period from the late tick.
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut n = 0u64;
            loop {
                tick.tick().await;
                let Some(inner) = weak.upgrade() else { break };
                let market = Market(inner);
                market.publish_quotes();
                market.publish_pressure();
                // Before the broadcast, and whether or not anyone is subscribed: the context
                // engine's output journals are drained so an unattended run stays bounded.
                market.drain_context();
                market.publish_context();
                n += 1;
                // Live: the quote rows go into the archive's recording now and then, the
                // startup summary of the next restart (`crate::history`).
                if n.is_multiple_of(crate::history::QUOTES_CHECKPOINT_TICKS) {
                    market.checkpoint_quotes();
                }
                if n.is_multiple_of(crate::observe::LOG_EVERY_TICKS) {
                    market.log_observations();
                }
            }
        });
    }

    /// Every `every` (wall time): re-read changed archive metadata, then, with
    /// `REPLAY_FOLLOW_LATEST`, follow a new latest day. Weak like the quotes task.
    fn spawn_meta_task(&self, every: Duration) {
        let weak: Weak<Inner> = Arc::downgrade(&self.0);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // The first tick is immediate; startup has just read everything.
            tick.tick().await;
            loop {
                tick.tick().await;
                let Some(inner) = weak.upgrade() else { break };
                let market = Market(inner);
                // Re-reading the closes file is ~10^5 entries of JSON: off the async workers.
                let m = market.clone();
                let _ = tokio::task::spawn_blocking(move || m.refresh_archive_meta()).await;
                if market.0.follow_latest {
                    market.follow_latest().await;
                }
            }
        });
    }

    /// Re-reads `assets.json` and `daily-closes.json` if either changed on disk since it was last
    /// read (two `stat`s otherwise). Called by the meta task (`ARCHIVE_META_CHECK_SECS`) and on
    /// every day switch, so a day imported tonight is quoted against its real previous close
    /// without a restart. Returns whether anything was reloaded.
    ///
    /// - Assets are swapped at once: they only name rows, and a row's name is sent when it enters
    ///   a client's view.
    /// - Closes become the index of the next rebuild's board (seek, day switch) and are handed
    ///   to the running board too ([`QuoteBoard::set_closes`]: a row whose reference changed
    ///   resends it, and the chart's `prevClose` reads the same index, so both follow at once).
    /// - A file that fails to read keeps the previous contents (logged): the maintainer writes
    ///   atomically, so an unreadable file is a real problem, and wiping every name or reference
    ///   over it would make it worse. A deleted file is a deliberate state and empties them.
    pub fn refresh_archive_meta(&self) -> bool {
        let Some(files) = &self.0.meta_files else {
            return false;
        };
        let [assets_changed, closes_changed] = files.changed();
        let archive = &files.archive;
        if assets_changed {
            match archive.read_assets() {
                Ok(m) => {
                    let m: HashMap<String, AssetMeta> = m.unwrap_or_default().into_iter().collect();
                    tracing::info!(assets = m.len(), "assets.json changed: reloaded");
                    *self.0.assets.lock().expect("assets lock") = Arc::new(m);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "assets.json changed but is unreadable: keeping the names already loaded")
                }
            }
        }
        if closes_changed {
            match archive.read_daily_closes() {
                Ok(m) => {
                    let m = m.unwrap_or_default();
                    let (idx, dropped) = PrevCloses::new(&m);
                    tracing::info!(
                        days = m.len(),
                        first = ?m.keys().next(),
                        last = ?m.keys().next_back(),
                        symbols = idx.symbols(),
                        dropped,
                        "daily-closes.json changed: reloaded"
                    );
                    let idx = Arc::new(idx);
                    *self.0.closes.lock().expect("closes lock") = idx.clone();
                    let mut core = self.0.core.lock().expect("core lock");
                    core.quotes.set_closes(idx);
                    // A step of its own in the sequence: the rows it changed go out in the next
                    // `quotes.update`, stamped with this seq, and a connection forwards only
                    // updates newer than its snapshot. Without the bump a paused replay (seq
                    // standing still) would hold the update back from every client whose
                    // snapshot predates the reload, while one taken after it already has it.
                    core.seq += 1;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "daily-closes.json changed but is unreadable: keeping the closes already loaded")
                }
            }
        }
        assets_changed || closes_changed
    }

    /// `REPLAY_FOLLOW_LATEST=1`: when the catalog's latest ready day is newer than any latest day
    /// seen before, switch the replay to it, exactly as the app's day menu would (so it resets
    /// the paper account and every client resyncs). A day is followed once: switching back to an
    /// older day by hand stays put until the next new day arrives. Returns whether it switched.
    pub async fn follow_latest(&self) -> bool {
        // Imported days only: a live recording offered as provisional is not "newly imported",
        // and following it would switch again (and reset the paper account) when its import lands.
        let Some(latest) = self.days().iter().rfind(|e| !e.provisional).map(|e| e.day) else {
            return false;
        };
        {
            let mut seen = self.0.latest_seen.lock().expect("latest lock");
            if seen.is_some_and(|s| latest <= s) {
                return false;
            }
            *seen = Some(latest);
        }
        if self.info().and_then(|i| i.day) == Some(latest) {
            return false;
        }
        tracing::info!(day = %latest, "REPLAY_FOLLOW_LATEST: switching the replay to the newly imported day");
        match self
            .control("day", Some(&Value::String(latest.to_string())))
            .await
        {
            Ok(_) => true,
            Err(e) => {
                tracing::warn!(day = %latest, error = ?e, "REPLAY_FOLLOW_LATEST: the switch was refused");
                false
            }
        }
    }

    pub(crate) fn current_closes(&self) -> Arc<PrevCloses> {
        self.0.closes.lock().expect("closes lock").clone()
    }

    pub fn mode(&self) -> ClockMode {
        self.0.mode
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Outbound>> {
        self.0.tx.subscribe()
    }

    pub fn default_bin(&self) -> Price {
        self.0.bin
    }

    pub fn clock(&self) -> Option<ClockState> {
        self.0.clock.lock().expect("clock lock").clone()
    }

    pub fn clock_frame(&self) -> Option<String> {
        self.clock().map(|c| clock_frame(&c))
    }

    /// The current provider description (a cheap `Arc` clone; the lock is released at once).
    pub fn info(&self) -> Option<Arc<ProviderInfo>> {
        self.0.info.lock().expect("info lock").clone()
    }

    fn known(&self, core: &Core, symbol: &str) -> bool {
        self.info()
            .is_some_and(|i| i.symbols.iter().any(|s| s == symbol))
            || core.engine.symbols().any(|s| s == symbol)
            || core.unloaded.contains_key(symbol)
    }

    /// Every symbol the provider knows, sorted, whether or not it is a candidate.
    fn all_symbols(&self, core: &Core) -> Vec<String> {
        let mut v: Vec<String> = self.info().map(|i| i.symbols.clone()).unwrap_or_default();
        v.extend(core.engine.symbols().map(String::from));
        v.extend(core.unloaded.keys().cloned());
        v.sort();
        v.dedup();
        v
    }

    /// The price band applied to every client-facing surface.
    pub fn band(&self) -> PriceBand {
        self.0.band
    }

    /// True when `symbol`'s latest price-qualifying trade at the clock is inside the band.
    /// A symbol with no such trade yet is not a candidate.
    pub fn is_candidate(&self, symbol: &str) -> bool {
        let core = self.0.core.lock().expect("core lock");
        Self::candidate_in(&core, self.0.band, symbol)
    }

    fn candidate_in(core: &Core, band: PriceBand, symbol: &str) -> bool {
        core.last_price
            .get(symbol)
            .is_some_and(|p| band.contains(*p))
    }

    /// The candidates: symbols currently inside the price band, sorted. This is the only symbol
    /// list clients get; out-of-band stocks never reach them.
    pub fn symbols(&self) -> Vec<String> {
        let core = self.0.core.lock().expect("core lock");
        self.all_symbols(&core)
            .into_iter()
            .filter(|s| Self::candidate_in(&core, self.0.band, s))
            .collect()
    }

    fn apply(&self, item: FeedItem) {
        match item {
            FeedItem::Event(e) => {
                // Price-band and paper-broker input: the same price rule the candles use (a
                // print that may not set OHLC, e.g. an odd lot or average-price report, can
                // neither move a stock into or out of the candidate set nor fill an order).
                let qualifying = match &e {
                    MarketEvent::Trade(t) if t.effect().updates_ohlc => {
                        Some((t.symbol.clone(), t.price, t.event_ts.millis()))
                    }
                    _ => None,
                };
                // The context board's previous closes, taken before `core` (lock order).
                let context_closes = (self.0.context_on.load(Ordering::Relaxed)
                    && matches!(e, MarketEvent::Trade(_)))
                .then(|| self.current_closes());
                let (seq, ups, rebuilding) = {
                    let mut core = self.0.core.lock().expect("core lock");
                    if let Some((symbol, price, _)) = &qualifying {
                        core.last_price.insert(symbol.clone(), *price);
                        if let MarketEvent::Trade(t) = &e
                            && let Some(s) = t.session
                        {
                            core.latest_trade_day = core.latest_trade_day.max(Some(s.date));
                        }
                    }
                    // Live: a symbol whose archived history is not in the engine yet is held
                    // back (crate::history), and the paper broker still gets the print below.
                    let held = crate::history::hold_unloaded(&mut core, &e);
                    // Stock context sees every print once, on arrival: a held one with its own
                    // verdict (it reaches the chart engine later, never the context again).
                    if held
                        && let (MarketEvent::Trade(t), Some(closes)) = (&e, context_closes.as_ref())
                        && let Some(cx) = core.context.as_mut()
                    {
                        cx.apply_trade(t, Counted::from_effect(t), closes);
                    }
                    if held {
                        // Alerts watch every live symbol, loaded into the engine or not.
                        if !core.rebuilding {
                            self.offer_alerts(&e, &[], None);
                        }
                        core.seq += 1;
                        (core.seq, Vec::new(), core.rebuilding)
                    } else {
                        let (ups, revision) = core.engine.apply_revising(&e);
                        // A rebuild re-reads history: nothing in it is news, so alerts skip it.
                        // try_send under the lock is a few atomics, never a wait.
                        if !core.rebuilding {
                            self.offer_alerts(&e, &ups, revision.as_ref());
                        }
                        // The quote board takes the engine's verdict on the print (its tape row comes
                        // first in `ups`), so it excludes exactly what the candles exclude; and for a
                        // correction or cancel, the engine's account of what it removed or replaced,
                        // so the board's volume and last follow the revised candles exactly.
                        if let MarketEvent::Trade(t) = &e
                            && let Some(ChartUpdate::Tape(row)) = ups.first()
                        {
                            let counted = Counted::from_tape(&row.counted);
                            core.quotes.apply_trade(t, counted);
                            core.pressure.apply_trade(t, counted);
                            if let (Some(closes), Some(cx)) =
                                (context_closes.as_ref(), core.context.as_mut())
                            {
                                cx.apply_trade(t, counted, closes);
                            }
                        }
                        for u in &ups {
                            if let ChartUpdate::Halt(h) = u {
                                core.pressure.apply_halt(&h.symbol, &h.halt);
                                if let Some(cx) = core.context.as_mut() {
                                    cx.set_halted(&h.symbol, h.halt.end.is_none());
                                }
                            }
                            // In order: a halt freezes the readings before the null band point
                            // that follows it clears the band (crate::pressure, "Halts").
                            if let ChartUpdate::Luld(l) = u {
                                core.pressure.apply_luld(&l.symbol, &l.point);
                            }
                        }
                        if let Some(rev) = &revision {
                            core.quotes.apply_revision(rev);
                        }
                        core.seq += 1;
                        let seq = core.seq;
                        (
                            seq,
                            Self::serialize_unless_rebuilding(&mut core, ups),
                            core.rebuilding,
                        )
                    }
                };
                if !rebuilding {
                    self.send_all(seq, ups);
                    // After the tape row went out, so a client sees the print before its fill.
                    if let Some((symbol, price, t)) = qualifying {
                        let mut paper = self.0.paper.lock().expect("paper lock");
                        let out = paper.on_trade(&symbol, price, t);
                        if out.changed {
                            self.publish_paper(&paper, &out.fills);
                        }
                    }
                }
            }
            FeedItem::Advance(t) => {
                let (seq, ups, rebuilding) = {
                    let mut core = self.0.core.lock().expect("core lock");
                    let ups = core.engine.advance_clock(t);
                    // The clock moves the bands too (the doubling windows, the close).
                    for u in &ups {
                        if let ChartUpdate::Luld(l) = u {
                            core.pressure.apply_luld(&l.symbol, &l.point);
                        }
                    }
                    if let Some(cx) = core.context.as_mut() {
                        cx.advance(t);
                    }
                    core.seq += 1;
                    let seq = core.seq;
                    (
                        seq,
                        Self::serialize_unless_rebuilding(&mut core, ups),
                        core.rebuilding,
                    )
                };
                if !rebuilding {
                    self.send_all(seq, ups);
                }
            }
            FeedItem::Reset(_) => {
                // The visible clock stays where it was until the rebuild's closing `Clock`, so a
                // client never sees the target time over a half-rebuilt engine.
                {
                    let closes = self.current_closes();
                    // A seek rebuilds stock context from the re-read day, like the candles.
                    let board = self.new_context_board();
                    let mut core = self.0.core.lock().expect("core lock");
                    let seq = core.seq + 1;
                    *core = Core::new(closes);
                    core.context = board;
                    core.seq = seq;
                    core.rebuilding = true;
                }
                // Time travel resets the account (contract). Announced now rather than at the
                // rebuild's end: the flat account is already the truth, and a command arriving
                // mid-rebuild then works on it.
                let mut paper = self.0.paper.lock().expect("paper lock");
                paper.reset();
                self.publish_paper(&paper, &[]);
            }
            FeedItem::Clock(c) => {
                let frame = clock_frame(&c);
                let now = c.now;
                *self.0.clock.lock().expect("clock lock") = Some(c);
                let (seq, resync) = {
                    let mut core = self.0.core.lock().expect("core lock");
                    if let Some(cx) = core.context.as_mut() {
                        cx.advance(now);
                    }
                    let resync = std::mem::take(&mut core.rebuilding);
                    if resync {
                        // Every subscriber takes a fresh quotes.snapshot on the Resync below, and
                        // the rows the rebuild changed are in it: drop them rather than resend
                        // them all in the next update.
                        let band = self.0.band;
                        core.quotes.mark_all_sent(band);
                    }
                    (core.seq, resync)
                };
                let _ = self.0.tx.send(Arc::new(Outbound {
                    seq,
                    kind: OutKind::Clock(frame),
                }));
                if resync {
                    let _ = self.0.tx.send(Arc::new(Outbound {
                        seq,
                        kind: OutKind::Resync,
                    }));
                }
            }
            FeedItem::Source(src) => {
                // A day switch. Only the description changes here; the `Reset` right behind it
                // drops the old day's derived state and the closing `Clock` resyncs every client.
                tracing::info!(day = %src.day, source = %src.label, symbols = src.symbols.len(), "replay switched day");
                // Before the `Reset` behind it builds the new day's board: a day imported since
                // the last check gets the closes file that came with it.
                self.refresh_archive_meta();
                // Same reason, same precedent: the new day's evidence is loaded before its board
                // is built and seeded from it (only with STOCK_CONTEXT; crate::evidence).
                self.load_evidence(Some(src.day));
                *self.0.info.lock().expect("info lock") =
                    Some(Arc::new(ProviderInfo::replay(&src)));
            }
            FeedItem::Error(e) => {
                tracing::error!(error = %e, "market data provider error");
                *self.0.status.lock().expect("status lock") = ProviderStatus {
                    state: "error",
                    detail: Some(e),
                };
            }
            FeedItem::Link(l) => {
                // Live only. "reconnecting" is distinct from "error": the source is retrying on
                // its own, and clients keep what they have rather than falling back to a fixture.
                let state = match l.state {
                    LinkState::Connecting => "starting",
                    LinkState::Up => "running",
                    LinkState::Down => "reconnecting",
                };
                match l.state {
                    LinkState::Down => {
                        tracing::warn!(detail = ?l.detail, "live market data disconnected")
                    }
                    _ => tracing::info!(state, detail = ?l.detail, "live market data link"),
                }
                *self.0.status.lock().expect("status lock") = ProviderStatus {
                    state,
                    detail: l.detail,
                };
            }
        }
    }

    /// [`Market::serialize`], except during a rebuild, where nothing is broadcast: then only the
    /// tape rows are serialized (for the per-symbol history the next snapshot sends) and the
    /// candle increments are dropped unserialized. Same state either way; it only makes a rebuild
    /// cheaper (a seek, a day switch, and above all the live journal's restore of a whole-market
    /// day, where serializing every increment was about a third of the time).
    fn serialize_unless_rebuilding(core: &mut Core, ups: Vec<ChartUpdate>) -> Vec<OutKind> {
        if !core.rebuilding {
            return Self::serialize(core, ups);
        }
        for u in &ups {
            if let ChartUpdate::Tape(t) = u {
                let json = serde_json::to_string(u).expect("wire types serialize");
                core.record_tape(&t.symbol, json);
            }
        }
        Vec::new()
    }

    /// Serializes increments once and records tape rows in the per-symbol history.
    fn serialize(core: &mut Core, ups: Vec<ChartUpdate>) -> Vec<OutKind> {
        ups.into_iter()
            .map(|u| {
                let json = serde_json::to_string(&u).expect("wire types serialize");
                if let ChartUpdate::Tape(t) = &u {
                    core.record_tape(&t.symbol, json.clone());
                }
                // Tape rows, `chart.halt` and `chart.luld` have no timeframe, so every
                // subscriber of the symbol gets them whatever timeframe it watches.
                let (symbol, timeframe) = (u.symbol().to_string(), u.timeframe());
                OutKind::Chart {
                    symbol,
                    timeframe,
                    update: Box::new(u),
                    json,
                }
            })
            .collect()
    }

    fn send_all(&self, seq: u64, items: Vec<OutKind>) {
        for kind in items {
            // No receivers is fine (nobody connected).
            let _ = self.0.tx.send(Arc::new(Outbound { seq, kind }));
        }
    }

    /// Broadcasts the current `paper.state`, then one `paper.fill` per fill. Called with the
    /// paper lock held, so frames from the ingest task and from commands go out in `seq` order.
    fn publish_paper(&self, paper: &Broker, fills: &[Fill]) {
        let mut frames =
            vec![serde_json::to_string(&paper.state()).expect("paper state serializes")];
        frames.extend(
            fills
                .iter()
                .map(|f| serde_json::to_string(&PaperFill::from(f)).expect("fill serializes")),
        );
        for frame in frames {
            let _ = self.0.tx.send(Arc::new(Outbound {
                seq: 0,
                kind: OutKind::Paper(frame),
            }));
        }
    }

    /// The current `paper.state` frame (sent to a connection right after `welcome`).
    pub fn paper_state_frame(&self) -> String {
        let paper = self.0.paper.lock().expect("paper lock");
        serde_json::to_string(&paper.state()).expect("paper state serializes")
    }

    /// The broker's notion of "now" for commands: the virtual (replay) or wall (live) clock the
    /// provider publishes, in epoch ms. 0 before any clock exists (an idle market).
    fn paper_now(&self) -> i64 {
        self.clock().map_or(0, |c| c.now.millis())
    }

    /// `order.place`. Always changes the state (a failing order is created `rejected`), so this
    /// always broadcasts. Returns the new order's id.
    pub fn paper_place(&self, cmd: PlaceCommand) -> String {
        let order = cmd.into_new_order();
        // Looked up before taking the paper lock: `known` needs the core lock, and the two are
        // never nested. "Known" is the full symbol list, candidates or not.
        let known = {
            let core = self.0.core.lock().expect("core lock");
            self.known(&core, &order.symbol)
        };
        let now = self.paper_now();
        let mut paper = self.0.paper.lock().expect("paper lock");
        let id = paper.place(order, now, |_| known);
        self.publish_paper(&paper, &[]);
        id
    }

    /// `order.modify`. On error nothing changed and nothing is broadcast.
    pub fn paper_modify(&self, cmd: ModifyCommand) -> Result<(), CommandError> {
        let (id, m) = cmd.into_modify();
        let now = self.paper_now();
        let mut paper = self.0.paper.lock().expect("paper lock");
        paper.modify(&id, m, now)?;
        self.publish_paper(&paper, &[]);
        Ok(())
    }

    /// `order.cancel`. On error nothing changed and nothing is broadcast.
    pub fn paper_cancel(&self, id: &str) -> Result<(), CommandError> {
        let now = self.paper_now();
        let mut paper = self.0.paper.lock().expect("paper lock");
        paper.cancel(id, now)?;
        self.publish_paper(&paper, &[]);
        Ok(())
    }

    /// `paper.reset`.
    pub fn paper_reset(&self) {
        let mut paper = self.0.paper.lock().expect("paper lock");
        paper.reset();
        self.publish_paper(&paper, &[]);
    }

    /// A `chart.snapshot` (with the clock) plus recent tape rows, consistent with `seq`.
    pub fn snapshot(
        &self,
        symbol: &str,
        tf: Timeframe,
        bin: Price,
    ) -> Result<SnapshotOut, SnapshotError> {
        let clock = self.clock().map(|c| clock_info(&c));
        let mut core = self.0.core.lock().expect("core lock");
        if self.known(&core, symbol) && !Self::candidate_in(&core, self.0.band, symbol) {
            return Err(SnapshotError::OutsidePriceBand);
        }
        // Live: a symbol whose history is still in the archive gets an empty chart now and its
        // load jumps the queue; the `Loaded` broadcast then sends this connection the real one.
        crate::history::request_load(self, &mut core, symbol);
        let mut snap = match core.engine.snapshot(symbol, tf, Some(bin)) {
            Some(s) => s,
            None if !self.known(&core, symbol) => {
                return Err(SnapshotError::UnknownSymbol);
            }
            None => ChartSnapshot {
                kind: "chart.snapshot".into(),
                symbol: symbol.to_string(),
                timeframe: tf,
                session: None,
                candles: Vec::new(),
                gaps: Vec::new(),
                profile: None,
                quality: DataQuality::default(),
                clock: None,
                halts: Vec::new(),
                luld: Vec::new(),
                prev_close: None,
            },
        };
        snap.clock = clock;
        snap.prev_close = self.prev_close_for(&core, &snap).map(Price::to_f64);
        Ok(SnapshotOut {
            seq: core.seq,
            json: serde_json::to_string(&snap).expect("snapshot serializes"),
            tape: core
                .tape
                .get(symbol)
                .map(|r| r.iter().cloned().collect())
                .unwrap_or_default(),
        })
    }

    /// The previous close for a chart snapshot: the closes file's latest date before the symbol's
    /// current session date, the quote board's rule and input (its row's trading date is the date
    /// of the latest session the symbol traded in, which is the engine's current session), so the
    /// header and the search row show the same change. A symbol that has not traded yet has no
    /// session; then the replay day stands in (live mode: none, the header falls back too). That
    /// is the CURRENT provider info, which a day switch replaces on `FeedItem::Source`: a day
    /// remembered from startup would hand the new day's chart the old day's previous close.
    ///
    /// Looked up in the board's own closes index, so a reload of the file reaches the chart and
    /// the board together.
    fn prev_close_for(&self, core: &Core, snap: &ChartSnapshot) -> Option<Price> {
        let day = snap
            .session
            .as_ref()
            .and_then(|s| s.id.parse::<SessionId>().ok())
            .map(|s| s.date)
            .or_else(|| self.info().and_then(|i| i.day))?;
        core.quotes
            .closes()
            .before(&snap.symbol, day)
            .map(|(_, p)| p)
    }

    /// The increment's JSON at another profile bin width: the touched bins re-binned, with
    /// absolute totals read from the engine now.
    pub fn rebin(&self, update: &ChartUpdate, json: &str, bin: Price) -> String {
        if bin == self.0.bin {
            return json.to_string();
        }
        let (session, rows) = match update {
            ChartUpdate::Candle(c) => (&c.session, &c.profile),
            ChartUpdate::Remove(c) => (&c.session, &c.profile),
            ChartUpdate::Gap(c) => (&c.session, &c.profile),
            ChartUpdate::Tape(_) | ChartUpdate::Halt(_) | ChartUpdate::Luld(_) => {
                return json.to_string();
            }
        };
        let mut touched: Vec<Price> = rows
            .iter()
            .filter_map(|r| Price::from_f64_rounded(r.p))
            .map(|p| p.floor_to(bin))
            .collect();
        touched.sort();
        touched.dedup();
        let symbol = update.symbol();
        let all = session
            .parse::<SessionId>()
            .ok()
            .and_then(|s| {
                let core = self.0.core.lock().expect("core lock");
                core.engine.profile_rows(symbol, s, bin)
            })
            .unwrap_or_default();
        let new_rows: Vec<chart_engine::wire::ProfileRow> = touched
            .iter()
            .map(|b| chart_engine::wire::ProfileRow {
                p: b.to_f64(),
                v: all.iter().find(|(p, _)| p == b).map_or(0, |(_, v)| *v),
            })
            .collect();
        let mut u = update.clone();
        match &mut u {
            ChartUpdate::Candle(c) => {
                c.profile_bin = bin.to_f64();
                c.profile = new_rows;
            }
            ChartUpdate::Remove(c) => {
                c.profile_bin = bin.to_f64();
                c.profile = new_rows;
            }
            ChartUpdate::Gap(c) => {
                c.profile_bin = bin.to_f64();
                c.profile = new_rows;
            }
            ChartUpdate::Tape(_) | ChartUpdate::Halt(_) | ChartUpdate::Luld(_) => {}
        }
        serde_json::to_string(&u).expect("wire types serialize")
    }

    /// Applies a `replay.control` action. Replay mode only.
    pub async fn control(
        &self,
        action: &str,
        value: Option<&Value>,
    ) -> Result<ClockState, ControlError> {
        let Some(handle) = &self.0.control else {
            return Err(ControlError::NotReplay);
        };
        let cmd = parse_control(action, value).map_err(ControlError::BadRequest)?;
        // Only days the catalog offers: the driver could open any archive day with a manifest,
        // but the app must not be able to reach one the menu would never show.
        if let (Command::Day(day), Some(days)) = (&cmd, &self.0.days)
            && !days.contains(*day)
        {
            return Err(ControlError::UnknownDay(day.to_string()));
        }
        handle.command(cmd).await.map_err(|e| match e {
            ReplayCommandError::Rejected(m) => ControlError::Rejected(m),
            ReplayCommandError::Stopped => ControlError::Failed(e.to_string()),
        })
    }

    /// The replayable days (empty in live mode and for sessions without an archive).
    pub fn days(&self) -> Vec<DayEntry> {
        self.0
            .days
            .as_ref()
            .map(DayCatalog::list)
            .unwrap_or_default()
    }

    /// Registers a `quotes.subscribe`; the count drops again when the guard is dropped (on
    /// `quotes.unsubscribe`, or when the connection ends, however it ends).
    pub fn quotes_subscribe(&self) -> QuotesGuard {
        self.0.quote_subs.fetch_add(1, Ordering::Relaxed);
        QuotesGuard(self.0.clone())
    }

    /// Connections currently subscribed to quotes.
    pub fn quote_subscribers(&self) -> usize {
        self.0.quote_subs.load(Ordering::Relaxed)
    }

    /// The board's rows inside the band as a `quotes.snapshot` frame, with the engine seq it is
    /// consistent with (a connection then forwards only `quotes.update`s with a larger seq).
    /// The rows are copied under the core lock; the JSON is built after it is released.
    pub fn quotes_snapshot(&self) -> (u64, String) {
        let t = self.clock().map(|c| c.now.millis());
        let (seq, rows) = {
            let core = self.0.core.lock().expect("core lock");
            (core.seq, core.quotes.snapshot(self.0.band))
        };
        (seq, self.quotes_frame("quotes.snapshot", t, &rows, &[]))
    }

    /// One tick of the quotes task: takes the changed rows and, when anyone is subscribed and
    /// something changed, broadcasts one `quotes.update`. Returns whether it broadcast. Public so
    /// tests can tick deterministically instead of waiting on wall time.
    ///
    /// The take happens even with no subscriber (it then only clears the dirty set): the board's
    /// record of which rows the clients hold must follow every take, or the first update a later
    /// subscriber gets could miss a `gone`.
    pub fn publish_quotes(&self) -> bool {
        let t = self.clock().map(|c| c.now.millis());
        let (seq, changes) = {
            let mut core = self.0.core.lock().expect("core lock");
            if core.rebuilding {
                return false;
            }
            let band = self.0.band;
            (core.seq, core.quotes.take_changes(band))
        };
        if changes.is_empty() || self.quote_subscribers() == 0 {
            return false;
        }
        let QuoteChanges { rows, gone } = changes;
        let frame = self.quotes_frame("quotes.update", t, &rows, &gone);
        let _ = self.0.tx.send(Arc::new(Outbound {
            seq,
            kind: OutKind::Quotes(frame),
        }));
        true
    }

    /// Registers a `pressure.subscribe` (the quotes guard's twin).
    pub fn pressure_subscribe(&self) -> PressureGuard {
        self.0.pressure_subs.fetch_add(1, Ordering::Relaxed);
        PressureGuard(self.0.clone())
    }

    /// Connections currently subscribed to the pressure board.
    pub fn pressure_subscribers(&self) -> usize {
        self.0.pressure_subs.load(Ordering::Relaxed)
    }

    /// Registers a `ross.subscribe` (the pressure guard's twin).
    pub fn ross_subscribe(&self) -> RossGuard {
        self.0.ross_subs.fetch_add(1, Ordering::Relaxed);
        RossGuard(self.0.clone())
    }

    /// The Ross Panel as a `ross.snapshot` frame (`enabled: false` with `STOCK_CONTEXT` off), with
    /// the engine seq it is consistent with.
    pub fn ross_snapshot(&self) -> (u64, String) {
        let core = self.0.core.lock().expect("core lock");
        let readiness = self.0.readiness.lock().expect("readiness lock").clone();
        let frame = match core.context.as_ref() {
            Some(cx) => cx.panel_frame("ross.snapshot", readiness.as_ref()),
            None => crate::context::disabled_frame(),
        };
        (core.seq, frame)
    }

    /// A symbol's latest stock-context decision as a `context.decision` frame.
    pub fn context_decision(&self, symbol: &str) -> String {
        let core = self.0.core.lock().expect("core lock");
        let cx = core.context.as_ref();
        crate::context::decision_frame(symbol, cx.and_then(|c| c.decision(symbol)), cx.is_some())
    }

    /// Drains the context engine's bounded output journals (panel snapshots, setup log), from the
    /// quotes task, **whatever any client is doing**.
    ///
    /// The engine appends and its caller takes; nothing used to take the setup log at all, and
    /// panels were only taken inside `publish_context`, which returns early with no `ross`
    /// subscriber. An unattended live market therefore grew both without bound. The drain is
    /// in-memory under the core lock (no disk, no provider, no serialization), and the engine's
    /// counters say what it had to drop.
    ///
    /// Each drained snapshot is also captured for the baseline observation/latency scaffolding
    /// (`crate::observe`): a bounded `try_send` under the lock, absorbed into the summary once the
    /// lock is released. Returns what this call took.
    pub(crate) fn drain_context(&self) -> crate::context::DrainReport {
        self.drain_context_at(&mut crate::observe::wall_now)
    }

    /// [`Market::drain_context`] with the stage clock injected, so a test can check **where** it
    /// is read.
    ///
    /// `clock` is read once per drain, with the core lock already held. That is the whole point:
    /// a sample taken before `core.lock()` leaves the time spent waiting for the lock out of
    /// barrier-to-drain and under-reports the segment exactly when contention makes it worth
    /// measuring. One read covers the drain because the whole journal is taken inside a single
    /// critical section, microseconds apart.
    pub(crate) fn drain_context_at(
        &self,
        clock: &mut dyn FnMut() -> i64,
    ) -> crate::context::DrainReport {
        let sink = &self.0.observations;
        let (took, total, tel) = {
            let mut core = self.0.core.lock().expect("core lock");
            let Some(cx) = core.context.as_mut() else {
                return Default::default();
            };
            let now = clock();
            let took = cx.drain_outputs(&mut |p| {
                sink.capture(crate::observe::Observation::of(
                    crate::observe::Stage::Drained,
                    p,
                    now,
                ));
            });
            (took, cx.drained(), cx.output_telemetry())
        };
        // Off the core lock: the queue is moved into the summary here, never under it.
        sink.absorb();
        if !took.is_empty() {
            tracing::debug!(
                panels = took.panels,
                setup_entries = took.setup_entries,
                panels_total = total.panels,
                setup_entries_total = total.setup_entries,
                complete = tel.complete(),
                "stock context outputs drained"
            );
        }
        if !tel.complete() {
            // Not a silent loss: a run whose journal is incomplete says so.
            tracing::warn!(
                panels_dropped = tel.panels_dropped,
                setup_entries_dropped = tel.setup_entries_dropped,
                panel_log_max = tel.panel_log_max,
                setup_log_max = tel.setup_log_max,
                "stock context outputs: the engine dropped outputs at its bound; the drained journal is incomplete"
            );
        }
        took
    }

    /// What the baseline observation/latency capture has recorded so far (`crate::observe`).
    /// Segments with too few samples report `insufficient_data`, never a percentile.
    pub(crate) fn observation_report(&self) -> crate::observe::ObservationReport {
        self.0.observations.report()
    }

    /// Logs the observation capture's counters and segments, from the quotes task about once a
    /// minute. A segment under its sample floor prints `insufficient_data` rather than a number,
    /// and `complete` says whether anything was dropped: this is capture scaffolding for the
    /// agreed measurement program, never a latency guarantee or a comparison with anything.
    pub(crate) fn log_observations(&self) {
        let r = self.observation_report();
        let w = self.warmup_stats();
        let readiness = self.readiness();
        if r.captured == 0 && w.candidates == 0 && readiness.is_none() {
            return;
        }
        tracing::info!(
            readiness = readiness
                .as_ref()
                .map(|x| x.summary_line())
                .unwrap_or_else(|| "not taken yet".into()),
            warmup_candidates = w.candidates,
            warmup_queued = w.queued,
            warmup_filed = w.filed,
            warmup_dropped_queue_full = w.full,
            warmup_flush_errors = w.flush_errors,
            captured = r.captured,
            dropped = r.dropped,
            absorbed = r.absorbed,
            queue_capacity = r.queue_capacity,
            untimed_replay = r.untimed_replay,
            complete = r.complete(),
            any_segment_measured =
                r.barrier_to_drain.measured() || r.barrier_to_publish.measured(),
            barrier_to_drain = %r.barrier_to_drain.line(),
            barrier_to_publish = %r.barrier_to_publish.line(),
            "stock context observations (capture scaffolding, not a latency guarantee)"
        );
    }

    /// One `ross.update` when the panel changed and anyone is subscribed (from the quotes task,
    /// at most once per wall second). Skipped during a rebuild, like the other boards.
    pub fn publish_context(&self) {
        if self.0.ross_subs.load(Ordering::Relaxed) == 0 {
            return;
        }
        let (seq, frame, snap) = {
            let mut core = self.0.core.lock().expect("core lock");
            if core.rebuilding {
                return;
            }
            let seq = core.seq;
            let readiness = self.0.readiness.lock().expect("readiness lock").clone();
            match core
                .context
                .as_mut()
                .and_then(|c| c.take_update(readiness.as_ref()))
            {
                Some((frame, snap)) => (seq, frame, snap),
                None => return,
            }
        };
        // The publication time of the snapshot this frame carries, for `crate::observe`: sampled
        // here, with the frame already serialized and the core lock released, so the segment ends
        // at **this server's own broadcast**. It says nothing about network transport, a client's
        // receipt or what a browser rendered; those are separate segments nothing measures yet.
        self.0.observations.capture(crate::observe::Observation::of(
            crate::observe::Stage::Published,
            &snap,
            crate::observe::wall_now(),
        ));
        let _ = self.0.tx.send(Arc::new(Outbound {
            seq,
            kind: OutKind::Ross(frame),
        }));
    }

    /// The last ranked board as a `pressure.snapshot` frame, with the engine seq it is consistent
    /// with. Empty until the first ranking (at most a second after start or a rebuild).
    pub fn pressure_snapshot(&self) -> (u64, String) {
        let t = self.clock().map(|c| c.now.millis());
        let (seq, cards) = {
            let core = self.0.core.lock().expect("core lock");
            (core.seq, core.pressure_cards.clone())
        };
        (seq, pressure_frame("pressure.snapshot", t, &cards))
    }

    /// One ranking of the pressure board ([`PressureBoard::tick`] at the market clock), and, when
    /// the cards changed and anyone is subscribed, one `pressure.update`. Ranks even with no
    /// subscriber (module docs). Returns whether it broadcast. Public so tests can tick
    /// deterministically.
    pub fn publish_pressure(&self) -> bool {
        let Some(clock) = self.clock() else {
            return false;
        };
        let band = self.0.band;
        let (seq, cards) = {
            let mut core = self.0.core.lock().expect("core lock");
            if core.rebuilding {
                return false;
            }
            let Core {
                pressure,
                last_price,
                quotes,
                ..
            } = &mut *core;
            let cards: Vec<PressureCardWire> = pressure
                .tick(clock.now.0, |s| {
                    last_price.get(s).is_some_and(|p| band.contains(*p))
                })
                .iter()
                .map(|c| PressureCardWire::new(c, quotes, last_price))
                .collect();
            // Price alerts on pressure (crate::alerts): the watched symbols' scores, on or off
            // the board, at the same clock. try_send, as on the ingest path.
            if let Some(sink) = self.0.alerts.get() {
                let session = market_events::classify(clock.now).map(|s| (s.date, s.kind));
                for symbol in sink.pressure_watch().iter() {
                    if let Some(sc) = pressure.score_of(symbol, clock.now.0) {
                        sink.offer(crate::alerts::eval::Input::Pressure {
                            symbol: symbol.clone(),
                            pressure: sc.pressure,
                            bull: sc.bull(),
                            ts: clock.now.millis(),
                            session,
                        });
                    }
                }
            }
            if cards == core.pressure_cards {
                return false;
            }
            core.pressure_cards = cards.clone();
            (core.seq, cards)
        };
        if self.pressure_subscribers() == 0 {
            return false;
        }
        let frame = pressure_frame("pressure.update", Some(clock.now.millis()), &cards);
        let _ = self.0.tx.send(Arc::new(Outbound {
            seq,
            kind: OutKind::Pressure(frame),
        }));
        true
    }

    /// `{"type", "t", "quotes": [row...], "gone"?: [symbol...]}`; the row shape is [`WireQuote`].
    fn quotes_frame(
        &self,
        kind: &'static str,
        t: Option<i64>,
        rows: &[QuoteRow],
        gone: &[Arc<str>],
    ) -> String {
        let assets = self.0.assets.lock().expect("assets lock").clone();
        let quotes: Vec<WireQuote<'_>> = rows
            .iter()
            .map(|r| {
                // Names only where the client cannot have them yet: every snapshot row, and an
                // update row whose symbol just entered the band. Never invented: a symbol missing
                // from assets.json has no name and the client shows the symbol.
                let meta = if r.entering {
                    assets.get(&*r.symbol)
                } else {
                    None
                };
                WireQuote {
                    s: &r.symbol,
                    p: r.last.to_f64(),
                    o: r.reference.to_f64(),
                    // Static for the day: every snapshot row and entering row, and an update row
                    // only when its trading date moved on and the reference with it.
                    pc: (r.entering || r.prev_close_changed)
                        .then_some(r.prev_close)
                        .flatten()
                        .map(Price::to_f64),
                    v: r.volume,
                    n: r.trades,
                    pre: (!r.regular_open).then_some(true),
                    name: meta.map(|m| m.name.as_str()).filter(|n| !n.is_empty()),
                    x: meta.map(|m| m.exchange.as_str()).filter(|x| !x.is_empty()),
                }
            })
            .collect();
        let gone: Vec<&str> = gone.iter().map(|s| &**s).collect();
        serde_json::to_string(&QuotesMessage {
            kind,
            t,
            quotes,
            gone,
        })
        .expect("quotes serialize")
    }

    /// `GET /api/replay/state`.
    ///
    /// `days`: every day `replay.control {action: "day"}` accepts, ascending, as
    /// `{day: "YYYY-MM-DD", fixture, symbols}` (empty when the day cannot be switched: live mode).
    /// It is here rather than on the socket because it changes a few times a month and only the
    /// day menu needs it; the clients fetch this route when the menu opens. `day` is the day
    /// being replayed now.
    pub fn state_json(&self) -> Value {
        let clock = self.clock();
        let info = self.info();
        let info = info.as_deref();
        let seq = self.0.core.lock().expect("core lock").seq;
        json!({
            "mode": self.0.mode.as_str(),
            "status": *self.0.status.lock().expect("status lock"),
            "source": info.map(|i| i.label.clone()),
            "day": info.and_then(|i| i.day).map(|d| d.to_string()),
            "fixture": info.is_some_and(|i| i.fixture),
            "symbols": self.symbols(),
            "controllable": self.0.control.is_some(),
            "clock": clock.as_ref().map(clock_info),
            "start": clock.as_ref().and_then(|c| c.start).map(UnixNanos::millis),
            "end": clock.as_ref().and_then(|c| c.end).map(UnixNanos::millis),
            "finished": clock.as_ref().is_some_and(|c| c.finished),
            "events": clock.as_ref().map_or(0, |c| c.events),
            "seq": seq,
            "days": self.days(),
            "session": crate::history::session_json(self),
            "history": crate::history::history_json(self),
        })
    }

    /// `GET /api/symbols`. Live: `session` names the trading day the board and charts show
    /// (`{day, current, provisional}`; `current: false` is the last session, shown while the
    /// market is closed), or null before any.
    pub fn symbols_json(&self) -> Value {
        let info = self.info();
        let info = info.as_deref();
        json!({
            "mode": self.0.mode.as_str(),
            "status": self.0.status.lock().expect("status lock").state,
            "source": info.map(|i| i.label.clone()),
            "day": info.and_then(|i| i.day).map(|d| d.to_string()),
            "fixture": info.is_some_and(|i| i.fixture),
            "symbols": self.symbols(),
            "priceBand": {"min": self.0.band.min.to_string(), "max": self.0.band.max.to_string()},
            "screened": self.screened(),
            "session": crate::history::session_json(self),
        })
    }

    /// How many known symbols were screened and how many are candidates.
    fn screened(&self) -> Value {
        let core = self.0.core.lock().expect("core lock");
        let all = self.all_symbols(&core);
        let candidates = all
            .iter()
            .filter(|s| Self::candidate_in(&core, self.0.band, s))
            .count();
        json!({"total": all.len(), "candidates": candidates})
    }
}

/// Decrements the quotes subscriber count when dropped.
pub struct QuotesGuard(Arc<Inner>);

impl Drop for QuotesGuard {
    fn drop(&mut self) {
        self.0.quote_subs.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Decrements the Ross Panel subscriber count when dropped.
pub struct RossGuard(Arc<Inner>);

impl Drop for RossGuard {
    fn drop(&mut self) {
        self.0.ross_subs.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Decrements the pressure subscriber count when dropped.
pub struct PressureGuard(Arc<Inner>);

impl Drop for PressureGuard {
    fn drop(&mut self) {
        self.0.pressure_subs.fetch_sub(1, Ordering::Relaxed);
    }
}

/// One Halt watcher card on the wire (`pressure.snapshot` / `pressure.update`):
///
/// - `s` symbol; `p` last price, `pc` previous close and `o` the fallback reference, exactly the
///   quote row's (`WireQuote`), so the card's change % is the search's and the chart header's;
/// - `risk` 0-100, the halt-risk score the gauge fills with (`crate::pressure`, "Halt risk");
///   `side` (`bull` | `bear`) the side the build-up pushes: buying toward the upper LULD band,
///   selling toward the lower;
/// - `rise` the build-up: that side's pressure now minus a horizon earlier (points, −100…100),
///   and `riseSecs` the horizon (`crate::pressure::HORIZON`, 30);
/// - `band` the modelled LULD band on that side (dollars) and `toBand` the distance to it in
///   percent of the price (two decimals; 0 at or past the band);
/// - `pressure` 0-100, the flow pressure itself, with `buyShare` (0-1, two decimals) and `pace`
///   (multiple of the symbol's average volume per minute so far this session, one decimal), its
///   inputs, for a tooltip;
/// - `halt` `{start, reason}` while halted (`start` epoch ms, `reason` the provider's, e.g.
///   `LUDP`); every reading above is then the one frozen at the halt.
///
/// Rounded so a board that did not visibly change compares equal and sends nothing.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PressureCardWire {
    s: String,
    p: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pc: Option<f64>,
    o: f64,
    risk: u8,
    side: &'static str,
    rise: i16,
    #[serde(rename = "riseSecs")]
    rise_secs: u8,
    band: f64,
    #[serde(rename = "toBand")]
    to_band: f64,
    pressure: u8,
    #[serde(rename = "buyShare")]
    buy_share: f64,
    pace: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    halt: Option<HaltInfo>,
}

impl PressureCardWire {
    fn new(c: &Card, quotes: &QuoteBoard, last_price: &HashMap<String, Price>) -> Self {
        let row = quotes.get(&c.symbol);
        // A ranked symbol is in the band, so it has a price; the quote row normally has it too.
        let p = row
            .as_ref()
            .map(|r| r.last)
            .or_else(|| last_price.get(&*c.symbol).copied())
            .unwrap_or(Price::ZERO);
        PressureCardWire {
            s: c.symbol.to_string(),
            p: p.to_f64(),
            pc: row.as_ref().and_then(|r| r.prev_close).map(Price::to_f64),
            o: row.as_ref().map_or(p, |r| r.reference).to_f64(),
            risk: c.risk.risk,
            side: if c.risk.up { "bull" } else { "bear" },
            rise: c.risk.rise,
            rise_secs: (crate::pressure::HORIZON / 1_000_000_000) as u8,
            band: c.risk.band.to_f64(),
            to_band: (c.risk.to_band * 10_000.0).round() / 100.0,
            pressure: c.score.pressure,
            buy_share: (c.score.buy_share * 100.0).round() / 100.0,
            pace: (c.score.pace * 10.0).round() / 10.0,
            halt: c.halt.clone(),
        }
    }
}

/// `pressure.snapshot` / `pressure.update`: `{"type", "t", "board": [card...]}`, the whole board
/// in display order either way (six cards at most, so an update replaces rather than patches).
fn pressure_frame(kind: &'static str, t: Option<i64>, cards: &[PressureCardWire]) -> String {
    #[derive(Serialize)]
    struct PressureMessage<'a> {
        #[serde(rename = "type")]
        kind: &'static str,
        t: Option<i64>,
        board: &'a [PressureCardWire],
    }
    serde_json::to_string(&PressureMessage {
        kind,
        t,
        board: cards,
    })
    .expect("pressure serializes")
}

/// `quotes.snapshot` / `quotes.update`.
#[derive(Serialize)]
struct QuotesMessage<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    /// The replay (or live) clock in epoch ms when the rows were taken; null before any clock.
    t: Option<i64>,
    quotes: Vec<WireQuote<'a>>,
    /// Update only: symbols that left the price band (the client drops them).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    gone: Vec<&'a str>,
}

/// One quote row. Compact keys, because a snapshot carries every candidate (thousands):
///
/// - `s` symbol; `p` last price; `pc` the previous trading day's official close, the change-%
///   reference whenever present (from `daily-closes.json`; sent in snapshots, for rows entering the
///   band, and in an update only when it changes, i.e. the row's trading date moved on: clients
///   keep the last one they got); `o` the fallback reference when there is no `pc` (the regular
///   open, or the day's first price while `pre` is set). Prices are JSON numbers in dollars like
///   every chart wire price (`Price::to_f64`, exact for whole cents and the sub-penny ticks below
///   $1); `/api/symbols`' `priceBand` uses decimal strings because it is configuration, not data;
/// - `v` day volume (shares) and `n` prints counted, by the candles' volume rule;
/// - `pre: true` only while the symbol has no print at/after 09:30 yet (omitted otherwise); it
///   describes `o`, so it only matters to a client without `pc`;
/// - `name` / `x` (listing exchange) only in snapshots and for update rows entering the band, and
///   only when `assets.json` has them.
#[derive(Serialize)]
struct WireQuote<'a> {
    s: &'a str,
    p: f64,
    o: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pc: Option<f64>,
    v: u64,
    n: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pre: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    x: Option<&'a str>,
}

/// `assets.json` from the archive root. Optional: a missing or unreadable file leaves names empty
/// (logged), it never stops the server.
fn load_assets(archive: &Archive) -> HashMap<String, AssetMeta> {
    match archive.read_assets() {
        Ok(Some(m)) => {
            tracing::info!(assets = m.len(), path = %archive.assets_path().display(), "asset metadata loaded");
            m.into_iter().collect()
        }
        Ok(None) => {
            tracing::warn!(path = %archive.assets_path().display(), "no assets.json: ticker names fall back to symbols (`swimport alpaca-assets` writes it)");
            HashMap::new()
        }
        Err(e) => {
            tracing::warn!(error = %e, "assets.json unreadable: ticker names fall back to symbols");
            HashMap::new()
        }
    }
}

/// `daily-closes.json` from the archive root, indexed per symbol. Optional like `assets.json`: a
/// missing or unreadable file leaves every row on the open fallback (logged), it never stops the
/// server.
fn load_closes(archive: &Archive) -> PrevCloses {
    let path = archive.daily_closes_path();
    match archive.read_daily_closes() {
        Ok(Some(m)) => {
            let (idx, dropped) = PrevCloses::new(&m);
            tracing::info!(
                days = m.len(),
                first = ?m.keys().next(),
                last = ?m.keys().next_back(),
                symbols = idx.symbols(),
                dropped,
                path = %path.display(),
                "daily closes loaded: change % is against the previous close"
            );
            idx
        }
        Ok(None) => {
            tracing::warn!(path = %path.display(), "no daily-closes.json: change % falls back to the open (`swimport alpaca-daily-closes` writes it)");
            PrevCloses::default()
        }
        Err(e) => {
            tracing::warn!(error = %e, "daily-closes.json unreadable: change % falls back to the open");
            PrevCloses::default()
        }
    }
}

fn clock_frame(c: &ClockState) -> String {
    serde_json::to_string(&ClockMessage {
        kind: "replay.clock",
        clock: &clock_info(c),
    })
    .expect("clock serializes")
}

/// `replay.control {action, value}` → a replay command.
///
/// - `play`, `pause`, `step` (one event): no value;
/// - `stepInterval`: value = timeframe label (`"15s"`, `"30s"`, `"1m"`, `"5m"`; default `"1m"`);
/// - `seek`: value = epoch ms (number) or RFC 3339 string;
/// - `speed`: value = multiplier number (`0` = max) or `"max"` / `"10x"`;
/// - `day`: value = `"YYYY-MM-DD"`, one of `/api/replay/state`'s `days`. Rebuilds like a seek on
///   that day, at the configured `REPLAY_START` time of day.
pub fn parse_control(action: &str, value: Option<&Value>) -> Result<Command, String> {
    Ok(match action {
        "play" => Command::Play,
        "pause" => Command::Pause,
        "step" => Command::Step,
        "stepInterval" => {
            let tf = match value {
                None | Some(Value::Null) => Timeframe::M1,
                Some(Value::String(s)) => s.parse()?,
                Some(other) => {
                    return Err(format!(
                        "stepInterval value must be a timeframe label, got {other}"
                    ));
                }
            };
            Command::StepInterval(tf.nanos())
        }
        "seek" => {
            let t = match value {
                Some(Value::Number(n)) => n
                    .as_i64()
                    .or_else(|| n.as_f64().map(|f| f as i64))
                    .map(UnixNanos::from_millis)
                    .ok_or("seek value must be epoch milliseconds")?,
                Some(Value::String(s)) => chrono::DateTime::parse_from_rfc3339(s)
                    .ok()
                    .and_then(|d| d.timestamp_nanos_opt())
                    .map(UnixNanos)
                    .ok_or_else(|| format!("invalid seek time {s:?}"))?,
                _ => return Err("seek needs a value (epoch ms or RFC 3339)".into()),
            };
            Command::Seek(t)
        }
        "speed" => Command::Speed(match value {
            Some(Value::Number(n)) => Speed::from_wire(n.as_f64().unwrap_or(f64::NAN))?,
            Some(Value::String(s)) => Speed::parse(s)?,
            _ => return Err("speed needs a value (multiplier, 0 = max)".into()),
        }),
        "day" => match value {
            Some(Value::String(s)) => Command::Day(
                NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
                    .map_err(|_| format!("day value must be YYYY-MM-DD, got {s:?}"))?,
            ),
            _ => return Err("day needs a value (\"YYYY-MM-DD\")".into()),
        },
        other => return Err(format!("unknown replay action {other:?}")),
    })
}

async fn ingest(mut items: mpsc::Receiver<FeedItem>, market: Market, sim: Option<Sim>) {
    while let Some(item) = items.recv().await {
        if let Some(s) = &sim {
            s.ingest(&item);
        }
        market.apply(item);
    }
    tracing::warn!("market data provider stream ended");
    let mut st = market.0.status.lock().expect("status lock");
    if st.state == "running" {
        *st = ProviderStatus {
            state: "stopped",
            detail: None,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The drain's stage clock is read with the core lock already held.
    ///
    /// Codex's review of the first slice caught the opposite: the sample was taken before
    /// `core.lock()`, so the wait for the lock fell outside barrier-to-drain and the segment
    /// under-reported under contention. The clock closure here proves where the read happens —
    /// `Mutex::try_lock` from the thread that already holds the guard cannot succeed, so a
    /// re-hoisted sample fails this test with no contention to arrange.
    #[test]
    fn the_drain_stage_clock_is_read_under_the_core_lock() {
        let market = Market::idle(ClockMode::Replay, "test", None);
        market.set_stock_context(true);
        assert!(
            market.0.core.lock().expect("core lock").context.is_some(),
            "the default band is the pillar policy's band, so a board exists"
        );
        let mut reads = 0u32;
        let mut clock = || {
            reads += 1;
            assert!(
                market.0.core.try_lock().is_err(),
                "the stage time must be read under the core lock, not before it"
            );
            0
        };
        let took = market.drain_context_at(&mut clock);
        assert!(took.is_empty(), "an idle market published nothing yet");
        assert_eq!(reads, 1, "read once per drain, inside the critical section");

        // And with no board at all the clock is not read: nothing was drained to stamp.
        market.set_stock_context(false);
        let mut never = || panic!("no board, no stage time");
        assert!(market.drain_context_at(&mut never).is_empty());
    }

    /// Warmup requests are fair across ticks: a first batch that never resolves does not block
    /// the candidates behind it.
    ///
    /// Codex's review of the warmup slice caught the opposite. `missing_evidence` took the first
    /// `WARMUP_SCAN_MAX` candidates in the engine's stable symbol order and the requester only
    /// discarded the already-asked ones *afterwards*, so with no evidence ever arriving the
    /// alphabetically first 200 were re-selected on every tick and nothing after them was ever
    /// requested. This drives the production path — `Market::request_warmups` — over more
    /// candidates than the budget, with an unresolved first batch, across several ticks.
    #[test]
    fn warmup_requests_move_past_an_unresolved_first_batch() {
        use market_events::{Price, Trade, UnixNanos};
        let tmp = tempfile::tempdir().unwrap();
        let a = Archive::new(market_archive::ArchiveConfig {
            root: tmp.path().join("archive"),
            retention_days: 0,
            min_free_bytes: 0,
        });
        let day = NaiveDate::from_ymd_opt(2026, 10, 16).expect("valid");
        let market = Market::idle(ClockMode::Replay, "test", None);
        market.set_stock_context(true);
        // A zero period loads once, synchronously, and spawns nothing: safe outside a runtime.
        market.init_evidence(
            a.clone(),
            Price::from_cents(2500),
            std::time::Duration::ZERO,
        );
        market.load_evidence(Some(day));

        // 250 symbols trading in the band with no stored evidence of any kind, so every one of
        // them is a candidate and none of them can ever resolve here.
        let n = crate::evidence::WARMUP_SCAN_MAX + 50;
        let open = market_events::session::et(day, 9, 30, 0, 0)
            .expect("valid")
            .nanos();
        {
            let mut core = market.0.core.lock().expect("core lock");
            let cx = core.context.as_mut().expect("a board");
            let closes = market_archive::PrevCloses::default();
            let counted = crate::quotes::Counted {
                ohlc: true,
                volume: true,
            };
            for k in 0..n {
                let sym = format!("S{k:04}");
                for j in 0..3u64 {
                    let ts =
                        UnixNanos(open + (k as i64) * 1_000_000_000 + (j as i64) * 100_000_000);
                    let tr = Trade::new(&sym, ts, Price::from_cents(300), 10_000, j);
                    cx.apply_trade(&tr, counted, &closes);
                }
            }
            cx.advance(UnixNanos(open + (n as i64 + 60) * 1_000_000_000));
        }

        // Tick one: the budget's worth, and no more.
        market.request_warmups();
        let first = a.pending_warmups(day, 10_000).expect("inbox");
        assert_eq!(
            first.len(),
            crate::evidence::WARMUP_SCAN_MAX,
            "the first tick files its budget"
        );

        // Tick two, with nothing resolved: the symbols behind the first batch are asked for.
        market.request_warmups();
        let second = a.pending_warmups(day, 10_000).expect("inbox");
        assert_eq!(
            second.len(),
            n,
            "every candidate is asked for by the second tick"
        );
        let before: std::collections::BTreeSet<&str> =
            first.iter().map(|r| r.symbol.as_str()).collect();
        let added: Vec<&str> = second
            .iter()
            .map(|r| r.symbol.as_str())
            .filter(|s| !before.contains(s))
            .collect();
        assert_eq!(
            added.len(),
            50,
            "the tail, not the same batch again: {added:?}"
        );
        assert!(
            added.contains(&format!("S{:04}", n - 1).as_str()),
            "including the last one: {added:?}"
        );

        // Tick three: everything is known, so nothing new is filed and nothing is re-asked.
        market.request_warmups();
        assert_eq!(a.pending_warmups(day, 10_000).expect("inbox").len(), n);
        let w = market.warmup_stats();
        assert_eq!(w.filed, n as u64, "each symbol filed once: {w:?}");
        assert_eq!(w.full, 0, "the queue was never overrun: {w:?}");
        assert_eq!(w.flush_errors, 0, "{w:?}");
        assert_eq!(w.tracked, n, "one dedup entry per symbol, for one date");
    }

    /// The dedup state is per session date and rolls: a long-running market does not accumulate
    /// an entry per symbol and date for its lifetime.
    #[test]
    fn the_warmup_dedup_state_rolls_with_the_session_date() {
        use market_providers::warmup::{Enqueue, WarmupQueue};
        let q = WarmupQueue::new(1_000);
        let now = Utc::now();
        let d1 = NaiveDate::from_ymd_opt(2026, 10, 15).expect("valid");
        let d2 = NaiveDate::from_ymd_opt(2026, 10, 16).expect("valid");
        for k in 0..100 {
            q.request(&format!("S{k:03}"), d1, "test", now);
        }
        assert_eq!(q.tracked(), 100);
        assert_eq!(q.rolled(), 0);

        // The date rolls: the old day's dedup goes, and the same symbol is askable again.
        assert_eq!(q.request("S000", d2, "test", now), Enqueue::Queued);
        assert_eq!(q.tracked(), 1, "only the new date is tracked");
        assert_eq!(q.rolled(), 1, "and it says what it dropped");
        assert_eq!(q.request("S000", d2, "test", now), Enqueue::Known);
    }

    #[test]
    fn a_halt_watcher_card_on_the_wire() {
        use crate::pressure::{Risk, Score};
        let card = Card {
            symbol: Arc::from("AAA"),
            score: Score {
                pressure: 72,
                buy_share: 0.876,
                pace: 4.26,
            },
            risk: Risk {
                risk: 81,
                rise: 45,
                up: true,
                band: Price::from_cents(550),
                to_band: 0.012_345,
                moved: 0.02,
                secs_to_band: Some(18.5),
                half_width: 0.1,
            },
            halt: None,
        };
        let mut last = HashMap::new();
        last.insert("AAA".to_string(), Price::from_cents(543));
        let w = PressureCardWire::new(&card, &QuoteBoard::new(), &last);
        assert_eq!(
            serde_json::to_value(&w).unwrap(),
            json!({
                "s": "AAA", "p": 5.43, "o": 5.43,
                "risk": 81, "side": "bull", "rise": 45, "riseSecs": 30,
                "band": 5.5, "toBand": 1.23,
                "pressure": 72, "buyShare": 0.88, "pace": 4.3
            })
        );
    }

    #[test]
    fn control_parsing() {
        assert_eq!(parse_control("play", None), Ok(Command::Play));
        assert_eq!(
            parse_control("speed", Some(&json!(0))),
            Ok(Command::Speed(Speed::Max))
        );
        assert_eq!(
            parse_control("speed", Some(&json!("60x"))),
            Ok(Command::Speed(Speed::Times(60)))
        );
        assert_eq!(
            parse_control("stepInterval", Some(&json!("15s"))),
            Ok(Command::StepInterval(15_000_000_000))
        );
        assert_eq!(
            parse_control("seek", Some(&json!(1_792_072_800_000_i64))),
            Ok(Command::Seek(UnixNanos::from_millis(1_792_072_800_000)))
        );
        assert!(parse_control("speed", Some(&json!(1.5))).is_err());
        assert_eq!(
            parse_control("day", Some(&json!("2026-10-14"))),
            Ok(Command::Day(NaiveDate::from_ymd_opt(2026, 10, 14).unwrap()))
        );
        assert!(parse_control("day", Some(&json!("10/14/2026"))).is_err());
        assert!(parse_control("day", Some(&json!(20261014))).is_err());
        assert!(parse_control("day", None).is_err());
        assert!(parse_control("rewind", None).is_err());
    }
}
