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

use crate::cue::RossCue;
use crate::detect::{
    self, DetectionPolicy, DetectionSnapshot, DetectionTelemetry, DetectionTransition, Detector,
    QuietReference, Scope, SymDetect,
};
use crate::evaluate::{
    self, Coverage, Integrity, NewsSummary, PriceView, RvolDiagnostics, VolumeView,
};
use crate::evidence::{
    DailyBaseline, EvidenceStore, FloatObservation, InstrumentId, IntradayBaseline, NewsCoverage,
    NewsItem, OutstandingObservation, PrevClose,
};
use crate::output::{BoundedLog, OutputTelemetry, PANEL_LOG_MAX};
use crate::panel::{
    Eligibility, Membership, Momentum, MomentumRules, MomentumStatus, PanelPolicy, momentum,
    momentum_order, update_membership,
};
use crate::pillar::{Counts, PillarId, PillarResult, Qualification, Tri, aggregate, price_pillar};
use crate::policy::Policy;
use crate::revision::{PrintRevision, RevisionTelemetry};
use crate::setup::{BARS_KEPT, Bar, spark};
use chrono::{Days, NaiveDate};
use market_events::session::{SessionKind, classify, et, trading_date};
use market_events::{Price, Resolution, Trade, TradeId, TradeRef, UnixNanos};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

const SEC: i64 = 1_000_000_000;
const MS: i64 = 1_000_000;

/// Execution time an admission ledger retains, beside [`LEDGER_MAX`].
///
/// The same as [`crate::revision::REVISION_WINDOW_SECS`], and twenty minutes rather than the
/// hour it was, on purpose. The engine sees every print of the market,
/// so the ledgers' peak is the busiest stretch of this window summed over every symbol: measured
/// on a synthetic whole-market hour skewed like a real day (`tests/ledger_memory.rs`), an hour's
/// window held 6.5M entries for an 8M-print hour, ~1.9 GB at the old 285 B/entry. Twenty minutes
/// still covers what a revision rebuild must reach back to (64 setup bars of 15 s = 16 min,
/// [`crate::setup::BARS_KEPT`]; the momentum context is 5 min), a reconnect redelivery (seconds)
/// and a correction or cancel arriving behind its print (seconds to minutes). A revision reaching
/// further back is counted unplaced and bounded ([`Sym::unplaced`]), not trusted blindly, and a
/// print older than the retained horizon is refused as before.
pub const LEDGER_WINDOW_SECS: i64 = crate::revision::REVISION_WINDOW_SECS;

/// A print's provider identity ([`TradeId`]) packed into two words, for the ledger.
///
/// The ledger and its identity set held the `TradeId` twice per entry, each with its own heap
/// `String` for an exchange code that is one letter on SIP. Packed: the source in the low byte
/// and the exchange's bytes in the other seven, exactly, for any code up to seven bytes (every
/// SIP and fixture code). A longer code is replaced by a 56-bit hash of it with the top bit set,
/// so it cannot equal any packed ASCII code; two long codes could collide only by hash, and no
/// provider this app reads has one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct LedgerId {
    tag: u64,
    seq: u64,
}

/// [`Sym::ids`]' key for one print.
fn identity_key(id: LedgerId, exec: i64) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (id.tag, id.seq, exec).hash(&mut h);
    h.finish()
}

impl LedgerId {
    fn of(id: &TradeId) -> LedgerId {
        let src: u64 = match id.source {
            market_events::Source::AlpacaSip => 1,
            market_events::Source::AlpacaBoats => 2,
            market_events::Source::AlpacaIex => 3,
            market_events::Source::Fixture => 4,
        };
        let x = id.exchange.as_bytes();
        let body = if x.len() <= 7 && !x.contains(&0) {
            x.iter()
                .enumerate()
                .fold(0u64, |acc, (i, b)| acc | (u64::from(*b) << (8 * i)))
        } else {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            id.exchange.hash(&mut h);
            (h.finish() >> 8) | (1 << 55)
        };
        LedgerId {
            tag: src | (body << 8),
            seq: id.seq,
        }
    }
}

/// Hard bound on one symbol's admission ledger, beside the time bound.
///
/// Two bounds, not one: a reconnect replay's timing is not something to assume, so the time
/// window alone could hold an unbounded number of entries for a symbol printing fast enough.
/// Whichever bites first, an eviction is counted and a **queued** entry evicted degrades the
/// symbol's coverage, because that is volume the engine will never count.
pub const LEDGER_MAX: usize = 20_000;

/// Symbols whose ledgers one clock call sweeps (`Engine::sweep_some`): ~13,700 symbols are
/// covered in about a minute at the market's one or two clock calls a second.
const SWEEP_PER_CALL: usize = 256;

/// [`LEDGER_MAX`] for a symbol printing far outside the discovery band ([`Engine::apply`]).
pub const LEDGER_MAX_OUTSIDE_BAND: usize = 1_000;

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
    /// Price fresh and inside the policy's discovery band (current default $0.10–$25).
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
    id: LedgerId,
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
    /// The identity-set key: a 64-bit hash of (identity, execution time). See [`Sym::ids`].
    fn key(&self) -> u64 {
        identity_key(self.id, self.exec)
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
    /// order ([`Admitted`]). Bounded twice — by execution time ([`LEDGER_WINDOW_SECS`]) and by a
    /// hard entry count ([`LEDGER_MAX`]) — because a reconnect replay's timing is not something
    /// to assume. The time bound is enforced on the symbol's own prints **and** by the clock
    /// ([`Engine::advance_to`] sweeps every ledger once a minute), so a symbol that stops
    /// trading releases its ledger instead of holding it overnight.
    ledger: VecDeque<Admitted>,
    /// The ledger's identities, for an O(1) duplicate check: 64-bit hashes of (identity,
    /// execution time) rather than the pairs, a third of the bytes per entry. A collision can
    /// only make a new print look like a redelivery and be dropped — an undercount, the safe
    /// direction for a lower bound — at odds of about one in 2⁶⁴ ÷ the ledger's length.
    ids: HashSet<u64>,
    /// Ledger entries still `queued`. They sit at the back (a barrier folds by admission time),
    /// so the per-barrier scans walk back from the end until they have seen this many, instead of
    /// the whole ledger of every symbol — which at a whole-market open is millions of entries
    /// under the core lock every 15 s.
    queued_n: usize,
    /// Shares the count may overstate: the sizes of corrections and cancels this engine could not
    /// place against exactly one admission ([`Integrity::volume_untrusted`]). `cum - unplaced` is
    /// still a lower bound, so an RVOL that clears the threshold on it passes. Reset with the
    /// date's facts.
    unplaced: u64,
    /// The trading date `coverage` is a statement about. A short recording, a refused restore or
    /// an overflowed ledger describes that date's prints, not the next one's, so a new date
    /// starts without it ([`Engine::evaluate`]).
    coverage_for: Option<NaiveDate>,
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
    /// First time StockWatcher saw any of the qualifying news at the latest decision.
    news_first_seen: Option<i64>,
    /// That earliest qualifying story's headline class (`NewsItem::event_type`: `financing`,
    /// `regulatory_decision`, …), so a row can mark dilution news without the headline text.
    news_kind: Option<String>,
    /// That story's source, form, publication (an SEC filing's acceptance) time and URL.
    news_evidence: Option<NewsEvidence>,
    /// The latest SEC offering filing visible at the latest decision ([`Offering`]).
    offering: Option<Offering>,
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
    /// Set when the restore could not prove it replayed the same prints in the same order the run
    /// admitted them — no admission journal for the day, or an incomplete one
    /// (`market_archive::admissions`). The day's totals are then still a sound lower bound, but
    /// which **barrier** each print folded into is not established, so a recent window reaching
    /// back into the prefix is not reportable ([`Integrity::window_untrusted`]). The doubt clears
    /// by itself once the window has moved past the prefix, within the window's own length.
    restored_through: Option<i64>,
    /// The detection families' state (`crate::detect`): every family with the Detection Panel
    /// on, the micro-pullback for a Ross member otherwise. The Ross row's cue reads it.
    det: SymDetect,
    /// The day's history is still being restored by a loader ([`Engine::set_history_pending`]):
    /// a temporary observation guard, never part of `integrity`.
    history_pending: bool,
}

