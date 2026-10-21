//! The causal per-symbol context engine.
//!
//! # Timing
//!
//! Decisions sit on a grid of completed bars (`grid.bar_secs`, 15 s in v1). The bar ending at `b`
//! is decided at the barrier `tau = b + settle` (2 s): it sees exactly the prints that executed
//! before `b` AND were admitted (applied) at or before `tau`. A print that executed before `b` but
//! arrived after `tau` is late: it joins the next decision, never the one already made. Evidence
//! records join as of `tau` (`EvidenceStore`). The engine is driven by the run's own clock
//! (`advance_to`, called with each item's admission time before the item applies), never by wall
//! time, so a linear replay and a rebuild after a seek produce the same decisions, and processing
//! delay can never widen what a decision admitted.
//!
//! # Output
//!
//! Every barrier evaluates the symbols that traded on the trading date and need it (new prints,
//! new evidence, or a time-based flip such as a price going stale). A symbol emits a [`Decision`]
//! only when its content changed; decisions are owned by the caller (append-only log), the engine
//! keeps only the latest per symbol. First-seen times live on the per-day [`Timeline`] and are
//! never rewritten.

use crate::evaluate::{
    self, Coverage, Integrity, NewsSummary, PriceView, RvolDiagnostics, VolumeView,
};
use crate::evidence::{
    DailyBaseline, EvidenceStore, FloatObservation, InstrumentId, IntradayBaseline, NewsCoverage,
    NewsItem, PrevClose,
};
use crate::output::{BoundedLog, OutputTelemetry, PANEL_LOG_MAX, SETUP_LOG_MAX};
use crate::panel::{
    Eligibility, Membership, Momentum, MomentumRules, MomentumStatus, PanelPolicy, momentum,
    momentum_order, update_membership,
};
use crate::pillar::{Counts, PillarId, PillarResult, Qualification, Tri, aggregate, price_pillar};
use crate::policy::Policy;
use crate::revision::{PrintRevision, REVISION_WINDOW_SECS, RevisionTelemetry};
use crate::setup::{
    BARS_KEPT, Bar, BullFlag, SetupLogEntry, SetupPolicy, SetupState, SetupView, Step, spark,
};
use chrono::{Days, NaiveDate};
use market_events::session::{SessionKind, classify, et, trading_date};
use market_events::{Price, Resolution, Trade, TradeId, TradeRef, UnixNanos};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

const SEC: i64 = 1_000_000_000;
const MS: i64 = 1_000_000;

/// Hard bound on one symbol's admission ledger, beside the time bound.
///
/// Two bounds, not one: a reconnect replay's timing is not something to assume, so the time
/// window alone could hold an unbounded number of entries for a symbol printing fast enough.
/// Whichever bites first, an eviction is counted and a **queued** entry evicted degrades the
/// symbol's coverage, because that is volume the engine will never count.
pub const LEDGER_MAX: usize = 20_000;

/// What kind of data a run decides on: recorded live arrival order, a reconstruction from
/// history without arrival times (no latency claims), or a synthetic fixture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Fidelity {
    RecordedLive,
    ReconstructedHistory,
    SyntheticFixture,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RunInfo {
    pub run_id: String,
    /// `live` or `replay`: runs never share state.
    pub source: String,
    pub fidelity: Fidelity,
    /// How completely this run collects today's prints (the RVOL numerator).
    pub numerator_coverage: Coverage,
}

/// The chart engine's verdict on a print, as the run's conditions policy decided it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Verdict {
    pub ohlc: bool,
    pub volume: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Timeline {
    /// First decision with a fresh price inside the discovery band.
    pub first_eligible: Option<i64>,
    /// First decision each pillar passed at.
    pub first_pass: BTreeMap<PillarId, i64>,
    pub first_qualified: Option<i64>,
    pub qualified_now: bool,
    /// Qualified at some decision earlier today, whatever it is now.
    pub qualified_earlier: bool,
    pub lost_at: Option<i64>,
    pub requalified: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct VolumeFacts {
    pub shares: u64,
    /// Dollar volume, whole dollars (truncated).
    pub dollars: u64,
    pub trades: u64,
    /// This symbol's own coverage when something is known about it, else the run's.
    pub coverage: Coverage,
    /// Why this symbol's coverage differs from the run's, when it does: `recorder_drops`,
    /// `identities_evicted`, `admission_ledger_overflow`, `quarantined_old_prints`. `None`: the
    /// run-wide figure.
    pub coverage_reason: Option<&'static str>,
    /// What about this symbol's trade-derived state cannot be trusted, and therefore is not
    /// reported ([`crate::evaluate::Integrity`]).
    pub integrity: Integrity,
}

/// One frozen decision. Times are UTC nanoseconds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Decision {
    pub seq: u64,
    pub decision_id: String,
    pub run_id: String,
    pub source: String,
    pub fidelity: Fidelity,
    /// `pillars-v1@<hash>`.
    pub policy: String,
    pub symbol: String,
    pub instrument: InstrumentId,
    pub trading_date: NaiveDate,
    /// End of the completed bar the decision covers (volume endpoint).
    pub bar_end: i64,
    /// The data barrier: nothing admitted after it is in this decision.
    pub cutoff: i64,
    pub pillars: Vec<PillarResult>,
    pub qualification: Qualification,
    pub counts: Counts,
    /// Price fresh and inside the discovery band (research default $0.15–$25).
    pub in_discovery_band: Option<bool>,
    /// Gain against the close as stored, whatever its basis: a named diagnostic only.
    pub gain_vs_stored_close_bp: Option<i64>,
    pub volume: VolumeFacts,
    pub rvol: RvolDiagnostics,
    pub news: NewsSummary,
    pub timeline: Timeline,
    /// Ross Panel membership after this decision (`five_pillars_membership_v1`).
    pub membership: Membership,
    /// Which parts differ from this symbol's previous decision.
    pub changed: Vec<&'static str>,
}

/// One print this engine admitted, as the **admission ledger** holds it.
///
/// The ledger is one structure doing four jobs that have to agree with each other: it is the
/// queue of prints waiting for their barrier, the duplicate index, what a revision is resolved
/// against, and what the retained-window state is rebuilt from after one. They were three
/// separate things, and they disagreed: a revision could subtract volume from a print the engine
/// had never counted, a cancel arriving before its print's barrier was ignored and the print
/// counted anyway, and the identity set was cleared after the first prints of a date had already
/// been queued, so a later redelivery of one counted twice.
struct Admitted {
    id: TradeId,
    exec: i64,
    admit: i64,
    price: i64,
    size: u64,
    verdict: Verdict,
    /// The print's own session, as delivered.
    session: Option<(NaiveDate, SessionKind)>,
    /// Waiting for the barrier that will fold it into the aggregates.
    queued: bool,
    /// Folded into the aggregates. A print the barrier skipped (another date, an excluded
    /// session) is neither `queued` nor `counted`: a revision of it resolves and changes nothing.
    counted: bool,
    /// The bar end of the barrier that folded it in, once one has.
    ///
    /// Not its execution time. A print executed inside one bar and admitted during a later one
    /// is folded in at that later barrier, so the cumulative checkpoints taken in between never
    /// held it — revising "every checkpoint after the execution" would have invented recent
    /// volume for the ones that did not, or clamped away volume that was real.
    folded_at: Option<i64>,
    /// Revised away: cancelled, or corrected into something that counts for nothing.
    removed: bool,
}

impl Admitted {
    fn key(&self) -> (TradeId, i64) {
        (self.id.clone(), self.exec)
    }

    /// Volume this print contributes to the aggregates right now.
    fn volume(&self) -> u64 {
        if self.counted && !self.removed && self.verdict.volume {
            self.size
        } else {
            0
        }
    }
}

