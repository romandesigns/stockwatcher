//! EXPERIMENTAL Detection Panel families (`policy/detection-v0.json`): quiet context, momentum
//! building / upward acceleration, the 1.15× range and body diagnostics, the primary experimental
//! ignition, range breakout, and the micro-pullback. Design, units and every choice marked as
//! ours: `docs/stock-context/detection-core.md`.
//!
//! The detectors read the engine's own completed 15 s bars and admitted prints — the same ones the
//! pillars use, whether or not a chart is open and whatever Ross membership says — and append
//! immutable transitions with deterministic ids. Nothing here is a trading signal, a probability
//! or a validated edge; audio and push stay off.
//!
//! # Timing
//!
//! A barrier `b` is decided at `tau = b + settle` under the engine's actual admission rule
//! (`exec < b` and admitted before `tau`; `tests/execution_boundary.rs`). Feature snapshots are
//! taken at that barrier and frozen into the transitions that cite them: a later print,
//! correction or reference can change current state, never a logged record. Trade-driven events
//! (micro-pullback crossings and invalidations) are evaluated as each print is admitted, against
//! the levels already armed before it, and carry the admission time as `observed_at`.

pub(crate) mod families;
pub mod features;
pub mod fixture;
pub mod math;
pub(crate) mod micro;
pub mod policy;

pub use features::{
    BaselineStats, BodyStats, FeatureSnapshot, RangeRelation, RangeStats, ReturnClass, WindowStats,
};
pub use policy::{DETECTION_V0, DetectionPolicy};

use crate::Fidelity;
use crate::evidence::InstrumentId;
use crate::output::BoundedLog;
use crate::pillar::Tri;
use crate::setup::Bar;
use chrono::NaiveDate;
use market_events::TradeId;
use market_events::session::{SessionId, SessionKind};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

pub(crate) const SEC: i64 = 1_000_000_000;

/// Bound on the recorded data-gap intervals (global and per symbol together).
pub const GAPS_KEPT: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    MomentumBuilding,
    UpwardAcceleration,
    Ignition,
    IgnitionGapEvent,
    RangeBreakout,
    MicroPullback,
}

impl Family {
    pub const ALL: [Family; 6] = [
        Family::MomentumBuilding,
        Family::UpwardAcceleration,
        Family::Ignition,
        Family::IgnitionGapEvent,
        Family::RangeBreakout,
        Family::MicroPullback,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Family::MomentumBuilding => "momentum_building",
            Family::UpwardAcceleration => "upward_acceleration",
            Family::Ignition => "ignition",
            Family::IgnitionGapEvent => "ignition_gap_event",
            Family::RangeBreakout => "range_breakout",
            Family::MicroPullback => "micro_pullback",
        }
    }

    fn index(self) -> usize {
        self as usize
    }

    /// Whether the family's episodes are default rows (the others are details-only).
    pub fn default_visible(self) -> bool {
        !matches!(self, Family::UpwardAcceleration | Family::IgnitionGapEvent)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Building,
    Ignited,
    Faded,
    Recognized,
    Observed,
    Completed,
    Confirmed,
    Retained,
    Rejected,
    PullbackForming,
    Armed,
    CrossingProvisional,
    TriggerObserved,
    Invalidated,
    Expired,
    Superseded,
    Interrupted,
}

impl State {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            State::Ignited
                | State::Faded
                | State::Completed
                | State::Retained
                | State::Rejected
                | State::TriggerObserved
                | State::Invalidated
                | State::Expired
                | State::Superseded
                | State::Interrupted
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Certainty {
    Provisional,
    Confirmed,
    Retracted,
    Terminal,
    /// A later revision of evidence an earlier record cited; the earlier record stands.
    Correction,
}

/// Past-only context of a prior window, precedence: insufficient → sparse → active
/// consolidation → already moving. `unavailable`: data were sufficient but no admissible quiet
/// reference exists, so quietness is not known (never assumed quiet or moving).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextClass {
    Insufficient,
    Sparse,
    ActiveConsolidation,
    AlreadyMoving,
    Unavailable,
}

/// A symbol's own past same-segment quietness percentiles, supplied by a loader that computed
/// them from **prior sessions only** (ignition-v0 proposal §4). The engine never derives them;
/// absent, quietness is `unavailable`. Read as of each decision cutoff via `observed_at`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuietReference {
    pub id: String,
    pub version: u32,
    /// The display symbol (`InstrumentId::symbol`).
    pub symbol: String,
    /// The trading date the reference applies to (computed from sessions before it).
    pub trading_date: NaiveDate,
    pub segment: SessionKind,
    /// Range `Q` and drift `D` percentile thresholds, ppm of price / log-ppm.
    pub q_low_ppm: i64,
    pub d_low_ppm: i64,
    pub percentile: u32,
    pub prior_sessions: u32,
    /// The last prior session the percentiles were computed from; must be before
    /// `trading_date` (no in-progress or future session can label quietness).
    pub prior_through: NaiveDate,
    /// Price basis of the prior sessions (e.g. `split_adjusted`), recorded with the reference.
    pub basis: String,
    /// Fewer than the proposal's 10 prior sessions: a pooled fallback, flagged.
    pub short_history: bool,
    /// A fixture's reference. Recorded on every feature that used it; never a real one.
    pub synthetic: bool,
    pub provenance: String,
    pub observed_at: i64,
}