impl Sym {
    fn new(name: &str) -> Sym {
        Sym {
            name: name.to_string(),
            inst: InstrumentId::symbol(name),
            date: None,
            ledger: VecDeque::new(),
            ids: HashSet::new(),
            queued_n: 0,
            unplaced: 0,
            coverage_for: None,
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
            news_first_seen: None,
            news_kind: None,
            news_evidence: None,
            offering: None,
            coverage: None,
            integrity: Integrity::default(),
            quarantine_through: None,
            restored_through: None,
            det: SymDetect::default(),
            history_pending: false,
        }
    }
}

impl Sym {
    /// Index of the oldest `queued` ledger entry (the ledger's length when none is): every entry
    /// before it is folded or skipped. Walks back from the end, which is where queued entries
    /// are, so a barrier costs the queued tail rather than the whole ledger.
    fn queued_start(&self) -> usize {
        let mut left = self.queued_n;
        let mut i = self.ledger.len();
        while left > 0 && i > 0 {
            i -= 1;
            if self.ledger[i].queued {
                left -= 1;
            }
        }
        debug_assert_eq!(
            left, 0,
            "queued_n ({}) exceeds the queued entries in the ledger",
            self.queued_n
        );
        debug_assert!(
            self.ledger.range(..i).all(|a| !a.queued),
            "a queued entry before the counted tail"
        );
        i
    }
}

/// Retires the front of a symbol's ledger: entries executed before `keep_from`, and any beyond
/// `cap` entries. With `folded_only` (the clock's sweep, and the short bound for a symbol far
/// outside the band) it stops at the first queued entry, which is still waiting for its barrier
/// and is not theirs to drop.
///
/// Every retired identity moves the quarantine horizon: nothing executed at or before it can be
/// proved new again. A retired queued entry is volume this engine will never count.
fn retire(
    sym: &mut Sym,
    revisions: &mut RevisionTelemetry,
    run: Coverage,
    keep_from: i64,
    cap: usize,
    folded_only: bool,
) {
    let mut evicted_counted = 0u64;
    let mut evicted_queued = 0u64;
    let mut last_date = None;
    while let Some(a) = sym.ledger.front() {
        if !(a.exec < keep_from || sym.ledger.len() > cap) || (folded_only && a.queued) {
            break;
        }
        let a = sym.ledger.pop_front().expect("just looked");
        sym.ids.remove(&a.key());
        sym.quarantine_through = Some(sym.quarantine_through.map_or(a.exec, |q| q.max(a.exec)));
        last_date = a.session.map(|(d, _)| d).or(last_date);
        if a.queued {
            sym.queued_n -= 1;
            evicted_queued += 1;
        } else {
            evicted_counted += 1;
        }
    }
    if evicted_counted > 0 {
        // Past the horizon the engine can no longer prove a duplicate or place a revision for
        // those prints. The volume it already counted is unaffected, but the limit is named
        // rather than left to be discovered.
        revisions.identities_evicted += evicted_counted;
        if sym.coverage.is_none() {
            sym.coverage = Some((run, "identities_evicted"));
            sym.coverage_for = last_date;
        }
    }
    if evicted_queued > 0 {
        // A queued print evicted is volume this engine will never count: a real gap in the
        // numerator, so the symbol's coverage says so.
        revisions.queued_evicted += evicted_queued;
        sym.coverage = Some((Coverage::Partial, "admission_ledger_overflow"));
        sym.coverage_for = last_date;
    }
    if sym.ledger.is_empty() {
        // An idle symbol gives its allocations back: a VecDeque and a HashSet keep their
        // capacity through `pop_front`, so without this a morning's runner held its peak
        // ledger all night.
        sym.ledger = VecDeque::new();
        sym.ids = HashSet::new();
    } else if folded_only && sym.ledger.capacity() > 4 * sym.ledger.len() + 64 {
        sym.ledger.shrink_to_fit();
        sym.ids.shrink_to_fit();
    }
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

/// The news item behind a passing news pillar, as a row shows it: which source passed it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NewsEvidence {
    /// `benzinga` (an Alpaca story) or `sec_edgar` (an issuer's filing); another provider's name
    /// as given.
    pub source: String,
    /// The SEC form type (`8-K`, `6-K`, `8-K/A`); `None` for a story.
    pub form: Option<String>,
    /// A story's creation time, a filing's EDGAR acceptance time, a wire's publication, a halt's
    /// start (UTC ns).
    pub published_at: Option<i64>,
    pub url: Option<String>,
    /// The sub-source of a `press_wire` item (`globenewswire`) or a halt's code (`T1`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    /// Display only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headline: Option<String>,
    /// Exchange-issued "news pending" (a T1 halt), not a headline: shown apart from one.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub pending: bool,
    /// The T1 halt's lifecycle (halt time, code, resume), for a `halt_t1` item.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub halt: Option<crate::evidence::HaltFacts>,
}

impl NewsEvidence {
    fn of(n: &NewsItem) -> NewsEvidence {
        let (provider, rest) = n.source.split_once('/').unwrap_or((n.source.as_str(), ""));
        let alpaca = provider == crate::policy::NEWS_PROVIDER_ALPACA;
        let edgar = provider == crate::policy::NEWS_PROVIDER_SEC_EDGAR;
        NewsEvidence {
            source: if alpaca && !rest.is_empty() {
                rest.to_string()
            } else {
                provider.to_string()
            },
            form: (edgar && !rest.is_empty()).then(|| rest.to_string()),
            published_at: n.published_at,
            url: n.url.clone(),
            channel: (!alpaca && !edgar && !rest.is_empty()).then(|| rest.to_string()),
            headline: n.headline.clone(),
            pending: n.event_type == crate::evidence::PENDING_EVENT,
            halt: n.halt.clone(),
        }
    }
}