struct Sym {
    name: String,
    inst: InstrumentId,
    date: Option<NaiveDate>,
    /// The admission ledger: every print this engine can still prove it admitted, in arrival
    /// order ([`Admitted`]). Bounded twice — by execution time
    /// ([`crate::revision::REVISION_WINDOW_SECS`]) and by a hard entry count
    /// ([`LEDGER_MAX`]) — because a reconnect replay's timing is not something to assume.
    ledger: VecDeque<Admitted>,
    /// The ledger's identities, for an O(1) duplicate check.
    ids: HashSet<(TradeId, i64)>,
    /// Had a counted print on `date`: only those symbols emit.
    active: bool,
    cum: u64,
    dollars_micros: u128,
    trades: u64,
    /// (execution, price) of the latest OHLC print admitted.
    last_price: Option<(i64, i64)>,
    /// (bar end, cumulative volume) history for the recent window.
    cums: VecDeque<(i64, u64)>,
    last: Option<Decision>,
    timeline: Timeline,
    recheck_at: i64,
    dirty: bool,
    /// The latest OHLC print per whole second, `(execution, price)`, for the momentum endpoints
    /// (every endpoint is a whole second, so the compression is exact).
    ticks: VecDeque<(i64, i64)>,
    day_high: Option<i64>,
    member: Membership,
    halted: bool,
    /// Completed setup bars of the trading date (the latest `BARS_KEPT`), oldest first.
    bars: VecDeque<Bar>,
    /// End of the bar holding today's first volume-counted print (the average-volume clock).
    first_bar: Option<i64>,
    setup: BullFlag,
    /// First time StockWatcher saw any of the qualifying news at the latest decision.
    news_first_seen: Option<i64>,
    /// How completely this symbol's own prints were collected today, when something is known
    /// about it that the run-wide figure does not say.
    ///
    /// Run-wide `Coverage` lives on [`RunInfo`] and production sets it to `Unverified`, which is
    /// honest but says nothing about a symbol whose history is known to be short — a recorder
    /// drop, a stream gap, or a restore that could not reach the day's start. Per symbol and in
    /// memory, so nothing queries a disk per barrier (the audit's F2).
    coverage: Option<(Coverage, &'static str)>,
    /// Whether this symbol's trade-derived state can be trusted, and for what
    /// ([`crate::evaluate::Integrity`]). A doubt here makes the dependent outputs UNKNOWN; it is
    /// not a log line.
    integrity: Integrity,
    /// Executions at or before this can no longer be proved new: their identities have left the
    /// ledger. A print that old is refused rather than counted again
    /// ([`RevisionTelemetry::quarantined_old`]).
    quarantine_through: Option<i64>,
    /// The end of the last barrier whose volume came from a restore rather than from this run's
    /// own admissions ([`Engine::adopt_facts`]).
    ///
    /// The day's totals a restore brings over are exact — the prints are the prints. Which
    /// **barrier** each one folded into is not: a recording stamps the collector's delivery time,
    /// and this run's own admission would have come a little later, through different queues. So
    /// a recent window reaching back into the restored prefix is approximate, and says so
    /// ([`Integrity::window_untrusted`]) until the window has moved past it, which it does by
    /// itself within the window's own length.
    restored_through: Option<i64>,
}

/// A non-passing pillar, for the row tag.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MissingPillar {
    pub pillar: PillarId,
    pub state: Tri,
    pub reason: &'static str,
}

/// Closes kept in a row's sparkline.
pub const SPARK_BARS: usize = 20;

/// One Ross Panel member at one barrier.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PanelRow {
    pub symbol: String,
    pub instrument: InstrumentId,
    pub membership: Membership,
    pub counts: Counts,
    /// Last qualifying price and the gain pillar's value (bp), when known.
    pub price: Option<i64>,
    pub gain_bp: Option<i64>,
    pub momentum: Momentum,
    /// 1-based among ranked rows; `None`: momentum unavailable, listed after the ranked rows.
    pub rank: Option<u32>,
    pub prev_rank: Option<u32>,
    /// The one-minute move (`momentum.r_primary_bp`), bp.
    pub move_1m_bp: Option<i64>,
    /// Below the day's high so far (`momentum.below_high_bp`), bp, 0 at the high.
    pub from_high_bp: Option<i64>,
    /// The RVOL pillar's daily-denominator value, thousandths, when computed.
    pub rvol_milli: Option<i64>,
    /// The float pillar's shares, when a float record is visible (even stale).
    pub float_shares: Option<u64>,
    /// When StockWatcher first saw a currently qualifying news item (its `observed_at`).
    pub news_first_seen: Option<i64>,
    /// The pillars not passing at the latest decision.
    pub missing: Vec<MissingPillar>,
    /// Closes (micros) of the last 20 completed 15 s setup bars that had an OHLC print, oldest
    /// first; fewer when not available, never padded or interpolated.
    pub spark: Vec<i64>,
    /// The `bull_flag_v0` setup view (EXPERIMENTAL, `setups-v0`).
    pub setup: SetupView,
}

/// The Ross Panel at one barrier: active members in momentum order, published with the same
/// decision revision as the membership it reflects.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PanelSnapshot {
    /// The engine's decision sequence at this barrier.
    pub revision: u64,
    pub run_id: String,
    pub source: String,
    pub panel_policy: String,
    pub pillar_policy: String,
    pub bar_end: i64,
    pub cutoff: i64,
    /// `setups-v0@<hash>`.
    pub setup_policy: String,
    /// Members whose setup is `near`, `crossing` or `triggered`.
    pub attention: u32,
    pub rows: Vec<PanelRow>,
}

/// Records an OHLC print in the per-second history: the latest execution within each second.
fn record_tick(ticks: &mut VecDeque<(i64, i64)>, exec: i64, price: i64) {
    let sec = exec.div_euclid(SEC);
    let pos = ticks.partition_point(|&(e, _)| e.div_euclid(SEC) < sec);
    match ticks.get_mut(pos) {
        Some(t) if t.0.div_euclid(SEC) == sec => {
            if exec >= t.0 {
                *t = (exec, price);
            }
        }
        _ => ticks.insert(pos, (exec, price)),
    }
}

pub struct Engine {
    policy: Policy,
    run: RunInfo,
    evidence: EvidenceStore,
    syms: BTreeMap<String, Sym>,
    by_inst: HashMap<InstrumentId, String>,
    next_bar: Option<i64>,
    seq: u64,
    panel_policy: PanelPolicy,
    panel_log: BoundedLog<PanelSnapshot>,
    last_panel: Option<PanelSnapshot>,
    setup_policy: SetupPolicy,
    setup_log: BoundedLog<SetupLogEntry>,
    revisions: RevisionTelemetry,
    restores: RestoreTelemetry,
}

/// What an engine adopted from per-symbol restores.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct RestoreTelemetry {
    /// Symbols whose restored facts were adopted into this engine.
    pub adopted: u64,
    /// Adoptions refused because the restore decided as another run, under another policy, or
    /// about another trading date ([`Adoption::Refused`]).
    pub refused: u64,
    /// Adoptions that kept a live symbol's already-published decisions and episodes.
    pub published_preserved: u64,
    /// Restores whose own replay-generated membership, episodes or latest decision were
    /// discarded rather than published as if this run had observed them.
    pub episodes_discarded: u64,
}

/// What [`Engine::adopt_facts`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Adoption {
    Adopted {
        /// The symbol was already live, so its published decisions and episodes were kept.
        published: bool,
    },
    /// The restore had nothing for the symbol.
    Nothing,
    /// Refused: `run_identity`, `pillar_policy`, `panel_policy`, `setup_policy`, `trading_date`.
    Refused(&'static str),
}

fn max_opt(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (x, None) => x,
        (None, y) => y,
    }
}

fn ceil_to(t: i64, step: i64) -> i64 {
    t.div_euclid(step) * step + if t.rem_euclid(step) == 0 { 0 } else { step }
}

impl Engine {
    pub fn new(policy: Policy, run: RunInfo) -> Self {
        Engine {
            policy,
            run,
            evidence: EvidenceStore::default(),
            syms: BTreeMap::new(),
            by_inst: HashMap::new(),
            next_bar: None,
            seq: 0,
            panel_policy: PanelPolicy::v1(),
            panel_log: BoundedLog::new(PANEL_LOG_MAX),
            last_panel: None,
            setup_policy: SetupPolicy::v0(),
            setup_log: BoundedLog::new(SETUP_LOG_MAX),
            revisions: RevisionTelemetry::default(),
            restores: RestoreTelemetry::default(),
        }
    }

