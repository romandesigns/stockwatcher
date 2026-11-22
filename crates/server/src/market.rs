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
//!
//! The movers (Top Gainers, Hot stocks Bull / Bear, [`crate::movers`]) are ranked from the same
//! quote board on the same tick, once for every client, and go out as ONE pre-serialized
//! `movers.update` with only the changed rows and day-line slices (`Outbound::seq` = the movers
//! generation). This replaced every dashboard holding a whole-board `quotes` subscription.
//!
//! Previous closes are put on the trading day's share basis with the splits the
//! corporate-actions ledger knows (`market_archive::PrevCloses::with_splits`, the ledger watched
//! like the closes file), and the quote board withholds a close the day plainly no longer trades
//! near (`crate::quotes::split_guard`), so a reverse split never reads as a +3,900 % gainer.

use crate::chart_context::{ContextFrame, ContextKey, ContextRegistry, LevelsKey};
use crate::context::ContextBoard;
use crate::days::{DayCatalog, DayEntry};
use crate::history::{History, Unloaded};
use crate::indicators::{ChartKey, IndicatorRegistry, Studies};
use crate::pressure::{Card, HaltInfo, PressureBoard};
use crate::quotes::{Counted, QuoteBoard, QuoteChanges, QuoteRow};
use chart_engine::luld::{LuldInputs, LuldParams, venue_code};
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
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};

/// Recent tape rows kept per symbol and sent after a snapshot.
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
/// How far past each wall-second boundary the once-a-second broadcast tick runs (quotes, panels,
/// the Runners frame). The Runners *decision* no longer waits for it ([`ROCKET_SETTLE`]).
pub const TICK_PHASE: Duration = Duration::from_millis(250);
/// Live: how long after a second ends the Runners decision for it runs (the rocket task,
/// `Market::spawn_rocket_task`). Every Runners path decides on whole seconds (a fast move at the
/// end of its second, a 15 s barrier, a burst boundary 1 s after it, a shelf slot 1 s after it),
/// so nothing can be decided before the second is over; this is only the grace for that second's
/// last prints to arrive from the SIP feed and be applied. It replaces the 250 ms
/// [`TICK_PHASE`] the decision used to share with the broadcasts.
///
/// What the shorter grace costs, measured on the 2026-10-22 live recording (`receive_ts`
/// against the end of each print's second, that day's busiest Runners symbols): a print
/// arrives 50–250 ms after its second ended in 0.06–0.29 % of prints, so 0.14–0.88 % of the
/// seconds a symbol traded in (SHFS 0.88 %, POM 0.35 %, TANH 0.26 %) are now read without one
/// print the 250 ms tick would have had (100 ms: 0.20–0.57 %; 200 ms: 0.06–0.12 %). Both miss
/// the 0.4–3.7 % of prints that arrive over 250 ms late. The fast rule reads the second once
/// (`FastSym::read`), so a late print can at most delay a fire to a later second or, rarely,
/// change one; the 15 s barriers, bursts and shelves wait a second or more and are unaffected.
pub const ROCKET_SETTLE: Duration = Duration::from_millis(50);
/// The least wall time between two Runners decision passes. Live they run once a second anyway;
/// this bounds a replay at speed (a new market second every 10 ms at 100×), so a pass takes the
/// core lock at most ~13 times a second.
pub const ROCKET_FLOOR: Duration = Duration::from_millis(75);
/// Replay, with no print to wake the rocket task: how often it looks at the virtual clock (a
/// paused replay stepped by `stepInterval`, a quiet stretch at speed). The 1 Hz tick it replaces
/// looked once a second.
const ROCKET_IDLE_POLL: Duration = Duration::from_millis(250);
/// Live: the provider's clock is a wall sample taken once a second; the Runners board's clock
/// carries it forward by at most this much (`Market::decision_now`), so a provider that stopped
/// ticking does not keep that clock running on its own for long.
const CLOCK_EXTRAPOLATION_MAX: Duration = Duration::from_secs(2);
/// Replay: a print at most this many seconds behind the newest print's second is a late
/// arrival and leaves the rocket task's "now" alone (`Market::wake_rocket`). A minute, the
/// tolerance the Runners board already applies to a print stamped ahead of its clock
/// (`RocketBoard::tick`).
const LATE_PRINT_WINDOW_S: i64 = 60;

/// The wait until the next tick: the first `k·QUOTES_INTERVAL + TICK_PHASE` (wall clock) that is
/// still ahead, at least 1 ms away.
fn until_next_tick() -> Duration {
    let period = QUOTES_INTERVAL.as_nanos();
    let phase = TICK_PHASE.as_nanos();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let into = (now + period - phase) % period; // how far past the last tick point we are
    let wait = (period - into).max(1_000_000);
    Duration::from_nanos(wait as u64)
}

#[cfg(test)]
mod tick_phase_tests {
    use super::*;

    #[test]
    fn the_next_tick_is_within_a_second_and_lands_on_the_phase() {
        let w = until_next_tick();
        assert!(w > Duration::ZERO && w <= QUOTES_INTERVAL, "{w:?}");
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            + w;
        let off = at.as_nanos() % QUOTES_INTERVAL.as_nanos();
        let want = TICK_PHASE.as_nanos();
        // A few ms of slack for the time between the two clock reads.
        assert!(
            off.abs_diff(want) < 20_000_000,
            "lands {off} ns past the second"
        );
    }
}

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
/// | `ROCKET_WATCH` | `1` | the EXPERIMENTAL Rocket watch panel (`crate::rocket`, `rocket.*`): read-only, O(1) per print, one pass over the active symbols per 15 s barrier; the live market keeps its forward log under `ARCHIVE_ROOT/rocket-watch/` |
/// | `ROCKET_ALERTS_PER_DAY` | `10` | Rocket watch's daily alert budget (the adaptive threshold's target and cap) |
/// | `RUNNERS_FAST` | `1` | with `ROCKET_WATCH`: the Runners "Fast move" entry (`crate::rocket::fast`: +8 % in 60 s or +12 % in 15 s, held, with real trading), its 1 s tapes (~3.8 KB per symbol printing in $0.05–$30, twice that with the batch filter's second tape) and its phone push; `0` turns it off |
/// | `FAST_BATCH_FILTER` | `shadow` | the Fast move's trade-report batch filter (`crate::rocket::fast::batch`: FINRA TRF late reports, venue D Form T, left out of a second tape after a detected TRF opening). `shadow` alerts as before and logs what the filtered tape would do (`fast_shadow`, `trf_batch` lines in the Runners log); `on` alerts from the filtered tape; `off` keeps no second tape |
/// | `ROCKET_THRESHOLD` | unset | a fixed Rocket watch threshold (overrides the scorer's frozen one), or `budget` to force the adaptive one |
/// | `RUNNERS_SCORER` | `r0` | the Runners strategy: `r0` (`precursors-R0`), `r2` (`precursors-R2`, LightGBM) or `placeholder` (`placeholder-b2`); `crate::rocket::ScorerChoice` |
/// | `BURST_ALERTS` | `1` | live market with `ROCKET_WATCH`: buying-burst rows in the Runners panel and their phone push (`crate::rocket::burst`, the research's V3-5 detector): O(1) per print, a 5 s check over the symbols that printed in the last 15 s; never on a replay |
/// | `BURST_ALERTS_PER_DAY` | `20` | the frozen threshold to alert at: `10`, `20` or `40` (the alert counts it gave a day on the research's DEV days) |
/// | `VOLUME_SHELVES` | `1` | with `ROCKET_WATCH`: volume shelves (`crate::zones`, the zones bake-off's winner) for every symbol with a confirmed +20 % run today, in the Runners frame's `zones` and on the chart; the live market keeps its forward log under `ARCHIVE_ROOT/zones/`. Its run trigger also feeds the Runners "Run +20%" entry (`crate::rocket::run`), so `0` turns that off too |
/// | `N1_LIVE` | `1` | live market with `ROCKET_WATCH`: the N1 "early leg" detector (`crate::n1_live`, Engine A T2-c6's entrant, X 8 % since T2-c4): its own notice kind `shadow.n1` beside production (push/desktop switch `n1`, on by default) and the Runners frame's `shadow` rows; `0` turns it off |
/// | `C4_LIVE` | `1` | live market with `ROCKET_WATCH`: Engine A T2-c4's additions (`crate::c4_live`): the F1u detector (`shadow.f1u`, switch `f1u`), the HALT add-on (`halt.bull`, switch `haltBull`), both on by default, and the POST<1 prune of Runners notices; `0` turns all three off |
/// | `BURST_THRESHOLD` | unset | a burst model probability threshold in (0, 1), overriding `BURST_ALERTS_PER_DAY`'s |
/// | `ADMISSION_JOURNAL` | `0` | live with a recorder: write the admission journal beside the recording (`crate::admit`; ~190–270 B per print, so off for whole-market SIP: see `Market::init_history`) |
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
    /// `DETECTION_PANEL` (off by default): independent experimental discovery, not Ross admission.
    pub detection_panel: bool,
    /// `ROCKET_WATCH` (on by default): the experimental Rocket watch (`crate::rocket`).
    pub rocket_watch: bool,
    /// `ROCKET_ALERTS_PER_DAY` / `ROCKET_THRESHOLD` (`crate::rocket::Policy`).
    pub rocket_policy: crate::rocket::Policy,
    /// `BURST_ALERTS` / `BURST_ALERTS_PER_DAY` / `BURST_THRESHOLD` (`crate::rocket::burst`).
    pub burst_policy: crate::rocket::burst::BurstPolicy,
    /// `VOLUME_SHELVES` (on by default, needs `ROCKET_WATCH`): `crate::zones`.
    pub volume_shelves: bool,
    /// `N1_LIVE` (on by default, needs `ROCKET_WATCH`, live market only): the N1 shadow detector
    /// (`crate::n1_live`); `0` is its kill switch.
    pub n1_live: bool,
    /// `C4_LIVE` (on by default, needs `ROCKET_WATCH`, live market only): Engine A T2-c4's F1u,
    /// HALT add-on and POST<1 prune (`crate::c4_live`); `0` is their kill switch.
    pub c4_live: bool,
    /// `ADMISSION_JOURNAL` (off by default): the live admission journal (`crate::admit`). Its
    /// size and the reader's record cap make it unusable for a whole-market day; see
    /// `Market::init_history`.
    pub admission_journal: bool,
    /// Optional locally provisioned causal reference JSONL; never fetched from a provider.
    pub detection_quiet_references: Option<std::path::PathBuf>,
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
            detection_panel: false,
            rocket_watch: true,
            rocket_policy: crate::rocket::Policy::default(),
            burst_policy: crate::rocket::burst::BurstPolicy::default(),
            volume_shelves: true,
            n1_live: true,
            c4_live: true,
            admission_journal: false,
            detection_quiet_references: None,
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
        if let Some(v) = var("DETECTION_PANEL") {
            c.detection_panel = on(&v);
        }
        if let Some(v) = var("ROCKET_WATCH") {
            c.rocket_watch = on(&v);
        }
        if let Some(v) = var("VOLUME_SHELVES") {
            c.volume_shelves = on(&v);
        }
        if let Some(v) = var("N1_LIVE") {
            c.n1_live = on(&v);
        }
        if let Some(v) = var("C4_LIVE") {
            c.c4_live = on(&v);
        }
        c.rocket_policy = crate::rocket::Policy::from_lookup(&get)?;
        c.burst_policy = crate::rocket::burst::BurstPolicy::from_lookup(&get)?;
        if let Some(v) = var("ADMISSION_JOURNAL") {
            c.admission_journal = on(&v);
        }
        c.detection_quiet_references = var("DETECTION_QUIET_REFERENCES").map(Into::into);
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
    /// A `chart.indicator` increment (or, after a deep revision, a `chart.indicators` snapshot)
    /// for one chart key (crate::indicators), published right after the chart frames of the
    /// apply that moved it, with the same seq. Forwarded once per connection that has a slot
    /// holding studies on that (symbol, timeframe, rth), whatever its profile bin.
    Indicator {
        symbol: String,
        timeframe: Timeframe,
        rth: bool,
        json: String,
    },
    /// A `chart.levels` frame (crate::chart_context, `levels.v1`) for one chart (symbol,
    /// timeframe): the whole state, broadcast when it changed, after the chart frames of the apply
    /// that changed it. Forwarded once per connection with a slot on that chart that asked for
    /// levels and has not already got this state in its own levels snapshot.
    Levels {
        key: crate::chart_context::LevelsKey,
        json: String,
    },
    /// A `chart.zones` frame (crate::chart_context, `zones.v1`) for one chart key: the whole set,
    /// broadcast when it changed. Forwarded like `Levels`, to slots that asked for zones.
    Zones { key: ChartKey, json: String },
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
    /// A pre-serialized `rocket.update` (the Rocket watch panel, `crate::rocket`) for the
    /// connections that sent `rocket.subscribe`. `Outbound::seq` is the engine seq when taken.
    Rocket(String),
    /// A pre-serialized `runners.event` (crate::time_marks) for the connections that sent
    /// `rocket.subscribe`, whatever their snapshot's seq: a toast is a moment, never contained in
    /// a snapshot, and the events memory is primed afresh for a first subscriber.
    RunnersEvent(String),
    /// A pre-serialized `favorites.update` (`crate::favorites`) for the connections that sent
    /// `favorites.subscribe`. `Outbound::seq` is the favorites generation, not the engine seq.
    Favorites(String),
    /// A pre-serialized `movers.update` (`crate::movers`) for the connections that sent
    /// `movers.subscribe`. `Outbound::seq` is the movers generation, not the engine seq.
    Movers(String),
    /// A pre-serialized `symbols.update` (`crate::symbols`) for the connections that sent
    /// `symbols.subscribe`. `Outbound::seq` is the list's `rev`, not the engine seq.
    Symbols(String),
    /// Full bounded replacement; its cursor is independent of the chart engine sequence.
    Detection {
        epoch: String,
        cursor: u64,
        frame: String,
    },
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
    /// Rocket watch (`ROCKET_WATCH`, `crate::rocket`): per-symbol 15 s state, alerts, outcomes.
    pub(crate) rocket: crate::rocket::RocketBoard,
    /// Volume shelves (`VOLUME_SHELVES`, `crate::zones`): fed the same prints as `rocket`,
    /// ticked just before it, published in its frame.
    pub(crate) zones: crate::zones::ZoneBoard,
    /// rw-mp5-1's per-bucket shares and its daily table (`crate::mp5_gate`): fed the same
    /// prints as `rocket`, ticked with it; off unless the live market runs the Detection Panel.
    pub(crate) mp5: crate::mp5_gate::Mp5Live,
    /// The chart indicators the connected charts asked for (crate::indicators), fed every
    /// engine increment under this lock. Unlike the boards above it survives a `Reset` (the
    /// slots' demand does), with its series cleared.
    pub(crate) indicators: IndicatorRegistry,
    /// The key levels, opening ranges and zones the connected charts (and the Runners mark) asked
    /// for (crate::chart_context). Survives a `Reset` like `indicators`, its values cleared.
    pub(crate) chart_context: ContextRegistry,
}

/// The LULD model's inputs from the archive metadata (`chart_engine::luld`): the previous
/// close, which picks the band percentage for the whole day under the Plan (`daily-closes.json`,
/// on the day's share basis), and the listing venue, whose opening print is the first reference
/// (`assets.json`). A snapshot of both files; [`Market::refresh_archive_meta`] hands the engine a
/// new one when either changes (it applies from the next session a symbol starts).
pub(crate) struct LuldMeta {
    assets: Arc<HashMap<String, AssetMeta>>,
    closes: Arc<PrevCloses>,
}

/// A close older than this is not "the previous trading day" (a symbol that stopped trading and
/// came back): the model falls back to the session's first reference instead.
const LULD_PREV_CLOSE_MAX_AGE_DAYS: i64 = 10;

impl LuldMeta {
    pub(crate) fn shared(
        assets: Arc<HashMap<String, AssetMeta>>,
        closes: Arc<PrevCloses>,
    ) -> Arc<dyn LuldInputs> {
        Arc::new(LuldMeta { assets, closes })
    }
}

impl std::fmt::Debug for LuldMeta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LuldMeta")
            .field("assets", &self.assets.len())
            .field("closes", &self.closes.symbols())
            .finish()
    }
}

impl LuldInputs for LuldMeta {
    fn params(&self, symbol: &str, date: NaiveDate) -> LuldParams {
        let close = self
            .closes
            .lookup(symbol, date)
            .filter(|c| (date - c.date).num_days() <= LULD_PREV_CLOSE_MAX_AGE_DAYS);
        LuldParams {
            prev_close: close.map(|c| c.price),
            // Not adjusted: the model checks it against the day's first print, the same guard
            // change % applies (`crate::quotes::split_guard`).
            prev_close_split_adjusted: close.is_some_and(|c| c.split_adjusted),
            listing_venue: self
                .assets
                .get(symbol)
                .and_then(|a| venue_code(&a.exchange))
                .map(str::to_string),
        }
    }
}

