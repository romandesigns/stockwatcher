//! Rocket watch (EXPERIMENTAL, `ROCKET_WATCH`, on by default): symbols a strategy flags as likely
//! to start an explosive bullish leg soon, so the owner sees them before the move.
//!
//! A "rocket" is the research bake-off's canonical event (`reports/rockets/definition.json`): a
//! leg of at least +30 % within 15 minutes from a start price in `$0.15–$25`, on 15 s bar closes,
//! with a liquidity floor. An alert is a HIT when such a leg starts 60–600 s after it.
//!
//! # Pieces
//!
//! - [`features`]: per-symbol rolling state on the research's 15 s grid, fed every print the chart
//!   engine counted (O(1) per print, under the core lock like the quote and pressure boards), and
//!   the research's 31 causal features at each completed barrier.
//! - [`scorer`]: the [`RocketScorer`] seam. `RUNNERS_SCORER` ([`ScorerChoice`]) picks the
//!   strategy: `r0` = `precursors-R0` ([`r0`], the bake-off's round-1 pick, frozen thresholds, the
//!   default), `r2` = `precursors-R2` ([`r2`], the round-2 LightGBM runner-up, evaluated in pure
//!   Rust, ready to switch to), `placeholder` = `placeholder-b2` (the volume-acceleration
//!   baseline).
//! - [`RocketBoard`] (here): the alert policy, the outcomes, the track record and the panel frame.
//! - [`log`]: the forward log on the archive volume (live market only).
//! - [`burst`]: buying bursts (live market only, `BURST_ALERTS`): the burst-onset research's
//!   detector, whose alerts become rows of this board ([`AlertKind::Burst`]) tracked exactly like
//!   a flag (momentum state, later outcomes, day line, levels) but with their own strict outcome
//!   ([`BurstRow::outcome`]), log lines (`burst`, `burst_outcome`) and push kind
//!   ([`NoticeKind::Burst`]). They never count in the Runners day record, budget or cap.
//! - [`running`]: the "running" entry, every stock in `$0.10–$25` that IS running by a
//!   price-relative rule (a move scaled by the symbol's own daily range, a fresh session high,
//!   real money), not a forecast. Its rows ([`AlertKind::Running`]) are tracked like the others,
//!   logged as `running` lines, at most one per symbol a day and none for a symbol that already
//!   has a row; their push ([`NoticeKind::Running`]) is opt-in per phone. Not in the day record.
//!
//! # Alert policy
//!
//! At each completed 15 s barrier (checked by the quotes task once per wall second, never on the
//! ingest path), every unit symbol is scored; candidates at or above the threshold become alerts,
//! strongest first, at most one per symbol per 30 minutes ([`DEDUP_NS`]).
//!
//! - **Budget** (default for a scorer without a frozen threshold): the threshold adapts so the day
//!   ends near `ROCKET_ALERTS_PER_DAY` alerts (default 10), which is also a hard cap. Each
//!   candidate's best score per symbol per 30-minute block approximates "one alert per episode";
//!   by time-of-day fraction `e` of 04:00–20:00, today's estimate is the `ceil(N·e)`-th largest
//!   of those. It is blended with the prior (the median of the last five days' `ideal_threshold`,
//!   the score that would have given exactly N that day, from the forward log), the prior
//!   weighing less as the day goes (all today's from 12:00). With no prior (the first day) no
//!   alert fires in the first hour. Never below the scorer's `floor`.
//! - **Fixed**: the scorer's frozen threshold (`RocketScorer::fixed_threshold`), or
//!   `ROCKET_THRESHOLD=<number>`; `ROCKET_THRESHOLD=budget` forces the adaptive one. Capped at
//!   `max(5·N, 50)` a day as a runaway guard.
//!
//! # Outcomes
//!
//! Each alert's outcome is computed after the fact from the same bars, causally (it only reads
//! bars that completed after the alert): the first canonical +30 % mark after the alert, its
//! start, and the max gain / max drawdown over the 30 minutes after it. `rocketed` once the start
//! of that leg is 60–600 s after the alert (the bake-off's HIT), `late` for 600–1,800 s, `moved`
//! when it went +30 % on a leg that started too early to count, `faded` when 45 minutes passed
//! without one. Settled outcomes go to the forward log.
//!
//! After that strict window every alert keeps being followed until 20:00 ET ([`track`]): the
//! later-move outcomes (max gain / drawdown over 2 hours, max gain to 20:00, a +30 % mark any time
//! later: `later` lines in the log) and the live momentum state shown on the row (`watching`,
//! `building`, `breaking_out`, `running`, `pullback`, `fading`, `halted`: `state` lines in the
//! log on every transition). The rows also carry the micro-pullback levels of the stock-context
//! engine (`STOCK_CONTEXT=1`; the engine runs MP5 for the flagged symbols as for Ross members)
//! and the day line (`crate::day_line`), both added by the market when it serializes a frame.
//!
//! # Phone pushes
//!
//! Each new alert, and each flagged row whose momentum state turns `breaking_out`, also leaves a
//! [`Notice`] ([`RocketBoard::take_notices`]). The live market hands them to the alerts service
//! (`crate::alerts::runners`), which sends the Expo push within its own limits; a replay board's
//! notices are taken and dropped. A restore from the forward log leaves none: a restart must not
//! re-announce the morning's flags.

pub mod burst;
pub mod features;
pub mod log;
pub mod r0;
pub mod r2;
pub mod running;
pub mod scorer;
pub mod track;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::NaiveDate;
use market_events::UnixNanos;
use serde_json::{Value, json};

pub use features::{FEATS, Features, NF, ix};
pub use log::{DaySummary, Loaded, RocketLog};
pub use scorer::{PlaceholderB2, RocketScorer, ScorerChoice, production_scorer, standard_reasons};

use features::{BAR_NS, Barrier, NSLOT, Tape};
use track::{MState, Track, Transition};

pub const MIN_NS: i64 = 60_000_000_000;
/// At most one alert per symbol per 30 minutes.
pub const DEDUP_NS: i64 = 30 * MIN_NS;
pub const DEFAULT_BUDGET: u32 = 10;
/// Whatever the policy, never more alerts than this in a day (memory and log bound).
pub const MAX_ALERTS_PER_DAY: usize = 200;
/// When an outcome is final: a leg starting 30 minutes after the alert has its +30 % mark at most
/// 15 minutes later.
const SETTLE_NS: i64 = 45 * MIN_NS;
/// Max gain / drawdown window after the alert.
const GAIN_WINDOW_NS: i64 = 30 * MIN_NS;
/// Rows sent to the panel (newest first).
pub const ROWS: usize = 50;
/// Notices kept while nobody takes them (a board ticked without a market, a test).
const MAX_NOTICES: usize = 256;

/// What a phone push may be about (module docs, "Phone pushes").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeKind {
    /// A new alert (a new row on the board).
    New,
    /// A flagged row's momentum state turned `breaking_out` (from any other state).
    Breakout,
    /// A new buying-burst row ([`burst`]).
    Burst,
    /// A new running row ([`running`]); per-phone opt-in, off by default.
    Running,
}

impl NoticeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            NoticeKind::New => "new",
            NoticeKind::Breakout => "breakout",
            NoticeKind::Burst => "burst",
            NoticeKind::Running => "running",
        }
    }
}

/// What a row is: a Runners flag (the precursor strategy) or a buying burst ([`burst`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AlertKind {
    #[default]
    Runner,
    Burst,
    /// A running entry ([`running`]).
    Running,
}

impl AlertKind {
    /// The row's `kind` on the wire and its source label in the clients.
    pub fn as_str(self) -> &'static str {
        match self {
            AlertKind::Runner => "runner",
            AlertKind::Burst => "burst",
            AlertKind::Running => "running",
        }
    }
}

