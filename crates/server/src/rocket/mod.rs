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
//!   has a row; their push ([`NoticeKind::Running`]) is per phone, on by default. Not in the day record.
//! - [`run`]: the "Run +20%" entry, the research labeller's +20 % leg (`rockets(d, 20)`) in
//!   `$0.10–$25`, computed once by the volume-shelf module's run trigger (`crate::zones`) and
//!   handed here ([`RocketBoard::add_runs`]). Its rows ([`AlertKind::Run`]) appear within
//!   seconds of the print that makes the leg (its early fire, [`run`]), are tracked like the
//!   others, logged as `run` lines and restored after a restart; one per symbol a day and none for a symbol that already has a row (an R2 flag, a
//!   burst or a running entry). Push ([`NoticeKind::Run`]) is per phone, on by default.
//!   Not in the day record, but the `day` line counts each source's rows (`rows`).
//! - [`fast`]: the "Fast move" entry, a stock in `$0.10–$25` that jumps +8 % in 60 s or +12 % in
//!   15 s at a held price with real trading behind it, read every second. Every counted print
//!   feeds a per-symbol 1 s tape ([`fast::FastTape`]) beside the 15 s tape; the once-a-second
//!   tick reads the second that just ended, only for symbols that printed in the 2 s before it.
//!   Its rows ([`AlertKind::Fast`]) are tracked like the run rows, logged as `fast` lines,
//!   restored after a restart (the 1 s tapes start empty then: 64 s of history is all they
//!   need), one per symbol a day and none for a symbol that already has a row. Push
//!   ([`NoticeKind::Fast`]) is per phone, on by default.
//!
//! So a row reaches Runners five ways: the strategy's flag (R2), a buying burst, a running entry,
//! a +20 % run or a fast move.
//!
//! # Follow-ups
//!
//! Each path adds a stock once a day (bursts excepted), so a later path firing on a stock that
//! already has a row used to be silent. It is new evidence (owner, 2026-10-23: Run +20% after a
//! Fast move, Running after Run +20%): the newest row of the stock records it ([`Alert::also`],
//! a secondary source chip, `also` on the wire, never a second row), the forward log gets an
//! `also` line (restored after a restart) and a [`NoticeKind::FollowUp`] notice leaves, once per
//! path per row. The row's own path, or a path already on it, adds nothing.
//!
//! A Runners stock's volume-shelf entry (`crate::zones`, handed over by
//! [`RocketBoard::add_shelf_entries`]) is a [`NoticeKind::Shelf`] notice too; no row changes.
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
//!
//! # Readiness and trading halts
//!
//! A provisional live-only prefix is excluded from precursor/running features and adaptive
//! calibration until history adoption. A short rebuild clears pending adoption but leaves a
//! Runners-only unavailable mark until the next real date roll or explicit successful adoption.
//! Other consumers continue taking live events; structural losses are never auto-retried.
//! Unverified delivery order is
//! still reported by scanner coverage and is not presented as complete feed coverage here.
//! Running also waits for the date's facts table (an explicitly loaded empty table retains the
//! known-absent fallback). Halted symbols cannot acquire new precursor/running rows. A matching
//! official resume admits them only after a completed bar whose OPEN is at or after that resume.
//! Halt state known when a barrier is processed wins even on an exact timestamp tie. Existing
//! rows remain visible; the precursor's windows may still span the halt. Open halts survive a
//! date roll and end only on a matching official resume, never on an invented timeout.

pub mod burst;
pub mod fast;
pub mod features;
pub mod log;
pub mod r0;
pub mod r2;
pub mod run;
pub mod running;
pub mod scorer;
pub mod track;

use std::collections::{HashMap, HashSet, VecDeque};
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
/// Rows sent to the panel: the most relevant ones when the day has more ([`row_rank`]), sent
/// newest first. 80 full rows are about 75 KB of JSON before the market adds levels and day
/// lines (`rocket::tests`, `the_row_limit_keeps_active_rows_then_the_newest`).
pub const ROWS: usize = 80;
/// Notices kept while nobody takes them (a board ticked without a market, a test).
const MAX_NOTICES: usize = 256;

/// What a phone push may be about (module docs, "Phone pushes").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NoticeKind {
    /// A new alert (a new row on the board).
    New,
    /// A flagged row's momentum state turned `breaking_out` (from any other state).
    Breakout,
    /// A new buying-burst row ([`burst`]).
    Burst,
    /// A new running row ([`running`]); per-phone switch, on by default.
    Running,
    /// A new "Run +20%" row ([`run`]); per-phone switch, on by default.
    Run,
    /// A new "Fast move" row ([`fast`]); per-phone switch, on by default.
    Fast,
    /// A Runners stock came down into a volume shelf (`crate::zones`, an `in_shelf` event).
    /// Held / lost outcomes never notify. On by default.
    Shelf,
    /// A later path (running, run, fast move) fired on a stock that already had a row: new
    /// evidence on the same row ([`Also`]). On by default.
    FollowUp,
    /// A Ross Panel member's MP5 pullback episode reached `crossing_provisional` (the cue's
    /// "Crossing"). One per episode. On by default. A member's crossing is never also sent as
    /// [`NoticeKind::SignalsCrossing`]: [`CueNotice::also_signals`] says it is on both panels.
    RossCrossing,
    /// The same for an episode on the Signals (Detection) panel whose stock is not a Ross member.
    SignalsCrossing,
    /// A Ross member's cue reached "At trigger" (armed, price within the near distance). Off by
    /// default.
    RossTrigger,
    /// The same on the Signals panel (not a Ross member). Off by default.
    SignalsTrigger,
    /// A Halt watcher gauge appeared (`crate::pressure`'s board gained the stock). Off by default.
    Halt,
    /// "Pullback setting up" (owner, 2026-10-25): an MP5 episode ARMED (trigger and stop fixed) on
    /// a stock with context, i.e. a Runners row earlier today or a Ross member
    /// (`crate::cue_notices`, "Pullback"). One per episode. On by default (owner asked for it).
    Pullback,
    /// The same episode's trigger being crossed: the setup's follow-up, under the same switch as
    /// [`NoticeKind::Pullback`]. Never for a Ross member, whose crossing is
    /// [`NoticeKind::RossCrossing`] already.
    PullbackCrossing,
}