/// An SEC offering filing (registration statement or prospectus: S-1, S-3, F-1, F-3, 424B*) on
/// a symbol within the news freshness window. Dilution, not a catalyst: it never passes the news
/// pillar (`market_archive::evidence_load::filing_items` marks it not substantive); a row flags
/// it on its own ("Offering filed").
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Offering {
    pub form: String,
    /// EDGAR acceptance time (UTC ns), when the record has one.
    pub published_at: Option<i64>,
    pub url: Option<String>,
}

/// `NewsItem::event_type` of an SEC offering filing.
pub const OFFERING_EVENT: &str = "offering_filed";

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
    /// That float record's `observed_at` (ns), so a row can say how old the figure is (pillars-v4
    /// keeps one for 4 days). Under pillars-v5's fallback, the outstanding figure's.
    pub float_as_of: Option<i64>,
    /// The shares outstanding the float pillar passed on (`below_max_via_outstanding`,
    /// pillars-v5) when no usable float was visible: an upper bound on the float, never the float
    /// itself, so it is not put in `float_shares`. Absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outstanding_shares: Option<u64>,
    /// When StockWatcher first saw a currently qualifying news item (its `observed_at`).
    pub news_first_seen: Option<i64>,
    /// That item's headline class (`NewsItem::event_type`), so a row can mark dilution news.
    pub news_kind: Option<String>,
    /// That item's source (pillars-v6: Benzinga or SEC EDGAR), form, time and link.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub news_evidence: Option<NewsEvidence>,
    /// An SEC offering filing within the news freshness window ([`Offering`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offering: Option<Offering>,
    /// The panel is news-optional (`ross-panel-v2`) and this member's news pillar is not
    /// passing: it was admitted or kept on the other four, so its catalyst is unconfirmed.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub news_unconfirmed: bool,
    /// The pillars not passing at the latest decision.
    pub missing: Vec<MissingPillar>,
    /// Closes (micros) of the last 20 completed 15 s setup bars that had an OHLC print, oldest
    /// first; fewer when not available, never padded or interpolated.
    pub spark: Vec<i64>,
    /// The pullback cue, read from the engine's `micro_pullback` episodes ([`crate::cue`],
    /// EXPERIMENTAL, descriptive: a stage, not an entry signal).
    pub setup: RossCue,
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
    /// The panel policy counts membership without the news pillar (`ross-panel-v2`).
    pub news_optional: bool,
    pub pillar_policy: String,
    pub bar_end: i64,
    pub cutoff: i64,
    /// The detection policy the cues come from (`detection-v2@<hash>`).
    pub setup_policy: String,
    /// Members whose cue is `near` or `crossing` ([`RossCue::attention`]).
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
    revisions: RevisionTelemetry,
    restores: RestoreTelemetry,
    /// The last symbol the ledger sweep reached (`sweep_some`); `None`: start from the first.
    sweep_cursor: Option<String>,
    /// The detection families (`crate::detect`). Always present: [`Scope::RossMembers`] runs the
    /// micro-pullback for Ross members only (the cues), [`Scope::All`] is the Detection Panel.
    detector: Detector,
    /// Symbols the micro-pullback also runs for under [`Scope::RossMembers`], whatever their
    /// membership: the server's Runners rows ([`Engine::set_watched`]).
    watched: BTreeSet<String>,
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