/// A buying-burst row's own facts ([`burst`]), beside what every tracked row has.
#[derive(Clone, Debug, PartialEq)]
pub struct BurstRow {
    /// The 5 s boundary scored (epoch ns); the alert is 1 s after it.
    pub boundary_ns: i64,
    /// The burst's first gate pass (epoch ns) and the last price before its window.
    pub start_ns: i64,
    pub start_price: f64,
    /// % from `start_price` to the alert price; % over the 30 s before the boundary.
    pub gain_pct: Option<f64>,
    pub move_pct: Option<f64>,
    /// Dollar rate over the last 15 s as a multiple of the 30-minute base; new-day-high prints,
    /// sweep seconds and up-tick volume share (%) in those 15 s.
    pub rate_x: f64,
    pub new_highs: f64,
    pub sweeps: f64,
    pub buy_pct: f64,
    /// The detector's outcome tracker key (`None` after a restart without one).
    pub seq: Option<u32>,
    /// The strict outcome (protocol §3) as last known.
    pub outcome: burst::Outcome,
}

/// One new flag or one breakout, as the push needs it.
#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub kind: NoticeKind,
    /// The board row's id (`Alert::id`).
    pub alert_id: String,
    pub symbol: String,
    /// The barrier it happened at (epoch ns).
    pub at_ns: i64,
    /// The price then (the flag price for a new alert).
    pub price: f64,
    /// % from the flag price (0 for a new alert); for a burst, % since the burst started.
    pub from_flag_pct: Option<f64>,
    /// The alert's reasons, strongest first.
    pub reasons: Vec<String>,
    /// A burst: minutes since it started.
    pub since_start_min: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Threshold {
    Budget,
    Fixed(f64),
}

/// `ROCKET_ALERTS_PER_DAY`, `ROCKET_THRESHOLD`, `RUNNERS_SCORER`.
#[derive(Clone, Debug, PartialEq)]
pub struct Policy {
    pub budget: u32,
    /// `None`: the scorer decides (its fixed threshold, else the budget).
    pub threshold: Option<Threshold>,
    /// `RUNNERS_SCORER`: which strategy a new board runs ([`production_scorer`]).
    pub scorer: ScorerChoice,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            budget: DEFAULT_BUDGET,
            threshold: None,
            scorer: ScorerChoice::default(),
        }
    }
}