impl NoticeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            NoticeKind::New => "new",
            NoticeKind::Breakout => "breakout",
            NoticeKind::Burst => "burst",
            NoticeKind::Running => "running",
            NoticeKind::Run => "run",
            NoticeKind::Fast => "fast",
            NoticeKind::Shelf => "shelf",
            NoticeKind::FollowUp => "followup",
            NoticeKind::RossCrossing => "ross_crossing",
            NoticeKind::SignalsCrossing => "signals_crossing",
            NoticeKind::RossTrigger => "ross_trigger",
            NoticeKind::SignalsTrigger => "signals_trigger",
            NoticeKind::Halt => "halt",
            NoticeKind::Pullback => "pullback",
            NoticeKind::PullbackCrossing => "pullback_crossing",
        }
    }

    pub const ALL: [NoticeKind; 15] = [
        NoticeKind::New,
        NoticeKind::Breakout,
        NoticeKind::Burst,
        NoticeKind::Running,
        NoticeKind::Run,
        NoticeKind::Fast,
        NoticeKind::Shelf,
        NoticeKind::FollowUp,
        NoticeKind::RossCrossing,
        NoticeKind::SignalsCrossing,
        NoticeKind::RossTrigger,
        NoticeKind::SignalsTrigger,
        NoticeKind::Halt,
        NoticeKind::Pullback,
        NoticeKind::PullbackCrossing,
    ];

    /// New evidence on a stock (owner, 2026-10-23): a burst, a crossing, a shelf entry or a
    /// follow-up. Its per-stock gap after an unrelated push is the burst's 2 minutes, so it is
    /// never swallowed by a push about something else minutes earlier
    /// (`crate::alerts::runners`, "Limits").
    pub fn is_new_evidence(self) -> bool {
        matches!(
            self,
            NoticeKind::Burst
                | NoticeKind::Shelf
                | NoticeKind::FollowUp
                | NoticeKind::RossCrossing
                | NoticeKind::SignalsCrossing
                // A setup almost always arms within minutes of the Runners push that put the
                // stock in context: as new evidence it waits 2 minutes after it, not 10.
                | NoticeKind::Pullback
                | NoticeKind::PullbackCrossing
        )
    }
}

/// What a notice beyond a row's own entry carries for its text (`crate::alerts::runners::message`).
#[derive(Clone, Debug, PartialEq)]
pub enum NoticeDetail {
    /// [`NoticeKind::FollowUp`]: the path that fired now, and the row's first source with when
    /// the row appeared (epoch ns).
    FollowUp {
        path: AlertKind,
        first: AlertKind,
        first_at_ns: i64,
    },
    /// [`NoticeKind::Shelf`]: the shelf's range (dollars).
    Shelf { lo: f64, hi: f64 },
    /// Ross / Signals crossing and at-trigger notices.
    Cue(CueNotice),
    /// [`NoticeKind::Halt`]: the gauge's side, its halt-risk score (0–100) and the LULD band it
    /// runs at (dollars).
    Halt { bull: bool, risk: u8, band: f64 },
    /// [`NoticeKind::Pullback`] / [`NoticeKind::PullbackCrossing`].
    Pullback(PullbackNotice),
}

/// A pullback setup's facts, read off the MP5 episode's own anchors at arming (never invented:
/// a value the episode does not have stays `None` and the text leaves it out).
#[derive(Clone, Debug, PartialEq)]
pub struct PullbackNotice {
    /// The detection episode: one notice per episode and stage.
    pub episode_id: String,
    /// The trigger (the impulse high H0) and the stop (the pullback low at arming), dollars.
    pub trigger: Option<f64>,
    pub stop: Option<f64>,
    /// How far the price dipped from the high, % of the stock price.
    pub dip_pct: Option<f64>,
    /// The share of the impulse retraced, %.
    pub retraced_pct: Option<f64>,
    /// Dollar volume per bar in the dip over the impulse's (0.4 = the dip traded 40% of the
    /// impulse's rate). Measured, never required (the MP5 rule's `volume_condition`).
    pub dip_volume_ratio: Option<f64>,
    /// The Signals panel lists this episode too (the Detection Panel is on, a non-member): the
    /// crossing then stands for the Signals one, so a device with only that switch still gets it.
    pub also_signals: bool,
}

/// A Ross or Signals cue notice's facts.
#[derive(Clone, Debug, PartialEq)]
pub struct CueNotice {
    /// The detection episode (`stock_context` `micro_pullback`): one notice per episode and stage.
    pub episode_id: String,
    /// The trigger level (dollars), when known.
    pub trigger: Option<f64>,
    /// A Ross member whose episode the Signals panel shows too (the Detection Panel is on): the
    /// one notice stands for both panels, so a device that wants either switch gets it.
    pub also_signals: bool,
}