/// One rule applied, three-valued.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Check {
    pub name: &'static str,
    pub state: Tri,
    pub value: Option<i64>,
    pub threshold: Option<i64>,
    pub units: &'static str,
    pub reason: &'static str,
}

impl Check {
    pub(crate) fn new(
        name: &'static str,
        state: Tri,
        value: Option<i64>,
        threshold: Option<i64>,
        units: &'static str,
        reason: &'static str,
    ) -> Check {
        Check {
            name,
            state,
            value,
            threshold,
            units,
            reason,
        }
    }

    pub(crate) fn tri(ok: Option<bool>) -> Tri {
        match ok {
            Some(true) => Tri::Pass,
            Some(false) => Tri::Fail,
            None => Tri::Unknown,
        }
    }
}

/// One version of a level. A revision is a new version, never an edit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LevelVersion {
    /// `<episode_id>:<kind>`.
    pub level_id: String,
    /// `trigger`, `invalidation`, `range_high`, `range_low`.
    pub kind: &'static str,
    pub version: u32,
    pub price: i64,
    /// The cutoff (or admission time) at which the engine knew the level.
    pub known_at: i64,
    /// When the level became a tested trigger/invalidation (micro arming).
    pub armed_at: Option<i64>,
    /// Bar ends of the first and last bar it was formed from.
    pub formed_from: (i64, i64),
    pub reason: &'static str,
}