impl Policy {
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Policy, String> {
        let var = |k: &str| {
            get(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let mut p = Policy::default();
        if let Some(v) = var("ROCKET_ALERTS_PER_DAY") {
            p.budget = v
                .parse::<u32>()
                .ok()
                .filter(|n| (1..=MAX_ALERTS_PER_DAY as u32).contains(n))
                .ok_or_else(|| {
                    format!("ROCKET_ALERTS_PER_DAY must be 1..={MAX_ALERTS_PER_DAY}: {v:?}")
                })?;
        }
        if let Some(v) = var("ROCKET_THRESHOLD") {
            p.threshold = Some(if v.eq_ignore_ascii_case("budget") {
                Threshold::Budget
            } else {
                Threshold::Fixed(v.parse::<f64>().ok().filter(|x| x.is_finite()).ok_or_else(
                    || format!("ROCKET_THRESHOLD is not a number or \"budget\": {v:?}"),
                )?)
            });
        }
        if let Some(v) = var("RUNNERS_SCORER") {
            p.scorer = ScorerChoice::parse(&v)?;
        }
        Ok(p)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mark {
    pub start_ns: i64,
    pub mark_ns: i64,
    pub start_price: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    /// Bars through this key were looked at.
    checked: u32,
    pub high: f64,
    pub low: f64,
    pub mark: Option<Mark>,
    pub settled: bool,
    pub partial: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Alert {
    pub id: String,
    pub symbol: Arc<str>,
    pub at_ns: i64,
    pub price: f64,
    pub score: f64,
    pub threshold: f64,
    pub reasons: Vec<String>,
    pub strategy: String,
    pub version: String,
    pub outcome: Outcome,
    /// The later-move outcomes and the momentum state ([`track`]).
    pub track: Track,
    pub kind: AlertKind,
    /// A burst row's facts (`kind == Burst`).
    pub burst: Option<Box<BurstRow>>,
}

impl Alert {
    fn start_offset(&self) -> Option<i64> {
        self.outcome.mark.map(|m| m.start_ns - self.at_ns)
    }
    /// The bake-off's HIT: a rocket starting 60–600 s after the alert.
    pub fn hit10(&self) -> bool {
        self.start_offset()
            .is_some_and(|d| (MIN_NS..=10 * MIN_NS).contains(&d))
    }
    /// The same out to 30 minutes.
    pub fn hit30(&self) -> bool {
        self.start_offset()
            .is_some_and(|d| (MIN_NS..=30 * MIN_NS).contains(&d))
    }
    pub fn max_gain_pct(&self) -> Option<f64> {
        (self.outcome.high.is_finite() && self.price > 0.0)
            .then(|| (self.outcome.high / self.price - 1.0) * 100.0)
    }
    pub fn max_drawdown_pct(&self) -> Option<f64> {
        (self.outcome.low.is_finite() && self.price > 0.0)
            .then(|| (self.outcome.low / self.price - 1.0) * 100.0)
    }
    /// Max gain / drawdown over the 2 hours after the alert, max gain to 20:00 ([`track`]).
    pub fn max_gain_2h_pct(&self) -> Option<f64> {
        self.track.pct(self.track.high_2h)
    }
    pub fn max_drawdown_2h_pct(&self) -> Option<f64> {
        self.track.pct(self.track.low_2h)
    }
    pub fn max_gain_session_pct(&self) -> Option<f64> {
        self.track.pct(self.track.high_day)
    }
    /// Not a strict hit, but +15 % or more within 2 hours (the panel's "ran later").
    pub fn ran_later(&self) -> bool {
        !self.hit10()
            && self
                .max_gain_2h_pct()
                .is_some_and(|g| g >= track::RAN_LATER_PCT)
    }

    /// `pending` | `rocketed` | `late` | `moved` | `faded` (module docs, "Outcomes").
    pub fn state(&self) -> &'static str {
        if self.kind == AlertKind::Burst {
            // A burst row has its own strict outcome (`BurstRow::outcome`), not this one.
            "pending"
        } else if self.hit10() {
            "rocketed"
        } else if self.hit30() {
            "late"
        } else if self.outcome.mark.is_some() {
            "moved"
        } else if self.outcome.settled {
            "faded"
        } else {
            "pending"
        }
    }

    fn alert_line(&self, date: NaiveDate, mode: &str) -> String {
        json!({
            "kind": "alert", "v": 1, "id": self.id, "date": date.to_string(),
            "symbol": &*self.symbol, "alert_ns": self.at_ns, "price": self.price,
            "score": self.score, "threshold": finite(self.threshold), "mode": mode,
            "reasons": self.reasons, "strategy": self.strategy, "version": self.version,
        })
        .to_string()
    }

    /// The `running` log line: the entry and what the rule read.
    fn running_line(&self, date: NaiveDate, e: &running::Entry) -> String {
        json!({
            "kind": "running", "v": 1, "id": self.id, "date": date.to_string(),
            "symbol": &*self.symbol, "alert_ns": self.at_ns, "price": self.price,
            "score": self.score, "reasons": self.reasons, "strategy": self.strategy,
            "version": self.version, "base_low": e.base_low, "gain": e.gain, "req": e.req,
            "dollars5": e.dollars5, "rvol30": e.rvol30,
        })
        .to_string()
    }

    fn outcome_line(&self, date: NaiveDate, settled_ns: i64) -> String {
        let m = self.outcome.mark;
        json!({
            "kind": "outcome", "v": 1, "id": self.id, "date": date.to_string(),
            "symbol": &*self.symbol, "alert_ns": self.at_ns, "settled_ns": settled_ns,
            "rocket10": self.hit10(), "rocket30": self.hit30(),
            "start_ns": m.map(|m| m.start_ns), "mark_ns": m.map(|m| m.mark_ns),
            "start_price": m.map(|m| m.start_price),
            "max_gain_pct": self.max_gain_pct().map(round2),
            "max_drawdown_pct": self.max_drawdown_pct().map(round2),
            "partial": self.outcome.partial,
        })
        .to_string()
    }
}

impl Alert {
    /// The 2-hour (`final: false`) or end-of-day (`final: true`) later-move line ([`log`]).
    fn later_line(&self, date: NaiveDate, at_ns: i64, final_: bool) -> String {
        let t = &self.track;
        json!({
            "kind": "later", "v": 1, "id": self.id, "date": date.to_string(),
            "symbol": &*self.symbol, "alert_ns": self.at_ns, "at_ns": at_ns, "final": final_,
            "max_gain_2h_pct": self.max_gain_2h_pct().map(round2),
            "max_drawdown_2h_pct": self.max_drawdown_2h_pct().map(round2),
            "max_gain_session_pct": self.max_gain_session_pct().map(round2),
            "rocket_later": t.later.is_some(),
            "later_start_ns": t.later.map(|m| m.start_ns),
            "later_mark_ns": t.later.map(|m| m.mark_ns),
            "complete_2h": at_ns >= self.at_ns + track::LATER_WINDOW_NS,
            "resumed": t.resumed,
        })
        .to_string()
    }

    fn state_line(&self, date: NaiveDate, tr: &Transition) -> String {
        json!({
            "kind": "state", "v": 1, "id": self.id, "date": date.to_string(),
            "symbol": &*self.symbol, "alert_ns": self.at_ns, "at_ns": tr.at_ns,
            "from": tr.from.as_str(), "to": tr.to.as_str(), "price": finite(tr.price),
            "high": finite(tr.high), "vwap": finite(tr.vwap),
            "from_flag_pct": self.track.pct(tr.price).map(round2),
            "from_high_pct": (tr.high > 0.0 && tr.price.is_finite())
                .then(|| round2((tr.price / tr.high - 1.0) * 100.0)),
        })
        .to_string()
    }
}

impl Alert {
    /// The `burst` log line ([`log`]): the alert, its burst facts and the 34 features.
    fn burst_line(&self, date: NaiveDate, per_day: u32, x: &[f64; burst::NF]) -> String {
        let b = self.burst.as_deref();
        let feats: serde_json::Map<String, Value> = burst::FEATS
            .iter()
            .zip(x)
            .map(|(n, v)| (n.to_string(), json!(finite(*v))))
            .collect();
        json!({
            "kind": "burst", "v": 1, "id": self.id, "date": date.to_string(),
            "symbol": &*self.symbol, "alert_ns": self.at_ns,
            "boundary_ns": b.map(|b| b.boundary_ns), "price": self.price,
            "score": self.score, "threshold": self.threshold, "per_day": per_day,
            "start_ns": b.map(|b| b.start_ns), "start_price": b.and_then(|b| finite(b.start_price)),
            "gain_pct": b.and_then(|b| b.gain_pct).map(round2),
            "move_pct": b.and_then(|b| b.move_pct).map(round2),
            "rate_x": b.map(|b| round2(b.rate_x)), "new_highs": b.map(|b| b.new_highs),
            "sweeps": b.map(|b| b.sweeps), "buy_pct": b.map(|b| round2(b.buy_pct)),
            "reasons": self.reasons, "features": feats,
            "strategy": self.strategy, "version": self.version,
        })
        .to_string()
    }

    /// The `burst_outcome` log line: CONT10/20/30 and the 30-minute MFE / MAE (protocol §3).
    fn burst_outcome_line(&self, date: NaiveDate, settled_ns: i64) -> Option<String> {
        let o = &self.burst.as_deref()?.outcome;
        let c = o.cont();
        Some(
            json!({
                "kind": "burst_outcome", "v": 1, "id": self.id, "date": date.to_string(),
                "symbol": &*self.symbol, "alert_ns": self.at_ns, "settled_ns": settled_ns,
                "price": o.pa, "cont10": c[0], "cont20": c[1], "cont30": c[2],
                "stopped": o.stopped(), "state": o.state(), "t20_s": o.t20_s(),
                "mfe_pct": finite(o.mfe_pct()).map(round2),
                "mae_pct": finite(o.mae_pct()).map(round2),
                "partial": o.partial,
            })
            .to_string(),
        )
    }
}

/// The end (epoch ns) of the slot `key` on the day starting at `t0` with ordinal `ord`.
fn end_of(t0: i64, ord: u32, key: u32) -> i64 {
    let slot = i64::from(key) - i64::from(features::key_of(ord, 0));
    t0 + (slot + 1) * BAR_NS
}

fn finite(x: f64) -> Option<f64> {
    x.is_finite().then_some(x)
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// A symbol's last price and previous close (dollars), for the panel rows.
pub type Quote = (Option<f64>, Option<f64>);

/// A symbol's micro-pullback levels for its row, from the stock-context engine (the market's
/// `levels_json`), `None` without one.
pub type Levels<'a> = &'a dyn Fn(&str) -> Option<Value>;

/// What one barrier cost, for the logs and the cost test.
#[derive(Clone, Copy, Debug, Default)]
pub struct Cost {
    pub elapsed: Duration,
    pub symbols: usize,
    pub scored: usize,
}

pub struct RocketBoard {
    enabled: bool,
    pub(crate) tape: Tape,
    scorer: Arc<dyn RocketScorer>,
    policy: Policy,
    date: Option<NaiveDate>,
    t0: i64,
    ord: u32,
    processed: u32,
    alerts: Vec<Alert>,
    last_alert: HashMap<Arc<str>, i64>,
    block: Option<i64>,
    block_max: HashMap<Arc<str>, f64>,
    /// The day's largest finished block maxima (at most `budget`).
    top: Vec<f64>,
    priors: Vec<(NaiveDate, String, String, f64)>,
    days: Vec<DaySummary>,
    day_written: bool,
    threshold: f64,
    pending: Vec<(NaiveDate, String)>,
    revision: u64,
    last_cost: Option<Cost>,
    /// Symbols with a trading halt in force (`chart.halt`, [`RocketBoard::set_halted`]).
    halted: HashSet<String>,
    /// The flagged symbols last handed to the stock-context engine ([`RocketBoard::take_watch`]).
    watch_sent: Vec<String>,
    /// New flags and breakouts since the last [`RocketBoard::take_notices`].
    notices: Vec<Notice>,
    /// Buying bursts ([`burst`]): live market only (`RocketBoard::enable_burst`).
    burst: Option<burst::BurstDetector>,
    /// The running entry's facts for `running_facts_date` ([`running::facts`]), and the date
    /// whose facts the market should load (once per date).
    running_facts: HashMap<String, running::Fact>,
    running_facts_date: Option<NaiveDate>,
    running_facts_wanted: Option<NaiveDate>,
}

impl RocketBoard {
    pub fn new(enabled: bool, scorer: Arc<dyn RocketScorer>, policy: Policy) -> Self {
        RocketBoard {
            enabled,
            tape: Tape::default(),
            scorer,
            policy,
            date: None,
            t0: 0,
            ord: 0,
            processed: 0,
            alerts: Vec::new(),
            last_alert: HashMap::new(),
            block: None,
            block_max: HashMap::new(),
            top: Vec::new(),
            priors: Vec::new(),
            days: Vec::new(),
            day_written: false,
            threshold: f64::INFINITY,
            pending: Vec::new(),
            revision: 0,
            last_cost: None,
            halted: HashSet::new(),
            watch_sent: Vec::new(),
            notices: Vec::new(),
            burst: None,
            running_facts: HashMap::new(),
            running_facts_date: None,
            running_facts_wanted: None,
        }
    }

    /// The trading date whose running facts ([`running::facts`]) the market should load.
    pub fn take_running_facts_request(&mut self) -> Option<NaiveDate> {
        self.running_facts_wanted.take()
    }

    /// Installs a trading date's running facts.
    pub fn set_running_facts(&mut self, date: NaiveDate, facts: HashMap<String, running::Fact>) {
        if self.date.is_none_or(|d| d == date) {
            self.running_facts = facts;
            self.running_facts_date = Some(date);
        }
    }

    /// Turns on the buying-burst detector ([`burst`]) with `policy` (a no-op when the board or
    /// `BURST_ALERTS` is off). Only the live market calls this: a replay never alerts on bursts.
    pub fn enable_burst(&mut self, policy: burst::BurstPolicy) {
        if self.enabled && policy.enabled {
            self.burst = Some(burst::BurstDetector::new(policy));
        }
    }

    pub fn burst(&self) -> Option<&burst::BurstDetector> {
        self.burst.as_ref()
    }

    /// The trading date whose ADV30 the burst detector wants loaded (once per date).
    pub fn take_burst_adv_request(&mut self) -> Option<NaiveDate> {
        self.burst.as_mut()?.take_adv_request()
    }

    pub fn set_burst_adv(&mut self, date: NaiveDate, adv: HashMap<String, f64>) {
        if let Some(b) = self.burst.as_mut() {
            b.set_adv(date, adv);
        }
    }

    /// A restart's history load: the detector to rebuild one symbol's day in, when bursts are
    /// on (`burst` module docs, "After a restart").
    pub fn burst_rebuilder(&self) -> Option<burst::BurstDetector> {
        self.burst
            .as_ref()
            .map(|_| burst::BurstDetector::rebuilder())
    }

    /// Adopts a symbol's rebuilt burst state ([`burst::BurstDetector::adopt`]).
    pub fn adopt_burst(&mut self, symbol: &str, rebuilt: burst::BurstDetector, now: i64) {
        if let Some(b) = self.burst.as_mut() {
            b.adopt(symbol, rebuilt, now);
        }
    }

    /// Whether a print of a symbol not loaded yet needs its history for bursts: the detector
    /// does not gate a symbol before its day is rebuilt, so one printing in its band must not
    /// wait for the background preload.
    pub fn burst_wants_history(&self, t: &market_events::Trade) -> bool {
        self.enabled && self.burst.is_some() && burst::in_wide_band(t.price.micros())
    }

    fn runners(&self) -> impl Iterator<Item = &Alert> {
        self.alerts.iter().filter(|a| a.kind == AlertKind::Runner)
    }

    /// Off: takes no prints, ticks nothing.
    pub fn disabled() -> Self {
        Self::new(false, Arc::new(PlaceholderB2), Policy::default())
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn alerts(&self) -> &[Alert] {
        &self.alerts
    }

    pub fn last_cost(&self) -> Option<Cost> {
        self.last_cost
    }

    /// Changes whenever an alert or an outcome does.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The effective threshold mode.
    pub fn mode(&self) -> Threshold {
        match self.policy.threshold {
            Some(t) => t,
            None => match self.scorer.fixed_threshold() {
                Some(x) => Threshold::Fixed(x),
                None => Threshold::Budget,
            },
        }
    }

    fn cap(&self) -> usize {
        let n = self.policy.budget as usize;
        match self.mode() {
            Threshold::Budget => n,
            Threshold::Fixed(_) => (5 * n).max(50),
        }
        .min(MAX_ALERTS_PER_DAY)
    }

    /// One print with the chart engine's verdict ([`features::Tape::apply_trade`]). O(1).
    pub fn apply_trade(&mut self, t: &market_events::Trade, counted: crate::quotes::Counted) {
        if self.enabled {
            self.tape.apply_trade(t, counted);
            if let Some(b) = self.burst.as_mut() {
                b.apply_trade(t, counted);
            }
        }
    }

    /// A print of a symbol whose history is not loaded yet (`crate::history::hold_unloaded`):
    /// the tape takes it as usual (the load replaces it), the burst detector marks the symbol
    /// unrestored until [`RocketBoard::adopt_burst`].
    pub fn apply_unrestored_trade(
        &mut self,
        t: &market_events::Trade,
        counted: crate::quotes::Counted,
    ) {
        if self.enabled {
            self.tape.apply_trade(t, counted);
            if let Some(b) = self.burst.as_mut() {
                b.apply_trade_unrestored(t, counted);
            }
        }
    }

    /// A trading halt started (`true`) or ended for `symbol` (the chart engine's `chart.halt`).
    pub fn set_halted(&mut self, symbol: &str, halted: bool) {
        if !self.enabled {
            return;
        }
        let changed = if halted {
            self.halted.insert(symbol.to_string())
        } else {
            self.halted.remove(symbol)
        };
        if changed && self.alerts.iter().any(|a| &*a.symbol == symbol) {
            self.revision += 1;
        }
    }

    /// Today's flagged symbols when they changed since the last call (sorted, deduplicated):
    /// what the stock-context engine runs the micro-pullback for (`stock_context::Engine::set_watched`).
    pub fn take_watch(&mut self) -> Option<Vec<String>> {
        let mut v: Vec<String> = self.alerts.iter().map(|a| a.symbol.to_string()).collect();
        v.sort();
        v.dedup();
        if v == self.watch_sent {
            return None;
        }
        self.watch_sent = v.clone();
        Some(v)
    }

    /// The forward-log lines produced since the last take, by trading date.
    pub fn take_log(&mut self) -> Vec<(NaiveDate, String)> {
        std::mem::take(&mut self.pending)
    }

    /// The new flags and breakouts since the last take (module docs, "Phone pushes").
    pub fn take_notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.notices)
    }

    /// Tests: a notice as if the board had produced it.
    #[cfg(test)]
    pub(crate) fn inject_notice(&mut self, n: Notice) {
        self.notice(n);
    }

    fn notice(&mut self, n: Notice) {
        if self.notices.len() < MAX_NOTICES {
            self.notices.push(n);
        }
    }

    /// The median of the last five days' ideal thresholds for this scorer.
    fn prior(&self) -> Option<f64> {
        let mut v: Vec<f64> = self
            .priors
            .iter()
            .rev()
            .filter(|(_, s, ver, t)| {
                s == self.scorer.id() && ver == self.scorer.version() && t.is_finite()
            })
            .take(5)
            .map(|p| p.3)
            .collect();
        if v.is_empty() {
            return None;
        }
        v.sort_by(f64::total_cmp);
        Some(v[v.len() / 2])
    }

    fn flush_block(&mut self) {
        let n = self.policy.budget.max(1) as usize;
        self.top.extend(self.block_max.drain().map(|(_, v)| v));
        self.top.sort_by(|a, b| b.total_cmp(a));
        self.top.truncate(n);
    }

    fn kth(&self, k: usize) -> Option<f64> {
        let mut vals: Vec<f64> = self.top.clone();
        vals.extend(self.block_max.values().copied());
        vals.sort_by(|a, b| b.total_cmp(a));
        vals.get(k.max(1) - 1).copied()
    }

    fn compute_threshold(&self, slot: i64) -> f64 {
        match self.mode() {
            Threshold::Fixed(t) => t,
            Threshold::Budget => {
                let n = f64::from(self.policy.budget.max(1));
                let e = (slot + 1) as f64 / NSLOT as f64;
                let today = self.kth((n * e).ceil() as usize);
                let est = match (self.prior(), today) {
                    (Some(p), Some(t)) => {
                        let w = (e * 2.0).min(1.0);
                        p * (1.0 - w) + t * w
                    }
                    (Some(p), None) => p,
                    (None, Some(t)) if slot >= 240 => t,
                    _ => f64::INFINITY,
                };
                est.max(self.scorer.floor())
            }
        }
    }

    /// The day ended (20:00 ET) or a new one began: settle what is open and write the day line.
    fn finish_day(&mut self, now: i64) {
        let Some(date) = self.date else { return };
        if self.day_written {
            return;
        }
        self.day_written = true;
        for i in 0..self.alerts.len() {
            if !self.alerts[i].outcome.settled {
                self.alerts[i].outcome.settled = true;
                self.alerts[i].outcome.partial = true;
                let line = self.alerts[i].outcome_line(date, now);
                self.pending.push((date, line));
            }
            if !self.alerts[i].track.logged_final {
                self.alerts[i].track.logged_final = true;
                let line = self.alerts[i].later_line(date, now, true);
                self.pending.push((date, line));
            }
        }
        self.flush_block();
        let ideal = self
            .top
            .get(self.policy.budget.max(1) as usize - 1)
            .copied();
        let s = self.summary(date);
        self.pending.push((
            date,
            json!({
                "kind": "day", "v": 1, "date": date.to_string(), "alerts": s.alerts,
                "hits10": s.hits10, "hits30": s.hits30,
                "ran_later": self.runners().filter(|a| a.ran_later()).count(),
                "ideal_threshold": ideal.and_then(finite),
                "strategy": self.scorer.id(), "version": self.scorer.version(),
            })
            .to_string(),
        ));
        if let Some(t) = ideal.and_then(finite) {
            self.priors.push((
                date,
                self.scorer.id().to_string(),
                self.scorer.version().to_string(),
                t,
            ));
        }
        self.revision += 1;
    }

    fn summary(&self, date: NaiveDate) -> DaySummary {
        DaySummary {
            date,
            alerts: self.runners().count() as u32,
            hits10: self.runners().filter(|a| a.hit10()).count() as u32,
            hits30: self.runners().filter(|a| a.hit30()).count() as u32,
        }
    }

    fn roll(&mut self, date: NaiveDate, now: i64) {
        if let Some(prev) = self.date {
            self.finish_day(now);
            let s = self.summary(prev);
            self.days.retain(|d| d.date != prev);
            self.days.push(s);
            let skip = self.days.len().saturating_sub(log::HISTORY_DAYS);
            self.days.drain(..skip);
        }
        let (t0, ord) = self.tape.day(date);
        self.date = Some(date);
        self.t0 = t0;
        self.ord = ord;
        self.processed = 0;
        self.alerts.clear();
        self.last_alert.clear();
        self.halted.clear();
        self.block = None;
        self.block_max.clear();
        self.top.clear();
        self.day_written = false;
        self.threshold = f64::INFINITY;
        self.revision += 1;
        if self.running_facts_date != Some(date) {
            self.running_facts.clear();
            self.running_facts_date = None;
            self.running_facts_wanted = Some(date);
        }
    }

    /// Processes the latest completed 15 s barrier at the market clock `now` (epoch ns), if it
    /// was not processed yet: scores, alerts, outcomes. Returns true when the alerts or their
    /// outcomes changed. `prev_close` is the quote board's previous close.
    pub fn tick(&mut self, now: i64, prev_close: impl Fn(&str) -> Option<f64>) -> bool {
        if !self.enabled {
            return false;
        }
        let rev = self.revision;
        let clock = now;
        // The tape may already hold prints a little past the clock (a replay at speed, a clock
        // frame behind the feed). The barrier is the slot before the one the newest print is in,
        // so a print past the barrier is always in the slot in progress, never in a finished
        // one; a print stamped implausibly far ahead (over a minute) cannot drag it along.
        let now = now.max(self.tape.latest_ts.min(now + MIN_NS));
        let date = market_events::session::trading_date(UnixNanos(now));
        if self.date != Some(date) {
            self.roll(date, now);
        }
        // Bursts run on the clock itself: a 5 s boundary is checked 1 s after it, with the
        // prints that arrived by then (`burst` module docs).
        self.burst_tick(clock, &prev_close);
        let mut slot = (now - self.t0).div_euclid(BAR_NS) - 1;
        if slot < 0 {
            return self.revision != rev;
        }
        let end_of_day = slot >= NSLOT - 1 && now >= self.t0 + NSLOT * BAR_NS;
        slot = slot.min(NSLOT - 1);
        let b = Barrier::new(self.t0, self.ord, slot);
        if b.key > self.processed {
            self.processed = b.key;
            self.barrier(&b, &prev_close);
        }
        if end_of_day {
            self.finish_day(now);
        }
        self.revision != rev
    }

    /// The burst detector's due checks: new burst rows (log line, phone notice), the open
    /// outcomes' progress, the settled outcomes (log line).
    fn burst_tick(&mut self, now: i64, prev_close: &dyn Fn(&str) -> Option<f64>) {
        let Some(det) = self.burst.as_mut() else {
            return;
        };
        let new = det.tick(now, prev_close);
        let settled = det.take_settled();
        let per_day = det.policy().per_day;
        let Some(date) = self.date else { return };
        for a in new {
            let p = &a.pass;
            let slot = (a.at_ns - self.t0).div_euclid(BAR_NS).clamp(0, NSLOT - 1);
            let row = BurstRow {
                boundary_ns: p.t_ns,
                start_ns: p.ep_start_ns,
                start_price: p.ep_ref,
                gain_pct: a.gain_pct(),
                move_pct: a.move_pct(),
                rate_x: p.rate_x,
                new_highs: p.new_highs,
                sweeps: p.sweeps,
                buy_pct: p.buy15 * 100.0,
                seq: Some(a.seq),
                outcome: burst::Outcome::new(a.seq, a.at_ns, a.price),
            };
            let alert = Alert {
                id: format!("{date}-{}-b{}", p.symbol, p.hi),
                symbol: p.symbol.clone(),
                at_ns: a.at_ns,
                price: a.price,
                score: p.score,
                threshold: a.threshold,
                reasons: a.reasons(),
                strategy: burst::STRATEGY.to_string(),
                version: burst::VERSION.to_string(),
                // The Runners strict outcome (a +30 % rocket mark) does not apply: settled from
                // the start so it never writes one. The burst's own is `BurstRow::outcome`.
                outcome: Outcome {
                    checked: features::key_of(self.ord, slot),
                    high: f64::NAN,
                    low: f64::NAN,
                    mark: None,
                    settled: true,
                    partial: false,
                },
                track: Track::new(a.at_ns, a.price),
                kind: AlertKind::Burst,
                burst: Some(Box::new(row)),
            };
            self.pending
                .push((date, alert.burst_line(date, per_day, &p.x)));
            self.notice(Notice {
                kind: NoticeKind::Burst,
                alert_id: alert.id.clone(),
                symbol: alert.symbol.to_string(),
                at_ns: alert.at_ns,
                price: alert.price,
                from_flag_pct: a.gain_pct().map(round2),
                reasons: alert.reasons.clone(),
                since_start_min: Some((a.at_ns - p.ep_start_ns) as f64 / MIN_NS as f64),
            });
            if self.alerts.len() < 2 * MAX_ALERTS_PER_DAY {
                self.alerts.push(alert);
            }
            self.revision += 1;
        }
        // Open outcomes: the latest progress onto the rows.
        let det = self.burst.as_ref().expect("checked above");
        for a in &mut self.alerts {
            let Some(b) = a.burst.as_deref_mut() else {
                continue;
            };
            if b.outcome.settled {
                continue;
            }
            if let Some(seq) = b.seq
                && let Some(o) = det.outcome(&a.symbol, seq)
                && *o != b.outcome
            {
                b.outcome = o.clone();
                self.revision += 1;
            }
        }
        for s in settled {
            let Some(a) = self.alerts.iter_mut().find(|a| {
                a.symbol == s.symbol
                    && a.burst
                        .as_deref()
                        .is_some_and(|b| b.seq == Some(s.outcome.seq))
            }) else {
                continue;
            };
            if let Some(b) = a.burst.as_deref_mut() {
                b.outcome = s.outcome.clone();
            }
            if let Some(line) = a.burst_outcome_line(date, s.settled_ns) {
                self.pending.push((date, line));
            }
            self.revision += 1;
        }
    }

    fn barrier(&mut self, b: &Barrier, prev_close: &dyn Fn(&str) -> Option<f64>) {
        let start = Instant::now();
        let mut cands: Vec<(f64, Features)> = Vec::new();
        let mut running: Vec<(Arc<str>, running::Entry)> = Vec::new();
        let mut live = 0usize;
        let scorer = self.scorer.clone();
        for s in self.tape.syms.iter_mut() {
            if !features::settle(s, b) {
                continue;
            }
            let s: &features::Sym = s;
            live += 1;
            if let Some(x) = features::run_inputs(s, b)
                && let Some(e) = running::decide(&x, self.running_facts.get(&*s.symbol))
            {
                running.push((s.symbol.clone(), e));
            }
            if let Some(f) = features::features(s, b, || prev_close(&s.symbol))
                && let Some(sc) = scorer.score(&f)
                && sc.is_finite()
            {
                cands.push((sc, f));
            }
        }
        let scored = cands.len();
        // Block maxima for the adaptive threshold (module docs).
        let block = b.slot / 120;
        if self.block != Some(block) {
            self.flush_block();
            self.block = Some(block);
        }
        for (sc, f) in &cands {
            let e = self.block_max.entry(f.symbol.clone()).or_insert(*sc);
            if *sc > *e {
                *e = *sc;
            }
        }
        self.threshold = self.compute_threshold(b.slot);
        cands.sort_by(|a, b| b.0.total_cmp(&a.0));
        let date = self.date.expect("rolled before a barrier");
        let mode = match self.mode() {
            Threshold::Budget => "budget",
            Threshold::Fixed(_) => "fixed",
        };
        for (sc, f) in cands {
            if sc < self.threshold || self.runners().count() >= self.cap() {
                break;
            }
            if self
                .last_alert
                .get(&f.symbol)
                .is_some_and(|t| b.at_ns - t < DEDUP_NS)
            {
                continue;
            }
            let a = Alert {
                id: format!("{date}-{}-{}", f.symbol, b.slot),
                symbol: f.symbol.clone(),
                at_ns: b.at_ns,
                price: f.price,
                score: sc,
                threshold: self.threshold,
                reasons: scorer.reasons(&f),
                strategy: scorer.id().to_string(),
                version: scorer.version().to_string(),
                outcome: Outcome {
                    checked: b.key,
                    high: f64::NAN,
                    low: f64::NAN,
                    mark: None,
                    settled: false,
                    partial: false,
                },
                track: Track::new(b.at_ns, f.price),
                kind: AlertKind::Runner,
                burst: None,
            };
            self.pending.push((date, a.alert_line(date, mode)));
            self.last_alert.insert(f.symbol.clone(), b.at_ns);
            self.notice(Notice {
                kind: NoticeKind::New,
                alert_id: a.id.clone(),
                symbol: a.symbol.to_string(),
                at_ns: a.at_ns,
                price: a.price,
                from_flag_pct: Some(0.0),
                reasons: a.reasons.clone(),
                since_start_min: None,
            });
            self.alerts.push(a);
            self.revision += 1;
        }
        self.running_entries(b, date, running);
        self.outcomes(b, date);
        self.last_cost = Some(Cost {
            elapsed: start.elapsed(),
            symbols: live,
            scored,
        });
    }

    /// New running rows ([`running`]) at barrier `b`: the strongest move first (gain over the
    /// symbol's requirement), none for a symbol with a row today, at most
    /// [`running::MAX_PER_DAY`].
    fn running_entries(
        &mut self,
        b: &Barrier,
        date: NaiveDate,
        mut entries: Vec<(Arc<str>, running::Entry)>,
    ) {
        entries.sort_by(|x, y| (y.1.gain / y.1.req).total_cmp(&(x.1.gain / x.1.req)));
        for (symbol, e) in entries {
            let today = self
                .alerts
                .iter()
                .filter(|a| a.kind == AlertKind::Running)
                .count();
            if today >= running::MAX_PER_DAY || self.alerts.len() >= 2 * MAX_ALERTS_PER_DAY {
                break;
            }
            if self.alerts.iter().any(|a| a.symbol == symbol) {
                continue;
            }
            let a = Alert {
                id: format!("{date}-{symbol}-r{}", b.slot),
                symbol: symbol.clone(),
                at_ns: b.at_ns,
                price: e.price,
                score: e.gain / e.req,
                threshold: 1.0,
                reasons: e.reasons(),
                strategy: running::STRATEGY.to_string(),
                version: running::VERSION.to_string(),
                // The Runners strict outcome (a +30 % rocket mark) does not apply: settled from
                // the start, like a burst row's.
                outcome: Outcome {
                    checked: b.key,
                    high: f64::NAN,
                    low: f64::NAN,
                    mark: None,
                    settled: true,
                    partial: false,
                },
                track: Track::new(b.at_ns, e.price),
                kind: AlertKind::Running,
                burst: None,
            };
            self.pending.push((date, a.running_line(date, &e)));
            self.notice(Notice {
                kind: NoticeKind::Running,
                alert_id: a.id.clone(),
                symbol: a.symbol.to_string(),
                at_ns: a.at_ns,
                price: a.price,
                from_flag_pct: Some(round2(e.gain * 100.0)),
                reasons: a.reasons.clone(),
                since_start_min: None,
            });
            self.alerts.push(a);
            self.revision += 1;
        }
    }

    /// Advances every alert through barrier `b`: the strict outcome while it is open (module
    /// docs, "Outcomes"), the later-move outcomes and the momentum state until the day ends
    /// ([`track`]).
    fn outcomes(&mut self, b: &Barrier, date: NaiveDate) {
        let (t0, ord) = (self.t0, self.ord);
        let base = features::key_of(ord, 0);
        for i in 0..self.alerts.len() {
            if self.alerts[i].track.logged_final {
                continue;
            }
            let before = (
                self.alerts[i].outcome.clone(),
                self.alerts[i].track.high_day,
                self.alerts[i].track.later,
            );
            let at = self.alerts[i].at_ns;
            let settled = self.alerts[i].outcome.settled;
            // A symbol with no state yet (a restart before its first print or history load):
            // leave `checked` where it is so the bars since the alert are read once they exist.
            let Some(s) = self.tape.sym(&self.alerts[i].symbol) else {
                continue;
            };
            let bars = features::bars_between(s, self.alerts[i].outcome.checked, b.key);
            let mut new: Vec<track::NewBar> = Vec::with_capacity(bars.len());
            for (key, close, high, low) in bars {
                let end = end_of(t0, ord, key);
                new.push((key, end, close, high, low));
                if settled {
                    continue;
                }
                let o = &mut self.alerts[i].outcome;
                if end <= at + GAIN_WINDOW_NS {
                    o.high = if o.high.is_nan() {
                        high
                    } else {
                        o.high.max(high)
                    };
                    o.low = if o.low.is_nan() { low } else { o.low.min(low) };
                }
                if o.mark.is_none()
                    && let Some((start, l)) = features::mark_at(s, key)
                {
                    o.mark = Some(Mark {
                        start_ns: end_of(t0, ord, start),
                        mark_ns: end,
                        start_price: l,
                    });
                }
            }
            let strict = self.alerts[i].outcome.mark;
            let a = &mut self.alerts[i];
            a.track.bars(&new, |key, end| {
                // The strict mark is the first mark after the alert too.
                if let Some(m) = strict.filter(|m| m.mark_ns <= end) {
                    return Some(m);
                }
                features::mark_at(s, key).map(|(start, l)| Mark {
                    start_ns: end_of(t0, ord, start),
                    mark_ns: end,
                    start_price: l,
                })
            });
            let w = features::window(s, b.key, base);
            let x = track::Inputs {
                key: b.key,
                at_ns: b.at_ns,
                price: w.map_or(f64::NAN, |w| w.price),
                vwap: w.map_or(f64::NAN, |w| w.vwap),
                bar_traded: w.is_some_and(|w| w.bar_traded),
                bar_high: w.map_or(f64::NAN, |w| w.bar_high),
                bar_vol: w.map_or(0.0, |w| w.bar_vol),
                vol12: w.map_or(0.0, |w| w.vol12),
                pace_slot: w.map_or(0.0, |w| w.pace_slot),
                min_lows: w.map_or([f64::NAN; 3], |w| w.min_lows),
                close_1m: w.map_or(f64::NAN, |w| w.close_1m),
                halted: self.halted.contains(&*a.symbol),
            };
            if let Some(tr) = a.track.step(&x) {
                a.track.transitions += 1;
                if a.track.transitions <= track::MAX_TRANSITIONS {
                    let line = a.state_line(date, &tr);
                    self.pending.push((date, line));
                }
                let breakout = (tr.to == MState::BreakingOut && tr.from != MState::BreakingOut)
                    .then(|| Notice {
                        kind: NoticeKind::Breakout,
                        alert_id: a.id.clone(),
                        symbol: a.symbol.to_string(),
                        at_ns: tr.at_ns,
                        price: tr.price,
                        from_flag_pct: a.track.pct(tr.price).map(round2),
                        reasons: a.reasons.clone(),
                        since_start_min: None,
                    });
                if let Some(n) = breakout {
                    self.notice(n);
                }
                self.revision += 1;
            }
            let a = &mut self.alerts[i];
            a.outcome.checked = b.key;
            if !settled && b.at_ns >= at + SETTLE_NS {
                a.outcome.settled = true;
                let line = a.outcome_line(date, b.at_ns);
                self.pending.push((date, line));
            }
            if !a.track.logged_2h && b.at_ns >= at + track::LATER_WINDOW_NS {
                a.track.logged_2h = true;
                let line = a.later_line(date, b.at_ns, false);
                self.pending.push((date, line));
            }
            if (a.outcome.clone(), a.track.high_day, a.track.later) != before {
                self.revision += 1;
            }
        }
    }

    /// Restores today's alerts and the record from the forward log (a restart).
    pub fn restore(&mut self, today: NaiveDate, loaded: Loaded) {
        self.days = loaded.days;
        self.priors = loaded.thresholds;
        if self.date != Some(today) {
            self.roll(today, 0);
            // A roll from "nothing" must not write a day line for no day.
            self.pending.clear();
        }
        let base = features::key_of(self.ord, 0);
        let mut later = loaded.later;
        let mut states = loaded.states;
        let today_rows = loaded
            .today
            .into_iter()
            .map(|(v, o)| (AlertKind::Runner, v, o));
        let burst_rows = loaded
            .bursts
            .into_iter()
            .map(|(v, o)| (AlertKind::Burst, v, o));
        let running_rows = loaded
            .running
            .into_iter()
            .map(|v| (AlertKind::Running, v, None));
        for (kind, v, o) in today_rows.chain(burst_rows).chain(running_rows) {
            let is_burst = kind == AlertKind::Burst;
            let (Some(symbol), Some(at_ns)) = (
                v.get("symbol").and_then(Value::as_str),
                v.get("alert_ns").and_then(Value::as_i64),
            ) else {
                continue;
            };
            let f = |k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(f64::NAN);
            let slot = ((at_ns - self.t0).div_euclid(BAR_NS) - 1).clamp(0, NSLOT - 1);
            let symbol: Arc<str> = Arc::from(symbol);
            let mut outcome = Outcome {
                checked: base + slot as u32,
                high: f64::NAN,
                low: f64::NAN,
                mark: None,
                settled: false,
                partial: false,
            };
            let price = f("price");
            let o_vals = o.clone();
            if let Some(o) = o.as_ref().filter(|_| kind == AlertKind::Runner) {
                let g = |k: &str| o.get(k).and_then(Value::as_f64);
                outcome.settled = true;
                outcome.partial = o.get("partial").and_then(Value::as_bool) == Some(true);
                outcome.high = g("max_gain_pct").map_or(f64::NAN, |p| price * (1.0 + p / 100.0));
                outcome.low = g("max_drawdown_pct").map_or(f64::NAN, |p| price * (1.0 + p / 100.0));
                if let (Some(s), Some(m)) = (
                    o.get("start_ns").and_then(Value::as_i64),
                    o.get("mark_ns").and_then(Value::as_i64),
                ) {
                    outcome.mark = Some(Mark {
                        start_ns: s,
                        mark_ns: m,
                        start_price: g("start_price").unwrap_or(f64::NAN),
                    });
                }
            }
            let str_of = |k: &str| {
                v.get(k)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            let a = Alert {
                id: str_of("id"),
                symbol: symbol.clone(),
                at_ns,
                price,
                score: f("score"),
                threshold: f("threshold"),
                reasons: v
                    .get("reasons")
                    .and_then(Value::as_array)
                    .map(|r| {
                        r.iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default(),
                strategy: str_of("strategy"),
                version: str_of("version"),
                outcome,
                track: Track::new(at_ns, price),
                kind: AlertKind::Runner,
                burst: None,
            };
            let mut a = a;
            a.track = Track::new(at_ns, price);
            a.track.resumed = true;
            // The bars since the alert are re-read from the ring (the last 64 minutes); what
            // the logged lines knew is the floor (track docs; `resumed` says so).
            let pct_px = |p: Option<f64>| p.map_or(f64::NAN, |p| price * (1.0 + p / 100.0));
            let og = |k: &str| {
                o_vals
                    .as_ref()
                    .and_then(|o| o.get(k))
                    .and_then(Value::as_f64)
            };
            let (h30, l30) = (og("max_gain_pct"), og("max_drawdown_pct"));
            a.track.high_2h = pct_px(h30);
            a.track.low_2h = pct_px(l30);
            a.track.high_day = pct_px(h30);
            if let Some(l) = later.remove(&a.id) {
                let g = |k: &str| l.get(k).and_then(Value::as_f64);
                a.track.high_2h = pct_px(g("max_gain_2h_pct"));
                a.track.low_2h = pct_px(g("max_drawdown_2h_pct"));
                a.track.high_day = pct_px(g("max_gain_session_pct"));
                a.track.logged_2h = true;
                a.track.logged_final = l.get("final").and_then(Value::as_bool) == Some(true);
                if let (Some(s), Some(m)) = (
                    l.get("later_start_ns").and_then(Value::as_i64),
                    l.get("later_mark_ns").and_then(Value::as_i64),
                ) {
                    a.track.later = Some(Mark {
                        start_ns: s,
                        mark_ns: m,
                        start_price: f64::NAN,
                    });
                }
            }
            if a.track.later.is_none() {
                a.track.later = a.outcome.mark;
            }
            if let Some((st, n)) = states.remove(&a.id) {
                if let Some(m) = st.get("to").and_then(Value::as_str).and_then(MState::parse) {
                    a.track.state = m;
                }
                if let Some(t) = st.get("at_ns").and_then(Value::as_i64) {
                    a.track.since_ns = t;
                }
                a.track.transitions = n;
            }
            if kind == AlertKind::Running {
                // Its strict outcome never applies (`running_entries`).
                a.kind = AlertKind::Running;
                a.outcome.settled = true;
                if self.alerts.len() < 2 * MAX_ALERTS_PER_DAY {
                    self.alerts.push(a);
                }
                continue;
            }
            if is_burst {
                self.restore_burst(&mut a, &v, o.as_ref());
                if self.alerts.len() < 2 * MAX_ALERTS_PER_DAY {
                    self.alerts.push(a);
                }
                continue;
            }
            let e = self.last_alert.entry(symbol).or_insert(at_ns);
            *e = (*e).max(at_ns);
            if self.alerts.len() < MAX_ALERTS_PER_DAY {
                self.alerts.push(a);
            }
        }
        self.alerts.sort_by_key(|a| a.at_ns);
        self.revision += 1;
    }

    /// A burst row back from its `burst` line (and `burst_outcome` line, once settled). An open
    /// one is followed again from now on, `partial` (the detector's tracker was lost).
    fn restore_burst(&mut self, a: &mut Alert, v: &Value, o: Option<&Value>) {
        let f = |k: &str| v.get(k).and_then(Value::as_f64);
        let price = a.price;
        let mut out = burst::Outcome::new(0, a.at_ns, price);
        let mut seq = None;
        if let Some(o) = o {
            let b = |k: &str| o.get(k).and_then(Value::as_bool) == Some(true);
            let g = |k: &str| o.get(k).and_then(Value::as_f64);
            out.settled = true;
            out.partial = b("partial");
            // The logged facts, re-expressed as touches in the right order.
            for (x, k) in ["cont10", "cont20", "cont30"].into_iter().enumerate() {
                if b(k) {
                    out.t_up[x] = Some(a.at_ns + 1);
                }
            }
            if b("stopped") {
                out.t_stop = Some(a.at_ns + 2);
            }
            out.mfe_p = g("mfe_pct").map_or(price, |p| price * (1.0 + p / 100.0));
            out.mae_at_mfe = g("mae_pct").map_or(price, |p| price * (1.0 + p / 100.0));
        } else if let Some(det) = self.burst.as_mut() {
            seq = det.restore_alert(&a.symbol, a.at_ns, price, a.at_ns);
            if let Some(s) = seq {
                out = det.outcome(&a.symbol, s).cloned().unwrap_or(out);
            }
        }
        a.kind = AlertKind::Burst;
        // The Runners strict outcome never applies to a burst row (`burst_tick`).
        a.outcome.settled = true;
        a.outcome.checked = features::key_of(
            self.ord,
            (a.at_ns - self.t0).div_euclid(BAR_NS).clamp(0, NSLOT - 1),
        );
        a.burst = Some(Box::new(BurstRow {
            boundary_ns: v
                .get("boundary_ns")
                .and_then(Value::as_i64)
                .unwrap_or(a.at_ns - burst::SETTLE_NS),
            start_ns: v.get("start_ns").and_then(Value::as_i64).unwrap_or(a.at_ns),
            start_price: f("start_price").unwrap_or(f64::NAN),
            gain_pct: f("gain_pct"),
            move_pct: f("move_pct"),
            rate_x: f("rate_x").unwrap_or(f64::NAN),
            new_highs: f("new_highs").unwrap_or(0.0),
            sweeps: f("sweeps").unwrap_or(0.0),
            buy_pct: f("buy_pct").unwrap_or(f64::NAN),
            seq,
            outcome: out,
        }));
    }

    /// The panel as a `rocket.snapshot` / `rocket.update` frame. `quote` gives a symbol's last
    /// price and previous close, `levels` its micro-pullback levels when the stock-context engine
    /// runs ([`Levels`]).
    pub fn frame(
        &self,
        kind: &str,
        t: Option<i64>,
        quote: impl Fn(&str) -> Quote,
        levels: impl Fn(&str) -> Option<Value>,
    ) -> String {
        let body = self.body(&quote, &levels);
        let mut v = json!({ "type": kind, "t": t });
        if let (Some(m), Value::Object(b)) = (v.as_object_mut(), body) {
            m.extend(b);
        }
        v.to_string()
    }

    /// The frame without its type and clock: what decides whether an update is due.
    pub fn body(&self, quote: &dyn Fn(&str) -> Quote, levels: Levels<'_>) -> Value {
        let today = self.date.map(|d| self.summary(d));
        let known = self
            .runners()
            .filter(|a| a.outcome.settled || a.hit10())
            .count();
        let rows: Vec<Value> = self
            .alerts
            .iter()
            .rev()
            .take(ROWS)
            .map(|a| {
                let (last, pc) = quote(&a.symbol);
                let t = &a.track;
                let live = last.filter(|p| p.is_finite() && *p > 0.0);
                let (state, provisional) = t.shown(live, self.halted.contains(&*a.symbol));
                let now = live.unwrap_or(t.price);
                let high = t.high().max(live.unwrap_or(0.0));
                let from_high = (high > 0.0 && now.is_finite()).then(|| (now / high - 1.0) * 100.0);
                let move_1m =
                    (t.close_1m > 0.0 && now.is_finite()).then(|| (now / t.close_1m - 1.0) * 100.0);
                json!({
                    "id": a.id, "s": &*a.symbol, "at": a.at_ns / 1_000_000,
                    "p": a.price, "last": last, "pc": pc,
                    "score": (a.score * 100.0).round() / 100.0,
                    "reasons": a.reasons,
                    "outcome": {
                        "state": a.state(),
                        "gain": a.max_gain_pct().map(round2),
                        "drawdown": a.max_drawdown_pct().map(round2),
                        "partial": a.outcome.partial,
                        "gain_2h": a.max_gain_2h_pct().map(round2),
                        "drawdown_2h": a.max_drawdown_2h_pct().map(round2),
                        "gain_session": a.max_gain_session_pct().map(round2),
                        "rocket_later": t.later.is_some(),
                    },
                    "state": state.as_str(),
                    "state_since": t.since_ns / 1_000_000,
                    "provisional": provisional,
                    "from_flag_pct": t.pct(now).map(round2),
                    "from_high_pct": from_high.map(round2),
                    "move_1m_pct": move_1m.map(round2),
                    "pullback_depth_pct": (state == MState::Pullback)
                        .then(|| from_high.map(|x| round2(-x)))
                        .flatten(),
                    "levels": levels(&a.symbol),
                    "kind": a.kind.as_str(),
                    "burst": a.burst.as_deref().map(burst_json),
                })
            })
            .collect();
        let mode = match self.mode() {
            Threshold::Budget => "budget",
            Threshold::Fixed(_) => "fixed",
        };
        let mut rows = rows;
        for r in &mut rows {
            // A Runners flag carries no `burst` (older clients never saw the key).
            if let Some(m) = r.as_object_mut()
                && m.get("burst").is_some_and(Value::is_null)
            {
                m.remove("burst");
            }
        }
        let mut v = json!({
            "enabled": self.enabled,
            "strategy": {
                "id": self.scorer.id(), "version": self.scorer.version(),
                "placeholder": self.scorer.placeholder(),
            },
            "mode": mode,
            "budget": self.policy.budget,
            "threshold": finite(self.threshold).map(|x| (x * 1000.0).round() / 1000.0),
            "today": {
                "date": self.date.map(|d| d.to_string()),
                "alerts": today.as_ref().map_or(0, |s| s.alerts),
                "hits": today.as_ref().map_or(0, |s| s.hits10),
                "hits30": today.as_ref().map_or(0, |s| s.hits30),
                "known": known,
                "ran_later": self.runners().filter(|a| a.ran_later()).count(),
            },
            "days": self.days,
            "rows": rows,
        });
        if let Some(b) = self.burst.as_ref()
            && let Some(m) = v.as_object_mut()
        {
            let p = b.policy();
            m.insert(
                "burst".into(),
                json!({
                    "enabled": true,
                    "threshold": (p.threshold * 10_000.0).round() / 10_000.0,
                    "per_day": p.per_day,
                    "today": self.alerts.iter().filter(|a| a.kind == AlertKind::Burst).count(),
                    "typical": {
                        "mfe30_med_pct": burst::TYPICAL_MFE30_MED_PCT,
                        "mae_med_pct": burst::TYPICAL_MAE_MED_PCT,
                        "cont10_pct": burst::TYPICAL_CONT10_PCT,
                        "cont20_pct": burst::TYPICAL_CONT20_PCT,
                        "fires_after_min": [burst::TYPICAL_FIRES_AFTER_MIN.0, burst::TYPICAL_FIRES_AFTER_MIN.1],
                    },
                }),
            );
        }
        v
    }
}

/// A burst row's `burst` object on the wire (packages/shared/src/rocket.ts).
fn burst_json(b: &BurstRow) -> Value {
    let o = &b.outcome;
    let c = o.cont();
    json!({
        "start": b.start_ns / 1_000_000,
        "start_price": finite(b.start_price),
        "gain_pct": b.gain_pct.map(round2),
        "move_pct": b.move_pct.map(round2),
        "rate_x": round2(b.rate_x),
        "new_highs": b.new_highs,
        "sweeps": b.sweeps,
        "buy_pct": round2(b.buy_pct),
        "outcome": {
            "state": o.state(),
            "c10": c[0], "c20": c[1], "c30": c[2],
            "stopped": o.stopped(),
            "mfe": finite(o.mfe_pct()).map(round2),
            "mae": finite(o.mae_pct()).map(round2),
            "settled": o.settled,
        },
    })
}

/// The frame for a market whose board is off (`ROCKET_WATCH=0`).
pub fn disabled_frame(kind: &str) -> String {
    json!({
        "type": kind, "t": null, "enabled": false, "strategy": null, "mode": "budget",
        "budget": 0, "threshold": null,
        "today": {"date": null, "alerts": 0, "hits": 0, "hits30": 0, "known": 0, "ran_later": 0},
        "days": [], "rows": [],
    })
    .to_string()
}

#[cfg(test)]
mod tests;