/// A later path's evidence on an existing row (a follow-up, [`NoticeKind::FollowUp`]): shown as
/// a secondary source chip on that row, never as a second row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Also {
    pub kind: AlertKind,
    pub at_ns: i64,
    pub price: f64,
}

/// What a row is: a Runners flag (the precursor strategy) or a buying burst ([`burst`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AlertKind {
    #[default]
    Runner,
    Burst,
    /// A running entry ([`running`]).
    Running,
    /// A "Run +20%" entry ([`run`]).
    Run,
    /// A "Fast move" entry ([`fast`]).
    Fast,
}

impl AlertKind {
    /// The row's `kind` on the wire and its source label in the clients.
    pub fn as_str(self) -> &'static str {
        match self {
            AlertKind::Runner => "runner",
            AlertKind::Burst => "burst",
            AlertKind::Running => "running",
            AlertKind::Run => "run",
            AlertKind::Fast => "fast",
        }
    }

    pub fn parse(s: &str) -> Option<AlertKind> {
        [
            AlertKind::Runner,
            AlertKind::Burst,
            AlertKind::Running,
            AlertKind::Run,
            AlertKind::Fast,
        ]
        .into_iter()
        .find(|k| k.as_str() == s)
    }

    /// The source as the panel's chip and the push text name it.
    pub fn label(self) -> &'static str {
        match self {
            AlertKind::Runner => "Flag",
            AlertKind::Burst => "Buying burst",
            AlertKind::Running => "Running",
            AlertKind::Run => "Run +20%",
            AlertKind::Fast => "Fast move",
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
    /// Execution time of the print that triggered it, when the source knows it (a "Run +20%"
    /// row: [`crate::zones::Run::exec_ns`]): the start of the push's latency log
    /// (`crate::alerts::runners`, "Latency").
    pub exec_ns: Option<i64>,
    /// The newer kinds' facts ([`NoticeDetail`]); `None` for a row's own entry or breakout.
    pub detail: Option<NoticeDetail>,
}

impl Notice {
    /// The event's identity: what every surface dedupes on (one push per event, ever). A row's
    /// flag and its breakout share `alert_id`, so the kind is part of it.
    pub fn event_id(&self) -> String {
        format!("runners-{}-{}", self.kind.as_str(), self.alert_id)
    }