impl Core {
    fn new(closes: Arc<PrevCloses>, luld: Arc<dyn LuldInputs>) -> Self {
        let mut engine = ChartEngine::default();
        engine.set_luld_inputs(Some(luld));
        Core {
            engine,
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
            rocket: crate::rocket::RocketBoard::disabled(),
            zones: crate::zones::ZoneBoard::disabled(),
            mp5: crate::mp5_gate::Mp5Live::new(),
            indicators: IndicatorRegistry::default(),
            chart_context: ContextRegistry::default(),
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
    /// The live market's order route when the owner picked his Alpaca paper account
    /// (crate::trading, docs/architecture/alpaca-paper.md). While set, chart orders go to Alpaca
    /// paper and this market's `paper.state` is Alpaca's; the simulator keeps matching silently
    /// and comes back as it was when the route is cleared. Never set on the replay market
    /// ([`Market::set_paper_route`] refuses): replay orders are never routed to a broker.
    paper_route: std::sync::RwLock<Option<alpaca_paper::Service>>,
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
    /// The Halt watcher has ranked once: its gauges from then on are new (a push notice), not
    /// the board as found at start.
    pressure_primed: AtomicBool,
    detection_on: AtomicBool,
    detection_subs: AtomicUsize,
    pub(crate) detection: std::sync::OnceLock<Arc<crate::detection::Runtime>>,
    detection_published: Mutex<Option<(String, u64)>>,
    /// Acquired before core; orders off-core captures from publisher and ingest. Never held by
    /// a file writer, so serialization cannot overtake and discard an older drained transition.
    detection_delivery: Mutex<()>,
    detection_quiet_refs: Mutex<Arc<Vec<stock_context::QuietReference>>>,
    detection_directory: Mutex<Option<std::path::PathBuf>>,
    detection_error_recovery: AtomicBool,
    detection_link_down: AtomicBool,
    /// The last per-pillar readiness summary (`crate::context::Readiness`), taken on the evidence
    /// task and sent with every `ross` frame so an empty panel says why. A whole-market pass
    /// belongs nowhere near the publish path, so it is cached rather than computed per frame.
    pub(crate) readiness: Mutex<Option<crate::context::Readiness>>,
    /// Connections that sent `ross.subscribe`. While 0, no `ross.update` is serialized.
    ross_subs: AtomicUsize,
    /// The owner's favorites and what the last `favorites.update` carried (`crate::favorites`).
    /// Locked after the core lock, never before it.
    favorites: Mutex<crate::favorites::Board>,
    /// Connections that sent `favorites.subscribe`.
    favorites_subs: AtomicUsize,
    /// Top Gainers and Hot stocks Bull / Bear (`crate::movers`): the last ranking, the day lines the
    /// last update carried, the frames' generation. Locked after the core lock, never before it.
    movers: Mutex<crate::movers::Board>,
    /// Connections that sent `movers.subscribe`.
    movers_subs: AtomicUsize,
    /// The price-band candidate list the last `symbols.*` frame published (`crate::symbols`).
    /// Locked before core (inside `refresh_symbols`), never after it.
    symbols: Mutex<crate::symbols::Published>,
    /// Connections that sent `symbols.subscribe`.
    symbols_subs: AtomicUsize,
    /// `ROCKET_WATCH` and its policy: every `Core` this market makes gets a board built from them.
    rocket_on: AtomicBool,
    rocket_policy: Mutex<crate::rocket::Policy>,
    /// Connections that sent `rocket.subscribe`.
    rocket_subs: AtomicUsize,
    /// The body of the last `rocket.*` frame broadcast (an unchanged panel sends nothing).
    rocket_published: Mutex<Option<serde_json::Value>>,
    /// The Runners rows' day lines the last `rocket.update` carried (`crate::day_line`).
    rocket_lines: Mutex<crate::day_line::Published>,
    /// The Runners toasts' memory (`runners.event`, crate::time_marks): primed again whenever
    /// nobody is subscribed, so a first subscriber never gets the day's earlier rows.
    runner_events: Mutex<crate::time_marks::Events>,
    /// The Runners rows' automatic zones, recomputed once a minute per symbol
    /// (`crate::auto_zones`, the demand-zone mark).
    rocket_auto_zones: Mutex<crate::auto_zones::ZoneCache>,
    /// Live: the forward log (`crate::rocket::log`), set once at start.
    rocket_log: std::sync::OnceLock<Arc<crate::rocket::RocketLog>>,
    /// Wakes the rocket task (`Market::spawn_rocket_task`) when a print opens a new market
    /// second. Its own `Arc` so the task can wait on it holding only a weak reference to the
    /// market.
    rocket_wake: Arc<tokio::sync::Notify>,
    /// The market second (epoch s) of the newest applied print: the wake fires when it changes,
    /// once a second rather than once a print; a replay's rocket task also reads it as the
    /// market's "now" between clock frames. Not a plain running max: a seek resets it, and a
    /// replay print further back than a late arrival is followed down (`Market::wake_rocket`).
    rocket_print_sec: AtomicI64,
    /// Profile reads and finished candle frames shared by the connections watching a chart
    /// (crate::profile_frames). Locked on its own, never while core is held.
    frame_cache: Mutex<crate::profile_frames::FrameCache>,
    /// When `clock` was last set, for `Market::decision_now` (live extrapolation).
    clock_set: Mutex<Option<std::time::Instant>>,
    /// `VOLUME_SHELVES` (with `ROCKET_WATCH`): every `Core` gets an enabled `crate::zones` board.
    zones_on: AtomicBool,
    /// Live: the volume shelves' forward log (`ARCHIVE_ROOT/zones`), set once at start.
    zones_log: std::sync::OnceLock<Arc<crate::rocket::RocketLog>>,
    /// Live with bursts on: the archive the burst detector's ADV30 is read from, once a day
    /// (`crate::rocket::burst::facts`).
    burst_archive: std::sync::OnceLock<Archive>,
    /// The archive the N1 shadow reads its daily facts from (`crate::n1_live::load_facts`).
    n1_archive: std::sync::OnceLock<Archive>,
    /// The archive rw-mp5-1 reads its table from and writes its day files under
    /// (`crate::mp5_gate`).
    mp5_archive: std::sync::OnceLock<Archive>,
    /// The archive the Runners running entry's facts are read from, once a day
    /// (`crate::rocket::running::facts`); live and replay.
    running_archive: std::sync::OnceLock<Archive>,
    /// The day's cue transitions on the archive (`crate::oplog`, "Cue transitions"); live only.
    transition_log: std::sync::OnceLock<crate::oplog::DailyLog>,
    /// Live: the archive root whose notice ledger and journals a new context board reads back
    /// (`crate::notice_memory`, [`Market::init_notice_memory`]).
    notice_memory_root: std::sync::OnceLock<std::path::PathBuf>,
    /// Baseline observation/latency capture for the context panel (`crate::observe`): a bounded
    /// queue filled under the core lock and absorbed off it, with counted drops. Measurement
    /// scaffolding, not a latency claim.
    observations: crate::observe::ObservationSink,
    /// Shadow measurement capture (`crate::shadow`), set once at start when a context board
    /// exists. Filled under the core lock with a bounded `try_send`; folded and appended off it.
    pub(crate) shadow: std::sync::OnceLock<std::sync::Arc<crate::shadow::ShadowSink>>,
    /// The admission journal's bounded queue (`crate::admit`), set once at start for a live
    /// market with an archive to write into; `None` for replays and test sessions.
    ///
    /// What it is for: a restart has to be able to prove it replayed the same inputs in the same
    /// order this run admitted them, and a recording cannot answer that
    /// (`market_archive::admissions`). Filling it is one atomic and a bounded `try_send` under
    /// the core lock; a disk write never happens there.
    pub(crate) admissions: std::sync::OnceLock<Arc<crate::admit::AdmissionSink>>,
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

/// The archive root's `assets.json`, `daily-closes.json` and the corporate-actions ledger (the
/// splits the previous closes are adjusted by) as last read. The archive maintainer
/// (`swimport maintain`, a separate process) rewrites them after every session; checking costs
/// two `stat`s, and a file is re-read only when its stamp moved.
struct MetaFiles {
    archive: Archive,
    /// `[assets, closes, corporate-actions ledger]`.
    stamps: Mutex<[FileStamp; 3]>,
}

impl MetaFiles {
    /// Stamps taken BEFORE the first read: a rewrite between the stamp and the read is then seen
    /// as a change on the next check (one extra reload) rather than missed.
    fn new(archive: Archive) -> Self {
        let stamps = [
            file_stamp(&archive.assets_path()),
            file_stamp(&archive.daily_closes_path()),
            file_stamp(&ledger_path(&archive)),
        ];
        MetaFiles {
            archive,
            stamps: Mutex::new(stamps),
        }
    }

    /// Which files changed since the last call: `[assets, closes, ledger]`.
    fn changed(&self) -> [bool; 3] {
        let now = [
            file_stamp(&self.archive.assets_path()),
            file_stamp(&self.archive.daily_closes_path()),
            file_stamp(&ledger_path(&self.archive)),
        ];
        let mut last = self.stamps.lock().expect("meta stamps lock");
        let out = [now[0] != last[0], now[1] != last[1], now[2] != last[2]];
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

/// A snapshot's session schedule (`sessions.v1`) extended to the market clock's trading date
/// and the next one. The engine's schedule covers the dates its candles span, so a symbol with
/// none yet today (the empty chart a symbol gets before its first print or history load, or one
/// whose last candles are from an earlier day) carried no schedule for the day its increments
/// then fill: an early close there read as the fixed 09:30-16:00 all day, and the regular-hours
/// candles stopped lining up with the server's indicators. Unchanged when it already reaches
/// that date (or a later one: a weekend's next trading date).
pub fn schedule_through(
    sessions: Vec<chart_engine::wire::SessionInfo>,
    now: UnixNanos,
) -> Vec<chart_engine::wire::SessionInfo> {
    let today = market_events::session::trading_date(now);
    let date_of = |s: &chart_engine::wire::SessionInfo| {
        s.id.split('/')
            .next()
            .and_then(|d| d.parse::<NaiveDate>().ok())
    };
    if sessions.iter().filter_map(date_of).any(|d| d >= today) {
        return sessions;
    }
    let mut dates: Vec<NaiveDate> = sessions.iter().filter_map(date_of).collect();
    dates.push(today);
    chart_engine::session_schedule(dates.into_iter(), None)
}

pub fn clock_info(c: &ClockState) -> ClockInfo {
    ClockInfo {
        mode: c.mode.as_str().to_string(),
        now: c.now.millis(),
        now_ns: Some(c.now.0),
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
    /// The slot's `chart.indicators` at the same seq, when it asked for studies.
    pub indicators: Option<String>,
}

fn mode_of(cfg: &ProviderConfig) -> ClockMode {
    match cfg.mode {
        MarketDataMode::Replay => ClockMode::Replay,
        MarketDataMode::Live => ClockMode::Live,
    }
}

/// No provider has supplied a run clock yet. Use its configured replay instant (or ET day
/// start) and actual admission now for live; never create a fictitious 1970 interruption.
fn unavailable_detection_clock(cfg: &MarketConfig, mode: ClockMode) -> UnixNanos {
    if mode == ClockMode::Live {
        return UnixNanos(crate::observe::wall_now());
    }
    let day = cfg
        .provider
        .day
        .unwrap_or_else(|| Utc::now().with_timezone(&New_York).date_naive());
    let time = cfg
        .provider
        .start_et
        .unwrap_or_else(|| chrono::NaiveTime::from_hms_opt(4, 0, 0).expect("valid hour"));
    market_events::session::et_instant(day, time)
        .or_else(|| market_events::session::et(day, 4, 0, 0, 0))
        .unwrap_or_else(|| UnixNanos(crate::observe::wall_now()))
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
        let assets = Arc::new(assets);
        let luld = LuldMeta::shared(assets.clone(), closes.clone());
        let market = Market(Arc::new(Inner {
            mode,
            info: Mutex::new(info.map(Arc::new)),
            days,
            status: Mutex::new(status),
            core: Mutex::new(Core::new(closes.clone(), luld)),
            clock: Mutex::new(None),
            tx: broadcast::channel(BROADCAST_CAPACITY).0,
            control,
            bin: ChartEngine::default().config().profile_bin,
            band,
            paper: Mutex::new(Broker::new()),
            paper_route: std::sync::RwLock::new(None),
            assets: Mutex::new(assets),
            closes: Mutex::new(closes),
            meta_files: files,
            follow_latest,
            latest_seen: Mutex::new(latest_seen),
            quote_subs: AtomicUsize::new(0),
            pressure_subs: AtomicUsize::new(0),
            context_on: AtomicBool::new(false),
            pressure_primed: AtomicBool::new(false),
            detection_on: AtomicBool::new(false),
            detection_subs: AtomicUsize::new(0),
            detection: std::sync::OnceLock::new(),
            detection_published: Mutex::new(None),
            detection_delivery: Mutex::new(()),
            detection_quiet_refs: Mutex::new(Arc::new(Vec::new())),
            detection_directory: Mutex::new(None),
            detection_error_recovery: AtomicBool::new(false),
            detection_link_down: AtomicBool::new(false),
            readiness: Mutex::new(None),
            ross_subs: AtomicUsize::new(0),
            favorites: Mutex::new(crate::favorites::Board::default()),
            favorites_subs: AtomicUsize::new(0),
            movers: Mutex::new(crate::movers::Board::default()),
            movers_subs: AtomicUsize::new(0),
            symbols: Mutex::new(crate::symbols::Published::default()),
            symbols_subs: AtomicUsize::new(0),
            rocket_on: AtomicBool::new(true),
            rocket_policy: Mutex::new(crate::rocket::Policy::default()),
            rocket_subs: AtomicUsize::new(0),
            rocket_published: Mutex::new(None),
            rocket_lines: Mutex::new(crate::day_line::Published::default()),
            runner_events: Mutex::new(crate::time_marks::Events::default()),
            rocket_auto_zones: Mutex::new(crate::auto_zones::ZoneCache::default()),
            rocket_log: std::sync::OnceLock::new(),
            rocket_wake: Arc::new(tokio::sync::Notify::new()),
            rocket_print_sec: AtomicI64::new(i64::MIN),
            frame_cache: Mutex::new(crate::profile_frames::FrameCache::default()),
            clock_set: Mutex::new(None),
            zones_on: AtomicBool::new(true),
            zones_log: std::sync::OnceLock::new(),
            burst_archive: std::sync::OnceLock::new(),
            n1_archive: std::sync::OnceLock::new(),
            mp5_archive: std::sync::OnceLock::new(),
            running_archive: std::sync::OnceLock::new(),
            transition_log: std::sync::OnceLock::new(),
            notice_memory_root: std::sync::OnceLock::new(),
            observations: crate::observe::ObservationSink::new(crate::observe::QUEUE_CAPACITY),
            admissions: std::sync::OnceLock::new(),
            shadow: std::sync::OnceLock::new(),
            history: std::sync::OnceLock::new(),
            evidence: std::sync::OnceLock::new(),
            alerts: std::sync::OnceLock::new(),
        }));
        let board = market.new_context_board();
        let rocket = market.new_rocket_board();
        {
            let mut core = market.0.core.lock().expect("core lock");
            core.context = board;
            core.rocket = rocket;
            core.zones = market.new_zone_board();
        }
        market
    }

    /// A fresh volume-shelf board (`crate::zones`) for a new `Core`: off with `ROCKET_WATCH=0`
    /// or `VOLUME_SHELVES=0` (the shelves ride in the Runners frame).
    fn new_zone_board(&self) -> crate::zones::ZoneBoard {
        let mut z = crate::zones::ZoneBoard::new(
            self.0.rocket_on.load(Ordering::Relaxed) && self.0.zones_on.load(Ordering::Relaxed),
        );
        // The Runners "Run +20%" entry fires within seconds (crate::zones, "Early fire").
        z.set_early(true);
        z
    }

    /// Turns the volume shelves on or off from now (production reads `VOLUME_SHELVES` once at
    /// start; tests). Rebuilds the board: its state starts over.
    pub fn set_volume_shelves(&self, on: bool) {
        self.0.zones_on.store(on, Ordering::Relaxed);
        let board = self.new_zone_board();
        self.0.core.lock().expect("core lock").zones = board;
    }

    /// Live: opens the volume shelves' forward log in `dir` (`ARCHIVE_ROOT/zones`), prunes it to
    /// the archive's retention window and reads today's keys back, so a restart logs nothing
    /// twice (`crate::zones`, "After a restart").
    pub fn init_zones_log(&self, dir: std::path::PathBuf, retention_days: u32, now: UnixNanos) {
        let log = Arc::new(crate::rocket::RocketLog::new(dir, retention_days));
        let today = market_events::session::trading_date(now);
        log.prune(today);
        let keys = crate::zones::logged_keys(log.dir(), today);
        tracing::info!(dir = %log.dir().display(), logged = keys.len(), "volume shelves: forward log opened");
        let mut core = self.0.core.lock().expect("core lock");
        if !core.zones.enabled() {
            return;
        }
        core.zones.enable_log(keys);
        drop(core);
        let _ = self.0.zones_log.set(log);
    }

    /// A fresh Rocket watch board (`crate::rocket`) for a new `Core`: off with `ROCKET_WATCH=0`.
    fn new_rocket_board(&self) -> crate::rocket::RocketBoard {
        if !self.0.rocket_on.load(Ordering::Relaxed) {
            return crate::rocket::RocketBoard::disabled();
        }
        let policy = self.0.rocket_policy.lock().expect("rocket policy").clone();
        let scorer = crate::rocket::production_scorer(policy.scorer);
        crate::rocket::RocketBoard::new(true, Arc::from(scorer), policy)
    }

    /// Turns Rocket watch on or off with a policy from now (production reads `ROCKET_WATCH` once
    /// at start; tests). Rebuilds the board: its state starts over.
    pub fn set_rocket_watch(&self, on: bool, policy: crate::rocket::Policy) {
        self.0.rocket_on.store(on, Ordering::Relaxed);
        *self.0.rocket_policy.lock().expect("rocket policy") = policy;
        let board = self.new_rocket_board();
        let zones = self.new_zone_board();
        {
            let mut core = self.0.core.lock().expect("core lock");
            core.rocket = board;
            core.zones = zones;
        }
        *self.0.rocket_published.lock().expect("rocket published") = None;
        *self.0.rocket_lines.lock().expect("rocket lines") = Default::default();
    }

    /// Live: turns on the buying-burst detector on the Runners board (`crate::rocket::burst`),
    /// with `archive` for its daily ADV30. Call after `set_rocket_watch` (which rebuilds the
    /// board) and before `init_rocket_log` (which restores today's burst rows into it). A replay
    /// never calls this: bursts are a live-market alert.
    pub fn enable_burst(
        &self,
        policy: crate::rocket::burst::BurstPolicy,
        archive: Option<Archive>,
    ) {
        if !policy.enabled
            || !self.0.rocket_on.load(Ordering::Relaxed)
            || self.0.mode != ClockMode::Live
        {
            return;
        }
        if let Some(a) = archive {
            let _ = self.0.burst_archive.set(a);
        }
        tracing::info!(
            per_day = policy.per_day,
            threshold = policy.threshold,
            "buying bursts: detector on ({} {})",
            crate::rocket::burst::STRATEGY,
            crate::rocket::burst::VERSION
        );
        self.0
            .core
            .lock()
            .expect("core lock")
            .rocket
            .enable_burst(policy);
    }

    /// Live: turns on the N1 shadow detector (`crate::n1_live`) with `archive` for its daily
    /// facts. Call after `set_rocket_watch` (which rebuilds the board). A replay never calls
    /// this: N1 is a live-market shadow alert.
    pub fn enable_n1(&self, archive: Option<Archive>) {
        if !self.0.rocket_on.load(Ordering::Relaxed) || self.0.mode != ClockMode::Live {
            return;
        }
        if let Some(a) = archive {
            let _ = self.0.n1_archive.set(a);
        }
        tracing::info!(
            config = crate::n1_live::logic::CONFIG,
            "n1 shadow: detector on (Engine A T2-c6 entrant N1, shadow beside production; N1_LIVE=0 turns it off)"
        );
        self.0.core.lock().expect("core lock").rocket.enable_n1();
    }

    /// Live: turns on Engine A T2-c4's additions (`crate::c4_live`) with `archive` for the daily
    /// facts F1u shares with N1. Call after `set_rocket_watch`. A replay never calls this.
    pub fn enable_c4(&self, archive: Option<Archive>) {
        if !self.0.rocket_on.load(Ordering::Relaxed) || self.0.mode != ClockMode::Live {
            return;
        }
        if let Some(a) = archive {
            let _ = self.0.n1_archive.set(a);
        }
        tracing::info!(
            config = crate::c4_live::logic::CONFIG,
            "c4: Engine A T2-c4 on (F1u, HALT add-on 09:30-11:30 ET, POST<1 prune; C4_LIVE=0 turns it off)"
        );
        self.0.core.lock().expect("core lock").rocket.enable_c4();
    }

    /// Live with the Detection Panel: turns on rw-mp5-1's inputs (`crate::mp5_gate`): the
    /// per-bucket shares from every counted print, the 20:00 ET day file and the 03:30 ET table.
    /// The gate itself runs in the alerts task. Needs the Runners board's clock (`ROCKET_WATCH`).
    pub fn enable_mp5(&self, archive: Archive) {
        if !self.0.rocket_on.load(Ordering::Relaxed) || self.0.mode != ClockMode::Live {
            return;
        }
        let _ = self.0.mp5_archive.set(archive);
        tracing::info!(
            detector = crate::mp5_gate::logic::DETECTOR,
            config = crate::mp5_gate::logic::CONFIG_SHA256,
            "rw-mp5-1: Signals crossings gated (A-G9 in play or prior runner, 5 s B0 dedup); switch `detectors` -> rw-mp5-1"
        );
        self.0.core.lock().expect("core lock").mp5.enable();
    }

    /// rw-mp5-1's G9 verdict on a crossing of `symbol` at `price` decided at `a_ns`, and the
    /// day's 04:00 ET (for the ledger's session). Takes the core lock for one lookup (at most
    /// 192 additions), from the alerts task, never from ingest.
    pub fn mp5_verdict(
        &self,
        symbol: &str,
        price: f64,
        a_ns: i64,
    ) -> (crate::mp5_gate::Verdict, Option<i64>) {
        let v = self
            .0
            .core
            .lock()
            .expect("core lock")
            .mp5
            .verdict(symbol, price, a_ns);
        let t0 = crate::mp5_gate::day_of(market_events::session::trading_date(
            market_events::UnixNanos(a_ns),
        ))
        .map(|d| d.t0);
        (v, t0)
    }

    /// Reads rw-mp5-1's table for `date` (`crate::mp5_gate::load_table`, which may rebuild
    /// missing baseline days from the archive's tape: minutes, so never under the lock) on a
    /// blocking thread, then hands it to the core.
    fn load_mp5_table(&self, date: NaiveDate) {
        let Some(archive) = self.0.mp5_archive.get().cloned() else {
            return;
        };
        let market = self.clone();
        std::thread::spawn(move || {
            let t = std::time::Instant::now();
            let table = crate::mp5_gate::load_table(&archive, date);
            tracing::info!(%date, ms = t.elapsed().as_millis() as u64, "rw-mp5-1: table read");
            market
                .0
                .core
                .lock()
                .expect("core lock")
                .mp5
                .set_table(std::sync::Arc::new(table));
        });
    }

    /// Writes a finished day's bucket figures (`crate::mp5_gate::write_day`) on a blocking thread.
    fn write_mp5_day(&self, f: crate::mp5_gate::Finished) {
        let Some(archive) = self.0.mp5_archive.get().cloned() else {
            return;
        };
        std::thread::spawn(move || {
            let dir = crate::mp5_gate::tod_dir(archive.root());
            match crate::mp5_gate::write_day(&dir, f.date, f.complete, "live", &f.syms) {
                Ok(()) => {
                    tracing::info!(date = %f.date, symbols = f.syms.len(), complete = f.complete, "rw-mp5-1: day file written")
                }
                Err(e) => {
                    tracing::warn!(date = %f.date, error = %e, "rw-mp5-1: day file not written; that session is missing from later baselines")
                }
            }
        });
    }

    /// Loads a trading date's N1 facts (`crate::n1_live::load_facts`) on a blocking thread, then
    /// hands them to the board. Never under the core lock.
    fn load_n1_facts(&self, date: NaiveDate) {
        let Some(archive) = self.0.n1_archive.get().cloned() else {
            return;
        };
        let market = self.clone();
        std::thread::spawn(move || {
            let t = std::time::Instant::now();
            let table = crate::n1_live::load_facts(&archive, date);
            tracing::info!(%date, ms = t.elapsed().as_millis() as u64, "n1 shadow: facts read");
            market
                .0
                .core
                .lock()
                .expect("core lock")
                .rocket
                .set_n1_facts(table);
        });
    }

    /// Live with bursts on: the symbols the burst detector holds and how many of them still wait
    /// for their day to be rebuilt after a restart (`crate::rocket::burst`, "After a restart").
    pub fn burst_symbols(&self) -> Option<(usize, usize)> {
        let core = self.0.core.lock().expect("core lock");
        core.rocket.burst().map(|b| (b.symbols(), b.unrestored()))
    }

    /// What the market's per-symbol structures hold right now (counts and estimated bytes), for
    /// the server's `MEM_STATS_SECS` log line and the memory bench. Cheap (one pass over the
    /// boards' symbol lists) but under the core lock, so it is meant for once a minute, not per
    /// tick. It is an attribution aid, not the heap: the chart engine's candles are counted in
    /// symbols, not bytes.
    pub fn memory_report(&self) -> Value {
        let core = self.0.core.lock().expect("core lock");
        let (zone_syms, zone_confirmed, zone_bytes) = core.zones.cost();
        let held: usize = core.unloaded.values().map(|u| u.held_len()).sum();
        serde_json::json!({
            "engine_symbols": core.engine.symbols().count(),
            "engine": core.engine.cost(),
            "context": core.context.as_ref().map(|c| c.cost()),
            // Frames still in the broadcast ring: a slot is released only once every receiver
            // has read it, so one stalled session keeps the whole ring full.
            "broadcast": {"held": self.0.tx.len(), "receivers": self.0.tx.receiver_count()},
            "tape_symbols": core.tape.len(),
            "tape_rows": core.tape.values().map(VecDeque::len).sum::<usize>(),
            "tape_bytes": core.tape.values().flatten().map(String::capacity).sum::<usize>(),
            "last_price": core.last_price.len(),
            "unloaded": core.unloaded.len(),
            "unloaded_held_events": held,
            "rocket": core.rocket.cost(),
            "zones": {"symbols": zone_syms, "confirmed": zone_confirmed, "bytes": zone_bytes},
            "movers": self.movers_cost(),
        })
    }

    /// Read-only Runners adoption/facts readiness, independent of strategy thresholds.
    pub fn runner_readiness(&self) -> Value {
        self.0.core.lock().expect("core lock").rocket.readiness()
    }

    /// Counted expired sided prints, separate from history/readiness provenance.
    pub fn pressure_expired_prints_dropped(&self) -> u64 {
        self.0
            .core
            .lock()
            .expect("core lock")
            .pressure
            .expired_prints_dropped()
    }

    /// Loads a trading date's running-entry facts (`crate::rocket::running::facts`) on a
    /// blocking thread, then hands them to the board. Never under the core lock. Without an
    /// archive the rule runs on its defaults (the cap, no volume ratio).
    fn load_running_facts(&self, date: NaiveDate) {
        let Some(archive) = self.0.running_archive.get().cloned() else {
            // Known absent differs from not loaded. Preserve the documented fallback only
            // after explicitly completing this date's facts request.
            self.0
                .core
                .lock()
                .expect("core lock")
                .rocket
                .set_running_facts(date, HashMap::new());
            return;
        };
        let market = self.clone();
        std::thread::spawn(move || {
            let t = std::time::Instant::now();
            let facts = crate::rocket::running::facts(&archive, date);
            tracing::info!(%date, symbols = facts.len(), ms = t.elapsed().as_millis() as u64, "runners running entry: facts loaded");
            market
                .0
                .core
                .lock()
                .expect("core lock")
                .rocket
                .set_running_facts(date, facts);
        });
    }

    /// Loads a trading date's ADV30 for the burst detector on a blocking thread (30 small
    /// archive files), then hands it to the board. Never under the core lock.
    fn load_burst_adv(&self, date: NaiveDate) {
        let Some(archive) = self.0.burst_archive.get().cloned() else {
            return;
        };
        let market = self.clone();
        std::thread::spawn(move || {
            let t = std::time::Instant::now();
            let adv = crate::rocket::burst::facts::adv30(&archive, date);
            tracing::info!(%date, symbols = adv.len(), ms = t.elapsed().as_millis() as u64, "buying bursts: ADV30 loaded");
            market
                .0
                .core
                .lock()
                .expect("core lock")
                .rocket
                .set_burst_adv(date, adv);
        });
    }

    /// Starts the day's cue-transition log under `dir` (`crate::oplog`): the micro-pullback
    /// transitions the context engine drains (Ross members, Runners rows, favorites) were counted
    /// and dropped before, so a missed or late cue could not be traced afterwards. Live only; a
    /// second call is ignored.
    pub fn init_transition_log(&self, dir: std::path::PathBuf) {
        let _ = self.0.transition_log.set(crate::oplog::DailyLog::start(
            "transitions",
            crate::oplog::DailyLogConfig {
                dir,
                prefix: "",
                retention_days: crate::oplog::TRANSITIONS_DAYS,
                cap_bytes: crate::oplog::TRANSITIONS_CAP,
            },
        ));
    }

    /// Live: what a previous process of this run already announced and numbered today, read back
    /// from the logs under `archive_root` (`crate::notice_memory`) into the current context board,
    /// and into every board built later in this process (a Detection Panel toggle rebuilds the
    /// engine, which would otherwise restart its episode numbers too). Before the ingest starts;
    /// a second call is ignored.
    pub fn init_notice_memory(&self, archive_root: std::path::PathBuf) {
        if self.0.mode != ClockMode::Live || self.0.notice_memory_root.set(archive_root).is_err() {
            return;
        }
        let run = {
            let core = self.0.core.lock().expect("core lock");
            core.context.as_ref().map(|c| c.run_id().to_string())
        };
        let Some(run) = run else {
            return;
        };
        let Some(m) = self.notice_memory(&run) else {
            return;
        };
        let mut core = self.0.core.lock().expect("core lock");
        if let Some(cx) = core.context.as_mut().filter(|c| c.run_id() == run) {
            cx.restore_notice_memory(&m);
        }
    }

    /// Reads today's notice memory of run `run` (off the core lock), when this market has a
    /// notice-memory root.
    fn notice_memory(&self, run: &str) -> Option<crate::notice_memory::NoticeMemory> {
        let root = self.0.notice_memory_root.get()?;
        let now = crate::history::wall_now().0;
        let t = std::time::Instant::now();
        let m = crate::notice_memory::read(root, run, now);
        tracing::info!(
            run,
            date = ?m.date,
            episode_floors = m.floors.len(),
            announced = m.seen.len(),
            setups_today = m.setup_days.len(),
            ms = t.elapsed().as_millis() as u64,
            "notice memory: restored what earlier processes announced today"
        );
        Some(m)
    }

    /// Live: opens the forward log in `dir` (`ARCHIVE_ROOT/rocket-watch`), prunes it to the
    /// archive's retention window and restores today's alerts and the record into the board.
    /// Reads a few small files once, before the ingest starts.
    pub fn init_rocket_log(&self, dir: std::path::PathBuf, retention_days: u32, now: UnixNanos) {
        if !self.0.rocket_on.load(Ordering::Relaxed) {
            return;
        }
        let log = Arc::new(crate::rocket::RocketLog::new(dir, retention_days));
        let today = market_events::session::trading_date(now);
        let pruned = log.prune(today);
        if !pruned.is_empty() {
            tracing::info!(days = ?pruned, "rocket watch: forward log pruned");
        }
        let loaded = log.load(today);
        tracing::info!(
            dir = %log.dir().display(),
            today = loaded.today.len(),
            days = loaded.days.len(),
            "rocket watch: forward log opened"
        );
        self.0
            .core
            .lock()
            .expect("core lock")
            .rocket
            .restore(today, loaded);
        let _ = self.0.rocket_log.set(log);
    }

    /// A fresh context board for this market's current day, or `None` with `STOCK_CONTEXT` off.
    /// Takes the `info` lock, then the evidence log's (nobody holding either takes another), so
    /// call it before locking `core`. The board starts with every evidence record already loaded
    /// for the day (`crate::evidence`): a seek's rebuild decides with the same evidence as the
    /// first pass, each record still joining only from its own `observed_at`.
    fn new_context_board(&self) -> Option<ContextBoard> {
        if !self.0.context_on.load(Ordering::Relaxed)
            && !self.0.detection_on.load(Ordering::Relaxed)
        {
            return None;
        }
        let day = self
            .0
            .info
            .lock()
            .expect("info lock")
            .as_ref()
            .and_then(|i| i.day);
        let mut board = if self.0.detection_on.load(Ordering::Relaxed)
            && self.detection_source() == "fixture"
        {
            ContextBoard::new_labeled(
                self.0.mode == ClockMode::Live,
                day,
                self.0.band.min,
                self.0.band.max,
                true,
            )
        } else {
            ContextBoard::new(
                self.0.mode == ClockMode::Live,
                day,
                self.0.band.min,
                self.0.band.max,
            )
        }?;
        // Enabling a board during a lazy restore must not briefly assess the held tail as a
        // whole day's trustworthy history. This guard exists before any print or barrier.
        let pending: Vec<String> = self
            .0
            .core
            .lock()
            .expect("core lock")
            .unloaded
            .keys()
            .cloned()
            .collect();
        for symbol in pending {
            board.set_history_pending(&symbol, true);
        }
        // A board built after startup (a Detection Panel toggle) starts with fresh episode
        // counters and notice memory: give it what this run already announced today.
        if let Some(m) = self.notice_memory(board.run_id()) {
            board.restore_notice_memory(&m);
        }
        if let Some(ev) = self.0.evidence.get()
            && let Some(batch) = ev.log.lock().expect("evidence log lock").since(None)
        {
            board.add_evidence(batch);
        }
        if self.0.detection_on.load(Ordering::Relaxed) {
            let _delivery = self
                .0
                .detection_delivery
                .lock()
                .expect("detection delivery");
            board = board.with_detection();
            // Provenance comes from the actual configured engine, rather than independently
            // guessing which profile current() means in a second component.
            let policy = board.detection_policy_tag()?;
            let directory = self
                .0
                .detection_directory
                .lock()
                .expect("detection directory")
                .clone();
            let runtime = self.0.detection.get_or_init(|| {
                Arc::new(crate::detection::Runtime::new(
                    self.0.mode.as_str(),
                    directory.as_deref(),
                    policy,
                ))
            });
            let epoch = runtime.reset(true, self.detection_source());
            board.set_detection_epoch(epoch);
            let refs = self
                .0
                .detection_quiet_refs
                .lock()
                .expect("quiet references")
                .clone();
            for reference in refs.iter() {
                board.add_detection_quiet_reference(reference.clone());
            }
        }
        // Detection-only monitoring does not opt the run into Ross's separate shadow study.
        if !self.0.context_on.load(Ordering::Relaxed) {
            return Some(board);
        }
        // One capture for the life of the market, shared by every board it rebuilds: a replay
        // seek replaces the board, and the sessions it had already observed are still the
        // sessions it observed.
        let sink = self.0.shadow.get_or_init(|| {
            // A segment of this boot's own, reserved so two boots on one date cannot share a
            // file: the header names the build and the epoch, and a second boot appending under
            // the first one's header would attribute its records to the wrong build.
            let (log, epoch) = match self.shadow_log_dir() {
                Some(dir) => crate::shadow::reserve_segment(&dir),
                None => (None, 0),
            };
            let sink = std::sync::Arc::new(crate::shadow::ShadowSink::new(
                crate::shadow::QUEUE_CAPACITY,
                log,
                crate::COMMIT.to_string(),
                // The same executable fingerprint the admission journal uses, warmed off any
                // lock at startup rather than hashed on a hot path.
                crate::admit::code_id_for_records().to_string(),
                epoch,
            ));
            // The capture folds and appends on a thread of its own: no disk on a runtime worker,
            // and no report waiting behind a file write.
            crate::shadow::ShadowSink::spawn_absorber(sink.clone());
            sink
        });
        Some(board.with_shadow(sink.clone()))
    }

    /// Where the shadow capture's segments live, when there is an archive to write beside.
    ///
    /// `None` leaves the capture in memory only, and the report then says so and withholds every
    /// figure: a cohort a restart would lose is not a forward corpus.
    ///
    /// **In production this is `None`**, and that is deliberate (review, 2026-10-21): `start`
    /// turns the board on before `init_history` sets the archive, and the capture is created
    /// once per process, so a live server keeps it in memory (bounded at
    /// [`crate::shadow::SESSIONS_TRACKED`]) and writes no `shadow/` files. Making it durable as it
    /// stands would write every decision of the whole market, unrotated and never pruned, for a
    /// cohort whose session bound is below the market's daily symbol count — so it could never
    /// report itself complete anyway. Persisting it needs a narrower capture (band/Ross
    /// candidates) and retention first. Tests that build the history first do get a log.
    fn shadow_log_dir(&self) -> Option<std::path::PathBuf> {
        let h = self.0.history.get()?;
        Some(h.archive_root().join("shadow"))
    }

    /// Turns stock context on or off for this market from now (tests; production reads
    /// `STOCK_CONTEXT` once at start). Rebuilds the board for the current day.
    pub fn set_stock_context(&self, on: bool) {
        self.0.context_on.store(on, Ordering::Relaxed);
        let board = self.new_context_board();
        self.0.core.lock().expect("core lock").context = board;
        self.refresh_detection(true);
    }

    /// Local/test feature switch. Production reads flags once at startup. Reconfiguration is
    /// an explicit new epoch, so old state is never relabeled as a newly detected event.
    pub fn set_detection_panel(&self, on: bool) {
        self.configure_detection(on, None);
    }

    /// A strictly labeled local demonstration seam. It has no provider, credentials, recording
    /// acquisition or live slot; all inputs still pass through the real Market admission path.
    pub fn detection_fixture(day: NaiveDate, log_directory: Option<&std::path::Path>) -> Market {
        let market = Market::build(
            ClockMode::Replay,
            Some(ProviderInfo {
                mode: ClockMode::Replay,
                label: "Detection fixture (synthetic, not market evidence)".into(),
                day: Some(day),
                symbols: Vec::new(),
                fixture: true,
            }),
            ProviderStatus {
                state: "running",
                detail: Some("labeled synthetic detector fixture".into()),
            },
            None,
            PriceBand::DEFAULT,
            ArchiveMeta::default(),
        );
        market.configure_detection(true, log_directory);
        if tokio::runtime::Handle::try_current().is_ok() {
            market.spawn_quotes_task();
        }
        market
    }

    pub fn apply_detection_fixture(&self, item: FeedItem) -> Result<(), &'static str> {
        if self.0.mode != ClockMode::Replay || !self.info().is_some_and(|i| i.fixture) {
            return Err("fixture input requires an explicitly labeled replay fixture market");
        }
        if let FeedItem::Source(source) = &item
            && !source.fixture
        {
            return Err("fixture source cannot become market evidence");
        }
        if let FeedItem::Event(event) = &item {
            let source = match event {
                MarketEvent::Trade(e) => e.source,
                MarketEvent::Quote(e) => e.source,
                MarketEvent::Correction(e) => e.source,
                MarketEvent::Cancel(e) => e.source,
                MarketEvent::Status(e) => e.source,
            };
            if source != market_events::Source::Fixture {
                return Err("fixture seam accepts only normalized fixture events");
            }
        }
        self.apply(item);
        Ok(())
    }

    /// Drives the core owner's adversarial script through real Market trades/revisions/clocks.
    /// Supplemental documentary gaps are explicit fixture inputs, never inferred from silence.
    pub fn apply_detection_fixture_step(
        &self,
        step: &stock_context::detect::fixture::FixtureStep,
    ) -> Result<(), String> {
        use stock_context::detect::fixture::FixtureStep;
        match step {
            FixtureStep::Print {
                symbol,
                exec,
                admit,
                price,
                size,
                seq,
            } => {
                let mut trade = market_events::Trade::new(
                    symbol.as_str(),
                    UnixNanos(*exec),
                    Price::from_micros(*price),
                    *size,
                    *seq,
                );
                trade.receive_ts = Some(UnixNanos(*admit));
                self.apply_detection_fixture(FeedItem::Event(MarketEvent::Trade(trade)))
                    .map_err(str::to_string)
            }
            FixtureStep::Cancel {
                symbol,
                seq,
                price,
                size,
                admit,
            } => self
                .apply_detection_fixture(FeedItem::Event(MarketEvent::Cancel(
                    market_events::Cancel {
                        symbol: symbol.clone(),
                        event_ts: UnixNanos(*admit),
                        receive_ts: Some(UnixNanos(*admit)),
                        source: market_events::Source::Fixture,
                        exchange: String::new(),
                        tape: Default::default(),
                        trade_id: *seq,
                        price: Price::from_micros(*price),
                        size: *size,
                        action: market_events::CancelAction::Cancel,
                    },
                )))
                .map_err(str::to_string),
            FixtureStep::Advance(at) => self
                .apply_detection_fixture(FeedItem::Advance(UnixNanos(*at)))
                .map_err(str::to_string),
            FixtureStep::QuietReference(reference) => self
                .provision_detection_quiet_references(vec![reference.clone()])
                .map(|_| ()),
            FixtureStep::Interrupt { symbol, at, reason } => {
                self.fixture_detection_status(symbol.as_deref(), *at, false, reason)
            }
            FixtureStep::Resume { symbol, at } => {
                self.fixture_detection_status(symbol.as_deref(), *at, true, "resume")
            }
        }
    }

    fn fixture_detection_status(
        &self,
        symbol: Option<&str>,
        at: i64,
        up: bool,
        reason: &'static str,
    ) -> Result<(), String> {
        if self.0.mode != ClockMode::Replay || self.detection_source() != "fixture" {
            return Err("documentary fixture status requires a fixture market".into());
        }
        {
            let mut core = self.0.core.lock().expect("core lock");
            if let Some(board) = core.context.as_mut() {
                board.detection_status(symbol, UnixNanos(at), up, reason);
            }
            core.seq += 1;
        }
        self.refresh_detection(false);
        Ok(())
    }

    pub fn run_detection_fixture(
        scenario: &str,
        log_directory: Option<&std::path::Path>,
    ) -> Result<Market, String> {
        let steps = stock_context::detect::fixture::script(scenario)
            .ok_or_else(|| format!("unknown detection fixture {scenario}"))?;
        let market = Self::detection_fixture(stock_context::detect::fixture::day(), log_directory);
        for step in &steps {
            market.apply_detection_fixture_step(step)?;
        }
        market.publish_detection();
        Ok(market)
    }

    /// A bounded local input seam. The core validates prior-only date/segment/basis/observed_at
    /// on every join; provisioning a file cannot declare that those inputs are valid.
    pub fn load_detection_quiet_references(&self, path: &std::path::Path) -> Result<usize, String> {
        use std::io::Read;
        const MAX_BYTES: u64 = 16 * 1024 * 1024;
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .map_err(|e| e.to_string())?
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err("quiet references exceed 16MiB".into());
        }
        let text = std::str::from_utf8(&bytes).map_err(|e| e.to_string())?;
        let mut refs = Vec::new();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            if line.len() > 1024 * 1024 || refs.len() >= 16_384 {
                return Err("quiet reference line/record bound".into());
            }
            refs.push(
                serde_json::from_str::<stock_context::QuietReference>(line)
                    .map_err(|e| e.to_string())?,
            );
        }
        self.provision_detection_quiet_references(refs)
    }

    /// Also used by labeled Rust fixtures; no HTTP path can inject research references.
    pub fn provision_detection_quiet_references(
        &self,
        refs: Vec<stock_context::QuietReference>,
    ) -> Result<usize, String> {
        if !self.0.detection_on.load(Ordering::Relaxed) {
            return Err("detection is disabled".into());
        }
        if refs.len() > 16_384 {
            return Err("quiet reference record bound".into());
        }
        if self.detection_source() != "fixture" && refs.iter().any(|r| r.synthetic) {
            return Err("synthetic quiet references require a fixture market".into());
        }
        let mut prior = self
            .0
            .detection_quiet_refs
            .lock()
            .expect("quiet references")
            .as_ref()
            .clone();
        let key = |r: &stock_context::QuietReference| {
            (
                r.symbol.clone(),
                r.trading_date,
                format!("{:?}", r.segment),
                r.id.clone(),
                r.version,
            )
        };
        let mut known: std::collections::BTreeMap<_, _> =
            prior.iter().enumerate().map(|(i, r)| (key(r), i)).collect();
        let mut added = Vec::new();
        for reference in refs {
            if let Some(index) = known.get(&key(&reference)) {
                if prior[*index] != reference {
                    return Err("conflicting quiet reference identity/version".into());
                }
                continue;
            }
            if prior.len() >= 16_384 {
                return Err("quiet reference lifetime bound".into());
            }
            known.insert(key(&reference), prior.len());
            prior.push(reference.clone());
            added.push(reference);
        }
        let count = added.len();
        *self
            .0
            .detection_quiet_refs
            .lock()
            .expect("quiet references") = Arc::new(prior);
        if let Some(board) = self.0.core.lock().expect("core lock").context.as_mut() {
            for reference in added {
                board.add_detection_quiet_reference(reference);
            }
        }
        self.refresh_detection(true);
        Ok(count)
    }

    fn configure_detection(&self, on: bool, directory: Option<&std::path::Path>) {
        self.0.detection_on.store(on, Ordering::Relaxed);
        if on {
            if let Some(directory) = directory {
                *self
                    .0
                    .detection_directory
                    .lock()
                    .expect("detection directory") = Some(directory.into());
            }
        } else if let Some(runtime) = self.0.detection.get() {
            runtime.reset(false, self.detection_source());
        }
        let board = self.new_context_board();
        self.0.core.lock().expect("core lock").context = board;
        self.refresh_detection(true);
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

    /// The chart's slot times for a sloped drawing alert (crate::alerts::geometry): the engine's
    /// candles and gap runs of `symbol` at the timeframe of `tf_ms`, merged as chart-core
    /// `buildSlots` merges the snapshot a chart opens with, extended hours shown. None for an
    /// unknown timeframe or a symbol not in the engine. Called by the alerts task at most once
    /// per new candle per (symbol, timeframe) a sloped drawing alert watches.
    pub(crate) fn alert_slots(&self, symbol: &str, tf_ms: i64) -> Option<Vec<i64>> {
        let tf = Timeframe::ALL
            .into_iter()
            .find(|t| t.secs() * 1000 == tf_ms)?;
        let snap = {
            let core = self.0.core.lock().expect("core lock");
            core.engine.snapshot(symbol, tf, Some(self.0.bin))?
        };
        let candles: Vec<i64> = snap.candles.iter().map(|c| c.t).collect();
        let gaps: Vec<i64> = snap.gaps.iter().map(|g| g.t).collect();
        Some(crate::alerts::geometry::build_slots(&candles, &gaps))
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
                let market = Market::idle(mode, state, Some(e.to_string()));
                if cfg.stock_context {
                    market.set_stock_context(true);
                }
                if cfg.detection_panel {
                    market.configure_detection(true, Some(&archive.root().join("detection")));
                    let at = unavailable_detection_clock(cfg, mode);
                    if let Some(cx) = market.0.core.lock().expect("core lock").context.as_mut() {
                        cx.detection_status(None, at, false, "provider_unavailable");
                    }
                    market.refresh_detection(true);
                    market.spawn_detection_task();
                }
                return (market, None);
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
        }
        market.set_rocket_watch(cfg.rocket_watch, cfg.rocket_policy.clone());
        let _ = market.0.running_archive.set(archive.clone());
        market.set_volume_shelves(cfg.volume_shelves);
        if mode == ClockMode::Live && cfg.rocket_watch && cfg.volume_shelves {
            market.init_zones_log(
                archive.root().join("zones"),
                archive.config().retention_days,
                crate::history::wall_now(),
            );
        }
        if mode == ClockMode::Live && cfg.rocket_watch {
            market.enable_burst(cfg.burst_policy.clone(), Some(archive.clone()));
        }
        if mode == ClockMode::Live && cfg.rocket_watch && cfg.n1_live {
            market.enable_n1(Some(archive.clone()));
        } else if mode == ClockMode::Live && !cfg.n1_live {
            tracing::info!("n1 shadow: off (N1_LIVE=0)");
        }
        if mode == ClockMode::Live && cfg.rocket_watch && cfg.detection_panel {
            market.enable_mp5(archive.clone());
        }
        if mode == ClockMode::Live && cfg.rocket_watch && cfg.c4_live {
            market.enable_c4(Some(archive.clone()));
        } else if mode == ClockMode::Live && !cfg.c4_live {
            tracing::info!("c4: off (C4_LIVE=0)");
        }
        if mode == ClockMode::Live && cfg.rocket_watch {
            // Before the ingest: today's alerts and the record come back after a restart.
            market.init_rocket_log(
                archive.root().join("rocket-watch"),
                archive.config().retention_days,
                crate::history::wall_now(),
            );
        }
        if cfg.detection_panel {
            market.configure_detection(true, Some(&archive.root().join("detection")));
            if let Some(path) = cfg.detection_quiet_references.as_deref()
                && let Err(error) = market.load_detection_quiet_references(path)
            {
                tracing::warn!(%error, "Detection quiet references unavailable; price-only families remain enabled");
            }
        }
        if mode == ClockMode::Live && (cfg.stock_context || cfg.detection_panel) {
            market.init_transition_log(archive.root().join(crate::oplog::TRANSITIONS_DIR));
            // Before the ingest: a restart must not re-announce or renumber today's episodes.
            market.init_notice_memory(archive.root().to_path_buf());
        }
        if cfg.stock_context || cfg.detection_panel {
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
                cfg.admission_journal,
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
    ///
    /// Writes the admission journal, unlike production's default (`ADMISSION_JOURNAL=0`): the
    /// restart tests built on this are what exercise the journal's proven-order restore, and a
    /// test session is small enough for its cost not to matter.
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
        market.init_history(archive, session.recorder.clone(), now, preload, true);
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
        self.spawn_detection_task();
        self.spawn_rocket_task();
        let weak: Weak<Inner> = Arc::downgrade(&self.0);
        tokio::spawn(async move {
            let mut n = 0u64;
            loop {
                // Phase-locked to the wall clock: TICK_PHASE after each second boundary, not
                // whenever the server happened to start, so a frame carries the second that just
                // ended. Each tick waits for a later boundary than the last, so it still fires at
                // most once per wall second (the ws protocol's promise); a tick that overruns a
                // second skips that boundary instead of firing twice. The Runners decision is not
                // here any more: it runs on its own task as soon as a second is over
                // (`spawn_rocket_task`); this tick only sends the panel the clients see.
                tokio::time::sleep(until_next_tick()).await;
                let Some(inner) = weak.upgrade() else { break };
                let market = Market(inner);
                market.publish_quotes();
                // Before the Runners frame, whose demand-zone mark reads the same zones.
                market.flush_chart_zones();
                market.publish_pressure();
                market.broadcast_rocket();
                // Before the broadcast, and whether or not anyone is subscribed: the context
                // engine's output journals are drained so an unattended run stays bounded.
                market.drain_context();
                market.publish_context();
                market.publish_favorites();
                market.publish_movers();
                // The candidate list, pushed when it changes (crate::symbols).
                market.publish_symbols(n);
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

    /// The Runners decision task: the shelves' and the Runners board's ticks, their logs and
    /// phone notices (`Market::decide_rocket`), as soon as a second can be decided rather than on
    /// the 1 Hz broadcast tick.
    ///
    /// Why its own task (owner: heavy and realtime work runs in Rust and is pushed as fast as the
    /// evidence allows). The decision used to ride the broadcast tick, [`TICK_PHASE`] past each
    /// wall second, reading the live provider's clock: a wall sample taken on a free-running
    /// one-second timer. A fast move or a 15 s barrier was decided 250 ms after its second ended
    /// (the newest print's time carried the board past the stale clock); a burst boundary or a
    /// shelf slot, which go by that clock alone, 0.25–1.25 s after it was due. Now:
    ///
    /// - **Live** it wakes [`ROCKET_SETTLE`] past each wall second (the earliest a whole-second
    ///   rule can decide, plus the grace for that second's late prints) and decides at
    ///   [`Market::decision_now`], the provider's clock carried forward to this instant.
    /// - **Replay** has no late prints and no wall pacing: a print that opens a new market
    ///   second wakes it at once (`rocket_wake`), so a replay at speed decides every second
    ///   instead of only the last three of each wall second (`RocketBoard::fast_tick` reads at
    ///   most three back); otherwise it looks every [`ROCKET_IDLE_POLL`].
    ///
    /// A pass runs only when the decidable second changed, and at least [`ROCKET_FLOOR`] after
    /// the last, so live this is still one pass a second and the board's cost is unchanged. The
    /// frame stays on the broadcast tick (1 Hz to clients). Weak like the other tasks: the wake
    /// has its own `Arc`, so waiting on it does not keep the market alive.
    fn spawn_rocket_task(&self) {
        let weak: Weak<Inner> = Arc::downgrade(&self.0);
        let wake = self.0.rocket_wake.clone();
        tokio::spawn(async move {
            let mut decided: Option<i64> = None;
            let mut last_pass: Option<tokio::time::Instant> = None;
            loop {
                let wait = {
                    let Some(inner) = weak.upgrade() else { break };
                    let market = Market(inner);
                    match market.rocket_schedule() {
                        // Any change, back too: a replay sent back in time decides again.
                        Some((due, _)) if decided != Some(due) => {
                            let since = last_pass.map(|t| t.elapsed());
                            if let Some(since) = since.filter(|e| *e < ROCKET_FLOOR) {
                                ROCKET_FLOOR - since
                            } else {
                                last_pass = Some(tokio::time::Instant::now());
                                if market.decide_rocket() {
                                    decided = Some(due);
                                    continue;
                                }
                                // Not decided (a replay rebuilding after a seek, the board off):
                                // the second stays due. Marking it decided here left a seek
                                // into a paused replay with an undecided board until the clock
                                // moved: the rebuild's own prints wake this task while every
                                // pass is refused, and the closing clock lands on the same
                                // second. Look again at the next wake (the closing clock wakes
                                // it) or poll; ROCKET_FLOOR still spaces the attempts.
                                ROCKET_IDLE_POLL
                            }
                        }
                        Some((_, wait)) => wait,
                        None => ROCKET_IDLE_POLL,
                    }
                };
                tokio::select! {
                    _ = wake.notified() => {}
                    _ = tokio::time::sleep(wait) => {}
                }
            }
        });
    }

    /// A print at `ts` (epoch ns) was applied: wakes the rocket task when it opens a new market
    /// second. One atomic load per print; a store and a wake about once a second. Called under
    /// the core lock (`Notify::notify_one` never waits).
    ///
    /// Prints do not arrive in time order: on the live feed about 9 % land after their second
    /// ended, some 10+ s late (2026-10-22 recording, SPCX/SOXL: `receive_ts - event_ts` p99
    /// ~10 s), and a replay of a recorded day plays them at the same arrival positions. Taken
    /// at face value each late print stepped this back and the next fresh one forward again,
    /// two stores and two wakes per late print instead of one a second, and in a replay a pass
    /// in between decided at the older second. So:
    ///
    /// - **Live** stores every print's second (the stalled-clock fallback in
    ///   [`Market::rocket_schedule`] wants a value a bogus far-future stamp cannot pin; the next
    ///   print overwrites it) but never wakes the task: it is paced by the wall, and a wake can
    ///   only make it look at the same schedule again.
    /// - **Replay** ignores a print up to [`LATE_PRINT_WINDOW_S`] behind the newest second (a
    ///   late arrival, not a move in time); one further back is followed down. A seek resets
    ///   the value anyway (`FeedItem::Reset`).
    fn wake_rocket(&self, ts: i64) {
        let sec = ts.div_euclid(1_000_000_000);
        let cur = self.0.rocket_print_sec.load(Ordering::Relaxed);
        if cur == sec {
            return;
        }
        if self.0.mode == ClockMode::Live {
            self.0.rocket_print_sec.store(sec, Ordering::Relaxed);
            return;
        }
        if cur != i64::MIN && sec < cur && cur - sec <= LATE_PRINT_WINDOW_S {
            return;
        }
        self.0.rocket_print_sec.store(sec, Ordering::Relaxed);
        self.0.rocket_wake.notify_one();
    }

    /// The market clock a Runners decision runs at (epoch ns). Live: the provider's clock is a
    /// wall sample taken once a second on a free-running timer, so it lags the wall by 0–1 s;
    /// it is carried forward by the wall time since it was set (at most
    /// [`CLOCK_EXTRAPOLATION_MAX`]), which is what the provider would read now. A burst boundary
    /// or a shelf slot (decided by this clock alone) is then decided when due, not up to a second
    /// later. Replay: the virtual clock (speed, pauses and steps are the driver's), or the start
    /// of the newest print's second when that is later: the driver sends its clock every 250 ms
    /// of wall time, and a print of the next second already proves this one over (a replay has
    /// no late prints). Without it the rocket task, woken by that print, would see the second
    /// still open (the Runners tape's newest print counts only in-band symbols) and decide it a
    /// second later.
    fn decision_now(&self, clock: &ClockState) -> i64 {
        if self.0.mode != ClockMode::Live {
            let printed = self
                .0
                .rocket_print_sec
                .load(Ordering::Relaxed)
                .saturating_mul(1_000_000_000);
            return clock.now.0.max(printed);
        }
        let since = self
            .0
            .clock_set
            .lock()
            .expect("clock stamp")
            .map_or(Duration::ZERO, |t| t.elapsed())
            .min(CLOCK_EXTRAPOLATION_MAX);
        clock.now.0.saturating_add(since.as_nanos() as i64)
    }

    /// The rocket task's schedule: the newest market second (epoch s) whose Runners decision is
    /// due, and how long to sleep before looking again once it is decided. Live, a second is due
    /// [`ROCKET_SETTLE`] after it ends by [`Market::decision_now`], and the sleep runs to the next
    /// such instant (live market time runs at wall speed). Replay, a second is due once the
    /// virtual clock or a print has moved past it (both in [`Market::decision_now`]); the sleep
    /// is the idle poll (the next print's wake usually comes first). `None` before the first
    /// clock.
    fn rocket_schedule(&self) -> Option<(i64, Duration)> {
        const SEC: i64 = 1_000_000_000;
        let clock = self.clock()?;
        let now = self.decision_now(&clock);
        if self.0.mode == ClockMode::Live {
            let settle = ROCKET_SETTLE.as_nanos() as i64;
            let due = (now - settle).div_euclid(SEC) - 1;
            let next = (due + 2) * SEC + settle;
            let wait = Duration::from_nanos((next - now).clamp(1_000_000, SEC) as u64);
            // A provider clock that stopped (past CLOCK_EXTRAPOLATION_MAX) while prints still
            // flow must not stop the board: a print of second P proves P - 2 over by a whole
            // second, so the board keeps deciding, a second late, off the prints alone (as the
            // 1 Hz tick did through the Runners tape's newest print).
            let printed = self.0.rocket_print_sec.load(Ordering::Relaxed);
            return Some((due.max(printed.saturating_sub(2)), wait));
        }
        Some((now.div_euclid(SEC) - 1, ROCKET_IDLE_POLL))
    }

    fn spawn_detection_task(&self) {
        let weak = Arc::downgrade(&self.0);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(250));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                let Some(inner) = weak.upgrade() else { break };
                Market(inner).publish_detection();
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
        let [assets_changed, closes_file_changed, ledger_changed] = files.changed();
        // A new split in the ledger moves a previous close exactly like a new close does.
        let closes_changed = closes_file_changed || ledger_changed;
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
                    let idx = idx.with_splits(load_splits(archive));
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
        if assets_changed || closes_changed {
            let luld = self.luld_inputs();
            self.0
                .core
                .lock()
                .expect("core lock")
                .engine
                .set_luld_inputs(Some(luld));
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

    /// The LULD model's inputs from the current `assets.json` and `daily-closes.json`.
    pub(crate) fn luld_inputs(&self) -> Arc<dyn LuldInputs> {
        let assets = self.0.assets.lock().expect("assets lock").clone();
        LuldMeta::shared(assets, self.current_closes())
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

    /// Benches only: sets the live market clock to `now`, as the provider's once-a-second
    /// `Clock` would. The memory bench replays a synthetic day far faster than real time, and the
    /// live provider stamps its own clock from the wall, so without this nothing that runs at the
    /// market clock (the Runners board, the shelves, fast moves) ever ticked in it: its shelves
    /// held every print they were given, and their bounds went unmeasured.
    #[doc(hidden)]
    pub fn bench_set_clock(&self, now: UnixNanos) {
        *self.0.clock_set.lock().expect("clock stamp") = Some(std::time::Instant::now());
        *self.0.clock.lock().expect("clock lock") = Some(ClockState {
            mode: self.0.mode,
            now,
            speed: market_providers::Speed::Times(1),
            playing: true,
            day: None,
            start: None,
            end: None,
            finished: false,
            events: 0,
        });
    }

    /// Benches only: what the live provider's once-a-wall-second idle tick delivers, the mark
    /// (`Advance`) and the `Clock`, through the real ingest. [`Market::bench_set_clock`] only
    /// sets the clock, so the engine's interval sweep and stock context's advance never ran in a
    /// bench, and growth paced by them went unmeasured.
    #[doc(hidden)]
    pub fn bench_tick(&self, now: UnixNanos) {
        self.apply(FeedItem::Advance(now));
        self.apply(FeedItem::Clock(ClockState {
            mode: self.0.mode,
            now,
            speed: market_providers::Speed::Times(1),
            playing: true,
            day: None,
            start: None,
            end: None,
            finished: false,
            events: 0,
        }));
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
        // FeedItem::Link does not carry provider receipt time. For a live connection change,
        // this wrapper samples actual local admission before taking core; replay/fixture uses
        // the run clock. The distinction is recorded as an input/telemetry limitation.
        let status_at = matches!(&item, FeedItem::Link(_) | FeedItem::Error(_)).then(|| {
            if self.0.mode == ClockMode::Live {
                UnixNanos(crate::observe::wall_now())
            } else {
                let fallback = self
                    .info()
                    .and_then(|i| i.day)
                    .and_then(|day| market_events::session::et(day, 4, 0, 0, 0));
                self.0
                    .core
                    .lock()
                    .expect("core lock")
                    .engine
                    .clock()
                    .or(fallback)
                    .unwrap_or_else(|| UnixNanos(crate::observe::wall_now()))
            }
        });
        match item {
            FeedItem::Event(e) => {
                // A continuing provider without Link messages has demonstrably delivered a
                // fresh input again. Close only the error gap, preserve it in history, and let
                // the core requalify from post-gap windows. A documented link-down requires Up.
                let recover_error = self
                    .0
                    .detection_error_recovery
                    .swap(false, Ordering::Relaxed)
                    && !self.0.detection_link_down.load(Ordering::Relaxed);
                if recover_error {
                    *self.0.status.lock().expect("status lock") = ProviderStatus {
                        state: "running",
                        detail: Some("provider resumed; recorded error gap retained".into()),
                    };
                }
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
                let context_closes = ((self.0.context_on.load(Ordering::Relaxed)
                    || self.0.detection_on.load(Ordering::Relaxed))
                    && matches!(e, MarketEvent::Trade(_)))
                .then(|| self.current_closes());
                let (seq, ups, rebuilding) = {
                    let mut core = self.0.core.lock().expect("core lock");
                    if recover_error && let Some(cx) = core.context.as_mut() {
                        cx.detection_status(
                            None,
                            market_providers::recorder::receive_time(&e),
                            true,
                            "provider_recovered",
                        );
                    }
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
                    if held && let Some(symbol) = e.symbol() {
                        if let Some(cx) = core.context.as_mut() {
                            cx.set_history_pending(symbol, true);
                        }
                        // Discovery owns its need for history even with no chart and explicit
                        // preload off. The existing bounded per-symbol request goes to the
                        // off-core loader; this lock performs no file or provider I/O.
                        //
                        // So does the Ross board: a symbol printing now is history-pending
                        // (RVOL UNKNOWN) until it loads, and the background preload is ordered by
                        // the archive's past volume, so a restart's gappers — printing today,
                        // quiet before — came last, minutes after the open. One request per
                        // symbol (`request_load` keeps a flag), moved to the front. Only when the
                        // background preload is on: with it off the operator asked for loads on
                        // demand, and the board reports the symbol pending until one.
                        let ross_wants = core.context.is_some()
                            && self.0.history.get().is_some_and(|h| h.preload);
                        // And so do buying bursts: the detector does not gate a symbol before its
                        // day is rebuilt (crate::rocket::burst, "After a restart"), so a symbol
                        // printing in its band must not wait for the background preload.
                        // Volume shelves too (crate::zones, "After a restart": a symbol has no
                        // shelf state until its day is adopted), but like the Ross board only
                        // when the background preload is on: then this only moves it forward.
                        let preload = self.0.history.get().is_some_and(|h| h.preload);
                        let burst_wants = matches!(&e, MarketEvent::Trade(t)
                            if core.rocket.burst_wants_history(t)
                                || (preload && core.zones.wants_history(t)));
                        if self.0.detection_on.load(Ordering::Relaxed) || ross_wants || burst_wants
                        {
                            crate::history::request_load(self, &mut core, symbol);
                        }
                    }
                    // The admission journal, before anything can refuse the event: the sequence
                    // has to count every input this process took, held or applied, or a restore
                    // reading it would see holes that were never drops.
                    let journal = self.0.admissions.get();
                    // Route normalized documentary status even when chart history is unloaded;
                    // a closed chart or a held symbol cannot mask a halt from discovery.
                    if let MarketEvent::Status(s) = &e
                        && let Some(cx) = core.context.as_mut()
                    {
                        match s.kind {
                            market_events::StatusKind::Halt => cx.detection_status(
                                s.symbol.as_deref(),
                                s.receive_ts.unwrap_or(s.event_ts),
                                false,
                                "halt",
                            ),
                            market_events::StatusKind::Resume => cx.detection_status(
                                s.symbol.as_deref(),
                                s.receive_ts.unwrap_or(s.event_ts),
                                true,
                                "resume",
                            ),
                            _ => {}
                        }
                    }
                    // Stock context sees every print once, on arrival: a held one with its own
                    // verdict (it reaches the chart engine later, never the context again).
                    if held
                        && let (MarketEvent::Trade(t), Some(closes)) = (&e, context_closes.as_ref())
                        && let Some(cx) = core.context.as_mut()
                    {
                        cx.apply_trade(t, Counted::from_effect(t), closes);
                    }
                    // A correction or cancel of a symbol whose history is still loading reaches
                    // the scanner too. There is no chart engine state to resolve it against,
                    // which is why the scanner resolves against its own admissions; leaving this
                    // path out kept the hole open for exactly the symbols a restart is loading.
                    if held
                        && matches!(e, MarketEvent::Correction(_) | MarketEvent::Cancel(_))
                        && let Some(cx) = core.context.as_mut()
                    {
                        cx.apply_revision(&e, None, revision_admit(&e));
                    }
                    if held {
                        // Alerts watch every live symbol, loaded into the engine or not.
                        if !core.rebuilding {
                            self.offer_alerts(&e, &[], None);
                        }
                        if let Some(j) = journal
                            && crate::admit::is_fact_input(&e)
                        {
                            // The chart has not classified a held print yet. Its condition-only
                            // provisional count is not an authoritative acceptance: a restore
                            // must derive the actual chart verdict and label it unproven.
                            j.record(&e, None, crate::observe::wall_now());
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
                            core.rocket.apply_trade(t, counted);
                            core.zones.apply_trade(t, counted);
                            core.mp5.apply_trade(t, counted);
                            self.wake_rocket(t.event_ts.0);
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
                                let (halted, resumed_at) = core.pressure.halt_state(&h.symbol);
                                core.rocket.set_halted(&h.symbol, halted, resumed_at);
                                // The exchange-time ns, else the ms stamp (an older wire).
                                let ns = |n: i64, ms: i64| if n != 0 { n } else { ms * 1_000_000 };
                                core.rocket.apply_halt_span(
                                    &h.symbol,
                                    ns(h.halt.start_ns, h.halt.start),
                                    h.halt.end.map(|e| ns(h.halt.end_ns.unwrap_or(0), e)),
                                );
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
                        if let Some(j) = journal
                            && crate::admit::is_fact_input(&e)
                        {
                            let v = match (&e, ups.first()) {
                                (MarketEvent::Trade(_), Some(ChartUpdate::Tape(row))) => {
                                    let c = Counted::from_tape(&row.counted);
                                    Some((c.ohlc, c.volume))
                                }
                                _ => None,
                            };
                            j.record(&e, v, crate::observe::wall_now());
                        }
                        // The scanner is told whether or not the chart engine could resolve it:
                        // it resolves against its own admissions and uses the chart engine's
                        // answer only for the date's last price and high.
                        if matches!(e, MarketEvent::Correction(_) | MarketEvent::Cancel(_))
                            && let Some(cx) = core.context.as_mut()
                        {
                            cx.apply_revision(&e, revision.as_ref(), revision_admit(&e));
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
                    // A clock advance decides which barrier folds the prints admitted before it,
                    // so it is part of the order a restore has to reproduce.
                    if let Some(j) = self.0.admissions.get() {
                        j.record_clock(t, crate::observe::wall_now());
                    }
                    let ups = core.engine.advance_clock(t);
                    // The clock moves the bands too (the closing doubling window, the close, the
                    // end of a reference's 30 s hold, 09:35 without an opening print).
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
                self.0
                    .detection_error_recovery
                    .store(false, Ordering::Relaxed);
                self.0.detection_link_down.store(false, Ordering::Relaxed);
                // The visible clock stays where it was until the rebuild's closing `Clock`, so a
                // client never sees the target time over a half-rebuilt engine.
                {
                    let closes = self.current_closes();
                    // A seek rebuilds stock context from the re-read day, like the candles.
                    let board = self.new_context_board();
                    let rocket = self.new_rocket_board();
                    let mut core = self.0.core.lock().expect("core lock");
                    let seq = core.seq + 1;
                    // The slots keep their studies across the rebuild; only the values go (they
                    // were computed over the old engine), and the resync snapshots rebuild them.
                    let mut indicators = std::mem::take(&mut core.indicators);
                    indicators.clear_computed();
                    let mut chart_context = std::mem::take(&mut core.chart_context);
                    chart_context.clear_computed();
                    // rw-mp5-1's shares are the live day's (a replay never turns it on): kept.
                    let mp5 = std::mem::take(&mut core.mp5);
                    *core = Core::new(closes, self.luld_inputs());
                    core.mp5 = mp5;
                    core.indicators = indicators;
                    core.chart_context = chart_context;
                    core.context = board;
                    core.rocket = rocket;
                    core.zones = self.new_zone_board();
                    core.seq = seq;
                    core.rebuilding = true;
                    // The replay went elsewhere in time: its newest print is no longer "now"
                    // (`Market::decision_now`); the rebuild's prints set it again.
                    self.0.rocket_print_sec.store(i64::MIN, Ordering::Relaxed);
                }
                self.refresh_detection(true);
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
                *self.0.clock_set.lock().expect("clock stamp") = Some(std::time::Instant::now());
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
                        // Likewise every chart.levels / chart.zones goes out fresh with the
                        // resync snapshots. What a snapshot taken mid-rebuild left behind (the
                        // values the skip rules compare with, the "last sent" a later change is
                        // compared with) belongs to a partial engine that the rebuild's
                        // increments, which the study does not follow, have since moved past.
                        core.chart_context.clear_computed();
                    }
                    (core.seq, resync)
                };
                if resync {
                    self.refresh_detection(true);
                    // The rebuild is over: the rocket task, whose passes it refused, decides at
                    // the target time now rather than at its next idle poll.
                    self.0.rocket_wake.notify_one();
                }
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
                self.0.detection_error_recovery.store(
                    self.0.detection_on.load(Ordering::Relaxed)
                        && !self.0.detection_link_down.load(Ordering::Relaxed),
                    Ordering::Relaxed,
                );
                if let Some(cx) = self.0.core.lock().expect("core lock").context.as_mut() {
                    cx.detection_status(
                        None,
                        status_at.expect("status time"),
                        false,
                        "provider_error",
                    );
                }
                tracing::error!(error = %e, "market data provider error");
                *self.0.status.lock().expect("status lock") = ProviderStatus {
                    state: "error",
                    detail: Some(e),
                };
            }
            FeedItem::Link(l) => {
                match l.state {
                    LinkState::Up => {
                        self.0.detection_link_down.store(false, Ordering::Relaxed);
                        self.0
                            .detection_error_recovery
                            .store(false, Ordering::Relaxed);
                    }
                    LinkState::Down => {
                        self.0.detection_link_down.store(true, Ordering::Relaxed);
                        self.0
                            .detection_error_recovery
                            .store(false, Ordering::Relaxed);
                    }
                    LinkState::Connecting => {}
                }
                if let Some(cx) = self.0.core.lock().expect("core lock").context.as_mut() {
                    match l.state {
                        LinkState::Up => cx.detection_status(
                            None,
                            status_at.expect("status time"),
                            true,
                            "link_up",
                        ),
                        LinkState::Down => cx.detection_status(
                            None,
                            status_at.expect("status time"),
                            false,
                            "link_down",
                        ),
                        LinkState::Connecting => {}
                    }
                }
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
        // Take every significant core transition whether anyone is watching or not. JSON,
        // actual emission stamping and durable append happen after core has been released.
        self.refresh_detection(false);
    }

    /// [`Market::serialize`], except during a rebuild, where nothing is broadcast: then only the
    /// tape rows are serialized (for the per-symbol history the next snapshot sends) and the
    /// candle increments are dropped unserialized. Same state either way; it only makes a rebuild
    /// cheaper (a seek, a day switch, and above all the live journal's restore of a whole-market
    /// day, where serializing every increment was about a third of the time).
    fn serialize_unless_rebuilding(core: &mut Core, ups: Vec<ChartUpdate>) -> Vec<OutKind> {
        // The indicators follow every increment, also during a rebuild: a series built by a
        // snapshot taken mid-rebuild has to stay in step with the engine until the resync.
        // A symbol with no chart is one hash lookup; no studies at all, nothing.
        let indicators = core.indicators.on_updates(&ups);
        if !core.rebuilding {
            // Levels and zones are recomputed from the engine, never folded, so a rebuild does not
            // need them in step: its resync snapshots compute them afresh.
            let context = {
                let Core {
                    engine,
                    quotes,
                    chart_context,
                    ..
                } = &mut *core;
                let pc = |s: &str, d: NaiveDate| quotes.prev_close_on(s, d).map(Price::to_f64);
                chart_context.on_updates(engine, &pc, &ups, std::time::Instant::now())
            };
            let mut out = Self::serialize(core, ups);
            // After the candle frames of the same apply, so a client has the bar before its
            // values (it joins them by `t`).
            out.extend(indicators.into_iter().map(|(k, json)| OutKind::Indicator {
                symbol: k.symbol,
                timeframe: k.timeframe,
                rth: k.rth,
                json,
            }));
            out.extend(context.into_iter().map(OutKind::from));
            return out;
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
        // Routed to Alpaca paper: the simulator's frames would replace the clients' Alpaca state.
        if self.paper_route().is_some() {
            return;
        }
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
        if let Some(route) = self.paper_route() {
            return route.state_frame();
        }
        let paper = self.0.paper.lock().expect("paper lock");
        serde_json::to_string(&paper.state()).expect("paper state serializes")
    }

    /// The order route, when this (live) market sends chart orders to Alpaca paper.
    pub fn paper_route(&self) -> Option<alpaca_paper::Service> {
        self.0.paper_route.read().expect("route lock").clone()
    }

    /// Switches where chart orders go, then sends every connection the now-current
    /// `paper.state` (clients replace their copy). Refused (false) on any market that is not
    /// live: replay orders never reach a broker.
    pub fn set_paper_route(&self, route: Option<alpaca_paper::Service>) -> bool {
        if route.is_some() && self.mode() != ClockMode::Live {
            tracing::error!(
                "refused to route a {:?} market's orders to Alpaca paper",
                self.mode()
            );
            return false;
        }
        {
            let mut r = self.0.paper_route.write().expect("route lock");
            if r.is_none() && route.is_none() {
                return true;
            }
            *r = route;
        }
        // The write lock is released first: the route's own publisher reads it.
        let frame = self.paper_state_frame();
        let _ = self.0.tx.send(Arc::new(Outbound {
            seq: 0,
            kind: OutKind::Paper(frame),
        }));
        true
    }

    /// Frames from the Alpaca paper route; dropped unless it is this market's current route
    /// (the owner switched back to the simulator).
    pub fn publish_route_frames(&self, frames: Vec<String>) {
        if self.paper_route().is_none() {
            return;
        }
        for frame in frames {
            let _ = self.0.tx.send(Arc::new(Outbound {
                seq: 0,
                kind: OutKind::Paper(frame),
            }));
        }
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
        self.snapshot_with(symbol, tf, bin, None)
    }

    /// [`Market::snapshot`] plus, for a slot holding `studies`, its `chart.indicators` taken under
    /// the same core lock: one seq for both, so every `chart.indicator` with a larger seq applies
    /// on top of it, exactly as `chart.update`s do on the candles.
    pub fn snapshot_with(
        &self,
        symbol: &str,
        tf: Timeframe,
        bin: Price,
        studies: Option<&Studies>,
    ) -> Result<SnapshotOut, SnapshotError> {
        let clock_state = self.clock();
        let clock = clock_state.as_ref().map(clock_info);
        let mut guard = self.0.core.lock().expect("core lock");
        let core = &mut *guard;
        if self.known(core, symbol) && !Self::candidate_in(core, self.0.band, symbol) {
            return Err(SnapshotError::OutsidePriceBand);
        }
        // Live: a symbol whose history is still in the archive gets an empty chart now and its
        // load jumps the queue; the `Loaded` broadcast then sends this connection the real one.
        crate::history::request_load(self, core, symbol);
        let mut snap = match core.engine.snapshot(symbol, tf, Some(bin)) {
            Some(s) => s,
            None if !self.known(core, symbol) => {
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
                sessions: Vec::new(),
            },
        };
        snap.clock = clock;
        if let Some(c) = &clock_state {
            snap.sessions = schedule_through(std::mem::take(&mut snap.sessions), c.now);
        }
        snap.prev_close = self.prev_close_for(core, &snap).map(Price::to_f64);
        let indicators = studies.map(|s| core.indicators.snapshot(&core.engine, s));
        Ok(SnapshotOut {
            seq: core.seq,
            json: serde_json::to_string(&snap).expect("snapshot serializes"),
            tape: core
                .tape
                .get(symbol)
                .map(|r| r.iter().cloned().collect())
                .unwrap_or_default(),
            indicators,
        })
    }

    /// Counts a slot's `studies` in the indicator registry's demand until the lease is dropped
    /// (the slot changes its studies, unsubscribes, or the connection ends, however it ends).
    pub fn indicators_acquire(&self, studies: Studies) -> IndicatorLease {
        self.0
            .core
            .lock()
            .expect("core lock")
            .indicators
            .acquire(&studies);
        IndicatorLease {
            inner: self.0.clone(),
            studies,
        }
    }

    /// A `chart.indicators` frame alone (`chart.indicators.set`, or a slot whose candles shared
    /// another slot's snapshot), with the engine seq it is consistent with.
    pub fn indicators_snapshot(&self, studies: &Studies) -> (u64, String) {
        let mut guard = self.0.core.lock().expect("core lock");
        let core = &mut *guard;
        (core.seq, core.indicators.snapshot(&core.engine, studies))
    }

    /// Counts a slot's levels or zones demand (crate::chart_context) until the lease is dropped.
    pub fn context_acquire(&self, key: ContextKey) -> ContextLease {
        self.0
            .core
            .lock()
            .expect("core lock")
            .chart_context
            .acquire(&key);
        ContextLease {
            inner: self.0.clone(),
            key,
        }
    }

    /// A `chart.levels` frame for `key`, computed afresh, with the engine seq it is current to: a
    /// broadcast `Levels` with a larger seq is newer.
    pub fn levels_snapshot(&self, key: &LevelsKey) -> (u64, String) {
        let mut guard = self.0.core.lock().expect("core lock");
        let Core {
            engine,
            quotes,
            chart_context,
            seq,
            ..
        } = &mut *guard;
        let pc = |s: &str, d: NaiveDate| quotes.prev_close_on(s, d).map(Price::to_f64);
        (*seq, chart_context.levels_frame(engine, &pc, key))
    }

    /// A `chart.zones` frame for `key`, computed afresh, with the engine seq it is current to.
    pub fn zones_snapshot(&self, key: &ChartKey) -> (u64, String) {
        let mut guard = self.0.core.lock().expect("core lock");
        let core = &mut *guard;
        let json = core
            .chart_context
            .zones_frame(&core.engine, key, std::time::Instant::now());
        (core.seq, json)
    }

    /// The zones whose forming bar moved since their last computation (crate::chart_context,
    /// at most once a second each), and the levels a slot's snapshot found ahead of what the
    /// other slots hold: from the 1 Hz broadcast tick. Public so tests can tick.
    pub fn flush_chart_zones(&self) {
        let (seq, frames) = {
            let mut guard = self.0.core.lock().expect("core lock");
            let Core {
                engine,
                quotes,
                chart_context,
                rebuilding,
                seq,
                ..
            } = &mut *guard;
            if *rebuilding {
                return;
            }
            let pc = |s: &str, d: NaiveDate| quotes.prev_close_on(s, d).map(Price::to_f64);
            let frames = chart_context.flush(engine, &pc, std::time::Instant::now());
            (*seq, frames)
        };
        self.send_all(seq, frames.into_iter().map(OutKind::from).collect());
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
        core.quotes.prev_close_on(&snap.symbol, day)
    }

    /// The candle increment `item` (whose `update` and `json` are passed in) as a connection
    /// watching it at profile bin `bin` must get it: the touched bins re-binned with absolute
    /// totals, plus the whole profile's `total`, `poc` and `valueArea` whenever rows changed
    /// (crate::profile_frames, which says why this happens here and how the reads are shared).
    /// A frame that changes no rows at the default bin goes out exactly as serialized.
    pub fn chart_frame(
        &self,
        item: &Arc<Outbound>,
        update: &ChartUpdate,
        json: &str,
        bin: Price,
    ) -> String {
        let Some((session, rows)) = crate::profile_frames::profile_of(update) else {
            return json.to_string();
        };
        if rows.is_empty() && bin == self.0.bin {
            return json.to_string();
        }
        if let Some(hit) = self.frames().frame(item, bin) {
            return hit;
        }
        let read = match session.parse::<SessionId>() {
            Ok(s) if !rows.is_empty() => self.profile_read(update.symbol(), s, bin, item.seq),
            _ => None,
        };
        let u = crate::profile_frames::build(update, bin, read.as_ref());
        let out = serde_json::to_string(&u).expect("wire types serialize");
        self.frames().put_frame(item, bin, out.clone());
        out
    }

    fn frames(&self) -> std::sync::MutexGuard<'_, crate::profile_frames::FrameCache> {
        self.0.frame_cache.lock().expect("frame cache lock")
    }

    /// The whole profile of (symbol, session) at `bin`, current for a frame at `seq`: from the
    /// cache, else read under the core lock (the cache lock is never held across it).
    fn profile_read(
        &self,
        symbol: &str,
        session: SessionId,
        bin: Price,
        seq: u64,
    ) -> Option<crate::profile_frames::ProfileRead> {
        if let Some(hit) = self.frames().read(symbol, session, bin, seq) {
            return Some(hit);
        }
        let (core_seq, rows) = {
            let core = self.0.core.lock().expect("core lock");
            (core.seq, core.engine.profile_rows(symbol, session, bin)?)
        };
        let read = crate::profile_frames::ProfileRead {
            seq: core_seq,
            derived: chart_engine::profile::derive(&rows, chart_engine::VALUE_AREA_PERCENT),
            rows: Arc::new(rows),
        };
        self.frames().put_read(symbol, session, bin, read.clone());
        Some(read)
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
        let t = self.clock().map(|c| c.now);
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
        let t = self.clock().map(|c| c.now);
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

    /// Registers a `rocket.subscribe` (the pressure guard's twin).
    pub fn rocket_subscribe(&self) -> RocketGuard {
        self.0.rocket_subs.fetch_add(1, Ordering::Relaxed);
        RocketGuard(self.0.clone())
    }

    /// Connections currently subscribed to Rocket watch.
    pub fn rocket_subscribers(&self) -> usize {
        self.0.rocket_subs.load(Ordering::Relaxed)
    }

    /// Whether `symbol` has a row on today's Runners board (the board clears its rows at the day
    /// roll): part of the halt notices' coverage (`crate::alerts::halts`). Asked only at a halt or
    /// resume status, so the core lock is taken a handful of times a day.
    pub fn on_runners_board(&self, symbol: &str) -> bool {
        let core = self.0.core.lock().expect("core lock");
        core.rocket.alerts().iter().any(|a| &*a.symbol == symbol)
    }

    /// The Rocket watch panel as a `rocket.snapshot` frame, with the engine seq it is consistent
    /// with (`enabled: false` with `ROCKET_WATCH=0`). Every row carries its whole day line.
    pub fn rocket_snapshot(&self) -> (u64, String) {
        let clock = self.clock();
        let t = clock.as_ref().map(|c| c.now.millis());
        let tau = clock.as_ref().map_or(0, |c| c.now.0);
        let mut core = self.0.core.lock().expect("core lock");
        if !core.rocket.enabled() {
            return (core.seq, crate::rocket::disabled_frame("rocket.snapshot"));
        }
        let mut body = core.rocket.body(&|s| Self::rocket_quote(&core, s), &|s| {
            self.rocket_levels(&core, s, tau)
        });
        Self::add_zones(&core, &mut body, tau);
        self.add_demand_zones(&mut core, &mut body);
        crate::time_marks::stamp_rows(&mut body, tau);
        let mut v = serde_json::json!({"type": "rocket.snapshot", "t": t});
        crate::time_marks::stamp_frame_ns(&mut v, clock.as_ref().map(|c| c.now.0));
        if let (Some(m), serde_json::Value::Object(b)) = (v.as_object_mut(), body) {
            m.extend(b);
        }
        let src = crate::day_line::Source {
            engine: &core.engine,
            closes: core.quotes.closes(),
        };
        crate::day_line::decorate_by(&mut v, "s", src, None);
        (core.seq, v.to_string())
    }

    /// The volume shelves (`crate::zones`) as the Runners frame's `zones` key, when they are on.
    /// In the body, so a shelf that changes sends an update like a row that changes.
    fn add_zones(core: &Core, body: &mut serde_json::Value, now: i64) {
        if core.zones.enabled()
            && let Some(m) = body.as_object_mut()
        {
            m.insert("zones".into(), core.zones.json(now));
        }
    }

    /// The demand-zone mark on Runners rows (`crate::auto_zones`): in the body, so a row entering
    /// or leaving a zone sends an update like any other row change.
    fn add_demand_zones(&self, core: &mut Core, body: &mut serde_json::Value) {
        let Core {
            engine,
            chart_context,
            ..
        } = core;
        self.0
            .rocket_auto_zones
            .lock()
            .expect("rocket auto zones")
            .decorate(chart_context, engine, body, std::time::Instant::now());
    }

    fn rocket_quote(core: &Core, symbol: &str) -> crate::rocket::Quote {
        (
            core.last_price.get(symbol).map(|p| p.to_f64()),
            core.quotes
                .get(symbol)
                .and_then(|r| r.prev_close)
                .map(|p| p.to_f64()),
        )
    }

    /// A Runners row's micro-pullback levels (`STOCK_CONTEXT=1` only).
    fn rocket_levels(&self, core: &Core, symbol: &str, tau: i64) -> Option<serde_json::Value> {
        core.context
            .as_ref()
            .filter(|_| self.0.context_on.load(Ordering::Relaxed))
            .and_then(|cx| cx.runner_levels(symbol, tau))
    }

    /// One Rocket watch decision and, when it changed what the panel shows, one `rocket.update`
    /// ([`Market::decide_rocket`] then [`Market::broadcast_rocket`]). Returns whether it
    /// broadcast. Public so tests can tick; the server runs the two halves on their own tasks
    /// (`spawn_rocket_task`, `spawn_quotes_task`).
    pub fn publish_rocket(&self) -> bool {
        self.decide_rocket();
        self.broadcast_rocket()
    }

    /// One Rocket watch step at the market clock ([`Market::decision_now`];
    /// `crate::rocket::RocketBoard::tick`: at most one 15 s barrier, scored only once it
    /// completed, the second that just ended for fast moves, the due burst boundaries) and the
    /// forward-log appends it produced (live, after the core lock is released). Runs with no
    /// subscriber too: the alerts, the momentum states and the forward log must not depend on a
    /// panel being open. The board's new flags and breakouts go to the phone push
    /// (`crate::alerts::runners`) on the LIVE market only, offered after the lock is released and
    /// never waited on; a replay's are dropped. Returns whether it decided (false with no clock
    /// yet, during a replay's rebuild, or with the board off): the rocket task keeps a second it
    /// could not decide due. Public so tests can decide without a frame.
    pub fn decide_rocket(&self) -> bool {
        let Some(clock) = self.clock() else {
            return false;
        };
        let now = self.decision_now(&clock);
        let (log, notices, adv_wanted, facts_wanted, n1_wanted, zones_log, mp5) = {
            let mut core = self.0.core.lock().expect("core lock");
            if core.rebuilding || !core.rocket.enabled() {
                return false;
            }
            // The shelves first, so their +20 % runs become Runners rows at this tick
            // (crate::rocket::run).
            core.zones.tick(now);
            let zones_log = core.zones.take_log();
            let Core {
                rocket,
                quotes,
                context,
                zones,
                ..
            } = &mut *core;
            rocket.add_runs(zones.take_entries());
            rocket.add_shelf_entries(zones.take_shelf_entries());
            rocket.tick(now, |s| {
                quotes.get(s).and_then(|r| r.prev_close).map(|p| p.to_f64())
            });
            // The flagged symbols get the micro-pullback too (a few dozen at most a day).
            if let Some(w) = rocket.take_watch()
                && let Some(cx) = context.as_mut()
            {
                cx.set_watched(w);
            }
            let log = core.rocket.take_log();
            // Taken whatever the market, so a replay's never pile up.
            let notices = core.rocket.take_notices();
            let adv_wanted = core.rocket.take_burst_adv_request();
            let facts_wanted = core.rocket.take_running_facts_request();
            let n1_wanted = core.rocket.take_n1_facts_request();
            core.mp5.tick(now);
            let mp5 = (core.mp5.take_table_request(), core.mp5.take_finished());
            (
                log,
                notices,
                adv_wanted,
                facts_wanted,
                n1_wanted,
                zones_log,
                mp5,
            )
        };
        // First, before the file appends and the once-a-day archive reads: seconds matter on a
        // +20 % leg.
        self.offer_notices(notices);
        if let Some(l) = self.0.zones_log.get() {
            let mut by_date: Vec<(NaiveDate, Vec<String>)> = Vec::new();
            for (d, line) in zones_log {
                match by_date.last_mut() {
                    Some((ld, v)) if *ld == d => v.push(line),
                    _ => by_date.push((d, vec![line])),
                }
            }
            for (d, lines) in by_date {
                l.append(d, &lines);
            }
        }
        if let Some(date) = adv_wanted {
            self.load_burst_adv(date);
        }
        if let Some(date) = facts_wanted {
            self.load_running_facts(date);
        }
        if let Some(date) = n1_wanted {
            self.load_n1_facts(date);
        }
        if let Some(date) = mp5.0 {
            self.load_mp5_table(date);
        }
        if let Some(f) = mp5.1 {
            self.write_mp5_day(f);
        }
        if let Some(l) = self.0.rocket_log.get() {
            let mut by_date: Vec<(NaiveDate, Vec<String>)> = Vec::new();
            for (d, line) in log {
                match by_date.last_mut() {
                    Some((ld, v)) if *ld == d => v.push(line),
                    _ => by_date.push((d, vec![line])),
                }
            }
            for (d, lines) in by_date {
                l.append(d, &lines);
            }
        }
        true
    }

    /// One `rocket.update` when the panel or a row's day line changed since the last one and
    /// anyone is subscribed: what [`Market::decide_rocket`] last decided, at the clients' 1 Hz
    /// (the broadcast tick). Decides nothing. Returns whether it broadcast.
    pub fn broadcast_rocket(&self) -> bool {
        if self.rocket_subscribers() == 0 {
            self.0.runner_events.lock().expect("runner events").reset();
            return false;
        }
        let Some(clock) = self.clock() else {
            return false;
        };
        let (seq, frame, events) = {
            let mut core = self.0.core.lock().expect("core lock");
            if core.rebuilding || !core.rocket.enabled() {
                // The rows are being rebuilt from the tape: their appearance is not news.
                self.0.runner_events.lock().expect("runner events").reset();
                return false;
            }
            let mut frame = None;
            let mut body = core.rocket.body(&|s| Self::rocket_quote(&core, s), &|s| {
                self.rocket_levels(&core, s, clock.now.0)
            });
            Self::add_zones(&core, &mut body, clock.now.0);
            self.add_demand_zones(&mut core, &mut body);
            crate::time_marks::stamp_rows(&mut body, clock.now.0);
            // Toasts only from the live market (a replay never pushes, crate::time_marks).
            let events = self.0.runner_events.lock().expect("runner events").take(
                &body,
                clock.now.0,
                self.0.mode == ClockMode::Live,
            );
            let mut v = serde_json::json!({"type": "rocket.update", "t": clock.now.millis()});
            crate::time_marks::stamp_frame_ns(&mut v, Some(clock.now.0));
            if let (Some(m), serde_json::Value::Object(b)) = (v.as_object_mut(), body.clone()) {
                m.extend(b);
            }
            // Day lines as slices against the last update's (crate::day_line); committed
            // only when this update is sent.
            let mut lines = self.0.rocket_lines.lock().expect("rocket lines");
            let mut next = lines.clone();
            let src = crate::day_line::Source {
                engine: &core.engine,
                closes: core.quotes.closes(),
            };
            crate::day_line::decorate_by(&mut v, "s", src, Some(&mut next));
            let mut last = self.0.rocket_published.lock().expect("rocket published");
            if last.as_ref() != Some(&body) || *lines != next {
                *last = Some(body);
                *lines = next;
                frame = Some(v.to_string());
            }
            (core.seq, frame, events)
        };
        // The panel first, so a toast's row is already on the client's panel.
        let sent = if let Some(frame) = frame {
            let _ = self.0.tx.send(Arc::new(Outbound {
                seq,
                kind: OutKind::Rocket(frame),
            }));
            true
        } else {
            false
        };
        for e in events {
            let _ = self.0.tx.send(Arc::new(Outbound {
                seq,
                kind: OutKind::RunnersEvent(e),
            }));
        }
        sent
    }

    /// Push notices (Runners, Ross, Signals, Halt watcher) to the alerts task
    /// (`crate::alerts::runners`): on the LIVE market only, called after the core lock is
    /// released, never waited on. A replay's are dropped.
    fn offer_notices(&self, notices: Vec<crate::rocket::Notice>) {
        if notices.is_empty() || self.0.mode != ClockMode::Live {
            return;
        }
        if let Some(sink) = self.alert_sink() {
            for n in notices {
                sink.offer_runner(n);
            }
        }
    }

    /// A Runners row's fire-time spread (`crate::spread`'s stage, after its notice was priced):
    /// the row shows it from the next `rocket.update` (the body changed, so one goes out).
    /// Display only. Takes the core lock briefly, from the stage's task, never from ingest.
    pub fn set_runner_spread(&self, alert_id: &str, bp: i32) {
        self.0
            .core
            .lock()
            .expect("core lock")
            .rocket
            .set_spread(alert_id, bp);
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

    /// Registers a `favorites.subscribe` (the Ross guard's twin).
    pub fn favorites_subscribe(&self) -> FavoritesGuard {
        self.0.favorites_subs.fetch_add(1, Ordering::Relaxed);
        FavoritesGuard(self.0.clone())
    }

    /// The owner's favorites (`crate::favorites::follow`). The next tick sends an update, and the
    /// stock-context engine starts watching them (their rows' micro-pullback levels).
    pub fn set_favorites(&self, list: Vec<String>) {
        {
            let mut b = self.0.favorites.lock().expect("favorites lock");
            if b.list == list {
                return;
            }
            b.list = list;
            b.list_changed = true;
        }
        // The engine side follows on the next tick (`sync_favorites`), under the core lock.
    }

    /// The favorites' rows (`crate::favorites::row`) at `tau`, in the list's order.
    fn favorites_rows(&self, core: &Core, list: &[String], tau: i64) -> Vec<Value> {
        let context = core
            .context
            .as_ref()
            .filter(|_| self.0.context_on.load(Ordering::Relaxed));
        list.iter()
            .map(|s| {
                let quote = core.quotes.get(s);
                let last = core.last_price.get(s.as_str()).copied();
                crate::favorites::row(
                    &crate::favorites::RowFacts {
                        symbol: s.clone(),
                        price: last
                            .map(|p| p.micros())
                            .or(quote.as_ref().map(|q| q.last.micros())),
                        prev_close: quote
                            .as_ref()
                            .and_then(|q| q.prev_close)
                            .map(|p| p.micros()),
                        reference: quote.as_ref().map(|q| q.reference.micros()),
                        basis: quote.as_ref().map(|q| q.basis).unwrap_or_default(),
                        volume: quote.as_ref().map(|q| q.volume),
                        trades: quote.as_ref().map(|q| q.trades),
                        last_trade_ns: core.quotes.last_trade_ns(s),
                        extremes: core.engine.day_extremes(s, crate::day_line::STEP),
                        runners: core.rocket.symbol_context(s, last.map(|p| p.to_f64())),
                        levels: self.rocket_levels(core, s, tau),
                        pillars: context.and_then(|cx| cx.pillars(s)),
                        halted: core.rocket.is_halted(s),
                    },
                    tau,
                )
            })
            .collect()
    }

    /// The favorites as a `favorites.snapshot` frame with whole day lines, and the favorites
    /// generation it is consistent with (`crate::favorites::Board::generation`).
    pub fn favorites_snapshot(&self) -> (u64, String) {
        let clock = self.clock();
        let tau = clock.as_ref().map_or(0, |c| c.now.0);
        let core = self.0.core.lock().expect("core lock");
        let (list, generation) = {
            let b = self.0.favorites.lock().expect("favorites lock");
            (b.list.clone(), b.generation)
        };
        let rows = self.favorites_rows(&core, &list, tau);
        let mut v = json!({
            "type": "favorites.snapshot",
            "t": clock.as_ref().map(|c| c.now.millis()),
            "tNs": clock.as_ref().map(|c| c.now.0),
            "symbols": list,
            "rows": rows,
        });
        let src = crate::day_line::Source {
            engine: &core.engine,
            closes: core.quotes.closes(),
        };
        crate::day_line::decorate(&mut v, src, None);
        (generation, v.to_string())
    }

    /// Keeps the stock-context engine's favorites equal to the list (a rebuilt core starts with
    /// none), then one `favorites.update` when a row, a day line or the list changed and anyone is
    /// subscribed. Called every tick, subscribed or not, for the first part.
    pub fn publish_favorites(&self) {
        let clock = self.clock();
        let tau = clock.as_ref().map_or(0, |c| c.now.0);
        let (seq, frame) = {
            let mut core = self.0.core.lock().expect("core lock");
            let mut board = self.0.favorites.lock().expect("favorites lock");
            if let Some(cx) = core.context.as_mut()
                && cx.favorites() != board.list.as_slice()
            {
                cx.set_favorites(&board.list);
            }
            if core.rebuilding || self.0.favorites_subs.load(Ordering::Relaxed) == 0 {
                return;
            }
            let list = board.list.clone();
            let rows = Value::Array(self.favorites_rows(&core, &list, tau));
            let rows_changed = board.rows.as_ref() != Some(&rows);
            let mut v = json!({
                "type": "favorites.update",
                "t": clock.as_ref().map(|c| c.now.millis()),
                "tNs": clock.as_ref().map(|c| c.now.0),
                "symbols": list,
                "rows": rows.clone(),
            });
            let src = crate::day_line::Source {
                engine: &core.engine,
                closes: core.quotes.closes(),
            };
            crate::day_line::decorate(&mut v, src, Some(&mut board.lines));
            if !rows_changed && !board.list_changed && !crate::favorites::has_new_points(&v) {
                return;
            }
            board.rows = Some(rows);
            board.list_changed = false;
            board.generation += 1;
            (board.generation, v.to_string())
        };
        let _ = self.0.tx.send(Arc::new(Outbound {
            seq,
            kind: OutKind::Favorites(frame),
        }));
    }

    /// Registers a `movers.subscribe` (the favorites guard's twin).
    pub fn movers_subscribe(&self) -> MoversGuard {
        self.0.movers_subs.fetch_add(1, Ordering::Relaxed);
        MoversGuard(self.0.clone())
    }

    /// Connections currently subscribed to the movers.
    pub fn movers_subscribers(&self) -> usize {
        self.0.movers_subs.load(Ordering::Relaxed)
    }

    /// What the movers board holds (the memory report): rows of the last ranking and day lines
    /// of the last update, both bounded by the lists (`crate::movers`, "Cost and memory").
    fn movers_cost(&self) -> Value {
        let b = self.0.movers.lock().expect("movers lock");
        json!({"rows": b.last.rows.len(), "lines": b.lines.len()})
    }

    /// One ticker search over the quote board (`quotes.search.v1`, crate::quote_search), with
    /// the market clock it was taken at. `None` during a rebuild: the board is being
    /// refilled and a result cut from it would flicker; the `Resync` at its end resends every
    /// search's snapshot.
    pub fn quote_search(
        &self,
        q: &crate::quote_search::Query,
    ) -> Option<(Option<UnixNanos>, crate::quote_search::Results)> {
        let t = self.clock().map(|c| c.now);
        let assets = self.assets();
        let core = self.0.core.lock().expect("core lock");
        if core.rebuilding {
            return None;
        }
        Some((
            t,
            crate::quote_search::search(
                &core.quotes,
                self.0.band,
                &assets,
                &|s| core.context.as_ref().and_then(|cx| cx.float_shares(s)),
                q,
            ),
        ))
    }

    /// `assets.json` as loaded (a cheap `Arc` clone): names and exchanges for search frames.
    pub fn assets(&self) -> Arc<HashMap<String, AssetMeta>> {
        self.0.assets.lock().expect("assets lock").clone()
    }

    /// The `quotes.directory` frame: every candidate with its name and exchange.
    pub fn quote_directory(&self) -> String {
        crate::quote_search::directory_frame(&self.symbols(), &self.assets())
    }

    /// The last ranking as a `movers.snapshot` with whole day lines, and the movers generation it
    /// is consistent with: an update with a larger one is newer than it.
    pub fn movers_snapshot(&self) -> (u64, String) {
        let now = self.clock().map(|c| c.now);
        let core = self.0.core.lock().expect("core lock");
        let board = self.0.movers.lock().expect("movers lock");
        let mut v = crate::movers::snapshot_frame(now.map(UnixNanos::millis), &board.last);
        crate::time_marks::stamp_frame_ns(&mut v, now.map(UnixNanos::nanos));
        let src = crate::day_line::Source {
            engine: &core.engine,
            closes: core.quotes.closes(),
        };
        crate::day_line::decorate(&mut v, src, None);
        (board.generation, v.to_string())
    }

    /// One movers tick (`crate::movers`): ranks the quote board once for every client, and when
    /// anyone is subscribed and something changed, broadcasts ONE `movers.update` with only the
    /// changed rows and day-line slices. Ranks with no subscriber too, so a new subscriber's
    /// snapshot is cut from a current ranking; the day lines are read only for subscribers.
    /// Throttled to one ranking per [`crate::movers::MIN_GAP`] of wall time. Returns whether it
    /// broadcast.
    pub fn publish_movers(&self) -> bool {
        self.publish_movers_at(std::time::Instant::now())
    }

    /// [`Market::publish_movers`] at a given wall instant (tests step it past the throttle).
    pub fn publish_movers_at(&self, now: std::time::Instant) -> bool {
        let clock_now = self.clock().map(|c| c.now);
        let t = clock_now.map(UnixNanos::millis);
        let (generation, frame) = {
            let core = self.0.core.lock().expect("core lock");
            if core.rebuilding {
                return false;
            }
            let mut board = self.0.movers.lock().expect("movers lock");
            if !board.throttle.ready(now) {
                return false;
            }
            let next = crate::movers::rank(&core.quotes, |s| core.rocket.is_halted(s));
            let d = crate::movers::diff(&board.last, &next);
            let mut frame = None;
            if self.movers_subscribers() > 0 {
                let mut all = crate::movers::member_rows(&next);
                let src = crate::day_line::Source {
                    engine: &core.engine,
                    closes: core.quotes.closes(),
                };
                crate::day_line::decorate(&mut all, src, Some(&mut board.lines));
                frame = crate::movers::update_frame(t, &d, all).map(|mut v| {
                    crate::time_marks::stamp_frame_ns(&mut v, clock_now.map(UnixNanos::nanos));
                    v.to_string()
                });
            }
            if !d.is_empty() || frame.is_some() {
                board.generation += 1;
            }
            board.last = next;
            (board.generation, frame)
        };
        let Some(frame) = frame else {
            return false;
        };
        let _ = self.0.tx.send(Arc::new(Outbound {
            seq: generation,
            kind: OutKind::Movers(frame),
        }));
        true
    }

    pub fn detection_subscribe(&self) -> DetectionGuard {
        self.0.detection_subs.fetch_add(1, Ordering::Relaxed);
        DetectionGuard(self.0.clone())
    }

    pub fn detection_snapshot(&self) -> (String, u64, String) {
        match self.0.detection.get() {
            Some(d) => d.frame("detection.snapshot"),
            None => (
                String::new(),
                0,
                crate::detection::frame_with_type(
                    "detection.snapshot",
                    json!({"enabled":false,
                "source":self.detection_source(),"runEpoch":"","cursor":0,"snapshot":null,
                "transitions":[],"replacement":true,"experimental":true,"alertsEnabled":false,
                "history":{"capacity":crate::detection::HISTORY_CAPACITY,"dropped":0,
                    "first_sequence":null,"last_sequence":null},"telemetry":{},
                "context":{},"profile":{"price_min_micros":self.0.band.min.micros(),"price_max_micros":self.0.band.max.micros(),
                    "research_min_micros":stock_context::DetectionPolicy::current().body.universe.research_band_min_micros,
                    "research_max_micros":stock_context::DetectionPolicy::current().body.universe.research_band_max_micros}}),
                ),
            ),
        }
    }

    pub fn detection_epoch_is_current(&self, epoch: &str) -> bool {
        self.0.detection.get().is_some_and(|d| d.is_epoch(epoch))
    }

    fn refresh_detection(&self, force: bool) {
        if !self.0.detection_on.load(Ordering::Relaxed) {
            return;
        }
        let Some(runtime) = self.0.detection.get() else {
            return;
        };
        let _delivery = self
            .0
            .detection_delivery
            .lock()
            .expect("detection delivery");
        let Some((capture, watermark, rebuilding, gains, cue)) = ({
            let mut core = self.0.core.lock().expect("core lock");
            let watermark = core.seq;
            let rebuilding = core.rebuilding;
            let capture = core.context.as_mut().and_then(|c| c.take_detection(force));
            // The rows' day gains: one board lookup per snapshot symbol (a few dozen at most),
            // only on the display cadence that walks the snapshot anyway.
            let gains = capture
                .as_ref()
                .and_then(|c| c.snapshot.as_ref())
                .map(|s| {
                    crate::detection::day_gains(crate::detection::snapshot_prices(s), |sym| {
                        core.quotes
                            .get(sym)
                            .and_then(|r| r.prev_close)
                            .map(|p| p.micros())
                    })
                })
                .unwrap_or_default();
            // The crossings this drain decided (crate::cue_notices), offered right away. With
            // nothing captured they stay for the next drain.
            let cue = match (&capture, core.context.as_mut()) {
                (Some(_), Some(c)) => c.take_cue_notices(),
                _ => Vec::new(),
            };
            capture.map(|c| (c, watermark, rebuilding, gains, cue))
        }) else {
            return;
        };
        self.offer_notices(cue);
        let ross_on = self.0.context_on.load(Ordering::Relaxed);
        let mut context: serde_json::Map<String, Value> = capture.context.iter().map(|d| {
            let catalyst = match d.pillars.iter().find(|p| p.pillar == stock_context::PillarId::News).map(|p| p.state) {
                Some(stock_context::Tri::Pass) => "qualifying",
                Some(stock_context::Tri::Fail) => "none",
                _ => "unknown",
            };
            (d.symbol.clone(),json!({"ross_member":ross_on.then(|| d.membership.active().is_some()),"catalyst":catalyst}))
        }).collect();
        // Merged into each symbol's context entry: additive keys, so an old client ignores them;
        // a symbol with no context decision gets an entry holding only its gain.
        for (symbol, gain) in gains {
            if let (Value::Object(entry), Value::Object(g)) =
                (context.entry(symbol).or_insert_with(|| json!({})), gain)
            {
                entry.extend(g);
            }
        }
        runtime.accept(crate::detection::Capture {
            epoch: capture.epoch,
            snapshot: capture.snapshot.map(|snapshot| serde_json::to_value(snapshot).expect("core detection serializes")),
            telemetry: serde_json::to_value(capture.telemetry).expect("core detection telemetry serializes"),
            transitions: capture.transitions.into_iter().map(|t| serde_json::to_value(t).expect("core transition serializes")).collect(),
            input_watermark: watermark,
            restored: rebuilding || self.detection_source() != "live",
            rebuilding,
            context: Value::Object(context),
            profile: json!({"price_min_micros":self.0.band.min.micros(),"price_max_micros":self.0.band.max.micros(),
                "research_min_micros":capture.research_band.0,"research_max_micros":capture.research_band.1}),
            policy_hash: Some(capture.policy_hash),
        });
    }

    /// Advances ordinary display cues at <=4Hz. Significant transitions were already captured
    /// by apply(), independent of this publisher and of all chart/row subscriptions.
    pub fn publish_detection(&self) {
        self.refresh_detection(true);
        if self.0.detection_subs.load(Ordering::Relaxed) == 0 {
            return;
        }
        let Some(runtime) = self.0.detection.get() else {
            return;
        };
        let seq = {
            let core = self.0.core.lock().expect("core lock");
            if core.rebuilding {
                return;
            }
            core.seq
        };
        let signature = runtime.signature();
        let mut published = self
            .0
            .detection_published
            .lock()
            .expect("detection publication");
        if published.as_ref() == Some(&signature) {
            return;
        }
        let (epoch, cursor, frame) = runtime.frame("detection.update");
        *published = Some((epoch.clone(), cursor));
        let _ = self.0.tx.send(Arc::new(Outbound {
            seq,
            kind: OutKind::Detection {
                epoch,
                cursor,
                frame,
            },
        }));
    }

    fn detection_source(&self) -> &'static str {
        if self
            .0
            .info
            .lock()
            .expect("info lock")
            .as_ref()
            .is_some_and(|i| i.fixture)
        {
            "fixture"
        } else if self.0.mode == ClockMode::Live {
            "live"
        } else {
            "replay"
        }
    }

    /// The Ross Panel as a `ross.snapshot` frame (`enabled: false` with `STOCK_CONTEXT` off), with
    /// the engine seq it is consistent with.
    pub fn ross_snapshot(&self) -> (u64, String) {
        let core = self.0.core.lock().expect("core lock");
        let readiness = self.0.readiness.lock().expect("readiness lock").clone();
        // The members' whole day lines (crate::day_line), read here at serialization time.
        let lines = crate::day_line::Source {
            engine: &core.engine,
            closes: core.quotes.closes(),
        };
        let frame = match core
            .context
            .as_ref()
            .filter(|_| self.0.context_on.load(Ordering::Relaxed))
        {
            Some(cx) => cx.panel_frame_with("ross.snapshot", readiness.as_ref(), Some(lines)),
            None => crate::context::disabled_frame(),
        };
        (core.seq, frame)
    }

    /// A symbol's latest stock-context decision as a `context.decision` frame.
    pub fn context_decision(&self, symbol: &str) -> String {
        let core = self.0.core.lock().expect("core lock");
        let cx = core
            .context
            .as_ref()
            .filter(|_| self.0.context_on.load(Ordering::Relaxed));
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
        let (took, total, tel, cue, transitions) = {
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
            (
                took,
                cx.drained(),
                cx.output_telemetry(),
                cx.take_cue_notices(),
                cx.take_setup_transitions(),
            )
        };
        self.offer_notices(cue);
        // Off the core lock; serialized on the log's own thread (crate::oplog).
        if let Some(log) = self.0.transition_log.get() {
            for t in transitions {
                log.send(
                    crate::oplog::et_date_ms(t.observed_at.div_euclid(1_000_000)),
                    move || transition_line(&t),
                );
            }
        }
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

    /// What the shadow measurement has so far (`crate::shadow`), or `None` when it is not
    /// attached.
    ///
    /// Settled against the **run's** clock — the latest barrier the board has decided at — not
    /// the wall clock: a replay's day is its own, and a session that closed yesterday must not
    /// reopen at this morning's 09:00.
    ///
    /// `insufficient_data` until a forward cohort of live closed sessions exists, which is the
    /// honest state and the only one this process can report.
    pub fn shadow_report(&self) -> Option<crate::shadow::ShadowReport> {
        // A read, and only a read. The capture's own thread folds and appends
        // (`crate::shadow::ShadowSink::spawn_absorber`); a reporter that did the folding would
        // put a file write behind whoever asked for a figure, and a capture that only advances
        // when someone looks at it is not a capture.
        Some(self.0.shadow.get()?.report())
    }

    /// What the admission journal has recorded so far (`crate::admit`), or `None` when this
    /// market writes none (a replay, or a live run with no recording to restore from).
    pub fn admission_report(&self) -> Option<crate::admit::AdmissionReport> {
        self.0.admissions.get().map(|j| j.report())
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
        // The journal's own counters, said out loud: a run whose journal has holes cannot be
        // restored from exactly, and that is worth knowing before a restart rather than after.
        // The shadow line stands on its own (it used to be logged only when a journal existed,
        // and the journal is now opt-in), and never waits for the capture's fold.
        {
            if let Some(sh) = self.0.shadow.get().and_then(|s| s.try_report()) {
                tracing::info!(
                    definitions = sh.definitions,
                    live_closed_sessions = sh.live_sessions,
                    candidate_then_strict = sh.candidate_then_strict,
                    candidate_only = sh.candidate_only,
                    neither = sh.neither,
                    censored = sh.censored_sessions,
                    missing = sh.missing_sessions,
                    rejected = sh.rejected_sessions,
                    synthetic = sh.synthetic_sessions,
                    dropped = sh.dropped_sessions,
                    complete = sh.complete(),
                    outcome = %sh.outcome.line(),
                    "shadow measurement (a description of what was observed, never a claim that either label is better)"
                );
            }
        }
        if let Some(j) = self.admission_report() {
            tracing::info!(
                admitted = j.admitted,
                written = j.written,
                dropped_queue_full = j.dropped,
                write_errors = j.errors,
                queue_capacity = j.queue_capacity,
                complete = j.complete(),
                "admission journal (what a restart of this day could prove)"
            );
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
        if !self.0.context_on.load(Ordering::Relaxed)
            || self.0.ross_subs.load(Ordering::Relaxed) == 0
        {
            return;
        }
        let (seq, frame, snap) = {
            let mut core = self.0.core.lock().expect("core lock");
            if core.rebuilding {
                return;
            }
            let seq = core.seq;
            let readiness = self.0.readiness.lock().expect("readiness lock").clone();
            // Disjoint borrows: the board's day-line baseline moves, the candles are only read.
            let Core {
                context,
                engine,
                quotes,
                ..
            } = &mut *core;
            let lines = crate::day_line::Source {
                engine,
                closes: quotes.closes(),
            };
            match context
                .as_mut()
                .and_then(|c| c.take_update_with(readiness.as_ref(), Some(lines)))
            {
                Some((frame, snap)) => (seq, frame, snap),
                None => return,
            }
        };
        // The publication time of the snapshot this frame carries, for `crate::observe`: sampled
        // here, with the frame already serialized and the core lock released, so the segment ends
        // at **this server's own broadcast**. It says nothing about network transport, a client's
        // receipt or what a browser rendered; those are separate segments nothing measures yet.
        //
        // `snap` is `None` when the frame goes out only because the readiness summary moved: the
        // panel in it is the one already published, so counting it again would inflate the
        // observation count and add a latency sample for a detection that did not happen.
        if let Some(snap) = &snap {
            self.0.observations.capture(crate::observe::Observation::of(
                crate::observe::Stage::Published,
                snap,
                crate::observe::wall_now(),
            ));
        }
        let _ = self.0.tx.send(Arc::new(Outbound {
            seq,
            kind: OutKind::Ross(frame),
        }));
    }

    /// The last ranked board as a `pressure.snapshot` frame, with the engine seq it is consistent
    /// with. Empty until the first ranking (at most a second after start or a rebuild).
    pub fn pressure_snapshot(&self) -> (u64, String) {
        let t = self.clock().map(|c| c.now);
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
        let (seq, cards, appeared) = {
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
            // A gauge that was not on the board a second ago (the Halt watcher push, off by
            // default: crate::alerts::runners). Not on the first ranking after a start.
            let primed = self.0.pressure_primed.swap(true, Ordering::Relaxed);
            let appeared: Vec<crate::rocket::Notice> = if primed {
                cards
                    .iter()
                    .filter(|c| !core.pressure_cards.iter().any(|o| o.s == c.s))
                    .map(|c| c.notice(clock.now.0))
                    .collect()
            } else {
                Vec::new()
            };
            // T2-c4's HALT add-on (crate::c4_live::halt_addon): the bull gauges it keeps, as
            // their own kind beside the gauge's notice.
            let addon: Vec<crate::rocket::Notice> = appeared
                .iter()
                .filter_map(|n| core.rocket.halt_addon(n))
                .collect();
            let mut appeared = appeared;
            appeared.extend(addon);
            if cards == core.pressure_cards {
                return false;
            }
            core.pressure_cards = cards.clone();
            (core.seq, cards, appeared)
        };
        self.offer_notices(appeared);
        if self.pressure_subscribers() == 0 {
            return false;
        }
        let frame = pressure_frame("pressure.update", Some(clock.now), &cards);
        let _ = self.0.tx.send(Arc::new(Outbound {
            seq,
            kind: OutKind::Pressure(frame),
        }));
        true
    }

    /// `{"type", "t", "tNs", "quotes": [row...], "gone"?: [symbol...]}`; the row shape is
    /// [`WireQuote`].
    fn quotes_frame(
        &self,
        kind: &'static str,
        now: Option<UnixNanos>,
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
                    pcb: r.basis.tag(),
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
            t: now.map(UnixNanos::millis),
            t_ns: now.map(UnixNanos::nanos),
            quotes,
            gone,
        })
        .expect("quotes serialize")
    }

    /// The archive this market reads (`ARCHIVE_ROOT`), once it started on one; `None` for an
    /// idle or provider-less market. The replay bar's alert dots are read beside it
    /// (`crate::replay_alerts`).
    pub fn archive_root(&self) -> Option<std::path::PathBuf> {
        self.0.running_archive.get().map(|a| a.root().to_path_buf())
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
    /// market is closed), or null before any. The same fields as a `symbols.snapshot`
    /// (crate::symbols), which is what a client subscribed over the socket gets instead.
    pub fn symbols_json(&self) -> Value {
        let (_, symbols, total) = self.symbol_parts();
        let mut v = self.symbols_meta(total, symbols.len(), crate::history::session_json(self));
        v["symbols"] = json!(symbols);
        v
    }

    /// Whether a rebuild is under way, the candidates (sorted) and how many symbols are known,
    /// from one pass under the core lock.
    fn symbol_parts(&self) -> (bool, Vec<String>, usize) {
        let core = self.0.core.lock().expect("core lock");
        let all = self.all_symbols(&core);
        let total = all.len();
        let candidates = all
            .into_iter()
            .filter(|s| Self::candidate_in(&core, self.0.band, s))
            .collect();
        (core.rebuilding, candidates, total)
    }

    /// `/api/symbols` without `symbols` (crate::symbols, the "meta").
    fn symbols_meta(&self, total: usize, candidates: usize, session: Value) -> Value {
        let info = self.info();
        let info = info.as_deref();
        json!({
            "mode": self.0.mode.as_str(),
            "status": self.0.status.lock().expect("status lock").state,
            "source": info.map(|i| i.label.clone()),
            "day": info.and_then(|i| i.day).map(|d| d.to_string()),
            "fixture": info.is_some_and(|i| i.fixture),
            "priceBand": {"min": self.0.band.min.to_string(), "max": self.0.band.max.to_string()},
            "screened": {"total": total, "candidates": candidates},
            "session": session,
        })
    }

    /// Brings the published candidate list up to date (crate::symbols) and broadcasts the
    /// `symbols.update` when it changed, under the list's lock so updates reach the channel in
    /// `rev` order. Skipped during a rebuild (a seek's or day switch's empty-then-growing list
    /// would go out as thousands of adds and removes; the rebuild's resync sends the snapshot
    /// instead), except before anything was ever published: a subscribe during the startup
    /// catch-up must still get the meta (mode, status, day). `session` is read again only when
    /// `read_session` (it reads the archive); otherwise the last one.
    /// Lock order: the list's lock, then core (`symbol_parts`, `session_json`); never core first.
    /// The list is read under the list's lock, not before it: the 1 Hz tick and a subscribe can
    /// refresh at once, and a list read first could be published after a newer one, so a stock
    /// would leave and come back (a spurious remove/add pair) until the next tick.
    fn refresh_symbols(&self, read_session: bool) {
        let mut p = self.0.symbols.lock().expect("symbols lock");
        let (rebuilding, symbols, total) = self.symbol_parts();
        if rebuilding && !p.meta.is_null() {
            return;
        }
        let session = if read_session || p.meta.is_null() {
            crate::history::session_json(self)
        } else {
            p.meta["session"].clone()
        };
        let meta = self.symbols_meta(total, symbols.len(), session);
        if let Some(frame) = crate::symbols::advance(&mut p, symbols, meta) {
            let _ = self.0.tx.send(Arc::new(Outbound {
                seq: p.rev,
                kind: OutKind::Symbols(frame),
            }));
        }
    }

    /// The 1 Hz tick's part (`n` = the tick count): nothing while nobody subscribes, so an idle
    /// server never walks the symbol list. A first subscribe catches the baseline up itself.
    pub fn publish_symbols(&self, n: u64) {
        if self.0.symbols_subs.load(Ordering::Relaxed) == 0 {
            return;
        }
        self.refresh_symbols(n.is_multiple_of(crate::symbols::SESSION_EVERY_TICKS));
    }

    /// The list now as a `symbols.snapshot`, with the `rev` it is at: a connection forwards only
    /// updates with a larger one. Refreshes first, so the snapshot is current and is the very
    /// baseline later updates are diffed against.
    pub fn symbols_snapshot(&self) -> (u64, String) {
        self.refresh_symbols(true);
        let p = self.0.symbols.lock().expect("symbols lock");
        (p.rev, crate::symbols::snapshot_frame(&p))
    }

    /// Registers a `symbols.subscribe`; the count drops with the guard.
    pub fn symbols_subscribe(&self) -> SymbolsGuard {
        self.0.symbols_subs.fetch_add(1, Ordering::Relaxed);
        SymbolsGuard(self.0.clone())
    }
}

/// A slot's hold on its indicator studies (crate::indicators): released when dropped. Takes the
/// core lock in `drop`, so it must never be dropped while that lock is held (only connections
/// hold one).
pub struct IndicatorLease {
    inner: Arc<Inner>,
    studies: Studies,
}

impl IndicatorLease {
    pub fn studies(&self) -> &Studies {
        &self.studies
    }
}

impl Drop for IndicatorLease {
    fn drop(&mut self) {
        if let Ok(mut core) = self.inner.core.lock() {
            core.indicators.release(&self.studies);
        }
    }
}

/// A slot's hold on its levels or zones (crate::chart_context): released when dropped. Takes the
/// core lock in `drop`, like [`IndicatorLease`], so it must never be dropped under that lock.
pub struct ContextLease {
    inner: Arc<Inner>,
    key: ContextKey,
}

impl ContextLease {
    pub fn key(&self) -> &ContextKey {
        &self.key
    }
}

impl Drop for ContextLease {
    fn drop(&mut self) {
        if let Ok(mut core) = self.inner.core.lock() {
            core.chart_context.release(&self.key);
        }
    }
}

impl From<ContextFrame> for OutKind {
    fn from(f: ContextFrame) -> Self {
        match f {
            ContextFrame::Levels(key, json) => OutKind::Levels { key, json },
            ContextFrame::Zones(key, json) => OutKind::Zones { key, json },
        }
    }
}

/// Decrements the `symbols.subscribe` count when dropped.
pub struct SymbolsGuard(Arc<Inner>);

impl Drop for SymbolsGuard {
    fn drop(&mut self) {
        self.0.symbols_subs.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Decrements the quotes subscriber count when dropped.
pub struct QuotesGuard(Arc<Inner>);

impl Drop for QuotesGuard {
    fn drop(&mut self) {
        self.0.quote_subs.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Decrements the Rocket watch subscriber count when dropped.
pub struct RocketGuard(Arc<Inner>);

impl Drop for RocketGuard {
    fn drop(&mut self) {
        self.0.rocket_subs.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Decrements the Ross Panel subscriber count when dropped.
pub struct RossGuard(Arc<Inner>);

pub struct DetectionGuard(Arc<Inner>);

impl Drop for DetectionGuard {
    fn drop(&mut self) {
        self.0.detection_subs.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Drop for RossGuard {
    fn drop(&mut self) {
        self.0.ross_subs.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Decrements the favorites subscriber count when dropped.
pub struct FavoritesGuard(Arc<Inner>);

impl Drop for FavoritesGuard {
    fn drop(&mut self) {
        self.0.favorites_subs.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Decrements the movers subscriber count when dropped.
pub struct MoversGuard(Arc<Inner>);

impl Drop for MoversGuard {
    fn drop(&mut self) {
        self.0.movers_subs.fetch_sub(1, Ordering::Relaxed);
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
/// - `toBandSecs` the projected seconds to that band at the pace of the last horizon
///   (`crate::pressure::Risk::eta_secs`, `pressure.eta.v1`): whole seconds, 0 at the band;
///   absent when the price is not moving toward it, would take over 10 minutes, or is halted
///   (a frozen projection would count down nothing);
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
    #[serde(rename = "toBandSecs", skip_serializing_if = "Option::is_none")]
    to_band_secs: Option<u32>,
    pressure: u8,
    #[serde(rename = "buyShare")]
    buy_share: f64,
    pace: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    halt: Option<HaltInfo>,
}

impl PressureCardWire {
    /// The Halt watcher push for this card appearing at `now` (epoch ns): one event per
    /// appearance (`crate::alerts::runners`).
    fn notice(&self, now: i64) -> crate::rocket::Notice {
        crate::rocket::Notice {
            spread_bp: None,
            kind: crate::rocket::NoticeKind::Halt,
            alert_id: format!("{}-h{}", self.s, now.div_euclid(1_000_000_000)),
            symbol: self.s.clone(),
            at_ns: now,
            price: self.p,
            from_flag_pct: None,
            reasons: Vec::new(),
            since_start_min: None,
            exec_ns: None,
            detail: Some(crate::rocket::NoticeDetail::Halt {
                bull: self.side == "bull",
                risk: self.risk,
                band: self.band,
                secs: self.to_band_secs,
            }),
        }
    }

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
            to_band_secs: c.halt.is_none().then(|| c.risk.eta_secs()).flatten(),
            pressure: c.score.pressure,
            buy_share: (c.score.buy_share * 100.0).round() / 100.0,
            pace: (c.score.pace * 10.0).round() / 10.0,
            halt: c.halt.clone(),
        }
    }
}

/// `pressure.snapshot` / `pressure.update`: `{"type", "t", "tNs", "board": [card...]}`, the whole
/// board in display order either way (six cards at most, so an update replaces rather than
/// patches).
fn pressure_frame(
    kind: &'static str,
    now: Option<UnixNanos>,
    cards: &[PressureCardWire],
) -> String {
    #[derive(Serialize)]
    struct PressureMessage<'a> {
        #[serde(rename = "type")]
        kind: &'static str,
        t: Option<i64>,
        /// `t` in exchange-time ns (`times.ns.v1`, crate::time_marks).
        #[serde(rename = "tNs")]
        t_ns: Option<i64>,
        board: &'a [PressureCardWire],
    }
    serde_json::to_string(&PressureMessage {
        kind,
        t: now.map(UnixNanos::millis),
        t_ns: now.map(UnixNanos::nanos),
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
    /// `t` in exchange-time ns (`times.ns.v1`, crate::time_marks).
    #[serde(rename = "tNs")]
    t_ns: Option<i64>,
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
    /// `split` when `pc` was put on today's basis by a known split, `guard` when the close was
    /// withheld because the day trades on another basis (crate::quotes::PcBasis); absent for a
    /// plain close. On every row that has one, so a client never keeps a stale `pc` beside it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pcb: Option<&'static str>,
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
            let idx = idx.with_splits(load_splits(archive));
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

/// The corporate-actions ledger (`$ARCHIVE_ROOT/corporate-actions/ledger.json`, kept by
/// `swimport maintain` from the provider's announcements).
fn ledger_path(archive: &Archive) -> std::path::PathBuf {
    archive.corporate_actions_root().join("ledger.json")
}

/// The splits the ledger knows, for putting raw previous closes on a later day's share basis
/// (`market_archive::PrevCloses::with_splits`; owner 2026-10-30: reverse splits topped Top
/// Gainers at +3,900 %). Optional: a missing ledger is none known, an unreadable one is logged
/// and treated the same; either way the quote board's split guard still holds a day that plainly
/// trades on another basis (`crate::quotes::split_guard`).
fn load_splits(archive: &Archive) -> Vec<market_archive::SplitFact> {
    match archive.read_action_ledger() {
        Ok(l) => {
            let s = market_archive::splits_from_ledger(&l);
            tracing::info!(
                actions = l.len(),
                splits = s.len(),
                "corporate-actions ledger read: previous closes split-adjusted"
            );
            s
        }
        Err(e) => {
            tracing::warn!(error = %e, "corporate-actions ledger unreadable: previous closes are raw (the split guard still applies)");
            Vec::new()
        }
    }
}

/// When a revision reached this process: its receive time when the feed knows it, else its
/// publication time. The same rule a print's admission uses, so a revision is never admitted
/// before the print it revises.
fn revision_admit(e: &MarketEvent) -> UnixNanos {
    match e {
        MarketEvent::Correction(c) => c.receive_ts.unwrap_or(c.event_ts),
        MarketEvent::Cancel(c) => c.receive_ts.unwrap_or(c.event_ts),
        other => other.event_ts(),
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
#[path = "market_latency_tests.rs"]
mod latency_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// A snapshot's schedule reaches the market clock's trading date even when the symbol has no
    /// candles that day: the empty chart before a first print on an early close (2026-11-27,
    /// regular to 13:00), and history from Thursday seen on Monday. One that already reaches the
    /// date (or past it: Saturday, with Monday listed) is left as the engine built it.
    #[test]
    fn the_snapshot_schedule_reaches_the_market_clocks_date() {
        let d = |s: &str| s.parse::<NaiveDate>().unwrap();
        let at = |day: &str, h| market_events::session::et(d(day), h, 0, 0, 0).unwrap();
        let ids = |v: &[chart_engine::wire::SessionInfo]| -> Vec<String> {
            v.iter().map(|s| s.id.clone()).collect()
        };

        let empty = schedule_through(Vec::new(), at("2026-11-27", 8));
        let regular = empty
            .iter()
            .find(|s| s.id == "2026-11-27/regular")
            .expect("the early close's regular session");
        assert_eq!(regular.end_ns, at("2026-11-27", 13).0);
        assert!(ids(&empty).contains(&"2026-11-30/regular".to_string()));

        let thursday = chart_engine::session_schedule([d("2026-10-29")].into_iter(), None);
        let monday = schedule_through(thursday, at("2026-11-02", 10));
        assert!(ids(&monday).contains(&"2026-11-02/regular".to_string()));
        assert!(ids(&monday).contains(&"2026-10-29/regular".to_string()));

        let friday = chart_engine::session_schedule([d("2026-10-30")].into_iter(), None);
        let saturday = schedule_through(friday.clone(), at("2026-10-31", 12));
        assert_eq!(ids(&saturday), ids(&friday));
        // 21:00 on Friday is no session; its trading date is Friday itself: still covered.
        assert_eq!(
            ids(&schedule_through(friday.clone(), at("2026-10-30", 21))),
            ids(&friday)
        );
    }

    /// Manual synthetic pressure portion of the real Core critical section; no feed or sockets.
    /// Excludes lock acquisition and every other ingest consumer, and is not production latency.
    #[test]
    #[ignore = "manual Core-lock pressure measurement; run with --ignored --nocapture"]
    fn pressure_cost_under_market_core_lock() {
        use std::time::Instant;

        const COUNT: usize = 100_000;
        const SEC: i64 = 1_000_000_000;
        let day = NaiveDate::from_ymd_opt(2026, 10, 16).unwrap();
        let start = market_events::session::et(day, 10, 0, 0, 0).unwrap().0;
        for late in [false, true] {
            let market = Market::idle(ClockMode::Live, "synthetic Core pressure bench", None);
            let trades: Vec<_> = (0..COUNT)
                .map(|i| {
                    let now = start + i as i64 * SEC / 1_000;
                    let exec = if late && i > 5_000 && i % 20 == 0 {
                        now - 5 * SEC
                    } else {
                        now
                    };
                    market_events::Trade::new(
                        "COST",
                        UnixNanos(exec),
                        market_events::Price::from_micros(1_000_000 + i as i64 % 100),
                        100,
                        i as u64,
                    )
                })
                .collect();
            let mut ns = Vec::with_capacity(COUNT);
            for trade in &trades {
                let mut core = market.0.core.lock().unwrap();
                let held = Instant::now();
                core.pressure.apply_trade(
                    trade,
                    Counted {
                        ohlc: true,
                        volume: true,
                    },
                );
                let elapsed = held.elapsed().as_nanos();
                drop(core);
                ns.push(elapsed);
            }
            ns.sort_unstable();
            println!(
                "actual_core_pressure_only late={late} n={} mean_ns={} p50_ns={} p95_ns={} p99_ns={} max_ns={} profile=debug synthetic=true lock_acquisition_excluded=true other_ingest_excluded=true",
                ns.len(),
                ns.iter().sum::<u128>() / ns.len() as u128,
                ns[ns.len() / 2],
                ns[ns.len() * 95 / 100],
                ns[ns.len() * 99 / 100],
                ns.last().unwrap(),
            );
        }
    }

    #[test]
    fn no_archive_explicitly_completes_the_running_facts_request() {
        let market = Market::idle(ClockMode::Live, "test", None);
        market.set_rocket_watch(true, crate::rocket::Policy::default());
        let day = NaiveDate::from_ymd_opt(2026, 10, 15).unwrap();
        let at = market_events::session::et(day, 10, 0, 0, 0).unwrap().0;
        market.0.core.lock().unwrap().rocket.tick(at, |_| None);
        assert_eq!(market.runner_readiness()["running_facts_ready"], false);
        assert!(market.0.running_archive.get().is_none());
        market.load_running_facts(day);
        assert_eq!(market.runner_readiness()["running_facts_ready"], true);
    }

    /// A real held-tail overflow cannot turn the chart/scanner into a repeated full-day
    /// rebuild. Partial coverage ends the hold; only Runners remains guarded for this date.
    #[test]
    fn overflow_adoption_releases_live_engine_and_scanner_but_keeps_runners_unavailable() {
        let tmp = tempfile::tempdir().unwrap();
        let day = NaiveDate::from_ymd_opt(2026, 10, 15).unwrap();
        let at = |h, m, s| market_events::session::et(day, h, m, s, 0).unwrap();
        let archive = Archive::new(market_archive::ArchiveConfig {
            root: tmp.path().to_path_buf(),
            retention_days: 0,
            min_free_bytes: 0,
        });
        let mut writer =
            market_archive::LiveWriter::open(&archive, day, 0, "overflow-regression").unwrap();
        writer
            .append(&MarketEvent::Trade(market_events::Trade::new(
                "HOT",
                at(9, 59, 1),
                Price::from_cents(200),
                100,
                1,
            )))
            .unwrap();
        writer.finish().unwrap();
        let market = Market::idle(ClockMode::Live, "test", None);
        market.set_rocket_watch(
            true,
            crate::rocket::Policy {
                fast: false,
                ..Default::default()
            },
        );
        market.set_stock_context(true);
        market.init_history(archive, None, at(10, 1, 0), false, false);
        for i in 0..=crate::history::HELD_MAX {
            let t = UnixNanos(at(10, 0, 1).0 + i as i64 * 10_000);
            market.apply(FeedItem::Event(MarketEvent::Trade(
                market_events::Trade::new("HOT", t, Price::from_cents(200), 100, i as u64 + 2),
            )));
        }
        assert_eq!(market.state_json()["history"]["pending"], 1);
        assert!(market.load_history("HOT").is_some());
        assert_eq!(
            market.state_json()["history"]["pending"],
            0,
            "overflow is a completed partial adoption, not a deferred load"
        );
        assert!(
            market.load_history("HOT").is_none(),
            "the loader cannot repeatedly rebuild a completed adoption"
        );
        market.apply(FeedItem::Advance(at(10, 1, 0)));
        let (engine_before, scanner_before) = {
            let core = market.0.core.lock().unwrap();
            assert!(!core.unloaded.contains_key("HOT"));
            let d = core.context.as_ref().unwrap().decision("HOT").unwrap();
            assert_eq!(d.volume.coverage, stock_context::Coverage::Partial);
            assert_eq!(d.volume.coverage_reason, Some("held_tail_overflow"));
            assert!(d.pillars.iter().all(|p| p.reason != "history_pending"));
            (
                core.engine
                    .snapshot("HOT", Timeframe::M1, None)
                    .unwrap()
                    .candles
                    .iter()
                    .map(|c| c.v)
                    .sum::<u64>(),
                d.volume.shares,
            )
        };
        market.apply(FeedItem::Event(MarketEvent::Trade(
            market_events::Trade::new("HOT", at(10, 1, 1), Price::from_cents(250), 777, 100_000),
        )));
        market.apply(FeedItem::Advance(at(10, 1, 30)));
        let mut core = market.0.core.lock().unwrap();
        let engine_after = core
            .engine
            .snapshot("HOT", Timeframe::M1, None)
            .unwrap()
            .candles
            .iter()
            .map(|c| c.v)
            .sum::<u64>();
        assert_eq!(
            engine_after,
            engine_before + 777,
            "the following print reaches the chart immediately"
        );
        let d = core.context.as_ref().unwrap().decision("HOT").unwrap();
        assert_eq!(
            d.volume.shares,
            scanner_before + 777,
            "the scanner resumes its normal live path"
        );
        assert!(d.pillars.iter().all(|p| p.reason != "history_pending"));
        core.rocket.tick(at(10, 1, 31).0, |_| None);
        assert_eq!(core.rocket.readiness()["unrestored"], 0);
        assert_eq!(core.rocket.readiness()["unavailable"], 1);
        assert_eq!(core.rocket.readiness()["withheld"], 1);
        assert!(core.rocket.alerts().is_empty() && core.rocket.take_notices().is_empty());
    }

    /// Real Market ingest must apply pressure's official status before syncing Runners.
    #[test]
    fn ingest_halt_and_resume_gate_the_next_runner_admission() {
        struct Fixed;
        impl crate::rocket::RocketScorer for Fixed {
            fn id(&self) -> &str {
                "test"
            }
            fn version(&self) -> &str {
                "t1"
            }
            fn score(&self, _: &crate::rocket::Features) -> Option<f64> {
                Some(1.0)
            }
        }
        let market = Market::idle(ClockMode::Live, "test", None);
        let policy = crate::rocket::Policy {
            threshold: Some(crate::rocket::Threshold::Fixed(0.5)),
            fast: false,
            ..Default::default()
        };
        market.set_rocket_watch(true, policy.clone());
        market.0.core.lock().unwrap().rocket =
            crate::rocket::RocketBoard::new(true, Arc::new(Fixed), policy);
        let day = NaiveDate::from_ymd_opt(2026, 10, 15).unwrap();
        let at = |m, s| market_events::session::et(day, 10, m, s, 0).unwrap();
        let trade = |ts| {
            MarketEvent::Trade(market_events::Trade::new(
                "STOP",
                ts,
                Price::from_cents(200),
                1000,
                1,
            ))
        };
        let status = |ts, kind| {
            MarketEvent::Status(market_events::Status {
                symbol: Some("STOP".into()),
                event_ts: ts,
                receive_ts: None,
                source: market_events::Source::Fixture,
                kind,
                code: if kind == market_events::StatusKind::Halt {
                    "H"
                } else {
                    "T"
                }
                .into(),
                reason: "LUDP".into(),
                tape: market_events::Tape::C,
            })
        };
        market.apply(FeedItem::Event(trade(at(0, 1))));
        market.apply(FeedItem::Event(status(
            at(0, 7),
            market_events::StatusKind::Halt,
        )));
        {
            let mut core = market.0.core.lock().unwrap();
            assert!(core.pressure.halt_state("STOP").0);
            core.rocket.tick(at(0, 16).0, |_| None);
            assert!(core.rocket.alerts().is_empty());
        }
        market.apply(FeedItem::Event(status(
            at(0, 22),
            market_events::StatusKind::Resume,
        )));
        market.apply(FeedItem::Event(trade(at(0, 23))));
        {
            let mut core = market.0.core.lock().unwrap();
            assert_eq!(core.pressure.halt_state("STOP"), (false, Some(at(0, 22).0)));
            core.rocket.tick(at(0, 31).0, |_| None);
            assert!(
                core.rocket.alerts().is_empty(),
                "bar containing resume is withheld"
            );
        }
        market.apply(FeedItem::Event(trade(at(0, 31))));
        let mut core = market.0.core.lock().unwrap();
        core.rocket.tick(at(0, 46).0, |_| None);
        assert_eq!(core.rocket.alerts().len(), 1);
        assert_eq!(
            core.rocket.take_notices()[0].kind,
            crate::rocket::NoticeKind::New
        );
    }

    /// `memory_report` (the `MEM_STATS_SECS` line) counts what a print creates on each board, and
    /// a fast-move tape costs its fixed ring, not something that grows with the prints.
    #[test]
    fn memory_report_counts_per_symbol_state() {
        let market = Market::idle(ClockMode::Live, "test", None);
        market.set_rocket_watch(true, crate::rocket::Policy::default());
        let r = market.memory_report();
        assert_eq!(r["engine_symbols"], 0);
        assert_eq!(r["rocket"]["fast_tapes"], 0);
        assert_eq!(r["zones"]["symbols"], 0);
        let day = NaiveDate::from_ymd_opt(2026, 10, 15).unwrap();
        let mut seq = 0;
        for s in 0..120 {
            let ts = market_events::session::et(day, 10, 0, 0, 0).unwrap().0 + s * 1_000_000_000;
            for sym in ["AAA", "BBB"] {
                seq += 1;
                market.apply(FeedItem::Event(MarketEvent::Trade(
                    market_events::Trade::new(sym, UnixNanos(ts), Price::from_cents(200), 100, seq),
                )));
            }
        }
        let r = market.memory_report();
        assert_eq!(r["engine_symbols"], 2);
        assert_eq!(r["rocket"]["fast_tapes"], 2);
        assert_eq!(r["zones"]["symbols"], 2);
        let fast = r["rocket"]["fast_bytes"].as_u64().unwrap();
        assert!(
            fast < 2 * 16_384,
            "two fast tapes hold {fast} B after 120 s of prints"
        );
    }

    /// The Runners board's notices reach the phone push from the LIVE market only, after the
    /// lock, without waiting: a replay's are taken and dropped, and a full sink costs the
    /// notice, never the board.
    #[test]
    fn runners_notices_leave_the_live_market_only() {
        let notice = |s: &str| crate::rocket::Notice {
            spread_bp: None,
            kind: crate::rocket::NoticeKind::New,
            alert_id: format!("x-{s}"),
            symbol: s.into(),
            at_ns: 0,
            price: 1.0,
            from_flag_pct: Some(0.0),
            reasons: vec![],
            since_start_min: None,
            exec_ns: None,
            detail: None,
        };
        for mode in [ClockMode::Live, ClockMode::Replay] {
            let market = Market::idle(mode, "test", None);
            market.set_rocket_watch(true, crate::rocket::Policy::default());
            *market.0.clock.lock().expect("clock lock") = Some(ClockState {
                mode,
                now: UnixNanos(1_792_670_400_000_000_000),
                speed: market_providers::Speed::Times(1),
                playing: true,
                day: None,
                start: None,
                end: None,
                finished: false,
                events: 0,
            });
            let (sink, _inputs, mut notices) = crate::alerts::AlertSink::for_test();
            market.attach_alerts(sink.clone());
            {
                let mut core = market.0.core.lock().expect("core lock");
                for s in ["A", "B", "C"] {
                    core.rocket.inject_notice(notice(s));
                }
            }
            market.publish_rocket();
            let mut got = Vec::new();
            while let Ok(n) = notices.try_recv() {
                got.push(n.symbol);
            }
            if mode == ClockMode::Live {
                // The sink holds 2: the third is dropped and counted, the board carries on.
                assert_eq!(got, ["A", "B"]);
                assert_eq!(sink.runners_dropped(), 1);
            } else {
                assert!(got.is_empty(), "a replay never pushes: {got:?}");
                assert_eq!(sink.runners_dropped(), 0);
            }
            // Taken either way: nothing is left to leak into a later tick.
            market.publish_rocket();
            assert!(notices.try_recv().is_err());
        }
    }

    /// Buying bursts are a live-market alert: a replay market never turns the detector on, so
    /// it never makes a burst row or a burst push; `BURST_ALERTS=0` keeps it off live too.
    #[test]
    fn burst_detector_runs_on_the_live_market_only() {
        use crate::rocket::burst::BurstPolicy;
        for (mode, policy, on) in [
            (ClockMode::Live, BurstPolicy::default(), true),
            (ClockMode::Replay, BurstPolicy::default(), false),
            (
                ClockMode::Live,
                BurstPolicy {
                    enabled: false,
                    ..BurstPolicy::default()
                },
                false,
            ),
        ] {
            let market = Market::idle(mode, "test", None);
            market.set_rocket_watch(true, crate::rocket::Policy::default());
            market.enable_burst(policy, None);
            let core = market.0.core.lock().expect("core lock");
            assert_eq!(core.rocket.burst().is_some(), on, "{mode:?}");
        }
    }

    #[test]
    fn unavailable_live_detection_clock_samples_actual_admission_now() {
        let before = crate::observe::wall_now();
        let at = unavailable_detection_clock(&MarketConfig::default(), ClockMode::Live);
        let after = crate::observe::wall_now();
        assert!((before..=after).contains(&at.0));
    }

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

    /// A stalled shadow append holds neither the reporter nor the publisher — and the
    /// synchronous call they used to make would block on exactly the same stall.
    ///
    /// The capture folded and appended from the quotes publisher and from the reporter, so a slow
    /// append sat on a runtime worker and behind whoever asked for a figure. The write is gated
    /// here (a `cfg(test)` latch, with no production path to it) to show the difference rather
    /// than assert it.
    ///
    /// A record is deliberately queued **after** the first append has blocked. Without it a
    /// restored synchronous absorb inside the reporter would find an empty queue, write nothing
    /// and return promptly — and this test would pass over the very thing it is for.
    #[test]
    fn a_stalled_shadow_append_holds_neither_the_reporter_nor_the_publisher() {
        use crate::shadow::{Lifecycle, LifecycleKind, ShadowEvent, ShadowSink};
        use std::sync::mpsc;
        use std::time::Instant;

        let tmp = tempfile::tempdir().expect("tmp");
        let (log, epoch) = crate::shadow::reserve_segment(tmp.path());
        assert!(
            log.is_some(),
            "a segment was reserved, so a write really is attempted"
        );
        let sink = Arc::new(ShadowSink::new(
            crate::shadow::QUEUE_CAPACITY,
            log,
            "testbuild".into(),
            "testcode00000001".into(),
            epoch,
        ));
        sink.hold_writes(true);

        let market = Market::idle(ClockMode::Replay, "test", None);
        market
            .0
            .shadow
            .set(sink.clone())
            .map_err(|_| "already set")
            .expect("a fresh market");
        market.set_stock_context(true);

        let date = chrono::NaiveDate::from_ymd_opt(2026, 10, 16).expect("valid");
        let event = |cutoff: i64| ShadowEvent {
            run_id: "replay".into(),
            source: "replay".into(),
            fidelity: "SyntheticFixture".into(),
            policy: "pillars-v2@fixture".into(),
            trading_date: date,
            symbol: "ABCD".into(),
            cutoff,
            candidate: true,
            strict: false,
            four_decided: true,
            rvol_decided: true,
            rvol_value: None,
            rvol_same_time_milli: None,
            rvol_recent_milli: None,
        };
        sink.capture_lifecycle(Lifecycle {
            run_id: "replay".into(),
            source: "replay".into(),
            fidelity: "SyntheticFixture".into(),
            kind: LifecycleKind::Start,
            at: 1,
        });
        sink.capture(event(2));

        // The synchronous call the publisher used to make: it blocks inside the gated append.
        let blocked = {
            let s = sink.clone();
            std::thread::spawn(move || s.absorb())
        };
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            !blocked.is_finished(),
            "a synchronous absorb waits for the append, which is why the publisher must not \
             make one"
        );

        // One more record, queued while the write is stuck. A reporter that folded would now
        // have something to write, and would block.
        sink.capture(event(3));

        // The reporter and the publisher, on a thread, so a regression fails rather than hangs.
        let (done, waited) = mpsc::channel();
        let reporter = {
            let market = market.clone();
            std::thread::spawn(move || {
                let t = Instant::now();
                let r = market.shadow_report().expect("a capture");
                market.drain_context();
                market.publish_context();
                let _ = done.send(t.elapsed());
                r
            })
        };
        let quick = waited.recv_timeout(Duration::from_secs(2));
        // Released before any assertion that could fail, so nothing is left hanging either way.
        sink.hold_writes(false);
        let took = quick.expect(
            "the reporter and the publisher returned while the append was stalled; a \
             synchronous fold inside either of them would have waited for it",
        );
        assert!(
            took < Duration::from_millis(500),
            "and returned promptly: {took:?}"
        );
        let r = reporter.join().expect("the reporter thread");
        assert!(r.captured >= 3, "the records were handed off: {r:?}");

        // The write lands once released, and the figures catch up.
        let folded = blocked.join().expect("the absorb finishes once released");
        assert!(
            folded >= 2,
            "it folded what was queued when it ran: {folded}"
        );
        sink.absorb();
        let r = sink.report();
        assert_eq!(r.persisted, r.absorbed, "{r:?}");
        assert!(r.absorbed >= 3, "{r:?}");
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
                "band": 5.5, "toBand": 1.23, "toBandSecs": 19,
                "pressure": 72, "buyShare": 0.88, "pace": 4.3
            })
        );
        // Halted: the frozen projection is not a countdown, so it is left off.
        let halted = Card {
            halt: Some(crate::pressure::HaltInfo {
                start: 1,
                reason: "LUDP".into(),
            }),
            ..card.clone()
        };
        let w = PressureCardWire::new(&halted, &QuoteBoard::new(), &last);
        assert_eq!(serde_json::to_value(&w).unwrap().get("toBandSecs"), None);
        // Not moving toward the band: none either.
        let away = Card {
            risk: Risk {
                secs_to_band: None,
                ..card.risk
            },
            ..card
        };
        let w = PressureCardWire::new(&away, &QuoteBoard::new(), &last);
        assert_eq!(serde_json::to_value(&w).unwrap().get("toBandSecs"), None);
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

/// One line of the cue-transition log (`crate::oplog`): the engine's transition as it serializes
/// it, tagged `kind: "transition"`, `v: 1`.
pub(crate) fn transition_line(t: &stock_context::DetectionTransition) -> String {
    let mut v = serde_json::to_value(t).unwrap_or(serde_json::Value::Null);
    if let Some(o) = v.as_object_mut() {
        o.insert("kind".into(), "transition".into());
        o.insert("v".into(), 1.into());
    }
    v.to_string()
}

#[cfg(test)]
mod symbols_tests {
    use super::*;

    fn frames(rx: &mut broadcast::Receiver<Arc<Outbound>>) -> Vec<(u64, Value)> {
        let mut out = Vec::new();
        while let Ok(o) = rx.try_recv() {
            if let OutKind::Symbols(f) = &o.kind {
                out.push((o.seq, serde_json::from_str(f).unwrap()));
            }
        }
        out
    }

    /// crate::symbols: nothing is walked or sent without a subscriber; a stock entering and
    /// leaving the band is one update each, numbered after the snapshot's `rev`.
    #[test]
    fn the_band_set_is_pushed_only_when_it_changes() {
        let market = Market::idle(ClockMode::Live, "test", None);
        let mut rx = market.subscribe();
        let day = NaiveDate::from_ymd_opt(2026, 10, 15).unwrap();
        let at = |s| market_events::session::et(day, 10, 0, s, 0).unwrap();
        let trade = |sym: &str, s, cents| {
            FeedItem::Event(MarketEvent::Trade(market_events::Trade::new(
                sym,
                at(s),
                Price::from_cents(cents),
                100,
                s as u64,
            )))
        };
        market.apply(trade("AAA", 1, 200));
        market.publish_symbols(1);
        assert!(frames(&mut rx).is_empty(), "no subscriber, no work");

        let _guard = market.symbols_subscribe();
        let (rev, snap) = market.symbols_snapshot();
        let snap: Value = serde_json::from_str(&snap).unwrap();
        assert_eq!(snap["symbols"], json!(["AAA"]));
        assert_eq!(snap["mode"], "live");
        // Bringing the baseline up to date on subscribe may itself publish (rev <= the snapshot's).
        assert!(frames(&mut rx).iter().all(|(r, _)| *r <= rev));

        market.publish_symbols(2);
        assert!(frames(&mut rx).is_empty(), "unchanged: nothing sent");

        market.apply(trade("BBB", 2, 500));
        market.apply(trade("AAA", 3, 3000)); // $30: out of the default $0.10–$25 band
        market.publish_symbols(3);
        let got = frames(&mut rx);
        assert_eq!(got.len(), 1);
        let (seq, f) = &got[0];
        assert_eq!(*seq, rev + 1);
        assert_eq!(f["type"], "symbols.update");
        assert_eq!(f["rev"].as_u64(), Some(rev + 1));
        assert_eq!(f["added"], json!(["BBB"]));
        assert_eq!(f["removed"], json!(["AAA"]));
        assert_eq!(f["screened"], json!({"total": 2, "candidates": 1}));
        assert_eq!(market.symbols_json()["symbols"], json!(["BBB"]));
    }
}

#[cfg(test)]
mod transition_log_tests {
    use super::transition_line;
    use stock_context::{Certainty, DetectionTransition, Family, State};

    #[test]
    fn a_transition_line_is_the_engines_record_tagged_for_the_log() {
        let t = DetectionTransition {
            seq: 7,
            transition_id: "e1:t7".into(),
            episode_id: "e1".into(),
            symbol: "SRTM".into(),
            family: Family::MicroPullback,
            variant: "micro_pullback_mp5".into(),
            from: Some(State::PullbackForming),
            to: State::Armed,
            event: "armed",
            reason: "levels_fixed",
            certainty: Certainty::Provisional,
            observed_at: 1_793_017_427_000_000_000,
            bar_end: None,
            cutoff: None,
            trade: None,
            features: None,
            level: None,
            anchors: None,
            checks: Vec::new(),
            correction_of: None,
            warmup: false,
            default_visible: true,
            related_episode_ids: Vec::new(),
        };
        let v: serde_json::Value = serde_json::from_str(&transition_line(&t)).unwrap();
        assert_eq!(v["kind"], "transition");
        assert_eq!(v["v"], 1);
        assert_eq!(v["symbol"], "SRTM");
        assert_eq!(v["episode_id"], "e1");
        assert_eq!(v["observed_at"], 1_793_017_427_000_000_000_i64);
    }
}
