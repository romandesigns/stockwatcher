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

use crate::quotes::{Counted, QuoteBoard, QuoteChanges, QuoteRow};
use chart_engine::wire::{ChartSnapshot, ChartUpdate, ClockInfo};
use chart_engine::{ChartEngine, DataQuality, Timeframe};
use chrono::{NaiveDate, Utc};
use chrono_tz::America::New_York;
use market_archive::fixture::{DEFAULT_SEED, DEFAULT_SYMBOLS, write_fixture_day};
use market_archive::{Archive, AssetMeta, PrevCloses};
use market_events::{MarketEvent, Price, PriceBand, SessionId, UnixNanos};
use market_providers::{
    ClockMode, ClockState, Command, FeedItem, MarketDataMode, ProviderConfig, ProviderError,
    ProviderInfo, ReplayHandle, Speed,
};
use paper_broker::wire::{ModifyCommand, PaperFill, PlaceCommand};
use paper_broker::{Broker, CommandError, Fill};
use provider_sim::{Sim, SimConfig};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};

/// Recent tape rows kept per symbol and sent after a snapshot (same as the browser fixture).
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
#[derive(Clone, Debug)]
pub struct MarketConfig {
    pub provider: ProviderConfig,
    pub sim: SimConfig,
    pub autofixture: bool,
    pub fixture_day: NaiveDate,
    pub fixture_symbols: Vec<String>,
    /// Only stocks whose latest price-qualifying trade is inside this band are let through.
    pub band: PriceBand,
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
        if let Some(v) = var("SIM_AUTOFIXTURE") {
            c.autofixture = matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on");
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
}

struct Core {
    engine: ChartEngine,
    seq: u64,
    tape: HashMap<String, VecDeque<String>>,
    /// Latest price-qualifying trade per symbol at the engine clock; drives the price band.
    last_price: HashMap<String, Price>,
    /// Last / open / volume per symbol for the ticker search (`quotes.*`).
    quotes: QuoteBoard,
    rebuilding: bool,
}

impl Core {
    fn new(closes: Arc<PrevCloses>) -> Self {
        Core {
            engine: ChartEngine::default(),
            seq: 0,
            tape: HashMap::new(),
            last_price: HashMap::new(),
            quotes: QuoteBoard::with_closes(closes),
            rebuilding: false,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ProviderStatus {
    /// `starting`, `running`, `no_data`, `not_configured`, `error`, `stopped`, `idle`.
    pub state: &'static str,
    pub detail: Option<String>,
}

struct Inner {
    mode: ClockMode,
    info: Option<ProviderInfo>,
    status: Mutex<ProviderStatus>,
    core: Mutex<Core>,
    clock: Mutex<Option<ClockState>>,
    tx: broadcast::Sender<Arc<Outbound>>,
    control: Option<ReplayHandle>,
    bin: Price,
    band: PriceBand,
    /// The one paper account (single-user app). Never locked while `core` is held, or the other
    /// way round, so the two locks cannot deadlock.
    paper: Mutex<Broker>,
    /// `assets.json` (name, exchange), read once at startup; empty when the file is missing.
    assets: HashMap<String, AssetMeta>,
    /// `daily-closes.json` indexed per symbol, read once at startup; empty when the file is
    /// missing (change % then falls back to the open everywhere). Shared with every rebuild's board.
    closes: Arc<PrevCloses>,
    /// Connections that sent `quotes.subscribe`. While 0, the quotes task builds nothing.
    quote_subs: AtomicUsize,
}

/// Shared market state; cheap to clone.
#[derive(Clone)]
pub struct Market(Arc<Inner>);

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
        assets: HashMap<String, AssetMeta>,
        closes: Arc<PrevCloses>,
    ) -> Self {
        Market(Arc::new(Inner {
            mode,
            info,
            status: Mutex::new(status),
            core: Mutex::new(Core::new(closes.clone())),
            clock: Mutex::new(None),
            tx: broadcast::channel(BROADCAST_CAPACITY).0,
            control,
            bin: ChartEngine::default().config().profile_bin,
            band,
            paper: Mutex::new(Broker::new()),
            assets,
            closes,
            quote_subs: AtomicUsize::new(0),
        }))
    }

    /// No provider (tests, or a provider that could not start).
    pub fn idle(mode: ClockMode, state: &'static str, detail: Option<String>) -> Self {
        Market::build(
            mode,
            None,
            ProviderStatus { state, detail },
            None,
            PriceBand::DEFAULT,
            HashMap::new(),
            Arc::default(),
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
        let market = Market::build(
            mode,
            Some(session.info.clone()),
            ProviderStatus {
                state: "running",
                detail: None,
            },
            session.control.clone(),
            cfg.band,
            assets,
            closes,
        );
        tokio::spawn(ingest(session.items, market.clone(), sim.clone()));
        market.spawn_quotes_task();
        (market, sim)
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
            assets,
            Arc::new(closes),
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
            loop {
                tick.tick().await;
                let Some(inner) = weak.upgrade() else { break };
                Market(inner).publish_quotes();
            }
        });
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

    fn known(&self, core: &Core, symbol: &str) -> bool {
        self.0
            .info
            .as_ref()
            .is_some_and(|i| i.symbols.iter().any(|s| s == symbol))
            || core.engine.symbols().any(|s| s == symbol)
    }

    /// Every symbol the provider knows, sorted, whether or not it is a candidate.
    fn all_symbols(&self, core: &Core) -> Vec<String> {
        let mut v: Vec<String> = self
            .0
            .info
            .as_ref()
            .map(|i| i.symbols.clone())
            .unwrap_or_default();
        v.extend(core.engine.symbols().map(String::from));
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
                let (seq, ups, rebuilding) = {
                    let mut core = self.0.core.lock().expect("core lock");
                    if let Some((symbol, price, _)) = &qualifying {
                        core.last_price.insert(symbol.clone(), *price);
                    }
                    let (ups, revision) = core.engine.apply_revising(&e);
                    // The quote board takes the engine's verdict on the print (its tape row comes
                    // first in `ups`), so it excludes exactly what the candles exclude; and for a
                    // correction or cancel, the engine's account of what it removed or replaced,
                    // so the board's volume and last follow the revised candles exactly.
                    if let MarketEvent::Trade(t) = &e
                        && let Some(ChartUpdate::Tape(row)) = ups.first()
                    {
                        core.quotes.apply_trade(t, Counted::from_tape(&row.counted));
                    }
                    if let Some(rev) = &revision {
                        core.quotes.apply_revision(rev);
                    }
                    core.seq += 1;
                    let seq = core.seq;
                    (seq, self.serialize(&mut core, ups), core.rebuilding)
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
                    core.seq += 1;
                    let seq = core.seq;
                    (seq, self.serialize(&mut core, ups), core.rebuilding)
                };
                if !rebuilding {
                    self.send_all(seq, ups);
                }
            }
            FeedItem::Reset(_) => {
                // The visible clock stays where it was until the rebuild's closing `Clock`, so a
                // client never sees the target time over a half-rebuilt engine.
                {
                    let mut core = self.0.core.lock().expect("core lock");
                    let seq = core.seq + 1;
                    *core = Core::new(self.0.closes.clone());
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
                *self.0.clock.lock().expect("clock lock") = Some(c);
                let (seq, resync) = {
                    let mut core = self.0.core.lock().expect("core lock");
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
            FeedItem::Error(e) => {
                tracing::error!(error = %e, "market data provider error");
                *self.0.status.lock().expect("status lock") = ProviderStatus {
                    state: "error",
                    detail: Some(e),
                };
            }
        }
    }

    /// Serializes increments once and records tape rows in the per-symbol history.
    fn serialize(&self, core: &mut Core, ups: Vec<ChartUpdate>) -> Vec<OutKind> {
        ups.into_iter()
            .map(|u| {
                let json = serde_json::to_string(&u).expect("wire types serialize");
                if let ChartUpdate::Tape(t) = &u {
                    let ring = core.tape.entry(t.symbol.clone()).or_default();
                    if ring.len() == TAPE_HISTORY {
                        ring.pop_front();
                    }
                    ring.push_back(json.clone());
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
        let core = self.0.core.lock().expect("core lock");
        if self.known(&core, symbol) && !Self::candidate_in(&core, self.0.band, symbol) {
            return Err(SnapshotError::OutsidePriceBand);
        }
        let mut snap = match core.engine.snapshot(symbol, tf, Some(bin)) {
            Some(s) => s,
            None if self.known(&core, symbol) => ChartSnapshot {
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
            None => return Err(SnapshotError::UnknownSymbol),
        };
        snap.clock = clock;
        snap.prev_close = self.prev_close_for(&snap).map(Price::to_f64);
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
    /// session; then the replay day stands in (live mode: none, the header falls back too).
    fn prev_close_for(&self, snap: &ChartSnapshot) -> Option<Price> {
        let day = snap
            .session
            .as_ref()
            .and_then(|s| s.id.parse::<SessionId>().ok())
            .map(|s| s.date)
            .or_else(|| self.0.info.as_ref().and_then(|i| i.day))?;
        self.0.closes.before(&snap.symbol, day).map(|(_, p)| p)
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
        handle.command(cmd).await.map_err(ControlError::Failed)
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

    /// `{"type", "t", "quotes": [row...], "gone"?: [symbol...]}`; the row shape is [`WireQuote`].
    fn quotes_frame(
        &self,
        kind: &'static str,
        t: Option<i64>,
        rows: &[QuoteRow],
        gone: &[Arc<str>],
    ) -> String {
        let quotes: Vec<WireQuote<'_>> = rows
            .iter()
            .map(|r| {
                // Names only where the client cannot have them yet: every snapshot row, and an
                // update row whose symbol just entered the band. Never invented: a symbol missing
                // from assets.json has no name and the client shows the symbol.
                let meta = if r.entering {
                    self.0.assets.get(&*r.symbol)
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
    pub fn state_json(&self) -> Value {
        let clock = self.clock();
        let info = self.0.info.as_ref();
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
        })
    }

    /// `GET /api/symbols`.
    pub fn symbols_json(&self) -> Value {
        let info = self.0.info.as_ref();
        json!({
            "mode": self.0.mode.as_str(),
            "status": self.0.status.lock().expect("status lock").state,
            "source": info.map(|i| i.label.clone()),
            "day": info.and_then(|i| i.day).map(|d| d.to_string()),
            "fixture": info.is_some_and(|i| i.fixture),
            "symbols": self.symbols(),
            "priceBand": {"min": self.0.band.min.to_string(), "max": self.0.band.max.to_string()},
            "screened": self.screened(),
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
/// - `speed`: value = multiplier number (`0` = max) or `"max"` / `"10x"`.
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
        assert!(parse_control("rewind", None).is_err());
    }
}
