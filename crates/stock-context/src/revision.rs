//! One print identity and one revision contract, for every path a print can reach the engine by.
//!
//! # Why this exists
//!
//! The context engine took a print with whatever verdict its caller had. On the server that is
//! the chart engine's verdict for a symbol whose history is loaded, and the print's own
//! conditions for one whose history is not (`Counted::from_effect`, which has no duplicate
//! exclusion and says so). So during the startup window a redelivered print — which a reconnect
//! replay produces by design — was counted twice in the RVOL numerator, a **false positive**, and
//! the two paths disagreed about the same day (the audit's F10).
//!
//! Worse, `Correction` and `Cancel` never reached the engine at all: the server routed a
//! revision to the quote board only, so the scanner kept a cancelled print's volume, trades and
//! day high for the rest of the day while the candles and the quote row agreed it was gone (the
//! audit's F3). An inflated numerator is not a safe lower bound, so the "missing prints can only
//! lower RVOL" argument did not hold.
//!
//! # The contract
//!
//! **Identity.** A print is identified by [`market_events::PrintId`] — the provider reference and
//! the execution time to the nanosecond — and the engine deduplicates on it itself, so both
//! paths behave the same whatever verdict the caller supplies.
//!
//! **The engine resolves its own revisions.** A revision names a print the way the provider does
//! ([`RevisionTarget`]: the reference plus the original price and size, with no execution time),
//! and the engine matches it against the prints **it** retained, by
//! [`market_events::TradeRef::resolve`]'s rule: exactly one fit is applied, none or several is
//! counted and never guessed at. It does not ask whether a chart is loaded, because the scanner's
//! facts must not depend on that; a chart engine contributes only the two things it alone can
//! recompute ([`PrintRevision::last`], [`PrintRevision::day_high`]), and their absence is counted
//! rather than assumed.
//!
//! **A print still queued is revised in the queue.** A cancel can arrive before the barrier that
//! would have counted its print. Adjusting aggregates then would subtract volume the engine never
//! added and let the print count afterwards anyway; the engine revises the queued print instead.
//!
//! **Forward-effective only.** A revision changes the symbol's *current* facts from the barrier
//! it is admitted at. Decisions already emitted are never rewritten and a frozen setup level
//! never moves: an episode that happened, happened. What a revision cannot repair in the past is
//! counted ([`RevisionTelemetry`]), never papered over.
//!
//! **Bounded.** Identities and retained prints are kept for [`REVISION_WINDOW_SECS`] of execution
//! time. A revision reaching further back than that cannot be matched and is counted, because
//! remembering a hot symbol's whole session is how a live engine's memory stops being bounded.

use crate::engine::Verdict;
use market_events::TradeId;

/// How long a print is remembered: for duplicate detection, and so a revision can name it.
///
/// A redelivery arrives within seconds (a reconnect replay) and a correction or cancel close
/// behind its print, so an hour is a generous bound on both.
pub const REVISION_WINDOW_SECS: i64 = 3_600;

/// Kept for compatibility with the identity rule's own name; the same window serves both.
pub const DEDUP_WINDOW_SECS: i64 = REVISION_WINDOW_SECS;

/// How the provider names the print a revision revises: the reference plus the original price and
/// size. There is no execution time on a correction or a cancel, which is why the match needs the
/// price and size and can still be ambiguous.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevisionTarget {
    pub id: TradeId,
    pub price: i64,
    pub size: u64,
}

/// The print that replaces the revised one (a correction).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CorrectedPrint {
    pub price: i64,
    pub size: u64,
    /// What the condition policy makes of the corrected conditions.
    pub verdict: Verdict,
}

/// The date's newest OHLC-qualifying print after a revision: its execution time and its price.
///
/// A named type rather than a tuple. The engine's own last price is `(exec, price)` and the
/// server built this as `(price, ts)`, so the fallback silently put a price where an execution
/// time belonged — a bare pair of `i64`s cannot say which way round it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LastPrint {
    pub exec: i64,
    pub price: i64,
}