fn max_opt<T: Ord>(a: Option<T>, b: Option<T>) -> Option<T> {
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
    /// The evidence held (read-only): e.g. the latest float for the ticker search's Float chips.
    pub fn evidence(&self) -> &EvidenceStore {
        &self.evidence
    }

    /// An engine with the Detection Panel off. The Ross rows' cues still come from the product
    /// detection policy ([`DetectionPolicy::current`]), run for the panel's members only.
    pub fn new(policy: Policy, run: RunInfo) -> Self {
        let g = &policy.body.grid;
        let detector = Detector::new(
            DetectionPolicy::current(),
            g.bar_secs,
            g.settle_ms,
            Scope::RossMembers,
        );
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
            revisions: RevisionTelemetry::default(),
            restores: RestoreTelemetry::default(),
            sweep_cursor: None,
            detector,
            watched: BTreeSet::new(),
        }
    }

    /// The same engine with the EXPERIMENTAL Detection Panel families on (`crate::detect`).
    /// Off by default. They read the same admitted prints and bars as everything else, for every
    /// symbol, whatever Ross membership, news or float say; they never change a decision or the
    /// Ross Panel's membership. The Ross rows' cues are this policy's micro-pullback episodes, so
    /// the two panels show one detector. A policy whose grid differs from the pillar grid stays
    /// disabled with a reason in the snapshot (and every cue is `watching`).
    pub fn with_detection_policy(mut self, p: DetectionPolicy) -> Self {
        let g = &self.policy.body.grid;
        let floors = self.detector.floors.take();
        self.detector = Detector::new(p, g.bar_secs, g.settle_ms, Scope::All);
        self.detector.floors = floors;
        self
    }

    fn detection_on(&self) -> bool {
        self.detector.scope == Scope::All
    }

    /// The Detection Panel's policy; `None` while the panel is off (the cues still run under
    /// [`Engine::setup_policy`]).
    pub fn detection_policy(&self) -> Option<&DetectionPolicy> {
        self.detection_on().then_some(&*self.detector.policy)
    }

    /// The detection snapshot: cursor, active episodes with current assessments, bounded recent
    /// history, data status and telemetry. `None` when detection is off.
    pub fn detection_snapshot(&self) -> Option<DetectionSnapshot> {
        self.detection_on().then(|| {
            detect::snapshot(
                &self.detector,
                &self.run,
                self.syms
                    .iter()
                    .map(|(k, s)| (k.as_str(), &s.det, s.history_pending)),
            )
        })
    }

    /// Detection transitions held now, oldest first; the journal is empty afterwards. Bounded:
    /// a caller that stops taking loses the oldest, counted in [`Engine::detection_telemetry`].
    /// Empty with the Detection Panel off: the Ross-only transitions are the setup log.
    pub fn take_detection_transitions(&mut self) -> Vec<DetectionTransition> {
        if !self.detection_on() {
            return Vec::new();
        }
        self.detector.log.take()
    }

    /// The feature snapshot of `symbol`'s latest completed candidate bar (body/range diagnostics,
    /// baseline, window context), whether or not it opened an episode. Immutable once taken.
    pub fn latest_features(&self, symbol: &str) -> Option<&detect::FeatureSnapshot> {
        if !self.detection_on() {
            return None;
        }
        self.syms.get(symbol)?.det.latest.as_ref()
    }

    pub fn detection_telemetry(&self) -> DetectionTelemetry {
        if !self.detection_on() {
            return DetectionTelemetry::default();
        }
        let active = self.syms.values().map(|s| s.det.active.len()).sum();
        self.detector.telemetry(active)
    }

    /// A symbol's prior-session quietness reference ([`QuietReference`]), read as of each
    /// decision cutoff. Without one, quietness is `unavailable`. Ignored with the Detection Panel
    /// off (only the ignition family reads it).
    pub fn add_quiet_reference(&mut self, r: QuietReference) {
        if self.detection_on() {
            self.detector.add_reference(r);
        }
    }

    /// Drops quiet references for dates before `date`.
    pub fn prune_quiet_references(&mut self, date: NaiveDate) {
        self.detector.prune_references(date);
    }

    /// An observation failure from `at` (`symbol` `None`: every symbol, e.g. a provider link
    /// down or a stream error; `Some`: a documented halt or a per-symbol gap). Decides the
    /// barriers before `at` first; then every active episode in scope is interrupted, no new one
    /// starts, and windows overlapping the gap are insufficient until requalified. No trades
    /// alone is never a reason to call this.
    pub fn interrupt_detection(
        &mut self,
        symbol: Option<&str>,
        at: UnixNanos,
        reason: &'static str,
    ) -> Vec<Decision> {
        let out = self.advance_to(at);
        let det = &mut self.detector;
        let at = at.nanos();
        match symbol {
            Some(name) => {
                if let Some(s) = self.syms.get_mut(name) {
                    detect::set_interrupted(det, Some(&mut s.det), Some(name), at, reason);
                }
            }
            None => {
                detect::set_interrupted(det, None, None, at, reason);
                for s in self.syms.values_mut() {
                    detect::interrupt_all(det, &mut s.det, reason, at, None);
                }
            }
        }
        out
    }

    /// The end of an observation failure: the gap is recorded as `[from, at)` and new episodes
    /// requalify from bars after `at`.
    pub fn resume_detection(&mut self, symbol: Option<&str>, at: UnixNanos) -> Vec<Decision> {
        let out = self.advance_to(at);
        let det = &mut self.detector;
        let at = at.nanos();
        match symbol {
            Some(name) => {
                if let Some(s) = self.syms.get_mut(name) {
                    let _ = detect::clear_interrupted(det, Some(&mut s.det), Some(name), at);
                }
            }
            None => {
                let end = detect::clear_interrupted(det, None, None, at);
                for s in self.syms.values_mut() {
                    s.det.requalify_from = s.det.requalify_from.max(end);
                }
            }
        }
        out
    }

    /// The same engine with other output journal bounds (the defaults are [`PANEL_LOG_MAX`] and
    /// [`crate::output::SETUP_LOG_MAX`]). Bounds only decide when the oldest held output is dropped and counted;
    /// they never change a decision, a panel or the latest snapshot.
    ///
    /// With the Detection Panel on, the cue transitions travel in the detection journal under the
    /// detection policy's own bound, and `setup_entries` changes nothing.
    pub fn with_output_bounds(mut self, panels: usize, setup_entries: usize) -> Self {
        self.panel_log = BoundedLog::new(panels);
        if !self.detection_on() {
            self.detector.log = BoundedLog::new(setup_entries);
        }
        self
    }

    /// What the two output journals have produced, handed over and dropped. The setup counts are
    /// the Ross-only detector's transitions; zero with the Detection Panel on, whose journal
    /// carries them ([`Engine::detection_telemetry`]).
    pub fn output_telemetry(&self) -> OutputTelemetry {
        let on = self.detection_on();
        let log = &self.detector.log;
        let n = |x: u64| if on { 0 } else { x };
        OutputTelemetry {
            panels_published: self.panel_log.logged(),
            panels_taken: self.panel_log.taken(),
            panels_dropped: self.panel_log.dropped(),
            panels_held: self.panel_log.held(),
            setup_entries_logged: n(log.logged()),
            setup_entries_taken: n(log.taken()),
            setup_entries_dropped: n(log.dropped()),
            setup_entries_held: if on { 0 } else { log.held() },
            panel_log_max: self.panel_log.max(),
            setup_log_max: if on { 0 } else { log.max() },
        }
    }

    /// The same engine with its one detector under another detection policy, keeping the scope:
    /// the cues (and the Detection Panel, when on) then come from `p`.
    pub fn with_setup_policy(mut self, p: DetectionPolicy) -> Self {
        let g = &self.policy.body.grid;
        let scope = self.detector.scope;
        let floors = self.detector.floors.take();
        self.detector = Detector::new(p, g.bar_secs, g.settle_ms, scope);
        self.detector.floors = floors;
        self
    }

    /// Episode numbers a previous process of this run already used on `date`, as
    /// `(symbol, family, highest n)`: episodes opened on that date are numbered past them, so ids
    /// stay unique across a restart (`<run>:<date>:<symbol>:<family>:<n>`). The server derives
    /// them from today's journals and notice ledger at startup. Another date is unaffected, and a
    /// replay (its own run id per day, rebuilt from scratch) never calls this.
    pub fn set_episode_floors(
        &mut self,
        date: NaiveDate,
        used: impl IntoIterator<Item = (String, crate::detect::Family, u32)>,
    ) {
        self.detector.add_floors(date, used);
    }

    /// The detection policy the Ross rows' cues come from, whether or not the Detection Panel
    /// is on (`detection-v2` in the product).
    pub fn setup_policy(&self) -> &DetectionPolicy {
        &self.detector.policy
    }

    /// The Ross-only detector's transitions (the cues' `micro_pullback` episodes) held now, oldest
    /// first; the journal is empty afterwards. Bounded ([`crate::output::SETUP_LOG_MAX`]): a
    /// caller that stops taking loses the oldest entries, counted in
    /// [`Engine::output_telemetry`]. Empty with the Detection Panel on: they are detection
    /// transitions then ([`Engine::take_detection_transitions`]).
    pub fn take_setup_log(&mut self) -> Vec<DetectionTransition> {
        if self.detection_on() {
            return Vec::new();
        }
        self.detector.log.take()
    }

    /// The same engine under another Ross Panel policy.
    pub fn with_panel_policy(mut self, p: PanelPolicy) -> Self {
        self.panel_policy = p;
        self
    }

    pub fn panel_policy(&self) -> &PanelPolicy {
        &self.panel_policy
    }

    /// Switches the Ross Panel policy of a running engine (the owner's "require a news catalyst"
    /// switch: `ross-panel-v1` on, `ross-panel-v2` off). Nothing already decided changes: every
    /// symbol is marked for a fresh decision, so the next barrier applies the new rule to the
    /// membership as it stands (an active episode continues under it; an admission still needs
    /// every counted pillar in one decision). Returns whether the policy changed.
    pub fn set_panel_policy(&mut self, p: PanelPolicy) -> bool {
        if p.hash == self.panel_policy.hash {
            return false;
        }
        self.panel_policy = p;
        for s in self.syms.values_mut() {
            s.dirty = true;
        }
        true
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
        let clock_date = self.next_bar.map(|b| trading_date(UnixNanos(b)));
        if let Some(s) = self.syms.get_mut(symbol) {
            // About the date the symbol is on, or the clock's if that is later: a statement made
            // now is about today's prints, whatever date the symbol last decided.
            s.coverage_for = max_opt(s.date, clock_date);
            if s.coverage != coverage {
                s.coverage = coverage;
                s.dirty = true;
            }
        }
    }

    /// Marks a symbol's day history as still being restored (`true`) or resolved (`false`).
    ///
    /// While pending, everything that depends on the day's history is unavailable rather than
    /// computed from the live tail alone: the RVOL pillar and its diagnostics are UNKNOWN
    /// (`history_pending`), Ross momentum is unavailable, and the detection families (the Ross
    /// cues' micro-pullback among them) interrupt and start nothing (without requalifying, so the restored
    /// baselines are usable at once). Inputs keep flowing into the symbol's facts as usual.
    ///
    /// Separate from [`Integrity`]: clearing it never clears a genuine doubt or a coverage
    /// reason. [`Engine::adopt_facts`] clears it when it adopts; a refused or empty restore
    /// leaves it set for the caller to resolve (keep it, or clear it and report partial
    /// coverage). Creates the symbol's state if absent, so it can be set before the first print.
    pub fn set_history_pending(&mut self, symbol: &str, pending: bool) {
        let s = self
            .syms
            .entry(symbol.to_string())
            .or_insert_with(|| Sym::new(symbol));
        self.by_inst
            .entry(s.inst.clone())
            .or_insert_with(|| symbol.to_string());
        if s.history_pending != pending {
            s.history_pending = pending;
            s.dirty = true;
        }
    }

    pub fn history_pending(&self, symbol: &str) -> bool {
        self.syms.get(symbol).is_some_and(|s| s.history_pending)
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
            || restored.det.has_active()
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
                live.queued_n = restored.queued_n;
                // The restore replayed the held tail too, so an unplaceable revision the live
                // tail met is normally in both: the larger bound, never the sum.
                live.unplaced = live.unplaced.max(restored.unplaced);
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
                if restored.coverage.is_some() {
                    live.coverage_for = restored.coverage_for.or(restored.date);
                }
                live.coverage = restored.coverage.or(live.coverage);
                live.quarantine_through =
                    max_opt(live.quarantine_through, restored.quarantine_through);
                live.restored_through = max_opt(live.restored_through, restored.restored_through);
                // Detection: the return baseline and latest print are facts; episodes are claims
                // and stay this run's own.
                live.det.returns = restored.det.returns;
                live.det.last_seen = restored.det.last_seen.or(live.det.last_seen);
                // The day's history has arrived: the temporary guard ends here. Genuine doubts
                // and coverage reasons were merged above and stay.
                live.history_pending = false;
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
                sym.last = None;
                sym.news_first_seen = None;
                sym.news_kind = None;
                sym.news_evidence = None;
                sym.offering = None;
                sym.det = SymDetect {
                    returns: std::mem::take(&mut sym.det.returns),
                    last_seen: sym.det.last_seen,
                    ..SymDetect::default()
                };
                sym.halted = false;
                sym.history_pending = false;
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
        // One detector: with the Detection Panel on in either engine its policy is named;
        // otherwise the cues' policy is.
        if self.detection_policy().map(|p| &p.hash) != other.detection_policy().map(|p| &p.hash) {
            return Some("detection_policy");
        }
        if self.setup_policy().hash != other.setup_policy().hash {
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

    /// Also runs the micro-pullback for `symbols` (replacing the previous set), members or not:
    /// the server's Runners rows get the same cue and levels as a Ross row. With the Detection
    /// Panel on every symbol runs anyway. A symbol that leaves the set (and is not a member) has
    /// its episodes interrupted at its next barrier (`left_panel`), as a member that leaves does.
    /// Bounded by the caller (a Runners day is at most a few dozen symbols).
    pub fn set_watched<I: IntoIterator<Item = String>>(&mut self, symbols: I) {
        self.watched = symbols.into_iter().collect();
    }

    /// The micro-pullback cue for a watched symbol (or a member) at `tau`, read exactly as a Ross
    /// The headline class of the earliest news story that qualifies `symbol`'s news pillar at its
    /// latest decision (`NewsItem::event_type`), if any.
    pub fn news_kind(&self, symbol: &str) -> Option<&str> {
        self.syms.get(symbol).and_then(|s| s.news_kind.as_deref())
    }

    /// row's is ([`crate::cue::cue`]); `None` for a symbol the engine has not seen today.
    pub fn watched_cue(&self, symbol: &str, tau: i64) -> Option<RossCue> {
        let s = self.syms.get(symbol)?;
        if s.date != Some(trading_date(UnixNanos(tau))) {
            return None;
        }
        if !(self.detector.scope == Scope::All
            || self.watched.contains(symbol)
            || s.member.active().is_some())
        {
            return None;
        }
        let hold_reason = if s.history_pending {
            Some("history_pending")
        } else if s.integrity.window_untrusted || s.integrity.volume_untrusted {
            Some("inputs_untrusted")
        } else {
            None
        };
        Some(crate::cue::cue(
            &self.detector,
            &s.det,
            s.halted,
            hold_reason,
            tau,
        ))
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

    /// What this engine holds, by structure: entry counts for the server's `memory` log line
    /// (2026-10-28: production grew 1-5 GB an hour and no counter covered stock context).
    pub fn cost(&self) -> serde_json::Value {
        let (mut ledger, mut ids, mut ids_cap, mut cums, mut ticks, mut bars) = (0, 0, 0, 0, 0, 0);
        for s in self.syms.values() {
            ledger += s.ledger.len();
            ids += s.ids.len();
            ids_cap += s.ids.capacity();
            cums += s.cums.len();
            ticks += s.ticks.len();
            bars += s.bars.len();
        }
        let d = &self.detector;
        serde_json::json!({
            "symbols": self.syms.len(),
            "ledger": ledger,
            "ids": ids,
            "ids_capacity": ids_cap,
            "cums": cums,
            "ticks": ticks,
            "bars": bars,
            "evidence": self.evidence.len(),
            "panel_log": self.panel_log.len(),
            "watched": self.watched.len(),
            "detector": {
                "log": d.log.len(),
                "recent": d.recent.len(),
                "refs": d.refs.values().map(Vec::len).sum::<usize>(),
                "gaps": d.gaps.len(),
                "cited": d.cited.len(),
            },
        })
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
    /// A shares-outstanding figure: read only by a policy with `float.outstanding_fallback`.
    pub fn add_outstanding(&mut self, r: OutstandingObservation) {
        let i = r.instrument.clone();
        self.evidence.add_outstanding(r);
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
        let sym = self
            .syms
            .entry(name.clone())
            .or_insert_with(|| Sym::new(&name));
        self.by_inst.entry(sym.inst.clone()).or_insert(name);
        let exec = t.event_ts.nanos();
        // One identity rule for every path, and one place that decides it. The ledger is never
        // cleared on a date change — doing that after a date's first prints were already queued
        // is how their identities were lost, so a later redelivery of one counted twice. Entries
        // carry their own session date instead, and the bounds below retire them.
        // A print far outside the discovery band (above twice its top, below half its bottom)
        // belongs to a symbol no panel shows. Its count still matters, so it is still taken, but
        // its ledger is kept short: most of the market's prints are on such symbols (the large
        // caps), and the ledger is the scanner's largest structure. What a short ledger costs is
        // safe in both directions: an older print is refused (the count stays a lower bound) and
        // a revision past it is counted unplaced and bounded (`Sym::unplaced`).
        // Only folded entries go to that shorter bound: a queued one is volume not yet counted,
        // and a large cap prints far more than a thousand times between two barriers.
        let band = &self.policy.body.discovery_band;
        let p = t.price.micros();
        let run = self.run.numerator_coverage;
        retire(
            sym,
            &mut self.revisions,
            run,
            exec - LEDGER_WINDOW_SECS * SEC,
            LEDGER_MAX - 1,
            false,
        );
        if p > band.max_micros.saturating_mul(2) || p < band.min_micros / 2 {
            retire(
                sym,
                &mut self.revisions,
                run,
                i64::MIN,
                LEDGER_MAX_OUTSIDE_BAND - 1,
                true,
            );
        }
        // Past the horizon the engine cannot tell a new old print from a redelivery of one it
        // already counted, and counting it again would overstate the numerator — on the unloaded
        // path there is no chart engine to refuse it either. Refusing keeps the count a lower
        // bound, which is what lets an RVOL already over the threshold stay a PASS; the cost is
        // that a genuinely new print this old is not counted, and that is the safe direction.
        if sym.quarantine_through.is_some_and(|q| exec <= q) {
            self.revisions.quarantined_old += 1;
            // A refused print that would have counted is volume missing from the count, so the
            // symbol's coverage stops claiming more than the run's — but it is not `Partial`.
            // Partial makes Ross momentum unavailable for the rest of the date, and an old print
            // says nothing about the recent prices momentum reads: one late report used to
            // unrank a runner for the whole day (and, before coverage was dated, for days). A
            // print that would not have counted anyway — another date's, or one whose verdict
            // counts no volume — is no gap at all and changes nothing.
            let date = t.session.map(|s| s.date);
            let would_count =
                verdict.volume && date.is_some() && (sym.date.is_none() || sym.date == date);
            if would_count && sym.coverage.is_none_or(|(c, _)| c != Coverage::Partial) {
                let run = self.run.numerator_coverage;
                let c = if run == Coverage::Partial {
                    Coverage::Partial
                } else {
                    Coverage::Unverified
                };
                sym.coverage = Some((c, "quarantined_old_prints"));
                sym.coverage_for = date;
            }
            sym.dirty = true;
            return out;
        }
        let id = LedgerId::of(&t.id());
        let key = identity_key(id, exec);
        if !sym.ids.insert(key) {
            self.revisions.duplicates_dropped += 1;
            return out;
        }
        // Every symbol's latest admitted price is kept (a new Ross member arms on it at once);
        // the level tests run over active episodes only, which outside the Detection Panel are
        // the members'.
        detect::on_print(
            &mut self.detector,
            &mut sym.det,
            &t.id(),
            exec,
            admit.nanos(),
            t.price.micros(),
            verdict.ohlc,
            sym.history_pending,
        );
        sym.queued_n += 1;
        sym.ledger.push_back(Admitted {
            id,
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
        let want = LedgerId::of(&target.id);
        let candidates: Vec<(usize, Price, u64)> = sym
            .ledger
            .iter()
            .enumerate()
            .filter(|(_, a)| a.id == want && !a.removed)
            .map(|(i, a)| (i, Price::from_micros(a.price), a.size))
            .collect();
        let i = match target.resolve(candidates) {
            Resolution::Found(i) => i,
            Resolution::NotFound => {
                // Never admitted here, or past the ledger's horizon. Nothing is subtracted —
                // but something the provider says is not there may still be in the count, by at
                // most the target's size. The count is no longer a lower bound by itself; the
                // count less every such size is, so that is what the RVOL pillar may pass on
                // ([`Sym::unplaced`]). It used to make RVOL UNKNOWN for the rest of the date,
                // and a ledger horizon is minutes on a runner while a bust can come later.
                self.revisions.unknown_target += 1;
                sym.integrity.volume_untrusted = true;
                sym.unplaced = sym.unplaced.saturating_add(rev.target.size);
                sym.dirty = true;
                return out;
            }
            Resolution::Ambiguous => {
                self.revisions.ambiguous += 1;
                sym.integrity.volume_untrusted = true;
                sym.unplaced = sym.unplaced.saturating_add(rev.target.size);
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
        {
            // What the print says after this revision: a price only if it still sets one.
            let now = rev.corrected.filter(|c| c.verdict.ohlc).map(|c| c.price);
            let (id, exec) = (rev.target.id.clone(), sym.ledger[i].exec);
            detect::on_revision(
                &mut self.detector,
                &mut sym.det,
                &id,
                exec,
                now,
                admit.nanos(),
            );
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
                    if e.queued {
                        e.queued = false;
                        sym.queued_n -= 1;
                    }
                }
            }
        }
        {
            // The detectors' "latest admitted price" must not survive the revision of the print
            // it came from: recompute it from the prints still standing, in admission order
            // (the ledger's), and make it unavailable when none is left.
            let latest = sym
                .ledger
                .iter()
                .rev()
                .find(|a| a.verdict.ohlc && !a.removed)
                .map(|a| (a.exec, a.admit, a.price));
            detect::set_latest_print(&mut sym.det, latest);
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
        // Only where the detector keeps a return baseline: everywhere with the Detection Panel
        // on, a Ross member's otherwise (no ring is grown for a symbol nothing steps).
        if self.detector.scope == Scope::All
            || sym.member.active().is_some()
            || self.watched.contains(&rev.symbol)
        {
            detect::features::rebuild_returns(&self.detector, &mut sym.det, &sym.bars, bar_ns);
        }
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

    /// What the admission ledgers hold across every symbol, and roughly what they cost in
    /// memory: entries, and the bytes of their allocations (the ledger's and the identity set's
    /// capacity, plus any heap an entry's identity owns).
    ///
    /// An estimate of allocations, not a measured RSS: allocator overhead per block is not
    /// counted. It is what bounds the whole-market cost, so it is what a test measures.
    pub fn ledger_footprint(&self) -> (usize, usize) {
        let entry = std::mem::size_of::<Admitted>();
        let key = std::mem::size_of::<u64>() + 1;
        let mut entries = 0;
        let mut bytes = 0;
        for s in self.syms.values() {
            entries += s.ledger.len();
            bytes += s.ledger.capacity() * entry + s.ids.capacity() * key;
        }
        (entries, bytes)
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
        // Every ledger gives up what its time bound no longer covers, a slice of the symbols per
        // call. Retiring only on a symbol's own next print left a symbol that stopped trading
        // holding its whole ledger — at the close, the whole market's last twenty minutes, all
        // night. This used to visit every symbol once a minute in one call, under the server's
        // core lock: a minute of every active ledger popped (each identity out of its set) and
        // shrunk at once, 0.3-0.6 s by midday and growing with the day's trading (production,
        // 2026-11-25: 1.16 s at 11:18, 1.46 s at 11:24 ET). Now each call sweeps the next
        // SWEEP_PER_CALL symbols after a rotating cursor; at the market's one or two clock calls a
        // second, every symbol still comes round within about a minute.
        self.sweep_some(now, SWEEP_PER_CALL);
        out
    }

    /// Retires the expired front of up to `n` symbols' ledgers, continuing after the last symbol
    /// the previous call reached (wrapping at the end), so successive calls cover every symbol.
    fn sweep_some(&mut self, now: i64, n: usize) {
        let run = self.run.numerator_coverage;
        let keep_from = now - LEDGER_WINDOW_SECS * SEC;
        let mut visited = 0;
        let mut last: Option<String> = None;
        let start = self.sweep_cursor.take();
        let after = start
            .as_deref()
            .map(|k| (std::ops::Bound::Excluded(k), std::ops::Bound::Unbounded));
        let first_pass = match after {
            Some(r) => self.syms.range_mut::<str, _>(r),
            None => self.syms.range_mut::<str, _>(..),
        };
        for (name, s) in first_pass.take(n) {
            visited += 1;
            last = Some(name.clone());
            if s.ledger.front().is_some_and(|a| a.exec < keep_from) {
                retire(s, &mut self.revisions, run, keep_from, LEDGER_MAX, true);
            }
        }
        // Reached the end: wrap to the start for the rest of this call's share.
        if visited < n && start.is_some() {
            let stop = start.clone().expect("checked");
            for (name, s) in self
                .syms
                .range_mut::<str, _>((
                    std::ops::Bound::Unbounded,
                    std::ops::Bound::Included(stop.as_str()),
                ))
                .take(n - visited)
            {
                last = Some(name.clone());
                if s.ledger.front().is_some_and(|a| a.exec < keep_from) {
                    retire(s, &mut self.revisions, run, keep_from, LEDGER_MAX, true);
                }
            }
        }
        self.sweep_cursor = last;
    }

    /// The earliest barrier at or after `b` where some symbol needs a decision (`i64::MAX` when
    /// none), aligned to the grid.
    fn next_due(&self, b: i64, bar: i64, settle: i64) -> i64 {
        let mut due = i64::MAX;
        for s in self.syms.values() {
            // An active member's momentum window slides every barrier, print or not.
            // So does a detection episode's lifecycle (expiry, staleness), print or not.
            if s.dirty || s.member.active().is_some() || s.det.has_active() {
                return b;
            }
            for a in s.ledger.range(s.queued_start()..).filter(|a| a.queued) {
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
                        .range(s.queued_start()..)
                        .any(|a| a.queued && a.exec < b && a.admit <= tau)
            };
            if due && let Some(d) = self.evaluate(&name, date, b, tau) {
                out.push(d);
            }
            self.detect_symbol(&name, date, b, tau);
        }
        self.publish_panel(b, tau);
    }

    /// Runs the detection families for one symbol at barrier `b`, when the symbol has a bar
    /// completing now or an episode whose lifecycle advances with the clock.
    ///
    /// With the Detection Panel off only Ross members are stepped, after this barrier's
    /// membership was decided ([`Engine::evaluate`] runs first): a member's micro-pullback runs
    /// from its first member barrier, whose 20-bar lookback already reaches a pullback that began
    /// before admission. A symbol that leaves the panel has its episodes interrupted
    /// (`left_panel`) and its held outcome dropped, so nothing about it is carried while it is
    /// not watched and a re-admitted member starts clean (ids keep counting for the day).
    fn detect_symbol(&mut self, name: &str, date: NaiveDate, b: i64, tau: i64) {
        let watched = self.watched.contains(name);
        let det = &mut self.detector;
        let bar_ns = self.policy.body.grid.bar_secs * SEC;
        let run_cov = self.run.numerator_coverage;
        let s = self.syms.get_mut(name).expect("known symbol");
        if det.scope == Scope::RossMembers
            && ((s.member.active().is_none() && !watched) || s.date != Some(date))
        {
            if s.det.has_active() {
                detect::interrupt_all(det, &mut s.det, "left_panel", tau, Some((b, tau)));
            }
            s.det.last_micro = None;
            return;
        }
        let fresh_bar = s.bars.back().is_some_and(|x| x.end == b);
        let pending_halt = s.halted || s.det.halted_since.is_some();
        if !(fresh_bar || s.det.has_active() || pending_halt) {
            return;
        }
        let x = detect::Barrier {
            b,
            tau,
            bar_ns,
            run_id: &self.run.run_id,
            fidelity: self.run.fidelity,
            symbol: &s.name,
            inst: &s.inst,
            date,
            bars: &s.bars,
            halted: s.halted,
            coverage_partial: s.coverage.map_or(run_cov, |(c, _)| c) == Coverage::Partial,
            untrusted: s.integrity.window_untrusted || s.integrity.volume_untrusted,
            history_pending: s.history_pending,
            restored_through: s.restored_through,
        };
        detect::step(det, &mut s.det, &x);
    }

    /// Ranks the active members at barrier `b` and logs the panel when it changed.
    fn publish_panel(&mut self, b: i64, tau: i64) {
        let rules = &self.panel_policy.body.momentum;
        let news_optional = self.panel_policy.news_optional();
        let date = trading_date(UnixNanos(b));
        let mut rows: Vec<PanelRow> = Vec::new();
        for s in self.syms.values_mut() {
            if s.member.active().is_none() || s.date != Some(date) {
                // Only members are on the panel ([`Engine::detect_symbol`] forgets a member that
                // left when the Detection Panel is off).
                continue;
            }
            // The cue is read, never stepped here: the detector already advanced this barrier
            // (`detect_symbol`), over the same bars and prints the Detection Panel reads. Inputs
            // it cannot stand behind interrupted the episode there; the reason is named here.
            let hold_reason = if s.history_pending {
                Some("history_pending")
            } else if s.integrity.window_untrusted || s.integrity.volume_untrusted {
                Some("inputs_untrusted")
            } else {
                None
            };
            let setup = crate::cue::cue(&self.detector, &s.det, s.halted, hold_reason, tau);
            let ticks = &s.ticks;
            let price_at = |e: i64| ticks.iter().rev().find(|(x, _)| *x < e).copied();
            let segment = |t: i64| {
                classify(UnixNanos(t))
                    .filter(|x| x.kind != SessionKind::Overnight)
                    .map(|x| x.kind as u8)
            };
            let mut m = momentum(
                rules,
                b,
                price_at,
                segment,
                // An unprovable high is no high: the distance below it is not reported.
                s.day_high.filter(|_| !s.integrity.high_untrusted),
                s.halted,
                s.coverage.map_or(self.run.numerator_coverage, |(c, _)| c),
            );
            if s.history_pending {
                // A tail is not the day: no rank from a window the restore has not filled.
                m.status = MomentumStatus::Unavailable;
                m.reason = "history_pending";
            }
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
                    .filter(|p| p.reason != evaluate::BELOW_MAX_VIA_OUTSTANDING)
                    .and_then(|p| p.value)
                    .map(|v| v as u64),
                float_as_of: pillar(PillarId::Float)
                    .filter(|p| p.value.is_some())
                    .and_then(|p| p.as_of),
                outstanding_shares: pillar(PillarId::Float)
                    .filter(|p| p.reason == evaluate::BELOW_MAX_VIA_OUTSTANDING)
                    .and_then(|p| p.value)
                    .map(|v| v as u64),
                news_first_seen: s.news_first_seen,
                news_kind: s.news_kind.clone(),
                news_evidence: s.news_evidence.clone(),
                offering: s.offering.clone(),
                news_unconfirmed: news_optional
                    && pillar(PillarId::News).is_none_or(|p| p.state != Tri::Pass),
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
        let attention = rows.iter().filter(|r| r.setup.attention()).count() as u32;
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
        // A policy switch with the same rows is still news: the header says which rule is on.
        let changed = match prev {
            None => !rows.is_empty(),
            Some(p) => p.rows != rows || p.panel_policy != self.panel_policy.tag(),
        };
        if !changed {
            return;
        }
        let snap = PanelSnapshot {
            revision: self.seq,
            run_id: self.run.run_id.clone(),
            source: self.run.source.clone(),
            panel_policy: self.panel_policy.tag(),
            news_optional,
            pillar_policy: self.policy.tag(),
            bar_end: b,
            cutoff: tau,
            setup_policy: self.detector.policy.tag(),
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
            s.news_first_seen = None;
            s.news_kind = None;
            s.news_evidence = None;
            s.offering = None;
            // The ledger is deliberately **not** cleared here. Clearing it after a date's first
            // prints had already been queued is what lost their identities, so a later
            // redelivery of one counted twice; its entries carry their own session date and the
            // bounds in `apply` retire them.
            //
            // The doubts are a property of the date's facts, which have just been reset.
            s.integrity = Integrity::default();
            s.unplaced = 0;
            // So is what was known about how completely the earlier date was collected. A short
            // recording, a refused restore or an overflowed ledger left `Partial` here for the
            // life of the process, and Partial makes Ross momentum unavailable: a symbol stayed
            // unranked on every later date until a restart. A statement made about *this* date
            // (a restore adopted before its first barrier) is kept.
            if s.coverage_for.is_none_or(|d| d < date) {
                s.coverage = None;
                s.coverage_for = None;
            }
        }
        let exclude_overnight = policy.body.volume.exclude_overnight_boats;
        let bar_ns = policy.body.grid.bar_secs * SEC;
        // The setup bar completing at `b`; a late print (executed in an earlier, already decided
        // bar) never edits a completed bar and stays out of the setup bars.
        let mut cur = Bar::new(b);
        // Fold in the ledger entries this barrier may count, marking them in place rather than
        // taking them out: the ledger stays the one record of what was admitted, so a revision
        // arriving later can still find them (and tell a counted print from a queued one).
        let start = s.queued_start();
        for a in s.ledger.range_mut(start..) {
            if !(a.queued && a.exec < b && a.admit <= tau) {
                continue;
            }
            a.queued = false;
            s.queued_n -= 1;
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
        // A window reaching back into a restored prefix the run could not prove it replayed in
        // its own admission order is not reportable: the checkpoints at its endpoints may hold a
        // different set of prints than the live run's did. The day's total is unaffected — the
        // prints are the prints — and the doubt clears as the window moves past the prefix.
        let mut integrity = s.integrity;
        integrity.history_pending = s.history_pending;
        if s.restored_through.is_some_and(|r| b - w <= r) {
            integrity.window_untrusted = true;
        }
        let vol = VolumeView {
            cum: s.cum,
            recent,
            offset: (b - day_start) / SEC,
            coverage,
            integrity,
            unplaced: s.unplaced,
        };

        let ev = &self.evidence;
        let g = evaluate::gain(policy, ev, &s.inst, date, tau, price);
        let (rvol_r, rvol_d) = evaluate::rvol(policy, ev, &s.inst, date, tau, vol);
        let float_r = evaluate::float(policy, ev, &s.inst, tau);
        let (news_r, news_s) = evaluate::news(policy, ev, &s.inst, tau);
        let first = ev
            .news_current(&s.inst, tau)
            .into_iter()
            .filter(|n| news_s.items.contains(&format!("{}@{}", n.id, n.version)))
            .min_by_key(|n| n.observed_at);
        s.news_first_seen = first.map(|n| n.observed_at);
        s.news_kind = first.map(|n| n.event_type.clone());
        s.news_evidence = first.map(NewsEvidence::of);
        s.offering = offering(
            ev,
            &s.inst,
            tau,
            policy.body.reference_profile.news_freshness_secs,
        );
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

/// The latest SEC offering filing visible to `inst` at `cutoff` within the freshness window.
fn offering(
    ev: &EvidenceStore,
    inst: &InstrumentId,
    cutoff: i64,
    fresh_secs: i64,
) -> Option<Offering> {
    let fresh = fresh_secs * SEC;
    ev.news_stories(inst, cutoff)
        .into_iter()
        .map(|st| st.current)
        .filter(|n| {
            n.tag == crate::evidence::Tag::Present
                && n.event_type == OFFERING_EVENT
                && n.published_at
                    .is_some_and(|p| p <= cutoff && cutoff - p <= fresh)
        })
        .max_by_key(|n| (n.published_at, n.observed_at))
        .map(|n| Offering {
            form: n
                .source
                .split_once('/')
                .map_or_else(String::new, |(_, f)| f.to_string()),
            published_at: n.published_at,
            url: n.url.clone(),
        })
}