    /// The same engine with other output journal bounds (the defaults are [`PANEL_LOG_MAX`] and
    /// [`SETUP_LOG_MAX`]). Bounds only decide when the oldest held output is dropped and counted;
    /// they never change a decision, a panel or the latest snapshot.
    pub fn with_output_bounds(mut self, panels: usize, setup_entries: usize) -> Self {
        self.panel_log = BoundedLog::new(panels);
        self.setup_log = BoundedLog::new(setup_entries);
        self
    }

    /// What the two output journals have produced, handed over and dropped.
    pub fn output_telemetry(&self) -> OutputTelemetry {
        OutputTelemetry {
            panels_published: self.panel_log.logged(),
            panels_taken: self.panel_log.taken(),
            panels_dropped: self.panel_log.dropped(),
            panels_held: self.panel_log.held(),
            setup_entries_logged: self.setup_log.logged(),
            setup_entries_taken: self.setup_log.taken(),
            setup_entries_dropped: self.setup_log.dropped(),
            setup_entries_held: self.setup_log.held(),
            panel_log_max: self.panel_log.max(),
            setup_log_max: self.setup_log.max(),
        }
    }

    /// The same engine under another setup policy.
    pub fn with_setup_policy(mut self, p: SetupPolicy) -> Self {
        self.setup_policy = p;
        self
    }

    pub fn setup_policy(&self) -> &SetupPolicy {
        &self.setup_policy
    }

    /// Setup transitions and level versions held now, oldest first; the journal is empty
    /// afterwards. Bounded ([`SETUP_LOG_MAX`]): a caller that stops taking loses the oldest
    /// entries, counted in [`Engine::output_telemetry`].
    pub fn take_setup_log(&mut self) -> Vec<SetupLogEntry> {
        self.setup_log.take()
    }

    /// The same engine under another Ross Panel policy.
    pub fn with_panel_policy(mut self, p: PanelPolicy) -> Self {
        self.panel_policy = p;
        self
    }

    pub fn panel_policy(&self) -> &PanelPolicy {
        &self.panel_policy
    }

    /// Panel snapshots held now (each one a change), oldest first; the journal is empty
    /// afterwards. Bounded ([`PANEL_LOG_MAX`]): a caller that stops taking loses the oldest
    /// snapshots, counted in [`Engine::output_telemetry`]. The latest snapshot is kept apart
    /// ([`Engine::panel`]), so what a client sees never depends on this.
    pub fn take_panels(&mut self) -> Vec<PanelSnapshot> {
        self.panel_log.take()
    }

    /// The latest panel snapshot (what a client snapshot sends).
    pub fn panel(&self) -> Option<&PanelSnapshot> {
        self.last_panel.as_ref()
    }

    /// Records what is known about one symbol's own print collection today, overriding the
    /// run-wide figure for it (`None` to go back to the run's).
    ///
    /// Called from memory — a link state, a recorder's drop counter, a restore that fell short —
    /// never from a disk query per barrier. `reason` is a fixed label so a reader can tell the
    /// kinds apart: a **recorder** drop means the recording this process restores from is short
    /// while the live numerator was fine, and a **stream** gap means the numerator itself is
    /// short. Conflating them would blame the wrong half.
    pub fn set_coverage(&mut self, symbol: &str, coverage: Option<(Coverage, &'static str)>) {
        if let Some(s) = self.syms.get_mut(symbol)
            && s.coverage != coverage
        {
            s.coverage = coverage;
            s.dirty = true;
        }
    }

    /// This symbol's own coverage, when something is known about it.
    pub fn coverage_of(&self, symbol: &str) -> Option<(Coverage, &'static str)> {
        self.syms.get(symbol).and_then(|s| s.coverage)
    }

    /// Moves one symbol's **trade-derived facts** out of a restore engine into this one, keeping
    /// whatever this engine has already published about the symbol.
    ///
    /// A restart restores a symbol by replaying its own recorded day on a second engine, in
    /// receipt order with the recorded delivery times, and then adopting the result. Feeding
    /// those prints into this engine instead would classify every one of them as late against a
    /// clock that has already advanced with the live tail, and would decide none of the day's
    /// barriers: the numerator would still start at zero after every deploy, which is what the
    /// audit found (F1).
    ///
    /// # Facts, not decisions
    ///
    /// What the restore brings over is the arithmetic of the day's prints: volume, dollars, trade
    /// count, the price and tick history, the day high, the setup bars, the admission ledger and
    /// what is known about their integrity and coverage. Those are statements about prints that
    /// really executed, and the recording is the evidence for them.
    ///
    /// What it does **not** bring over is membership, the timeline's first-seen times, the setup
    /// state with its frozen levels, or the latest decision. Those are claims that this run
    /// observed something at a time — a 5/5 at 09:41, a pole and a pullback — and a recording
    /// made by a previous process is not evidence that this one saw it. Publishing them would
    /// backdate decisions this engine never made, under this engine's run id, and would overwrite
    /// an episode the live tail may already have broadcast with frozen levels. So they are
    /// discarded and counted ([`RestoreTelemetry::episodes_discarded`]); the live symbol's own
    /// published state stays exactly as it was, and the next barrier decides the symbol again on
    /// the full day's facts.
    ///
    /// The restored facts are therefore a **bootstrap**: from the next barrier the symbol's RVOL
    /// numerator is the day's, not the tail's, and anything it then qualifies for is qualified
    /// prospectively, as of that barrier.
    ///
    /// # Refusals
    ///
    /// The restore must have decided as this run and under the same three policies, and must be
    /// about the same trading date as the live state. Anything else is refused rather than
    /// reconciled: adopting facts measured under other rules would mix two incompatible
    /// arithmetics in one symbol with no way to tell afterwards which half is which.
    pub fn adopt_facts(&mut self, from: &mut Engine, symbol: &str) -> Adoption {
        let Some(restored) = from.syms.remove(symbol) else {
            return Adoption::Nothing;
        };
        from.by_inst.remove(&restored.inst);
        if let Some(why) = self.identity_gap(from) {
            self.restores.refused += 1;
            return Adoption::Refused(why);
        }
        // The adoption boundary: one trading date. A restore of another date is not this
        // symbol's day and its cumulative volume is not today's numerator.
        let live_date = self.syms.get(symbol).and_then(|s| s.date);
        if let (Some(a), Some(b)) = (live_date, restored.date)
            && a != b
        {
            self.restores.refused += 1;
            return Adoption::Refused("trading_date");
        }
        if restored.member.status.is_some()
            || !restored.member.episodes.is_empty()
            || restored.last.is_some()
        {
            self.restores.episodes_discarded += 1;
        }
        // The bar grid is a property of the clock, not of a symbol, so an engine that has not
        // seen a print of its own takes the restore's anchor rather than staying unanchored and
        // deciding nothing. An engine already running keeps its own.
        self.next_bar = self.next_bar.or(from.next_bar);
        let published = match self.syms.get_mut(symbol) {
            Some(live) => {
                // Facts in, published state untouched.
                live.date = restored.date.or(live.date);
                live.ledger = restored.ledger;
                live.ids = restored.ids;
                live.active = restored.active || live.active;
                live.cum = restored.cum;
                live.dollars_micros = restored.dollars_micros;
                live.trades = restored.trades;
                live.last_price = restored.last_price;
                live.cums = restored.cums;
                live.ticks = restored.ticks;
                live.day_high = restored.day_high;
                live.bars = restored.bars;
                live.first_bar = restored.first_bar;
                live.integrity = live.integrity.or(restored.integrity);
                live.coverage = restored.coverage.or(live.coverage);
                live.quarantine_through = max_opt(live.quarantine_through, restored.quarantine_through);
                live.restored_through = max_opt(live.restored_through, restored.restored_through);
                live.dirty = true;
                live.recheck_at = i64::MAX;
                true
            }
            None => {
                // Nothing published about the symbol yet, so there is nothing to keep — but the
                // restore's own membership, timeline, setup and decision are still not this
                // run's observations and do not become them by arriving first.
                let mut sym = restored;
                sym.member = Membership::default();
                sym.timeline = Timeline::default();
                sym.setup = BullFlag::default();
                sym.last = None;
                sym.news_first_seen = None;
                sym.halted = false;
                sym.dirty = true;
                sym.recheck_at = i64::MAX;
                self.by_inst.insert(sym.inst.clone(), symbol.to_string());
                self.syms.insert(symbol.to_string(), sym);
                false
            }
        };
        self.restores.adopted += 1;
        if published {
            self.restores.published_preserved += 1;
        }
        Adoption::Adopted { published }
    }

    /// Marks a symbol's facts as reaching back into a restored prefix, whose barrier attribution
    /// is the recording's rather than this run's ([`Sym::restored_through`]).
    pub fn set_restored_through(&mut self, symbol: &str, through: Option<i64>) {
        if let Some(s) = self.syms.get_mut(symbol) {
            s.restored_through = max_opt(s.restored_through, through);
            s.dirty = true;
        }
    }

    /// Why another engine's decisions cannot be adopted into this one, if they cannot.
    fn identity_gap(&self, other: &Engine) -> Option<&'static str> {
        if self.run.run_id != other.run.run_id || self.run.source != other.run.source {
            return Some("run_identity");
        }
        if self.policy.hash != other.policy.hash {
            return Some("pillar_policy");
        }
        if self.panel_policy.hash != other.panel_policy.hash {
            return Some("panel_policy");
        }
        if self.setup_policy.hash != other.setup_policy.hash {
            return Some("setup_policy");
        }
        None
    }