/// The micro-pullback's causal impulse anchors and evolving pullback measurements.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MicroAnchors {
    pub l0: i64,
    pub l0_bar_end: i64,
    pub h0: i64,
    pub h0_bar_end: i64,
    /// `(H0 − L0) / L0`, bp.
    pub impulse_bp: i64,
    pub impulse_slots: i64,
    pub impulse_coverage_milli: i64,
    /// The cutoff the anchors were frozen at (retreat start).
    pub frozen_at: i64,
    /// Lowest pullback low since the retreat started (frozen at arming as `I`).
    pub pullback_low: i64,
    pub pullback_low_bar_end: i64,
    /// `(H0 − Lp) / (H0 − L0)`, thousandths: fraction of the impulse retraced.
    pub depth_milli: i64,
    /// `(H0 − Lp) / H0`, bp: the stock-price retreat.
    pub retreat_bp: i64,
    pub pullback_bars: u32,
    pub pullback_slots: i64,
    /// Dollar-volume rate in the pullback over the impulse, thousandths, over covered bars.
    pub dv_rate_ratio_milli: Option<i64>,
    pub armed_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TradeEvidence {
    pub trade_id: TradeId,
    pub exec: i64,
    pub admit: i64,
    pub price: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Resolution {
    pub state: State,
    pub reason: &'static str,
    pub at: i64,
    pub bar_end: Option<i64>,
    pub transition_id: String,
}

/// Where the latest admitted price stands against the episode's levels. Computed at snapshot
/// time; never sequenced, so proximity never re-orders anything.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Assessment {
    pub price: i64,
    pub price_exec: i64,
    pub price_age_ms: i64,
    pub stale: bool,
    pub trigger: Option<i64>,
    /// At or above the trigger: no "approaching" distance is shown.
    pub at_or_above_trigger: Option<bool>,
    pub distance_to_trigger_micros: Option<i64>,
    pub distance_to_trigger_bp: Option<i64>,
    pub invalidation: Option<i64>,
    pub distance_to_invalidation_micros: Option<i64>,
    pub distance_to_invalidation_bp: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Episode {
    /// `<run>:<date>:<symbol>:<family>:<n>`.
    pub episode_id: String,
    pub family: Family,
    pub variant: String,
    pub policy: String,
    pub experimental: bool,
    pub symbol: String,
    pub instrument: InstrumentId,
    pub trading_date: NaiveDate,
    pub segment: SessionId,
    pub state: State,
    pub default_visible: bool,
    pub first_detected_at: i64,
    pub first_bar_end: Option<i64>,
    pub last_material_seq: u64,
    pub last_material_at: i64,
    pub base_secs: i64,
    pub fidelity: Fidelity,
    /// Its recognition read a window reaching into a restored prefix (not this run's own live
    /// observation): never a fresh notification.
    pub warmup: bool,
    pub parent_episode_id: Option<String>,
    pub related_episode_ids: Vec<String>,
    pub context: Option<ContextClass>,
    pub levels: Vec<LevelVersion>,
    pub anchors: Option<MicroAnchors>,
    pub recognition: Option<FeatureSnapshot>,
    pub checks: Vec<Check>,
    /// Filled in snapshots only.
    pub current: Option<Assessment>,
    pub resolution: Option<Resolution>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DetectionTransition {
    /// Engine-wide detection sequence, from 1.
    pub seq: u64,
    /// `<episode_id>:t<n>`.
    pub transition_id: String,
    pub episode_id: String,
    pub symbol: String,
    pub family: Family,
    pub variant: String,
    pub from: Option<State>,
    pub to: State,
    pub event: &'static str,
    pub reason: &'static str,
    pub certainty: Certainty,
    /// Logical time: the cutoff for barrier events, the admission time for trade events.
    pub observed_at: i64,
    pub bar_end: Option<i64>,
    pub cutoff: Option<i64>,
    pub trade: Option<TradeEvidence>,
    pub features: Option<FeatureSnapshot>,
    pub level: Option<LevelVersion>,
    pub anchors: Option<MicroAnchors>,
    pub checks: Vec<Check>,
    pub correction_of: Option<String>,
    pub warmup: bool,
    pub default_visible: bool,
    pub related_episode_ids: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct DetectionTelemetry {
    pub transitions_logged: u64,
    pub transitions_taken: u64,
    pub transitions_dropped: u64,
    pub transitions_held: usize,
    pub transition_log_max: usize,
    pub episodes_opened: u64,
    pub episodes_active: usize,
    pub recent_held: usize,
    pub recent_dropped: u64,
    pub recent_max: usize,
    pub quiet_references: usize,
    pub interruptions: u64,
    pub corrections: u64,
}

impl DetectionTelemetry {
    /// Nothing dropped: taken + held is the whole transition record of the run.
    pub fn complete(&self) -> bool {
        self.transitions_dropped == 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Gap {
    /// `None`: every symbol (a link or global observation failure).
    pub symbol: Option<String>,
    pub from: i64,
    /// `None` while open.
    pub to: Option<i64>,
    pub reason: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DataStatus {
    pub global_interruption: Option<Gap>,
    pub interrupted_symbols: Vec<Gap>,
    /// Recently closed gaps, bounded ([`GAPS_KEPT`]).
    pub recent_gaps: Vec<Gap>,
    /// Detection is configured but cannot run (e.g. `bar_secs_mismatch`).
    pub disabled_reason: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SymbolDetections {
    pub symbol: String,
    pub dominant_episode_id: Option<String>,
    pub active_episode_ids: Vec<String>,
    pub interrupted: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DetectionSnapshot {
    pub schema: u32,
    pub run_id: String,
    pub source: String,
    pub fidelity: Fidelity,
    pub policy: String,
    pub experimental: bool,
    pub alerts_enabled: bool,
    /// Seq of the last transition logged: apply deltas strictly after it.
    pub cursor: u64,
    pub bar_end: Option<i64>,
    pub cutoff: Option<i64>,
    /// The latest admission time applied (the logical clock).
    pub as_of: Option<i64>,
    pub symbols: Vec<SymbolDetections>,
    pub active: Vec<Episode>,
    pub recent: Vec<Episode>,
    pub data_status: DataStatus,
    pub telemetry: DetectionTelemetry,
}

// ------------------------------------------------------------------------------------------
// Internal state
// ------------------------------------------------------------------------------------------

/// The latest admitted price-qualifying print.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Seen {
    pub exec: i64,
    pub admit: i64,
    pub price: i64,
}

#[derive(Clone, Debug)]
pub(crate) struct RetEntry {
    pub end: i64,
    pub seg: SessionId,
    pub ppm: i64,
}

#[derive(Clone, Debug)]
pub(crate) enum Extra {
    Momentum {
        range_low: i64,
    },
    Ignition,
    Breakout {
        level: i64,
        extended: bool,
        seen_after: u32,
    },
    Micro(Box<micro::Inner>),
}

#[derive(Clone, Debug)]
pub(crate) struct Ep {
    pub view: Episode,
    pub transitions: u32,
    pub extra: Extra,
}

/// One symbol's detector state (lives on the engine's per-symbol record).
#[derive(Clone, Debug, Default)]
pub(crate) struct SymDetect {
    pub date: Option<NaiveDate>,
    pub returns: VecDeque<RetEntry>,
    pub active: Vec<Ep>,
    pub counters: [u32; 6],
    /// Closed gap intervals `[from, to)` of this symbol.
    pub gaps: VecDeque<(i64, i64, &'static str)>,
    pub interrupted: Option<(i64, &'static str)>,
    pub halted_since: Option<i64>,
    pub last_seen: Option<Seen>,
    /// Ignition cooldown: last barrier the rule was true and the current episode's t0.
    pub ign_last_true: Option<i64>,
    pub ign_t0: Option<i64>,
    pub gap_last_true: Option<i64>,
    pub gap_t0: Option<i64>,
    pub mom_resolved_at: Option<i64>,
    pub brk_resolved_at: Option<i64>,
    /// A new micro impulse peak must be in a bar ending after this.
    pub micro_search_from: i64,
    /// Bars used by any new episode must start at or after this (after an interruption).
    pub requalify_from: i64,
    /// The feature snapshot of this symbol's latest completed candidate bar (details view).
    pub latest: Option<FeatureSnapshot>,
}

impl SymDetect {
    pub fn has_active(&self) -> bool {
        self.active.iter().any(|e| !e.view.state.is_terminal())
    }
}

/// The engine-wide detector: policy, sequence, bounded logs, references, global status.
pub(crate) struct Detector {
    /// Shared so a step can hold its rules while it mutates the detector (no per-call clone).
    pub policy: std::sync::Arc<DetectionPolicy>,
    pub seq: u64,
    pub log: BoundedLog<DetectionTransition>,
    pub recent: VecDeque<Episode>,
    pub recent_dropped: u64,
    pub episodes_opened: u64,
    /// By symbol (borrowed `&str` lookups, no per-call allocation).
    pub refs: HashMap<String, Vec<QuietReference>>,
    pub refs_held: usize,
    pub global: Option<(i64, &'static str)>,
    pub gaps: VecDeque<Gap>,
    pub last_barrier: Option<(i64, i64)>,
    pub as_of: Option<i64>,
    pub interruptions: u64,
    pub corrections: u64,
    pub disabled: Option<&'static str>,
    /// Terminal episodes whose outcome cites one print `(id, exec, transition)`, bounded, so a
    /// later revision of that print appends a correction notice.
    pub cited: VecDeque<(TradeId, i64, String, Ep)>,
}

impl Detector {
    pub fn new(policy: DetectionPolicy, grid_bar_secs: i64, grid_settle_ms: i64) -> Detector {
        let disabled = (policy.body.timing.bar_secs != grid_bar_secs
            || policy.body.timing.settle_ms != grid_settle_ms)
            .then_some("grid_mismatch");
        Detector {
            log: BoundedLog::new(policy.body.lifecycle.transition_log_max),
            policy: std::sync::Arc::new(policy),
            seq: 0,
            recent: VecDeque::new(),
            recent_dropped: 0,
            episodes_opened: 0,
            refs: HashMap::new(),
            refs_held: 0,
            global: None,
            gaps: VecDeque::new(),
            last_barrier: None,
            as_of: None,
            interruptions: 0,
            corrections: 0,
            disabled,
            cited: VecDeque::new(),
        }
    }

    pub fn add_reference(&mut self, r: QuietReference) {
        let v = self.refs.entry(r.symbol.clone()).or_default();
        if v.iter().any(|x| x.id == r.id && x.version == r.version) {
            return;
        }
        v.push(r);
        v.sort_by_key(|x| (x.observed_at, x.version));
        self.refs_held += 1;
    }

    /// The latest reference observed at or before `tau`.
    pub fn reference(&self, symbol: &str, seg: SessionId, tau: i64) -> Option<&QuietReference> {
        self.refs
            .get(symbol)?
            .iter()
            .rev()
            .find(|r| r.trading_date == seg.date && r.segment == seg.kind && r.observed_at <= tau)
    }

    /// Drops references for dates before `date` (bounded memory over a long run).
    pub fn prune_references(&mut self, date: NaiveDate) {
        for v in self.refs.values_mut() {
            v.retain(|r| r.trading_date >= date);
        }
        self.refs.retain(|_, v| !v.is_empty());
        self.refs_held = self.refs.values().map(|v| v.len()).sum();
    }

    fn push_gap(&mut self, g: Gap) {
        while self.gaps.len() >= GAPS_KEPT {
            self.gaps.pop_front();
        }
        self.gaps.push_back(g);
    }

    /// Whether `[from, to)` overlaps a recorded or open data gap for `symbol`.
    pub fn gap_overlaps(&self, sd: &SymDetect, from: i64, to: i64) -> bool {
        let hit = |a: i64, b: i64| a < to && from < b;
        self.global.is_some_and(|(s, _)| s < to)
            || sd.interrupted.is_some_and(|(s, _)| s < to)
            || sd.halted_since.is_some_and(|s| s < to)
            || sd.gaps.iter().any(|&(a, b, _)| hit(a, b))
            || self
                .gaps
                .iter()
                .filter(|g| g.symbol.is_none())
                .any(|g| hit(g.from, g.to.unwrap_or(i64::MAX)))
    }

    pub fn telemetry(&self, active: usize) -> DetectionTelemetry {
        DetectionTelemetry {
            transitions_logged: self.log.logged(),
            transitions_taken: self.log.taken(),
            transitions_dropped: self.log.dropped(),
            transitions_held: self.log.held(),
            transition_log_max: self.log.max(),
            episodes_opened: self.episodes_opened,
            episodes_active: active,
            recent_held: self.recent.len(),
            recent_dropped: self.recent_dropped,
            recent_max: self.policy.body.lifecycle.recent_max,
            quiet_references: self.refs_held,
            interruptions: self.interruptions,
            corrections: self.corrections,
        }
    }

    pub fn retire(&mut self, ep: Episode) {
        while self.recent.len() >= self.policy.body.lifecycle.recent_max {
            self.recent.pop_front();
            self.recent_dropped += 1;
        }
        self.recent.push_back(ep);
    }
}

/// A session segment and its UTC bounds, resolved once per barrier step (the calendar's
/// time-zone conversion is far too slow to repeat for every slot of every window).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Seg {
    pub id: SessionId,
    pub start: i64,
    pub end: i64,
}

impl Seg {
    pub fn of(id: SessionId) -> Option<Seg> {
        let (s, e) = id.bounds()?;
        Some(Seg {
            id,
            start: s.nanos(),
            end: e.nanos(),
        })
    }

    /// Whether the slot `[end − bar, end)` lies inside the segment.
    pub fn holds(&self, end: i64, bar_ns: i64) -> bool {
        end - bar_ns >= self.start && end <= self.end
    }
}

/// Everything about one symbol a barrier step reads.
pub(crate) struct Barrier<'a> {
    pub b: i64,
    pub tau: i64,
    pub bar_ns: i64,
    pub run_id: &'a str,
    pub fidelity: Fidelity,
    pub symbol: &'a str,
    pub inst: &'a InstrumentId,
    pub date: NaiveDate,
    pub bars: &'a VecDeque<Bar>,
    pub halted: bool,
    pub coverage_partial: bool,
    pub untrusted: bool,
    pub restored_through: Option<i64>,
}

/// What a new transition says, beyond the episode it belongs to.
pub(crate) struct Change {
    pub to: State,
    pub event: &'static str,
    pub reason: &'static str,
    pub certainty: Certainty,
    pub observed_at: i64,
    pub bar_end: Option<i64>,
    pub cutoff: Option<i64>,
    pub trade: Option<TradeEvidence>,
    pub features: Option<FeatureSnapshot>,
    pub level: Option<LevelVersion>,
    pub checks: Vec<Check>,
    pub correction_of: Option<String>,
}

impl Change {
    pub fn at_barrier(
        to: State,
        event: &'static str,
        reason: &'static str,
        certainty: Certainty,
        b: i64,
        tau: i64,
    ) -> Change {
        Change {
            to,
            event,
            reason,
            certainty,
            observed_at: tau,
            bar_end: Some(b),
            cutoff: Some(tau),
            trade: None,
            features: None,
            level: None,
            checks: Vec::new(),
            correction_of: None,
        }
    }

    pub fn at(
        to: State,
        event: &'static str,
        reason: &'static str,
        certainty: Certainty,
        observed_at: i64,
    ) -> Change {
        Change {
            to,
            event,
            reason,
            certainty,
            observed_at,
            bar_end: None,
            cutoff: None,
            trade: None,
            features: None,
            level: None,
            checks: Vec::new(),
            correction_of: None,
        }
    }

    pub fn features(mut self, f: Option<FeatureSnapshot>) -> Change {
        self.features = f;
        self
    }
    pub fn level(mut self, l: Option<LevelVersion>) -> Change {
        self.level = l;
        self
    }
    pub fn checks(mut self, c: Vec<Check>) -> Change {
        self.checks = c;
        self
    }
    pub fn trade(mut self, t: Option<TradeEvidence>) -> Change {
        self.trade = t;
        self
    }
    pub fn correction_of(mut self, id: Option<String>) -> Change {
        self.correction_of = id;
        self
    }
}

/// Appends one transition for `ep` and moves it to `c.to`. Material by definition: only state
/// changes and corrections are ever logged, never a price or proximity update.
pub(crate) fn emit(det: &mut Detector, ep: &mut Ep, c: Change) -> String {
    det.seq += 1;
    ep.transitions += 1;
    let id = format!("{}:t{}", ep.view.episode_id, ep.transitions);
    let from = (ep.transitions > 1).then_some(ep.view.state);
    let anchors = ep.view.anchors.clone();
    if c.to.is_terminal() && c.certainty != Certainty::Correction {
        ep.view.resolution = Some(Resolution {
            state: c.to,
            reason: c.reason,
            at: c.observed_at,
            bar_end: c.bar_end,
            transition_id: id.clone(),
        });
    }
    if c.certainty == Certainty::Correction {
        det.corrections += 1;
    }
    det.log.push(DetectionTransition {
        seq: det.seq,
        transition_id: id.clone(),
        episode_id: ep.view.episode_id.clone(),
        symbol: ep.view.symbol.clone(),
        family: ep.view.family,
        variant: ep.view.variant.clone(),
        from,
        to: c.to,
        event: c.event,
        reason: c.reason,
        certainty: c.certainty,
        observed_at: c.observed_at,
        bar_end: c.bar_end,
        cutoff: c.cutoff,
        trade: c.trade,
        features: c.features,
        level: c.level,
        anchors,
        checks: c.checks,
        correction_of: c.correction_of,
        warmup: ep.view.warmup,
        default_visible: ep.view.default_visible,
        related_episode_ids: ep.view.related_episode_ids.clone(),
    });
    ep.view.state = c.to;
    ep.view.last_material_seq = det.seq;
    ep.view.last_material_at = c.observed_at;
    id
}

/// Creates an episode (not yet logged: the caller emits its first transition).
#[allow(clippy::too_many_arguments)]
pub(crate) fn open(
    det: &mut Detector,
    sd: &mut SymDetect,
    x: &Barrier,
    family: Family,
    variant: &str,
    seg: Seg,
    first_bar_end: Option<i64>,
    warmup: bool,
    extra: Extra,
) -> Ep {
    sd.counters[family.index()] += 1;
    det.episodes_opened += 1;
    let n = sd.counters[family.index()];
    Ep {
        view: Episode {
            episode_id: format!(
                "{}:{}:{}:{}:{}",
                x.run_id,
                x.date,
                x.symbol,
                family.as_str(),
                n
            ),
            family,
            variant: variant.to_string(),
            policy: det.policy.tag(),
            experimental: det.policy.body.experimental,
            symbol: x.symbol.to_string(),
            instrument: x.inst.clone(),
            trading_date: x.date,
            segment: seg.id,
            state: State::Observed,
            default_visible: family.default_visible(),
            first_detected_at: x.tau,
            first_bar_end,
            last_material_seq: 0,
            last_material_at: x.tau,
            base_secs: x.bar_ns / SEC,
            fidelity: x.fidelity,
            warmup,
            parent_episode_id: None,
            related_episode_ids: Vec::new(),
            context: None,
            levels: Vec::new(),
            anchors: None,
            recognition: None,
            checks: Vec::new(),
            current: None,
            resolution: None,
        },
        transitions: 0,
        extra,
    }
}

/// Moves every terminal episode of a symbol to the bounded recent history.
pub(crate) fn retire_terminal(det: &mut Detector, sd: &mut SymDetect) {
    let mut keep = Vec::with_capacity(sd.active.len());
    for ep in sd.active.drain(..) {
        if ep.view.state.is_terminal() {
            det.retire(ep.view);
        } else {
            keep.push(ep);
        }
    }
    sd.active = keep;
}

/// Interrupts every active episode of a symbol (halt, gap, coverage, segment end).
pub(crate) fn interrupt_all(
    det: &mut Detector,
    sd: &mut SymDetect,
    reason: &'static str,
    at: i64,
    barrier: Option<(i64, i64)>,
) {
    let mut any = false;
    for ep in sd.active.iter_mut() {
        if ep.view.state.is_terminal() {
            continue;
        }
        let c = match barrier {
            Some((b, tau)) => Change::at_barrier(
                State::Interrupted,
                "interrupted",
                reason,
                Certainty::Terminal,
                b,
                tau,
            ),
            None => Change::at(
                State::Interrupted,
                "interrupted",
                reason,
                Certainty::Terminal,
                at,
            ),
        };
        emit(det, ep, c);
        if let Some(b) = ep.view.first_bar_end {
            sd.micro_search_from = sd.micro_search_from.max(b);
        }
        any = true;
    }
    if any {
        det.interruptions += 1;
    }
    retire_terminal(det, sd);
    // A real observation failure: new episodes and baselines restart after it. A segment end
    // is not one (the next segment's windows never reach back anyway).
    if reason != "segment_end" {
        sd.requalify_from = sd.requalify_from.max(at);
    }
}

/// The barrier step for one symbol: data status, old levels first, then new detections.
pub(crate) fn step(det: &mut Detector, sd: &mut SymDetect, x: &Barrier) {
    if det.disabled.is_some() {
        return;
    }
    det.last_barrier = Some((x.b, x.tau));
    if sd.date != Some(x.date) {
        // A new trading date: its own ids, cooldowns and requalification. Episodes of the old
        // date end at the segment check below.
        let carry = std::mem::take(&mut sd.active);
        let returns = std::mem::take(&mut sd.returns);
        let last_seen = sd.last_seen;
        *sd = SymDetect {
            active: carry,
            returns,
            last_seen,
            gaps: std::mem::take(&mut sd.gaps),
            interrupted: sd.interrupted,
            halted_since: sd.halted_since,
            ..SymDetect::default()
        };
        sd.date = Some(x.date);
    }
    let seg = market_events::session::classify(market_events::UnixNanos(x.b - 1))
        .filter(|s| s.kind != SessionKind::Overnight);
    // Windows never cross a segment: an episode ends with its segment.
    if sd
        .active
        .iter()
        .any(|e| Some(e.view.segment) != seg && !e.view.state.is_terminal())
    {
        let (keep, old): (Vec<Ep>, Vec<Ep>) = std::mem::take(&mut sd.active)
            .into_iter()
            .partition(|e| Some(e.view.segment) == seg);
        sd.active = old;
        interrupt_all(det, sd, "segment_end", x.tau, Some((x.b, x.tau)));
        sd.active = keep;
    }
    // Observed status and integrity: interrupt, and decide nothing new.
    if x.halted {
        if sd.halted_since.is_none() {
            sd.halted_since = Some(x.tau);
        }
        interrupt_all(det, sd, "halted", x.tau, Some((x.b, x.tau)));
        return;
    }
    if let Some(h) = sd.halted_since.take() {
        while sd.gaps.len() >= GAPS_KEPT {
            sd.gaps.pop_front();
        }
        sd.gaps.push_back((h, x.tau, "halted"));
        sd.requalify_from = sd.requalify_from.max(x.tau);
    }
    if let Some((_, why)) = det.global.or(sd.interrupted) {
        interrupt_all(det, sd, why, x.tau, Some((x.b, x.tau)));
        return;
    }
    if x.coverage_partial || x.untrusted {
        let why = if x.untrusted {
            "inputs_untrusted"
        } else {
            "coverage_partial"
        };
        interrupt_all(det, sd, why, x.tau, Some((x.b, x.tau)));
        return;
    }
    let Some(seg) = seg.and_then(Seg::of) else {
        return;
    };
    let f = features::candidate(det, sd, x, seg);
    // Old levels first: every active episode is tested before anything new forms.
    micro::on_barrier(det, sd, x, seg);
    families::on_barrier(det, sd, x, f.as_ref());
    // New detections need a bar completing now: nothing else can change their inputs.
    if f.is_some() {
        let ignited = families::detect(det, sd, x, seg, f.as_ref());
        if !ignited {
            families::momentum(det, sd, x, seg, f.as_ref());
        }
        micro::search(det, sd, x, seg);
    }
    retire_terminal(det, sd);
    // The candidate joins the return baseline only after it was tested against it.
    if let Some(f) = f {
        if f.return_class == ReturnClass::Adjacent
            && let Some(r) = f.log_return_ppm
        {
            features::push_return(det, sd, x.b, seg.id, r);
        }
        sd.latest = Some(f);
    }
}

/// Records a price-qualifying admitted print and tests armed levels against it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn on_print(
    det: &mut Detector,
    sd: &mut SymDetect,
    id: &TradeId,
    exec: i64,
    admit: i64,
    price: i64,
    ohlc: bool,
) {
    det.as_of = Some(det.as_of.map_or(admit, |a| a.max(admit)));
    if !ohlc || det.disabled.is_some() {
        return;
    }
    // The latest admitted print, in admission order (what a live observer knew last).
    sd.last_seen = Some(Seen { exec, admit, price });
    if det.global.is_some() || sd.interrupted.is_some() || sd.halted_since.is_some() {
        return;
    }
    micro::on_print(det, sd, id, exec, admit, price);
    retire_terminal(det, sd);
}

/// A revision the engine resolved to one ledger print `(id, exec)`. `now` is `Some(price)` when
/// the corrected print still sets a price, `None` for a cancel or a print that no longer does.
pub(crate) fn on_revision(
    det: &mut Detector,
    sd: &mut SymDetect,
    id: &TradeId,
    exec: i64,
    now: Option<i64>,
    admit: i64,
) {
    det.as_of = Some(det.as_of.map_or(admit, |a| a.max(admit)));
    if det.disabled.is_some() {
        return;
    }
    micro::on_revision(det, sd, id, exec, now, admit);
    retire_terminal(det, sd);
}

/// Opens or closes a gap for a symbol or the whole market.
pub(crate) fn set_interrupted(
    det: &mut Detector,
    sd: Option<&mut SymDetect>,
    symbol: Option<&str>,
    at: i64,
    reason: &'static str,
) {
    match sd {
        Some(sd) => {
            if sd.interrupted.is_none() {
                sd.interrupted = Some((at, reason));
            }
            interrupt_all(det, sd, reason, at, None);
        }
        None => {
            if symbol.is_none() && det.global.is_none() {
                det.global = Some((at, reason));
            }
        }
    }
}

pub(crate) fn clear_interrupted(
    det: &mut Detector,
    sd: Option<&mut SymDetect>,
    symbol: Option<&str>,
    at: i64,
) {
    match sd {
        Some(sd) => {
            if let Some((from, why)) = sd.interrupted.take() {
                while sd.gaps.len() >= GAPS_KEPT {
                    sd.gaps.pop_front();
                }
                sd.gaps.push_back((from, at, why));
                det.push_gap(Gap {
                    symbol: symbol.map(str::to_string),
                    from,
                    to: Some(at),
                    reason: why,
                });
            }
            sd.requalify_from = sd.requalify_from.max(at);
        }
        None => {
            if symbol.is_none()
                && let Some((from, why)) = det.global.take()
            {
                det.push_gap(Gap {
                    symbol: None,
                    from,
                    to: Some(at),
                    reason: why,
                });
            }
        }
    }
}

/// Priority for the one dominant episode per symbol (higher first).
fn dominance(ep: &Episode) -> u8 {
    match (ep.family, ep.state) {
        (Family::MicroPullback, State::Armed | State::CrossingProvisional) => 6,
        (Family::Ignition, _) => 5,
        (Family::RangeBreakout, _) => 4,
        (Family::MicroPullback, _) => 3,
        (Family::MomentumBuilding, _) => 2,
        (Family::IgnitionGapEvent, _) => 1,
        (Family::UpwardAcceleration, _) => 0,
    }
}

/// Builds the snapshot from every symbol's state (`syms`: symbol order).
pub(crate) fn snapshot<'a>(
    det: &Detector,
    run: &crate::RunInfo,
    syms: impl Iterator<Item = (&'a str, &'a SymDetect)>,
) -> DetectionSnapshot {
    let fresh = det.policy.body.timing.price_fresh_secs * SEC;
    let mut symbols = Vec::new();
    let mut active = Vec::new();
    let mut interrupted_symbols = Vec::new();
    for (name, sd) in syms {
        if let Some((from, why)) = sd.interrupted {
            interrupted_symbols.push(Gap {
                symbol: Some(name.to_string()),
                from,
                to: None,
                reason: why,
            });
        }
        if !sd.has_active() {
            continue;
        }
        let mut eps: Vec<Episode> = sd
            .active
            .iter()
            .filter(|e| !e.view.state.is_terminal())
            .map(|e| {
                let mut v = e.view.clone();
                v.current = assess(&v, &e.extra, sd.last_seen, det.as_of, fresh);
                v
            })
            .collect();
        eps.sort_by(|a, b| a.episode_id.cmp(&b.episode_id));
        let dominant = eps
            .iter()
            .max_by(|a, b| {
                dominance(a)
                    .cmp(&dominance(b))
                    .then(a.last_material_seq.cmp(&b.last_material_seq))
                    .then(b.episode_id.cmp(&a.episode_id))
            })
            .map(|e| e.episode_id.clone());
        symbols.push(SymbolDetections {
            symbol: name.to_string(),
            dominant_episode_id: dominant,
            active_episode_ids: eps.iter().map(|e| e.episode_id.clone()).collect(),
            interrupted: sd.interrupted.map(|(_, w)| w),
        });
        active.extend(eps);
    }
    let n_active = active.len();
    DetectionSnapshot {
        schema: 1,
        run_id: run.run_id.clone(),
        source: run.source.clone(),
        fidelity: run.fidelity,
        policy: det.policy.tag(),
        experimental: det.policy.body.experimental,
        alerts_enabled: det.policy.body.alerts_enabled,
        cursor: det.seq,
        bar_end: det.last_barrier.map(|(b, _)| b),
        cutoff: det.last_barrier.map(|(_, t)| t),
        as_of: det.as_of,
        symbols,
        active,
        recent: det.recent.iter().cloned().collect(),
        data_status: DataStatus {
            global_interruption: det.global.map(|(from, why)| Gap {
                symbol: None,
                from,
                to: None,
                reason: why,
            }),
            interrupted_symbols,
            recent_gaps: det.gaps.iter().cloned().collect(),
            disabled_reason: det.disabled,
        },
        telemetry: det.telemetry(n_active),
    }
}

/// Distances from the latest admitted price to the episode's current levels.
fn assess(
    ep: &Episode,
    extra: &Extra,
    seen: Option<Seen>,
    as_of: Option<i64>,
    fresh: i64,
) -> Option<Assessment> {
    let s = seen?;
    let now = as_of.unwrap_or(s.admit).max(s.admit);
    let (trigger, invalidation) = match extra {
        Extra::Micro(m) => (
            m.trigger.as_ref().map(|l| l.price),
            m.invalidation.as_ref().map(|l| l.price),
        ),
        Extra::Breakout { level, .. } => (Some(*level), None),
        _ => {
            let hi = ep.levels.iter().rev().find(|l| l.kind == "range_high");
            (hi.map(|l| l.price), None)
        }
    };
    let p = s.price;
    let at_or_above = trigger.map(|t| p >= t);
    let (dt, dtb) = match trigger {
        Some(t) if p < t && p > 0 => (Some(t - p), Some(math::bp(t - p, p))),
        _ => (None, None),
    };
    let (di, dib) = match invalidation {
        Some(i) if p > i && p > 0 => (Some(p - i), Some(math::bp(p - i, p))),
        _ => (None, None),
    };
    Some(Assessment {
        price: p,
        price_exec: s.exec,
        price_age_ms: (now - s.exec) / 1_000_000,
        stale: now - s.exec > fresh,
        trigger,
        at_or_above_trigger: at_or_above,
        distance_to_trigger_micros: dt,
        distance_to_trigger_bp: dtb,
        invalidation,
        distance_to_invalidation_micros: di,
        distance_to_invalidation_bp: dib,
    })
}