/// The day's highest OHLC-qualifying price after a revision, as far as the resolver that supplied
/// it still retains that date.
///
/// `complete` false makes it a **lower bound**, not the high: earlier candles of the date have
/// been evicted, so a revision that removed the high cannot be proved to have lowered it. The
/// engine then leaves its own day high where it is and counts the shortfall rather than
/// pretending either way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DayHigh {
    pub price: i64,
    pub complete: bool,
}

/// A correction or cancel of a print, as the provider published it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrintRevision {
    pub symbol: String,
    pub target: RevisionTarget,
    /// The replacement; `None` for a cancel.
    pub corrected: Option<CorrectedPrint>,
    /// The date's newest OHLC-qualifying print after the revision, when a resolver that retains
    /// the date could recompute it. `None`: not available, so the engine falls back to its own
    /// ledger and, failing that, leaves its last price where it is.
    pub last: Option<LastPrint>,
    /// The date's highest OHLC-qualifying price after the revision ([`DayHigh`]), when a resolver
    /// could recompute it.
    pub day_high: Option<DayHigh>,
}

/// What the engine did with the revisions and duplicates it was given. Every number is counted,
/// so an incomplete repair is visible instead of assumed away.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct RevisionTelemetry {
    /// Redelivered prints dropped on identity, both paths together.
    pub duplicates_dropped: u64,
    /// Revisions applied to a print this engine had already counted.
    pub applied: u64,
    /// Revisions applied to a print still queued for its barrier: revised in the queue, so it is
    /// counted as corrected or not counted at all, rather than counted and then adjusted.
    pub applied_queued: u64,
    /// Revisions naming no print this engine retains: a symbol it does not hold, a print from
    /// before it existed, or one older than [`REVISION_WINDOW_SECS`]. Nothing is subtracted,
    /// because subtracting a print the engine never added would take volume off an unrelated one.
    pub unknown_target: u64,
    /// Revisions whose reference fitted more than one retained print. Applying one would be a
    /// guess ([`market_events::Resolution::Ambiguous`]).
    pub ambiguous: u64,
    /// Revisions applied to the volume side where no resolver could prove the date's high, so
    /// this engine's day high (and the `below_high_bp` and setup levels derived from it) may
    /// still reflect a print that is gone.
    pub day_high_unproven: u64,
    /// Revisions for a trading date the engine has moved on from: the facts they refer to are no
    /// longer the current ones and prior decisions are immutable.
    pub too_late: u64,
    /// Revisions where the retained-window state could not be rebuilt exactly from the ledger,
    /// because the ledger no longer reached the window's start. The aggregates are still exact;
    /// the recent-window and setup inputs may be short.
    pub windows_unproven: u64,
    /// Ledger entries retired at a bound whose print had already been counted. The volume stands;
    /// what is gone is the ability to prove a duplicate or place a revision for them.
    pub identities_evicted: u64,
    /// Ledger entries retired at a bound while still **queued**: volume the engine will never
    /// count, so the symbol's coverage is degraded, not just counted.
    pub queued_evicted: u64,
    /// Prints refused because they execute at or before the ledger's own horizon, so the engine
    /// cannot prove they are not redeliveries of something it already counted.
    ///
    /// Refusing them keeps the count a **lower bound**, which is what lets an RVOL that already
    /// clears the threshold stay a PASS. Admitting them would have inflated it: after an
    /// identity left the ledger, replaying that print inserted it again and counted it twice,
    /// and the unloaded path has no chart engine to refuse it.
    pub quarantined_old: u64,
}

impl RevisionTelemetry {
    /// Nothing was left unrepaired: every revision the engine was given was applied in full.
    pub fn complete(&self) -> bool {
        self.unknown_target == 0
            && self.ambiguous == 0
            && self.day_high_unproven == 0
            && self.too_late == 0
            && self.windows_unproven == 0
            && self.queued_evicted == 0
    }
}