    /// Drops a symbol's state (the live market reloading it from the archive).
    pub fn remove_symbol(&mut self, symbol: &str) -> bool {
        match self.syms.remove(symbol) {
            Some(s) => {
                self.by_inst.remove(&s.inst);
                true
            }
            None => false,
        }
    }

    /// What this engine adopted from restores.
    pub fn restore_telemetry(&self) -> RestoreTelemetry {
        self.restores
    }

    /// Observed trading status: a halted symbol is never ranked on a frozen pre-halt return.
    pub fn set_halted(&mut self, symbol: &str, halted: bool) {
        if let Some(s) = self.syms.get_mut(symbol)
            && s.halted != halted
        {
            s.halted = halted;
            s.dirty = true;
        }
    }

    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    /// What this engine decides as: its run id, source and fidelity.
    ///
    /// A restart restores a symbol on a second engine and adopts the result
    /// ([`Engine::adopt_symbol`]), and that engine has to decide under the same identity and the
    /// same policies — a reconstruction of this run's own history is this run's, not a different
    /// one whose ids would then travel on the adopted setup levels.
    pub fn run_info(&self) -> &RunInfo {
        &self.run
    }

    fn bar(&self) -> i64 {
        self.policy.body.grid.bar_secs * SEC
    }

    fn settle(&self) -> i64 {
        self.policy.body.grid.settle_ms * MS
    }

    fn mark(&mut self, inst: &InstrumentId) {
        if let Some(name) = self.by_inst.get(inst)
            && let Some(s) = self.syms.get_mut(name)
        {
            s.dirty = true;
        }
    }

    pub fn add_prev_close(&mut self, r: PrevClose) {
        let i = r.instrument.clone();
        self.evidence.add_prev_close(r);
        self.mark(&i);
    }
    pub fn add_daily(&mut self, r: DailyBaseline) {
        let i = r.instrument.clone();
        self.evidence.add_daily(r);
        self.mark(&i);
    }
    pub fn add_intraday(&mut self, r: IntradayBaseline) {
        let i = r.instrument.clone();
        self.evidence.add_intraday(r);
        self.mark(&i);
    }
    pub fn add_float(&mut self, r: FloatObservation) {
        let i = r.instrument.clone();
        self.evidence.add_float(r);
        self.mark(&i);
    }
    pub fn add_news(&mut self, r: NewsItem) {
        let i = r.instrument.clone();
        self.evidence.add_news(r);
        self.mark(&i);
    }
    /// A market-wide record ([`InstrumentId::market`]) may change any symbol's news pillar, so
    /// every symbol is re-evaluated at the next barrier.
    pub fn add_coverage(&mut self, r: NewsCoverage) {
        let i = r.instrument.clone();
        self.evidence.add_coverage(r);
        if i.is_market() {
            for s in self.syms.values_mut() {
                s.dirty = true;
            }
        } else {
            self.mark(&i);
        }
    }

    /// Drops the evidence no future decision can read ([`EvidenceStore::prune`]): every barrier
    /// still to be decided has a cutoff at or after the next one's. Returns how many records went.
    /// Nothing before the engine has a clock.
    pub fn prune_evidence(&mut self) -> usize {
        let Some(b) = self.next_bar else {
            return 0;
        };
        let fresh = self.policy.body.reference_profile.news_freshness_secs * SEC;
        // Strictly before the next cutoff: a record observed exactly at it is still "at or
        // before" for that decision, and pruning keeps the latest at or before `before` anyway.
        self.evidence.prune(b + self.settle() - 1, fresh)
    }

    /// Evidence records held (logs and tests).
    pub fn evidence_len(&self) -> usize {
        self.evidence.len()
    }

    /// Applies one print admitted at `admit` (the run clock when it applied). Decides every
    /// barrier passed before it first, so the print can only reach later decisions.
    pub fn apply(&mut self, t: &Trade, verdict: Verdict, admit: UnixNanos) -> Vec<Decision> {
        let out = self.advance_to(admit);
        let name = t.symbol.clone();
        let sym = self.syms.entry(name.clone()).or_insert_with(|| {
            let inst = InstrumentId::symbol(&name);
            Sym {
                name: name.clone(),
                inst,
                date: None,
                ledger: VecDeque::new(),
                ids: HashSet::new(),
                active: false,
                cum: 0,
                dollars_micros: 0,
                trades: 0,
                last_price: None,
                cums: VecDeque::new(),
                last: None,
                timeline: Timeline::default(),
                recheck_at: i64::MAX,
                dirty: false,
                ticks: VecDeque::new(),
                day_high: None,
                member: Membership::default(),
                halted: false,
                bars: VecDeque::new(),
                first_bar: None,
                setup: BullFlag::default(),
                news_first_seen: None,
                coverage: None,
                integrity: Integrity::default(),
                quarantine_through: None,
                restored_through: None,
            }
        });
        self.by_inst.entry(sym.inst.clone()).or_insert(name);
        let exec = t.event_ts.nanos();
        // One identity rule for every path, and one place that decides it. The ledger is never
        // cleared on a date change — doing that after a date's first prints were already queued
        // is how their identities were lost, so a later redelivery of one counted twice. Entries
        // carry their own session date instead, and the bounds below retire them.
        let mut evicted_counted = 0u64;
        let mut evicted_queued = 0u64;
        while sym
            .ledger
            .front()
            .is_some_and(|a| exec - a.exec > REVISION_WINDOW_SECS * SEC)
            || sym.ledger.len() >= LEDGER_MAX
        {
            let Some(a) = sym.ledger.pop_front() else {
                break;
            };
            sym.ids.remove(&a.key());
            // Nothing executing at or before this can be proved new again.
            sym.quarantine_through = Some(
                sym.quarantine_through
                    .map_or(a.exec, |q: i64| q.max(a.exec)),
            );
            if a.queued {
                evicted_queued += 1;
            } else {
                evicted_counted += 1;
            }
        }
        if evicted_counted > 0 {
            // Past the horizon the engine can no longer prove a duplicate or place a revision for
            // those prints. The volume it already counted is unaffected, but the limit is named
            // rather than left to be discovered.
            self.revisions.identities_evicted += evicted_counted;
            if sym.coverage.is_none() {
                sym.coverage = Some((self.run.numerator_coverage, "identities_evicted"));
            }
        }
        if evicted_queued > 0 {
            // A queued print evicted is volume this engine will never count: a real gap in the
            // numerator, so the symbol's coverage says so.
            self.revisions.queued_evicted += evicted_queued;
            sym.coverage = Some((Coverage::Partial, "admission_ledger_overflow"));
        }
        // Past the horizon the engine cannot tell a new old print from a redelivery of one it
        // already counted, and counting it again would overstate the numerator — on the unloaded
        // path there is no chart engine to refuse it either. Refusing keeps the count a lower
        // bound, which is what lets an RVOL already over the threshold stay a PASS; the cost is
        // that a genuinely new print this old is not counted, and that is the safe direction.
        if sym.quarantine_through.is_some_and(|q| exec <= q) {
            self.revisions.quarantined_old += 1;
            // A refused print is a real gap in the count, which is a stronger statement than
            // "identities were retired", so it replaces that reason rather than queuing behind
            // it.
            sym.coverage = Some((Coverage::Partial, "quarantined_old_prints"));
            sym.dirty = true;
            return out;
        }
        let key = (t.id(), exec);
        if !sym.ids.insert(key) {
            self.revisions.duplicates_dropped += 1;
            return out;
        }
        sym.ledger.push_back(Admitted {
            id: t.id(),
            exec,
            admit: admit.nanos(),
            price: t.price.micros(),
            size: t.size,
            verdict,
            session: t.session.map(|s| (s.date, s.kind)),
            queued: true,
            counted: false,
            folded_at: None,
            removed: false,
        });
        out
    }

