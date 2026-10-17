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

use chart_engine::wire::{ChartSnapshot, ChartUpdate, ClockInfo};
use chart_engine::{ChartEngine, DataQuality, Timeframe};
use chrono::{NaiveDate, Utc};
use chrono_tz::America::New_York;
use market_archive::Archive;
use market_archive::fixture::{DEFAULT_SEED, DEFAULT_SYMBOLS, write_fixture_day};
use market_events::{MarketEvent, Price, PriceBand, SessionId, UnixNanos, trade_effect};
use market_providers::{
    ClockMode, ClockState, Command, FeedItem, MarketDataMode, ProviderConfig, ProviderError,
    ProviderInfo, ReplayHandle, Speed,
};
use provider_sim::{Sim, SimConfig};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, mpsc};

/// Recent tape rows kept per symbol and sent after a snapshot (same as the browser fixture).
pub const TAPE_HISTORY: usize = 40;
/// Broadcast capacity: how far one client may fall behind before it is resynchronized.
pub const BROADCAST_CAPACITY: usize = 16_384;

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
}

struct Core {
    engine: ChartEngine,
    seq: u64,
    tape: HashMap<String, VecDeque<String>>,
    /// Latest price-qualifying trade per symbol at the engine clock; drives the price band.
    last_price: HashMap<String, Price>,
    rebuilding: bool,
}

impl Core {
    fn new() -> Self {
        Core {
            engine: ChartEngine::default(),
            seq: 0,
            tape: HashMap::new(),
            last_price: HashMap::new(),
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
    ) -> Self {
        Market(Arc::new(Inner {
            mode,
            info,
            status: Mutex::new(status),
            core: Mutex::new(Core::new()),
            clock: Mutex::new(None),
            tx: broadcast::channel(BROADCAST_CAPACITY).0,
            control,
            bin: ChartEngine::default().config().profile_bin,
            band,
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
        );
        tokio::spawn(ingest(session.items, market.clone(), sim.clone()));
        (market, sim)
    }

    /// Starts on an already-running provider session (tests).
    pub fn from_session(session: market_providers::ProviderSession, sim: Option<Sim>) -> Market {
        let market = Market::build(
            session.info.mode,
            Some(session.info.clone()),
            ProviderStatus {
                state: "running",
                detail: None,
            },
            session.control.clone(),
            PriceBand::DEFAULT,
        );
        tokio::spawn(ingest(session.items, market.clone(), sim));
        market
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
                let (seq, ups, rebuilding) = {
                    let mut core = self.0.core.lock().expect("core lock");
                    // Price-band input: the same price rule the candles use (a print that may
                    // not set OHLC, e.g. an odd lot or average-price report, cannot move a stock
                    // into or out of the candidate set).
                    if let MarketEvent::Trade(t) = &e
                        && trade_effect(&t.conditions, t.tape).updates_ohlc
                    {
                        core.last_price.insert(t.symbol.clone(), t.price);
                    }
                    let ups = core.engine.apply(&e);
                    core.seq += 1;
                    let seq = core.seq;
                    (seq, self.serialize(&mut core, ups), core.rebuilding)
                };
                if !rebuilding {
                    self.send_all(seq, ups);
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
                let mut core = self.0.core.lock().expect("core lock");
                let seq = core.seq + 1;
                *core = Core::new();
                core.seq = seq;
                core.rebuilding = true;
            }
            FeedItem::Clock(c) => {
                let frame = clock_frame(&c);
                *self.0.clock.lock().expect("clock lock") = Some(c);
                let (seq, resync) = {
                    let mut core = self.0.core.lock().expect("core lock");
                    let resync = std::mem::take(&mut core.rebuilding);
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
                let (symbol, timeframe) = match &u {
                    ChartUpdate::Candle(c) => (c.symbol.clone(), Some(c.timeframe)),
                    ChartUpdate::Remove(c) => (c.symbol.clone(), Some(c.timeframe)),
                    ChartUpdate::Gap(c) => (c.symbol.clone(), Some(c.timeframe)),
                    ChartUpdate::Tape(t) => {
                        let ring = core.tape.entry(t.symbol.clone()).or_default();
                        if ring.len() == TAPE_HISTORY {
                            ring.pop_front();
                        }
                        ring.push_back(json.clone());
                        (t.symbol.clone(), None)
                    }
                };
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
            },
            None => return Err(SnapshotError::UnknownSymbol),
        };
        snap.clock = clock;
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
            ChartUpdate::Tape(_) => return json.to_string(),
        };
        let mut touched: Vec<Price> = rows
            .iter()
            .filter_map(|r| Price::from_f64_rounded(r.p))
            .map(|p| p.floor_to(bin))
            .collect();
        touched.sort();
        touched.dedup();
        let symbol = match update {
            ChartUpdate::Candle(c) => &c.symbol,
            ChartUpdate::Remove(c) => &c.symbol,
            ChartUpdate::Gap(c) => &c.symbol,
            ChartUpdate::Tape(t) => &t.symbol,
        };
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
            ChartUpdate::Tape(_) => {}
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