    /// Tests: a plain notice of `kind` for `symbol` at time 0.
    #[cfg(test)]
    pub fn test(kind: NoticeKind, symbol: &str) -> Notice {
        Notice {
            kind,
            alert_id: format!("{symbol}-1"),
            symbol: symbol.into(),
            at_ns: 0,
            price: 2.41,
            from_flag_pct: None,
            reasons: Vec::new(),
            since_start_min: None,
            exec_ns: None,
            detail: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Threshold {
    Budget,
    Fixed(f64),
}

/// `ROCKET_ALERTS_PER_DAY`, `ROCKET_THRESHOLD`, `RUNNERS_SCORER`, `RUNNERS_FAST`.
#[derive(Clone, Debug, PartialEq)]
pub struct Policy {
    pub budget: u32,
    /// `None`: the scorer decides (its fixed threshold, else the budget).
    pub threshold: Option<Threshold>,
    /// `RUNNERS_SCORER`: which strategy a new board runs ([`production_scorer`]).
    pub scorer: ScorerChoice,
    /// `RUNNERS_FAST` (default `1`): the "Fast move" entry ([`fast`]); `0` takes no prints into
    /// its tapes and makes no fast rows.
    pub fast: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            budget: DEFAULT_BUDGET,
            threshold: None,
            scorer: ScorerChoice::default(),
            fast: true,
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
        if let Some(v) = var("RUNNERS_FAST") {
            p.fast = match v.as_str() {
                "1" => true,
                "0" => false,
                _ => return Err(format!("RUNNERS_FAST must be 0 or 1: {v:?}")),
            };
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
    /// Later paths that fired on this stock after the row appeared (follow-ups, oldest first, at
    /// most one per path): the row's secondary source chips (`also` on the wire).
    pub also: Vec<Also>,
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
        if self.kind != AlertKind::Runner {
            // A burst row has its own strict outcome (`BurstRow::outcome`), not this one; a
            // running or run row has none (settled from the start, it would read "faded").
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

    /// The `run` log line: the entry and the run it came from (`crate::zones::Run`).
    fn run_line(&self, date: NaiveDate, r: &crate::zones::Run) -> String {
        json!({
            "kind": "run", "v": 1, "id": self.id, "date": date.to_string(),
            "symbol": &*self.symbol, "alert_ns": self.at_ns, "price": self.price,
            "score": self.score, "reasons": self.reasons, "strategy": self.strategy,
            "version": self.version, "start_ns": r.start_ns, "start_price": r.start_price,
            "plus20_ns": r.plus20_ns, "early": r.early, "exec_ns": r.exec_ns,
        })
        .to_string()
    }

    /// The `fast` log line: the entry, its triggering second (epoch s) and what the rule read.
    fn fast_line(&self, date: NaiveDate, sec: i64, e: &fast::FastEntry) -> String {
        json!({
            "kind": "fast", "v": 1, "id": self.id, "date": date.to_string(),
            "symbol": &*self.symbol, "alert_ns": self.at_ns, "price": self.price,
            "score": self.score, "reasons": self.reasons, "strategy": self.strategy,
            "version": self.version, "sec": sec, "secs": e.secs, "held": e.held, "low": e.low,
            "gain": e.gain, "trades": e.trades, "dollars": e.dollars,
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
    resumed_at: HashMap<String, i64>,
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
    /// The volume-shelf module's +20 % runs not made into rows yet ([`RocketBoard::add_runs`]).
    runs: Vec<(Arc<str>, crate::zones::Run)>,
    /// Volume-shelf entries (`crate::zones::ZoneBoard::take_shelf_entries`) not read yet: a
    /// Runners stock's one is a [`NoticeKind::Shelf`] notice at the next tick.
    shelves: Vec<(Arc<str>, crate::zones::Event)>,
    /// Fast move ([`fast`]): each symbol's 1 s tape (created on its first print in
    /// [`FAST_WIDE_BAND`], dropped at the day roll).
    fast: HashMap<Arc<str>, FastSym>,
    /// `(second, symbol)` on each symbol's first print of a second, oldest first: the symbols
    /// the tick reads (printed in the 2 s before the second read).
    fast_touched: VecDeque<(i64, Arc<str>)>,
    /// The last second the fast rule was read for.
    fast_read: i64,
}

/// A symbol's fast-move state ([`fast`]).
#[derive(Debug)]
struct FastSym {
    name: Arc<str>,
    tape: fast::FastTape,
    /// The second of its newest print, the last second read, the last fire (epoch ns).
    printed: i64,
    read: i64,
    fired_ns: Option<i64>,
}

/// Prices a fast-move tape starts at (wider than the rule's band, like the 15 s tape's, so a
/// stock coming into the band already has its minute).
const FAST_WIDE_BAND: (f64, f64) = (0.05, 30.0);
/// A bound on the touched queue when nobody ticks (a board fed without a market).
const FAST_TOUCHED_MAX: usize = 100_000;
const SEC_NS: i64 = 1_000_000_000;

/// The panel's keep order when a day has more than [`ROWS`] rows: rows in an active momentum
/// state (`breaking_out`, `running`, `pullback`) first, then the quiet ones, `fading` last;
/// newest first inside each.
fn row_rank(state: MState) -> u8 {
    match state {
        MState::BreakingOut | MState::Running | MState::Pullback => 0,
        MState::Fading => 2,
        _ => 1,
    }
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
            resumed_at: HashMap::new(),
            watch_sent: Vec::new(),
            notices: Vec::new(),
            burst: None,
            running_facts: HashMap::new(),
            running_facts_date: None,
            running_facts_wanted: None,
            runs: Vec::new(),
            shelves: Vec::new(),
            fast: HashMap::new(),
            fast_touched: VecDeque::new(),
            fast_read: i64::MIN,
        }
    }

    /// The volume-shelf module's +20 % runs (`crate::zones::ZoneBoard::take_entries`), made into
    /// "Run +20%" rows ([`run`]) at the next [`RocketBoard::tick`]. The market hands them over
    /// under the core lock right before the tick, so a row appears with its mark slot's decision.
    /// The volume-shelf module's new `in_shelf` entries (`crate::zones::ZoneBoard::take_shelf_entries`),
    /// read at the next [`RocketBoard::tick`]: a stock with a Runners row gets a
    /// [`NoticeKind::Shelf`] notice, any other stock nothing (the push is about Runners stocks).
    pub fn add_shelf_entries(&mut self, entries: Vec<(Arc<str>, crate::zones::Event)>) {
        if !self.enabled {
            return;
        }
        self.shelves.extend(entries);
        let over = self.shelves.len().saturating_sub(1024);
        self.shelves.drain(..over);
    }

    pub fn add_runs(&mut self, runs: Vec<(Arc<str>, crate::zones::Run)>) {
        if !self.enabled {
            return;
        }
        self.runs.extend(runs);
        let over = self.runs.len().saturating_sub(4096);
        self.runs.drain(..over);
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

    /// What the board holds, for the memory report (`Market::memory_report`): rows, 15 s tape
    /// symbols, fast-move tapes and the bytes those tapes hold (estimate), running facts.
    pub fn cost(&self) -> Value {
        let fast_bytes = self.fast.capacity() * (std::mem::size_of::<(Arc<str>, FastSym)>() + 8)
            + self.fast.len() * fast::FastTape::heap_bytes()
            + self.fast_touched.capacity() * std::mem::size_of::<(i64, Arc<str>)>();
        json!({
            "alerts": self.alerts.len(),
            "tape_symbols": self.tape.syms.len(),
            "fast_tapes": self.fast.len(),
            "fast_touched": self.fast_touched.len(),
            "fast_bytes": fast_bytes,
            "running_facts": self.running_facts.len(),
            "pending_log": self.pending.len(),
        })
    }

    /// Read-only startup/adoption counters, also exposed through the market's history status.
    pub fn readiness(&self) -> Value {
        let unrestored = self.tape.syms.iter().filter(|s| s.unrestored).count();
        let unavailable = self.tape.syms.iter().filter(|s| s.unavailable).count();
        let withheld = self
            .tape
            .syms
            .iter()
            .filter(|s| s.unrestored || s.unavailable)
            .count();
        json!({
            "symbols": self.tape.syms.len(), "unrestored": unrestored,
            "unavailable": unavailable, "withheld": withheld,
            "running_facts_ready": self.date.is_some() && self.running_facts_date == self.date,
        })
    }

    /// Called atomically with the engine/quote/pressure adoption, never from the ingest.
    pub(crate) fn adopt_tape(
        &mut self,
        symbol: &str,
        other: &Tape,
        unavailable: bool,
        date: NaiveDate,
    ) {
        self.tape.adopt(symbol, other, unavailable.then_some(date));
        // No provisional score may survive to become a later adaptive block maximum.
        self.block_max.remove(symbol);
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
            self.fast_apply(t, counted);
        }
    }

    /// One print into its symbol's fast-move tape ([`fast`]). O(1): two map lookups, a ring
    /// slot, and a queue push on the symbol's first print of a second.
    fn fast_apply(&mut self, t: &market_events::Trade, counted: crate::quotes::Counted) {
        if !self.policy.fast || (!counted.ohlc && !counted.volume) {
            return;
        }
        let Some(session) = t.session else { return };
        if session.kind == market_events::session::SessionKind::Overnight {
            return;
        }
        let price = t.price.to_f64();
        if !self.fast.contains_key(t.symbol.as_str()) {
            if !(FAST_WIDE_BAND.0..=FAST_WIDE_BAND.1).contains(&price) {
                return;
            }
            let name: Arc<str> = Arc::from(t.symbol.as_str());
            self.fast.insert(
                name.clone(),
                FastSym {
                    name,
                    tape: fast::FastTape::default(),
                    printed: i64::MIN,
                    read: i64::MIN,
                    fired_ns: None,
                },
            );
        }
        let Some(s) = self.fast.get_mut(t.symbol.as_str()) else {
            return;
        };
        let ts = t.event_ts.0;
        s.tape
            .apply(ts, price, t.size, counted.ohlc, counted.volume);
        let sec = ts.div_euclid(SEC_NS);
        if sec > s.printed {
            s.printed = sec;
            self.fast_touched.push_back((sec, s.name.clone()));
            if self.fast_touched.len() > FAST_TOUCHED_MAX {
                self.fast_touched.pop_front();
            }
        }
    }

    /// A print of a symbol whose history is not loaded yet (`crate::history::hold_unloaded`):
    /// precursor/running scoring waits for adoption; the independent fast path needs no history.
    pub fn apply_unrestored_trade(
        &mut self,
        t: &market_events::Trade,
        counted: crate::quotes::Counted,
    ) {
        if self.enabled {
            self.tape.apply_unrestored_trade(t, counted);
            if let Some(b) = self.burst.as_mut() {
                b.apply_trade_unrestored(t, counted);
            }
            // Provisional like the rest; the fast tape needs no history (module docs, [`fast`]).
            self.fast_apply(t, counted);
        }
    }

    /// A trading halt started (`true`) or ended for `symbol` (the chart engine's `chart.halt`).
    pub fn set_halted(&mut self, symbol: &str, halted: bool, resumed_at: Option<i64>) {
        if !self.enabled {
            return;
        }
        let changed = if halted {
            self.halted.insert(symbol.to_string())
        } else {
            self.halted.remove(symbol)
        };
        if let Some(at) = resumed_at {
            self.resumed_at
                .entry(symbol.to_string())
                .and_modify(|t| *t = (*t).max(at))
                .or_insert(at);
        }
        if changed && self.alerts.iter().any(|a| &*a.symbol == symbol) {
            self.revision += 1;
        }
    }

    /// Whether a trading halt is in force for `symbol` (any symbol, not only flagged ones).
    pub fn is_halted(&self, symbol: &str) -> bool {
        self.halted.contains(symbol)
    }

    /// `symbol`'s Runners context today for the Favorites panel (crate::favorites): its first
    /// entry (path and time, ms), its latest, how many paths fired, and the latest row's shown
    /// momentum state at `last`. `None` when no Runners path flagged it today.
    pub fn symbol_context(&self, symbol: &str, last: Option<f64>) -> Option<Value> {
        let mut mine = self.alerts.iter().filter(|a| &*a.symbol == symbol);
        let first = mine.next()?;
        let (mut earliest, mut latest, mut entries) = (first, first, 1 + first.also.len());
        for a in mine {
            entries += 1 + a.also.len();
            if a.at_ns < earliest.at_ns {
                earliest = a;
            }
            if a.at_ns > latest.at_ns {
                latest = a;
            }
        }
        let live = last.filter(|p| p.is_finite() && *p > 0.0);
        let (state, _) = latest.track.shown(live, self.halted.contains(symbol));
        Some(json!({
            "kind": earliest.kind.as_str(),
            "at": earliest.at_ns / 1_000_000,
            "latest_kind": latest.kind.as_str(),
            "latest_at": latest.at_ns / 1_000_000,
            "entries": entries,
            "state": state.as_str(),
        }))
    }

    fn admission_ready(&self, symbol: &str, b: &Barrier) -> bool {
        !self.halted.contains(symbol)
            && self
                .resumed_at
                .get(symbol)
                .is_none_or(|resume| b.at_ns - BAR_NS >= *resume)
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
                // Every source's rows that day: the owner's row budget across the four paths.
                "rows": self.rows_by_kind(),
                "ideal_threshold": ideal.and_then(finite),
                "strategy": self.scorer.id(), "version": self.scorer.version(),
                "admission_gate": "restored-facts-halt-v1", "build": crate::COMMIT,
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

    /// Today's rows by source: `{runner, burst, running, run, fast}`.
    fn rows_by_kind(&self) -> Value {
        let n = |k: AlertKind| self.alerts.iter().filter(|a| a.kind == k).count();
        json!({
            "runner": n(AlertKind::Runner), "burst": n(AlertKind::Burst),
            "running": n(AlertKind::Running), "run": n(AlertKind::Run),
            "fast": n(AlertKind::Fast),
        })
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
        // Official open halts can cross dates. Only the pressure board's matching resume ends one.
        self.block = None;
        self.block_max.clear();
        self.top.clear();
        self.day_written = false;
        self.threshold = f64::INFINITY;
        // The 1 s tapes keep 64 s: nothing worth carrying into a new day, and the map would
        // otherwise keep every symbol that ever printed.
        self.fast.clear();
        self.fast_touched.clear();
        self.fast_read = i64::MIN;
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
        // After the barrier, so an R2 flag or a running entry at the run's own slot keeps its row.
        self.run_entries();
        self.shelf_notices();
        // Last: the other four paths keep their row when they fire at the same tick.
        if self.policy.fast {
            self.fast_tick(now);
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
                also: Vec::new(),
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
                exec_ns: None,
                detail: None,
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
            if s.unrestored || s.unavailable {
                continue;
            }
            if self.running_facts_date == self.date
                && let Some(x) = features::run_inputs(s, b)
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
            if !self.admission_ready(&f.symbol, b) {
                continue;
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
                also: Vec::new(),
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
                exec_ns: None,
                detail: None,
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
            if !self.admission_ready(&symbol, b) {
                continue;
            }
            // A stock with a row: new evidence on that row, not a second one.
            let reasons = e.reasons();
            let mv = (Some(round2(e.gain * 100.0)), None);
            if self.follow_up(
                &symbol,
                AlertKind::Running,
                b.at_ns,
                e.price,
                &reasons,
                mv,
                None,
            ) {
                continue;
            }
            let today = self
                .alerts
                .iter()
                .filter(|a| a.kind == AlertKind::Running)
                .count();
            if today >= running::MAX_PER_DAY || self.alerts.len() >= 2 * MAX_ALERTS_PER_DAY {
                break;
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
                also: Vec::new(),
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
                exec_ns: None,
                detail: None,
            });
            self.alerts.push(a);
            self.revision += 1;
        }
    }

    /// New "Run +20%" rows ([`run`]) from the runs handed over ([`RocketBoard::add_runs`]): in the
    /// order they were decided, none for a symbol with a row today, at most [`run::MAX_PER_DAY`].
    /// A run from another day is dropped.
    fn run_entries(&mut self) {
        let Some(date) = self.date else {
            self.runs.clear();
            return;
        };
        for (symbol, r) in std::mem::take(&mut self.runs) {
            let slot = (r.plus20_ns - self.t0).div_euclid(BAR_NS) - 1;
            if !(0..NSLOT).contains(&slot) {
                continue; // another day's
            }
            // A stock with a row: new evidence on that row (the early fire and its 15 s
            // confirmation are one run: the second finds the path already on the row).
            let reasons = run::reasons(&r);
            if self.follow_up(
                &symbol,
                AlertKind::Run,
                r.plus20_ns,
                r.close,
                &reasons,
                (
                    Some(round2(run::gain(&r) * 100.0)),
                    Some((r.plus20_ns - r.start_ns) as f64 / MIN_NS as f64),
                ),
                Some(r.exec_ns),
            ) {
                continue;
            }
            let today = self
                .alerts
                .iter()
                .filter(|a| a.kind == AlertKind::Run)
                .count();
            if today >= run::MAX_PER_DAY || self.alerts.len() >= 2 * MAX_ALERTS_PER_DAY {
                break;
            }
            let a = Alert {
                id: format!("{date}-{symbol}-g{slot}"),
                symbol: symbol.clone(),
                at_ns: r.plus20_ns,
                price: r.close,
                score: run::score(&r),
                threshold: 1.0,
                reasons: run::reasons(&r),
                strategy: run::STRATEGY.to_string(),
                version: run::VERSION.to_string(),
                // The Runners strict outcome (a +30 % rocket mark) does not apply: settled from
                // the start, like a running row. Tracked from the mark slot on.
                outcome: Outcome {
                    checked: features::key_of(self.ord, slot),
                    high: f64::NAN,
                    low: f64::NAN,
                    mark: None,
                    settled: true,
                    partial: false,
                },
                track: Track::new(r.plus20_ns, r.close),
                kind: AlertKind::Run,
                burst: None,
                also: Vec::new(),
            };
            self.pending.push((date, a.run_line(date, &r)));
            self.notice(Notice {
                kind: NoticeKind::Run,
                alert_id: a.id.clone(),
                symbol: a.symbol.to_string(),
                at_ns: a.at_ns,
                price: a.price,
                from_flag_pct: Some(round2(run::gain(&r) * 100.0)),
                reasons: a.reasons.clone(),
                since_start_min: Some((r.plus20_ns - r.start_ns) as f64 / MIN_NS as f64),
                exec_ns: Some(r.exec_ns),
                detail: None,
            });
            self.alerts.push(a);
            self.revision += 1;
        }
    }

    /// The fast-move rule ([`fast::decide`]) for the seconds that ended since the last tick (at
    /// most the last 3), each read only for the symbols that printed in the 2 s before it, then
    /// their rows. `now` is the board's clock (the market clock, or the newest print when the
    /// clock is behind the feed).
    fn fast_tick(&mut self, now: i64) {
        let Some(date) = self.date else { return };
        let end = now.div_euclid(SEC_NS) - 1;
        if end < self.fast_read.saturating_sub(5) {
            // The clock went back (a replay restarted): read from now on.
            self.fast_read = i64::MIN;
        }
        let from = self.fast_read.saturating_add(1).max(end - 2);
        let (day0, day1) = (self.t0, self.t0 + NSLOT * BAR_NS);
        let mut fires: Vec<(Arc<str>, i64, fast::FastEntry)> = Vec::new();
        for sec in from..=end {
            while self.fast_touched.front().is_some_and(|x| x.0 < sec - 2) {
                self.fast_touched.pop_front();
            }
            if !(day0..day1).contains(&(sec * SEC_NS)) {
                continue;
            }
            for (_, name) in &self.fast_touched {
                let Some(s) = self.fast.get_mut(name) else {
                    continue;
                };
                if s.read >= sec || sec - s.printed > 2 {
                    continue;
                }
                s.read = sec;
                let Some(e) = s.tape.inputs_settled(sec).and_then(|x| fast::decide(&x)) else {
                    continue;
                };
                let at = sec * SEC_NS;
                if s.fired_ns.is_some_and(|f| at - f < fast::REFRACTORY_NS) {
                    continue;
                }
                s.fired_ns = Some(at);
                fires.push((s.name.clone(), sec, e));
            }
        }
        self.fast_read = self.fast_read.max(end);
        self.fast_entries(date, fires);
    }

    /// New "Fast move" rows ([`fast`]): in the order they fired, none for a symbol with a row
    /// today, at most [`fast::MAX_PER_DAY`].
    fn fast_entries(&mut self, date: NaiveDate, fires: Vec<(Arc<str>, i64, fast::FastEntry)>) {
        for (symbol, sec, e) in fires {
            // The row is at the end of the second that fired it.
            let at_ns = (sec + 1) * SEC_NS;
            // A stock with a row: new evidence on that row, not a second one.
            let reasons = e.reasons();
            if self.follow_up(
                &symbol,
                AlertKind::Fast,
                at_ns,
                e.price,
                &reasons,
                (Some(round2(e.gain * 100.0)), Some(e.secs as f64 / 60.0)),
                Some(sec * SEC_NS),
            ) {
                continue;
            }
            let today = self
                .alerts
                .iter()
                .filter(|a| a.kind == AlertKind::Fast)
                .count();
            if today >= fast::MAX_PER_DAY || self.alerts.len() >= 2 * MAX_ALERTS_PER_DAY {
                break;
            }
            let slot = ((at_ns - self.t0).div_euclid(BAR_NS) - 1).clamp(0, NSLOT - 1);
            let need = fast::ARMS
                .iter()
                .find(|a| a.secs == e.secs)
                .map_or(e.gain, |a| a.gain);
            let a = Alert {
                id: format!("{date}-{symbol}-f{sec}"),
                symbol: symbol.clone(),
                at_ns,
                price: e.price,
                // The held move over the arm's requirement (>= 1).
                score: e.gain / need,
                threshold: 1.0,
                reasons: e.reasons(),
                strategy: fast::STRATEGY.to_string(),
                version: fast::VERSION.to_string(),
                // The Runners strict outcome (a +30 % rocket mark) does not apply: settled from
                // the start, like a run row.
                outcome: Outcome {
                    checked: features::key_of(self.ord, slot),
                    high: f64::NAN,
                    low: f64::NAN,
                    mark: None,
                    settled: true,
                    partial: false,
                },
                track: Track::new(at_ns, e.price),
                kind: AlertKind::Fast,
                burst: None,
                also: Vec::new(),
            };
            self.pending.push((date, a.fast_line(date, sec, &e)));
            self.notice(Notice {
                kind: NoticeKind::Fast,
                alert_id: a.id.clone(),
                symbol: a.symbol.to_string(),
                at_ns,
                price: a.price,
                from_flag_pct: Some(round2(e.gain * 100.0)),
                reasons: a.reasons.clone(),
                since_start_min: Some(e.secs as f64 / 60.0),
                // The second it fired: the start of the push's latency log.
                exec_ns: Some(sec * SEC_NS),
                detail: None,
            });
            self.alerts.push(a);
            self.revision += 1;
        }
    }

    /// The shelf entries handed over ([`RocketBoard::add_shelf_entries`]) as notices: one per
    /// entry (the shelf module makes at most one entry per shelf), only for a stock with a row
    /// today, none from another day. Held / lost outcomes are never handed over.
    fn shelf_notices(&mut self) {
        let shelves = std::mem::take(&mut self.shelves);
        if self.date.is_none() {
            return;
        }
        let day = (self.t0, self.t0 + NSLOT * BAR_NS);
        for (symbol, e) in shelves {
            if !(day.0..=day.1).contains(&e.t_ns) {
                continue;
            }
            let Some(a) = self.alerts.iter().rev().find(|a| a.symbol == symbol) else {
                continue;
            };
            let n = Notice {
                kind: NoticeKind::Shelf,
                alert_id: format!("{}-z{}", a.id, e.t_ns / SEC_NS),
                symbol: symbol.to_string(),
                at_ns: e.t_ns,
                price: e.entry,
                from_flag_pct: a.track.pct(e.entry).map(round2),
                reasons: Vec::new(),
                since_start_min: None,
                exec_ns: None,
                detail: Some(NoticeDetail::Shelf { lo: e.lo, hi: e.hi }),
            };
            self.notice(n);
        }
    }

    /// A path firing on a stock that already has a row today (module docs, "Follow-ups"). True
    /// when the stock has a row: the caller then adds none. The first time this path fires on
    /// that row it is recorded there ([`Alert::also`], the secondary chip), logged (an `also`
    /// line, so a restart keeps the chip) and leaves a [`NoticeKind::FollowUp`] notice; the
    /// row's own path, or a path already on it, adds nothing.
    #[allow(clippy::too_many_arguments)]
    fn follow_up(
        &mut self,
        symbol: &str,
        path: AlertKind,
        at_ns: i64,
        price: f64,
        reasons: &[String],
        // The path's own move (% and minutes), as its own row's notice carries it.
        mv: (Option<f64>, Option<f64>),
        exec_ns: Option<i64>,
    ) -> bool {
        // The newest row (a stock can have an R2 flag and burst rows).
        let Some(i) = self.alerts.iter().rposition(|a| &*a.symbol == symbol) else {
            return false;
        };
        let Some(date) = self.date else { return true };
        let a = &mut self.alerts[i];
        if a.kind == path || a.also.iter().any(|x| x.kind == path) {
            return true;
        }
        // "after" names the latest evidence before this one: the last follow-up, else the row.
        let (first, first_at_ns) = a
            .also
            .last()
            .map_or((a.kind, a.at_ns), |x| (x.kind, x.at_ns));
        a.also.push(Also {
            kind: path,
            at_ns,
            price,
        });
        let line = json!({
            "kind": "also", "v": 1, "id": a.id, "date": date.to_string(),
            "symbol": &*a.symbol, "path": path.as_str(), "at_ns": at_ns, "price": price,
            "reasons": reasons,
        })
        .to_string();
        let n = Notice {
            kind: NoticeKind::FollowUp,
            alert_id: format!("{}-{}", a.id, path.as_str()),
            symbol: symbol.to_string(),
            at_ns,
            price,
            from_flag_pct: mv.0,
            reasons: reasons.to_vec(),
            since_start_min: mv.1,
            exec_ns,
            detail: Some(NoticeDetail::FollowUp {
                path,
                first,
                first_at_ns,
            }),
        };
        self.pending.push((date, line));
        self.notice(n);
        self.revision += 1;
        true
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
                        exec_ns: None,
                        detail: None,
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
        let run_rows = loaded.runs.into_iter().map(|v| (AlertKind::Run, v, None));
        let fast_rows = loaded.fasts.into_iter().map(|v| (AlertKind::Fast, v, None));
        for (kind, v, o) in today_rows
            .chain(burst_rows)
            .chain(running_rows)
            .chain(run_rows)
            .chain(fast_rows)
        {
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
                also: Vec::new(),
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
            if matches!(kind, AlertKind::Running | AlertKind::Run | AlertKind::Fast) {
                // Its strict outcome never applies (`running_entries`, `run_entries`,
                // `fast_entries`).
                a.kind = kind;
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
        // The rows' follow-ups (`also` lines): their chips come back, and the same path firing
        // again after the restart finds itself on the row and announces nothing.
        for a in &mut self.alerts {
            for v in loaded.also.get(&a.id).into_iter().flatten() {
                let (Some(kind), Some(at_ns)) = (
                    v.get("path")
                        .and_then(Value::as_str)
                        .and_then(AlertKind::parse),
                    v.get("at_ns").and_then(Value::as_i64),
                ) else {
                    continue;
                };
                if kind != a.kind && !a.also.iter().any(|x| x.kind == kind) {
                    a.also.push(Also {
                        kind,
                        at_ns,
                        price: v.get("price").and_then(Value::as_f64).unwrap_or(f64::NAN),
                    });
                }
            }
        }
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
        // Each row's shown state once; the panel keeps the most relevant ROWS (`row_rank`),
        // still sent newest first.
        let mut shown: Vec<(usize, Quote, MState, bool)> = self
            .alerts
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let q = quote(&a.symbol);
                let live = q.0.filter(|p| p.is_finite() && *p > 0.0);
                let (state, provisional) = a.track.shown(live, self.halted.contains(&*a.symbol));
                (i, q, state, provisional)
            })
            .collect();
        let newest = |x: &(usize, Quote, MState, bool), y: &(usize, Quote, MState, bool)| {
            self.alerts[y.0]
                .at_ns
                .cmp(&self.alerts[x.0].at_ns)
                .then(y.0.cmp(&x.0))
        };
        if shown.len() > ROWS {
            shown.sort_by(|x, y| row_rank(x.2).cmp(&row_rank(y.2)).then(newest(x, y)));
            shown.truncate(ROWS);
        }
        shown.sort_by(newest);
        let rows: Vec<Value> = shown
            .into_iter()
            .map(|(i, (last, pc), state, provisional)| {
                let a = &self.alerts[i];
                let t = &a.track;
                let live = last.filter(|p| p.is_finite() && *p > 0.0);
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
                    "also": a.also.iter().map(|x| json!({
                        "kind": x.kind.as_str(), "at": x.at_ns / 1_000_000,
                    })).collect::<Vec<_>>(),
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
            // Likewise a row without follow-ups carries no `also`.
            if let Some(m) = r.as_object_mut()
                && m.get("also")
                    .and_then(Value::as_array)
                    .is_some_and(Vec::is_empty)
            {
                m.remove("also");
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