    /// Applies a correction or cancel of a print, resolved against this engine's **own**
    /// admission ledger ([`PrintRevision`]).
    ///
    /// The resolution does not ask whether a chart is loaded. The scanner's facts must not depend
    /// on that — a correction for a symbol whose history is still loading has to land too — so
    /// the engine matches the provider's reference against the prints it retained, by
    /// [`market_events::TradeRef::resolve`]'s rule: exactly one fit is applied, none or several
    /// is counted and never guessed at. A chart engine contributes only the two things it alone
    /// can recompute, the date's last price and its high, and their absence is counted.
    ///
    /// Proving the identity is the point. Subtracting a chart-resolved print without it would
    /// take volume off an unrelated print whenever the named one came from before this engine
    /// existed — a restart, a symbol still loading, a print past the ledger's horizon.
    ///
    /// A print still queued is revised **in the queue**: adjusting aggregates then would subtract
    /// volume the engine had not added yet and let the print count afterwards anyway.
    ///
    /// Forward-effective: the symbol's current facts change from the barrier this is admitted at.
    /// Decisions already emitted are never rewritten and a frozen setup level never moves. The
    /// retained-window state the next decisions are computed from — the cumulative checkpoints,
    /// the per-second price history and the setup bars — is made exact again from the ledger, or
    /// counted as unproven when the ledger no longer reaches far enough back.
    pub fn apply_revision(&mut self, rev: &PrintRevision, admit: UnixNanos) -> Vec<Decision> {
        let out = self.advance_to(admit);
        let mo = self.panel_policy.body.momentum.clone();
        let (bar_ns, recent_w) = (
            self.policy.body.grid.bar_secs * SEC,
            self.policy.body.diagnostics.recent_window_secs * SEC,
        );
        let Some(sym) = self.syms.get_mut(&rev.symbol) else {
            self.revisions.unknown_target += 1;
            return out;
        };
        // The one print the reference names, among those still standing.
        let target = TradeRef {
            id: rev.target.id.clone(),
            price: Price::from_micros(rev.target.price),
            size: rev.target.size,
        };
        let candidates: Vec<(usize, Price, u64)> = sym
            .ledger
            .iter()
            .enumerate()
            .filter(|(_, a)| a.id == target.id && !a.removed)
            .map(|(i, a)| (i, Price::from_micros(a.price), a.size))
            .collect();
        let i = match target.resolve(candidates) {
            Resolution::Found(i) => i,
            Resolution::NotFound => {
                // Never admitted here, or past the ledger's horizon. Nothing is subtracted —
                // but something the provider says is not there may still be in the count, so the
                // numerator is no longer a lower bound and the pillar must say so.
                self.revisions.unknown_target += 1;
                sym.integrity.volume_untrusted = true;
                sym.dirty = true;
                return out;
            }
            Resolution::Ambiguous => {
                self.revisions.ambiguous += 1;
                sym.integrity.volume_untrusted = true;
                sym.dirty = true;
                return out;
            }
        };
        // The date check applies to a print the aggregates already hold: those belong to
        // `sym.date`. A print still **queued** is revised whatever the symbol's date is — before
        // the first barrier of a date there is no date yet, and discarding the revision then is
        // what let a cancel lose to its own print's barrier.
        if !sym.ledger[i].queued && sym.ledger[i].session.map(|(d, _)| d) != sym.date {
            // The engine has moved on from that trading date: the facts the revision speaks
            // about are not the current ones, and the old decisions stand.
            self.revisions.too_late += 1;
            return out;
        }
        let was_volume = sym.ledger[i].volume();
        let was_counted = sym.ledger[i].counted;
        let queued = sym.ledger[i].queued;
        let folded_at = sym.ledger[i].folded_at;
        let touched_ohlc =
            sym.ledger[i].verdict.ohlc || rev.corrected.is_some_and(|c| c.verdict.ohlc);
        {
            let e = &mut sym.ledger[i];
            match rev.corrected {
                Some(c) => {
                    e.price = c.price;
                    e.size = c.size;
                    e.verdict = c.verdict;
                    e.removed = !c.verdict.volume && !c.verdict.ohlc;
                }
                None => {
                    e.removed = true;
                    e.queued = false;
                }
            }
        }
        if queued {
            // Revised before its barrier: nothing to undo, and the barrier will count whatever
            // the entry now says.
            self.revisions.applied_queued += 1;
            sym.dirty = true;
            return out;
        }
        if !was_counted {
            // The barrier skipped it (another date, an excluded session): the revision is placed
            // and changes nothing.
            self.revisions.applied += 1;
            sym.dirty = true;
            return out;
        }
        let now_volume = sym.ledger[i].volume();
        let now_price = sym.ledger[i].price;
        // Whole-day aggregates: adjusted by the delta, which the ledger proves is this print's.
        if was_volume > 0 {
            sym.cum = sym.cum.saturating_sub(was_volume);
            sym.dollars_micros = sym
                .dollars_micros
                .saturating_sub(rev.target.price as u128 * was_volume as u128);
            sym.trades = sym.trades.saturating_sub(1);
        }
        if now_volume > 0 {
            sym.cum += now_volume;
            sym.dollars_micros += now_price as u128 * now_volume as u128;
            sym.trades += 1;
        }
        // The cumulative checkpoints the recent window reads. Keyed to the barrier that folded
        // the print in, not to its execution: a print executed in one bar and admitted during a
        // later one was not in the checkpoints between the two, and moving those would have
        // invented recent volume (or clamped real volume away).
        if let Some(folded) = folded_at {
            for (at, c) in sym.cums.iter_mut() {
                if *at >= folded {
                    *c = c.saturating_sub(was_volume) + now_volume;
                }
            }
        }
        // The per-second price history and the setup bars are rebuilt from the ledger, so the
        // next decisions see the same window a stream without the revised print would have
        // produced. `rebuilt` is false when the ledger no longer reaches the window's start.
        let b = sym
            .cums
            .back()
            .map(|&(at, _)| at)
            .or(folded_at)
            .unwrap_or(0);
        let rebuilt = Self::rebuild_windows(sym, &mo, bar_ns, b);
        if !rebuilt {
            // The window the next decisions read is not provably the stream's, so anything
            // measured over a window stops being reported until the symbol's state is sound.
            self.revisions.windows_unproven += 1;
            sym.integrity.window_untrusted = true;
        }
        if touched_ohlc {
            // The date's newest OHLC price: the ledger holds the most recent prints, so its
            // maximum execution time is the date's, when it has one at all.
            match Self::last_of_date(sym) {
                Some(last) => sym.last_price = Some(last),
                // Nothing of the date is left in the ledger: fall back to the resolver's answer,
                // which is a named `(exec, price)` and not a pair that can be read either way.
                None => sym.last_price = rev.last.map(|l| (l.exec, l.price)),
            }
            // The date's **high** is a whole-day extremum, which a windowed ledger cannot prove.
            match rev.day_high {
                Some(h) if h.complete => sym.day_high = Some(h.price),
                _ => {
                    // Left where it is, and no longer reported: the distance below the high and
                    // the setup levels taken from it would be measured against a print that may
                    // be gone.
                    self.revisions.day_high_unproven += 1;
                    sym.integrity.high_untrusted = true;
                }
            }
        }
        let _ = recent_w;
        self.revisions.applied += 1;
        sym.dirty = true;
        out
    }

    /// The date's newest counted OHLC price and its execution time, from the ledger.
    fn last_of_date(sym: &Sym) -> Option<(i64, i64)> {
        sym.ledger
            .iter()
            .filter(|a| {
                a.counted && !a.removed && a.verdict.ohlc && a.session.map(|(d, _)| d) == sym.date
            })
            .max_by_key(|a| a.exec)
            .map(|a| (a.exec, a.price))
    }

    /// Rebuilds the retained-window state from the ledger: the per-second price history and the
    /// setup bars, both of which depend on individual prints rather than a total.
    ///
    /// Returns whether the ledger reached back far enough to do it exactly. It normally does —
    /// the windows are minutes wide and the ledger an hour — and when it does not the caller
    /// counts it instead of publishing a window it cannot stand behind.
    fn rebuild_windows(sym: &mut Sym, mo: &MomentumRules, bar_ns: i64, b: i64) -> bool {
        let date = sym.date;
        let ticks_from = b - (mo.context_secs + mo.endpoint_max_age_secs + 1) * SEC;
        let bars_from = b - (BARS_KEPT as i64) * bar_ns;
        let need_from = ticks_from.min(bars_from);
        let covers = sym.ledger.front().is_some_and(|a| a.exec <= need_from)
            || sym.first_bar.is_some_and(|f| f >= need_from);
        let mut ticks: VecDeque<(i64, i64)> = VecDeque::new();
        // Grouped by execution time, not arrival order: a late print arrives after a newer one,
        // and grouping in arrival order would make two bars for the same interval.
        let mut live: Vec<(i64, i64, u64, Verdict)> = Vec::new();
        for a in sym
            .ledger
            .iter()
            .filter(|a| a.counted && !a.removed && a.session.map(|(d, _)| d) == date)
        {
            if a.verdict.ohlc && a.exec >= ticks_from {
                record_tick(&mut ticks, a.exec, a.price);
            }
            if a.exec >= bars_from {
                live.push((a.exec, a.price, a.size, a.verdict));
            }
        }
        live.sort_unstable_by_key(|x| x.0);
        let mut bars: VecDeque<Bar> = VecDeque::new();
        let mut cur: Option<Bar> = None;
        for (exec, price, size, verdict) in live {
            let end = ceil_to(exec + 1, bar_ns);
            if cur.as_ref().is_some_and(|c| c.end != end)
                && let Some(c) = cur.take()
                && (c.ohlc.is_some() || c.volume > 0)
            {
                bars.push_back(c);
            }
            let c = cur.get_or_insert_with(|| Bar::new(end));
            c.add(exec, price, size, verdict.ohlc, verdict.volume);
        }
        if let Some(c) = cur
            && (c.ohlc.is_some() || c.volume > 0)
        {
            bars.push_back(c);
        }
        while bars.len() > BARS_KEPT {
            bars.pop_front();
        }
        sym.ticks = ticks;
        sym.bars = bars;
        covers
    }

    /// What the engine did with the revisions and duplicates it was given.
    pub fn revision_telemetry(&self) -> RevisionTelemetry {
        self.revisions
    }

    /// Decides every barrier `b + settle <= now` not yet decided.
    pub fn advance_to(&mut self, now: UnixNanos) -> Vec<Decision> {
        let now = now.nanos();
        let (bar, settle) = (self.bar(), self.settle());
        let mut b = *self.next_bar.get_or_insert(ceil_to(now, bar));
        // The first barrier `now` cannot decide yet.
        let undecidable = ceil_to(now - settle + 1, bar);
        let mut out = Vec::new();
        while b + settle <= now {
            // Skip barriers where nothing can change (no due print, flip or evidence): the
            // decisions are the same as deciding each of them, at a fraction of the cost.
            let due = self.next_due(b, bar, settle);
            if due > b {
                b = due.min(undecidable);
                continue;
            }
            self.decide(b, b + settle, &mut out);
            b += bar;
        }
        self.next_bar = Some(b);
        out
    }

    /// The earliest barrier at or after `b` where some symbol needs a decision (`i64::MAX` when
    /// none), aligned to the grid.
    fn next_due(&self, b: i64, bar: i64, settle: i64) -> i64 {
        let mut due = i64::MAX;
        for s in self.syms.values() {
            // An active member's momentum window slides every barrier, print or not.
            if s.dirty || s.member.active().is_some() {
                return b;
            }
            for a in s.ledger.iter().filter(|a| a.queued) {
                // Drained at the first barrier past its execution that it was admitted by.
                let by_exec = ceil_to(a.exec + 1, bar);
                let by_admit = ceil_to(a.admit - settle, bar);
                due = due.min(by_exec.max(by_admit));
            }
            if s.recheck_at != i64::MAX {
                // A flip at time t shows at the first barrier whose bar end is past it.
                due = due.min(ceil_to(s.recheck_at, bar));
            }
        }
        due.max(b)
    }

    fn decide(&mut self, b: i64, tau: i64, out: &mut Vec<Decision>) {
        let date = trading_date(UnixNanos(b));
        let names: Vec<String> = self.syms.keys().cloned().collect();
        for name in names {
            let due = {
                let s = &self.syms[&name];
                s.dirty
                    || s.date != Some(date)
                    || b >= s.recheck_at
                    || s.ledger
                        .iter()
                        .any(|a| a.queued && a.exec < b && a.admit <= tau)
            };
            if due && let Some(d) = self.evaluate(&name, date, b, tau) {
                out.push(d);
            }
        }
        self.publish_panel(b, tau);
    }

    /// Ranks the active members at barrier `b` and logs the panel when it changed.
    fn publish_panel(&mut self, b: i64, tau: i64) {
        let rules = &self.panel_policy.body.momentum;
        let date = trading_date(UnixNanos(b));
        let bar_secs = self.policy.body.grid.bar_secs;
        let mut rows: Vec<PanelRow> = Vec::new();
        for s in self.syms.values_mut() {
            if s.member.active().is_none() || s.date != Some(date) {
                // Only members are monitored; a member that left starts from `watching` again.
                if !s.setup.is_idle() {
                    s.setup.leave();
                }
                continue;
            }
            // The setup advances at every barrier while the stock is a member, print or not —
            // unless its own inputs are not reportable. With the bars or the day high
            // unprovable the family is left to re-arm and is given no bars for this barrier, so
            // it claims nothing rather than claiming a flag off a window it cannot stand behind.
            let untrusted = s.integrity.window_untrusted
                || s.integrity.high_untrusted
                || s.integrity.volume_untrusted;
            if untrusted && !s.setup.is_idle() {
                s.setup.leave();
            }
            let no_bars: VecDeque<Bar> = VecDeque::new();
            let bars: &VecDeque<Bar> = if untrusted { &no_bars } else { &s.bars };
            let prefix = format!("{}:{}:{}", self.run.run_id, date, s.name);
            let mut setup = s.setup.step(
                &self.setup_policy,
                bar_secs,
                &Step {
                    bar_end: b,
                    cutoff: tau,
                    bar_ns: bar_secs * SEC,
                    bars,
                    last_price: s.last_price,
                    halted: s.halted,
                    day_volume: s.cum,
                    day_slots: s
                        .first_bar
                        .filter(|&f| f <= b)
                        .map_or(0, |f| ((b - f) / (bar_secs * SEC) + 1) as u64),
                    symbol: &s.name,
                    prefix: &prefix,
                },
                &mut self.setup_log,
            );
            if untrusted {
                setup.reason = "inputs_untrusted";
            }
            let ticks = &s.ticks;
            let price_at = |e: i64| ticks.iter().rev().find(|(x, _)| *x < e).copied();
            let segment = |t: i64| {
                classify(UnixNanos(t))
                    .filter(|x| x.kind != SessionKind::Overnight)
                    .map(|x| x.kind as u8)
            };
            let m = momentum(
                rules,
                b,
                price_at,
                segment,
                // An unprovable high is no high: the distance below it is not reported.
                s.day_high.filter(|_| !s.integrity.high_untrusted),
                s.halted,
                s.coverage.map_or(self.run.numerator_coverage, |(c, _)| c),
            );
            let last = s.last.as_ref();
            let pillar = |p: PillarId| last.and_then(|d| d.pillars.iter().find(|x| x.pillar == p));
            rows.push(PanelRow {
                symbol: s.name.clone(),
                instrument: s.inst.clone(),
                membership: s.member.clone(),
                counts: last.map(|d| d.counts).unwrap_or_default(),
                price: pillar(PillarId::Price).and_then(|p| p.value),
                gain_bp: pillar(PillarId::Gain).and_then(|p| p.value),
                move_1m_bp: m.r_primary_bp,
                from_high_bp: m.below_high_bp,
                momentum: m,
                rank: None,
                prev_rank: None,
                rvol_milli: pillar(PillarId::Rvol).and_then(|p| p.value),
                float_shares: pillar(PillarId::Float)
                    .and_then(|p| p.value)
                    .map(|v| v as u64),
                news_first_seen: s.news_first_seen,
                missing: s
                    .member
                    .nonpassing
                    .iter()
                    .map(|&(pillar, state, reason)| MissingPillar {
                        pillar,
                        state,
                        reason,
                    })
                    .collect(),
                spark: spark(&s.bars, SPARK_BARS),
                setup,
            });
        }
        let attention = rows
            .iter()
            .filter(|r| {
                matches!(
                    r.setup.state,
                    SetupState::Near | SetupState::Crossing | SetupState::Triggered
                )
            })
            .count() as u32;
        // Ranked rows first, in momentum order; unranked after, by symbol.
        rows.sort_by(|a, b| match (a.momentum.status, b.momentum.status) {
            (MomentumStatus::Ready, MomentumStatus::Ready) => {
                momentum_order((&a.momentum, &a.symbol), (&b.momentum, &b.symbol))
            }
            (MomentumStatus::Ready, _) => std::cmp::Ordering::Less,
            (_, MomentumStatus::Ready) => std::cmp::Ordering::Greater,
            _ => a.symbol.cmp(&b.symbol),
        });
        let prev = self.last_panel.as_ref();
        let mut k = 0u32;
        for r in &mut rows {
            if r.momentum.status == MomentumStatus::Ready {
                k += 1;
                r.rank = Some(k);
            }
            r.prev_rank = prev
                .and_then(|p| p.rows.iter().find(|x| x.symbol == r.symbol))
                .and_then(|x| x.rank);
        }
        let changed = match prev {
            None => !rows.is_empty(),
            Some(p) => p.rows != rows,
        };
        if !changed {
            return;
        }
        let snap = PanelSnapshot {
            revision: self.seq,
            run_id: self.run.run_id.clone(),
            source: self.run.source.clone(),
            panel_policy: self.panel_policy.tag(),
            pillar_policy: self.policy.tag(),
            bar_end: b,
            cutoff: tau,
            setup_policy: self.setup_policy.tag(),
            attention,
            rows,
        };
        self.panel_log.push(snap.clone());
        self.last_panel = Some(snap);
    }

    fn evaluate(&mut self, name: &str, date: NaiveDate, b: i64, tau: i64) -> Option<Decision> {
        let policy = &self.policy;
        let ps = &policy.body.price_staleness;
        let w = policy.body.diagnostics.recent_window_secs * SEC;
        let s = self.syms.get_mut(name).expect("known symbol");
        s.dirty = false;
        if s.date != Some(date) {
            // A new trading date: its own volume window (04:00 →), price and first-seen times.
            s.date = Some(date);
            s.active = false;
            s.cum = 0;
            s.dollars_micros = 0;
            s.trades = 0;
            s.last_price = None;
            s.cums.clear();
            s.timeline = Timeline::default();
            s.last = None;
            // A new trading date needs a new admission; the old date stays in the decision log.
            s.ticks.clear();
            s.day_high = None;
            s.member = Membership::default();
            s.bars.clear();
            s.first_bar = None;
            s.setup = BullFlag::default();
            s.news_first_seen = None;
            // The ledger is deliberately **not** cleared here. Clearing it after a date's first
            // prints had already been queued is what lost their identities, so a later
            // redelivery of one counted twice; its entries carry their own session date and the
            // bounds in `apply` retire them.
            //
            // The doubts are a property of the date's facts, which have just been reset.
            s.integrity = Integrity::default();
        }
        let exclude_overnight = policy.body.volume.exclude_overnight_boats;
        let bar_ns = policy.body.grid.bar_secs * SEC;
        // The setup bar completing at `b`; a late print (executed in an earlier, already decided
        // bar) never edits a completed bar and stays out of the setup bars.
        let mut cur = Bar::new(b);
        // Fold in the ledger entries this barrier may count, marking them in place rather than
        // taking them out: the ledger stays the one record of what was admitted, so a revision
        // arriving later can still find them (and tell a counted print from a queued one).
        for a in s.ledger.iter_mut() {
            if !(a.queued && a.exec < b && a.admit <= tau) {
                continue;
            }
            a.queued = false;
            a.folded_at = Some(b);
            let Some((d, kind)) = a.session else { continue };
            if d != date || (exclude_overnight && kind == SessionKind::Overnight) {
                // Skipped, and recorded as skipped: neither queued nor counted.
                continue;
            }
            a.counted = true;
            if a.removed {
                // Cancelled before its barrier: the barrier counts nothing for it.
                continue;
            }
            if a.exec >= b - bar_ns {
                cur.add(a.exec, a.price, a.size, a.verdict.ohlc, a.verdict.volume);
            }
            if a.verdict.volume {
                if s.first_bar.is_none() {
                    s.first_bar = Some(ceil_to(a.exec + 1, bar_ns));
                }
                s.cum += a.size;
                s.dollars_micros += a.price as u128 * a.size as u128;
                s.trades += 1;
                s.active = true;
            }
            if a.verdict.ohlc {
                s.active = true;
                if s.last_price.is_none_or(|(e, _)| a.exec >= e) {
                    s.last_price = Some((a.exec, a.price));
                }
                s.day_high = Some(s.day_high.map_or(a.price, |h| h.max(a.price)));
                record_tick(&mut s.ticks, a.exec, a.price);
            }
        }
        if cur.ohlc.is_some() || cur.volume > 0 {
            s.bars.push_back(cur);
            if s.bars.len() > BARS_KEPT {
                s.bars.pop_front();
            }
        }
        s.cums.push_back((b, s.cum));
        // The momentum endpoints reach back `context + endpoint age`; keep one print before that.
        let mo = &self.panel_policy.body.momentum;
        let horizon = b - (mo.context_secs + mo.endpoint_max_age_secs + 1) * SEC;
        while s.ticks.len() > 1 && s.ticks[1].0 < horizon {
            s.ticks.pop_front();
        }
        while s.cums.len() > 2 && s.cums[1].0 <= b - w {
            s.cums.pop_front();
        }
        if !s.active {
            s.recheck_at = i64::MAX;
            return None;
        }

        // Price: the latest OHLC print before the bar end, stale past the session's limit.
        let regular = classify(UnixNanos(b)).is_some_and(|x| x.kind == SessionKind::Regular);
        let limit = if regular {
            ps.regular_secs
        } else {
            ps.extended_secs
        } * SEC;
        let stale = s.last_price.is_some_and(|(e, _)| b - e > limit);
        let price = PriceView {
            micros: s.last_price.map(|(_, p)| p),
            stale,
        };
        let prof = &policy.body.reference_profile;
        let price_r = price_pillar(
            price.micros,
            stale,
            prof.price_min_micros,
            prof.price_max_micros,
        )
        .as_of(s.last_price.map(|(e, _)| e));
        let band = &policy.body.discovery_band;
        let in_band = match price.micros {
            Some(p) if !stale => Some(p >= band.min_micros && p <= band.max_micros),
            _ => None,
        };

        // Volume endpoints are the bar end; the recent window needs a known cumulative at b - w.
        let day_start = et(date, 4, 0, 0, 0).map(|t| t.nanos()).unwrap_or(b);
        let recent = if b - w <= day_start {
            Some(s.cum)
        } else {
            s.cums
                .iter()
                .rev()
                .find(|(t, _)| *t <= b - w)
                .map(|(_, c)| s.cum - c)
        };
        let coverage = s.coverage.map_or(self.run.numerator_coverage, |(c, _)| c);
        // A window reaching back into a restored prefix is measured against checkpoints taken on
        // the recording's delivery times rather than this run's own admissions, so its endpoints
        // are approximate and it is not reportable. The day's total is unaffected — the prints
        // are the prints — and the doubt clears by itself as the window moves past the prefix.
        let mut integrity = s.integrity;
        if s.restored_through.is_some_and(|r| b - w <= r) {
            integrity.window_untrusted = true;
        }
        let vol = VolumeView {
            cum: s.cum,
            recent,
            offset: (b - day_start) / SEC,
            coverage,
            integrity,
        };

        let ev = &self.evidence;
        let g = evaluate::gain(policy, ev, &s.inst, date, tau, price);
        let (rvol_r, rvol_d) = evaluate::rvol(policy, ev, &s.inst, date, tau, vol);
        let float_r = evaluate::float(policy, ev, &s.inst, tau);
        let (news_r, news_s) = evaluate::news(policy, ev, &s.inst, tau);
        s.news_first_seen = ev
            .news_current(&s.inst, tau)
            .into_iter()
            .filter(|n| news_s.items.contains(&format!("{}@{}", n.id, n.version)))
            .map(|n| n.observed_at)
            .min();
        let pillars = vec![price_r, g.pillar, rvol_r, float_r, news_r];
        let (qualification, counts) = aggregate(pillars.iter().map(|p| p.state));

        // Ross Panel membership: platform eligibility is a fresh price inside the platform band
        // in a supported (non-overnight) session; it is not a pillar.
        let platform = &self.panel_policy.body.membership.platform_band;
        let supported = classify(UnixNanos(b)).is_some_and(|x| x.kind != SessionKind::Overnight);
        let eligibility = match price.micros {
            Some(p) if !stale && supported => {
                if p >= platform.min_micros && p <= platform.max_micros {
                    Eligibility::Eligible
                } else {
                    Eligibility::OutOfBand
                }
            }
            _ => Eligibility::Ineligible,
        };
        let decision_id = format!("{}:{}:{}", self.run.run_id, s.name, b);
        let membership_changed = update_membership(
            &mut s.member,
            &self.panel_policy.body.membership,
            &pillars,
            counts,
            eligibility,
            tau,
            &decision_id,
            &format!("{}:{}:{}", self.run.run_id, date, s.name),
        );

        // When could this symbol's result change with no new print? Stale price, stale float,
        // news ageing out, evidence becoming visible, or the next trading date (20:00 ET).
        let mut recheck = i64::MAX;
        if let Some((e, _)) = s.last_price {
            for l in [ps.regular_secs, ps.extended_secs] {
                let t = e + l * SEC + 1;
                if t > b {
                    recheck = recheck.min(t);
                }
            }
        }
        if let Some(f) = ev.float(&s.inst, tau) {
            recheck = recheck.min(f.observed_at + policy.body.float.stale_after_secs * SEC + 1);
        }
        // A corporate-action assumption expires on the clock, with no new print and no new
        // record, so the barrier it expires at has to be due by itself
        // (`ActionsEvidence::valid_until`). Past the horizon there is nothing left to recheck
        // until evidence arrives, which marks the symbol dirty anyway.
        for until in [
            ev.prev_close(&s.inst, tau)
                .and_then(|r| r.actions.valid_until),
            ev.daily(&s.inst, tau).and_then(|r| r.basis.valid_until),
            ev.intraday(&s.inst, tau).and_then(|r| r.basis.valid_until),
        ]
        .into_iter()
        .flatten()
        {
            if until + 1 > b {
                recheck = recheck.min(until + 1);
            }
        }
        let fresh = prof.news_freshness_secs * SEC;
        for n in ev.news(&s.inst, tau) {
            if let Some(p) = n.published_at
                && p + fresh >= tau
            {
                recheck = recheck.min(p + fresh + 1);
            }
        }
        // A coverage record turns a missing story into FAIL once its start is a full freshness
        // window back, and stops doing so once the cutoff passes its end (plus the policy's lag).
        if let Some(c) = ev.coverage(&s.inst, tau) {
            let lag = policy.body.news.coverage_max_lag_secs * SEC;
            for t in [c.covered_from + fresh, c.covered_through + lag + 1] {
                if t > tau {
                    recheck = recheck.min(t);
                }
            }
        }
        if let Some(t) = ev.next_after(&s.inst, tau) {
            recheck = recheck.min(t);
        }
        let rollover = [date, date + Days::new(1)]
            .into_iter()
            .filter_map(|d| et(d, 20, 0, 0, 0))
            .map(|t| t.nanos())
            .find(|&t| t > b);
        if let Some(t) = rollover {
            recheck = recheck.min(t);
        }
        s.recheck_at = recheck;

        // Timeline: the cutoff is when a result became known, never the bar end.
        let tl = &mut s.timeline;
        if in_band == Some(true) && tl.first_eligible.is_none() {
            tl.first_eligible = Some(tau);
        }
        for p in &pillars {
            if p.state == Tri::Pass {
                tl.first_pass.entry(p.pillar).or_insert(tau);
            }
        }
        match (qualification == Qualification::Qualified, tl.qualified_now) {
            (true, false) => {
                if tl.first_qualified.is_none() {
                    tl.first_qualified = Some(tau);
                } else {
                    tl.requalified += 1;
                }
                tl.qualified_now = true;
                tl.qualified_earlier = true;
            }
            (false, true) => {
                tl.qualified_now = false;
                tl.lost_at = Some(tau);
            }
            _ => {}
        }

        let volume = VolumeFacts {
            shares: s.cum,
            dollars: (s.dollars_micros / 1_000_000) as u64,
            trades: s.trades,
            coverage,
            coverage_reason: s.coverage.map(|(_, why)| why),
            integrity,
        };
        let mut changed = Vec::new();
        match &s.last {
            None => changed.push("new"),
            Some(prev) => {
                for (a, p) in prev.pillars.iter().zip(&pillars) {
                    if a != p {
                        changed.push(match p.pillar {
                            PillarId::Price => "pillars.price",
                            PillarId::Gain => "pillars.gain",
                            PillarId::Rvol => "pillars.rvol",
                            PillarId::Float => "pillars.float",
                            PillarId::News => "pillars.news",
                        });
                    }
                }
                if prev.qualification != qualification {
                    changed.push("qualification");
                }
                if prev.in_discovery_band != in_band {
                    changed.push("discovery_band");
                }
                if prev.volume != volume {
                    changed.push("volume");
                }
                if prev.rvol != rvol_d {
                    changed.push("rvol_diagnostics");
                }
                if prev.news != news_s {
                    changed.push("news");
                }
                if prev.gain_vs_stored_close_bp != g.vs_stored_close_bp {
                    changed.push("gain_diagnostic");
                }
            }
        }
        if membership_changed && !changed.contains(&"new") {
            changed.push("membership");
        }
        if changed.is_empty() {
            return None;
        }
        self.seq += 1;
        let d = Decision {
            seq: self.seq,
            decision_id,
            run_id: self.run.run_id.clone(),
            source: self.run.source.clone(),
            fidelity: self.run.fidelity,
            policy: policy.tag(),
            symbol: s.name.clone(),
            instrument: s.inst.clone(),
            trading_date: date,
            bar_end: b,
            cutoff: tau,
            pillars,
            qualification,
            counts,
            in_discovery_band: in_band,
            gain_vs_stored_close_bp: g.vs_stored_close_bp,
            volume,
            rvol: rvol_d,
            news: news_s,
            timeline: s.timeline.clone(),
            membership: s.member.clone(),
            changed,
        };
        s.last = Some(d.clone());
        Some(d)
    }

    /// The latest decision per symbol (what a snapshot sends), in symbol order.
    pub fn current(&self) -> impl Iterator<Item = &Decision> {
        self.syms.values().filter_map(|s| s.last.as_ref())
    }

    /// The last decision sequence number emitted.
    pub fn seq(&self) -> u64 {
        self.seq
    }
}
