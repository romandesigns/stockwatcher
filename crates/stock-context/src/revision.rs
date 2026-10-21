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
//! paths behave the same whatever verdict the caller supplies. Bounded: identities older than
//! [`DEDUP_WINDOW_SECS`] than the newest print are evicted, which is far beyond the seconds a
//! reconnect replay takes.
//!
//! **Revision.** [`PrintRevision`] is what the run's chart engine resolved: the print as it was
//! counted, the print that replaced it (a correction) or nothing (a cancel), and the day's last
//! price and high recomputed from what that engine still retains. The context engine keeps
//! aggregates, not prints, so it cannot undo a print on its own; it applies the delta the
//! resolver gives it and counts anything it cannot apply exactly.
//!
//! **Forward-effective only.** A revision changes the symbol's *current* facts from the barrier
//! it is admitted at. Decisions already emitted are never rewritten and a frozen setup level
//! never moves: an episode that happened, happened. What a revision cannot repair in the past is
//! counted ([`RevisionTelemetry`]), never papered over.

use crate::engine::Verdict;

/// How far back a print identity is remembered for duplicate detection.
///
/// A redelivery arrives within seconds of the original (a reconnect replay), so an hour is a
/// generous bound. Holding every identity of a whole session for a hot symbol is how a live
/// engine's memory stops being bounded.
pub const DEDUP_WINDOW_SECS: i64 = 3_600;

/// One side of a [`PrintRevision`]: the print as the resolver counted it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RevisedPrint {
    /// Execution time. A correction keeps the original's.
    pub exec: i64,
    pub price: i64,
    pub size: u64,
    pub verdict: Verdict,
}

/// The day's highest OHLC-qualifying price after a revision, as far as the resolver still
/// retains that date.
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

/// A correction or cancel of a print already applied, as the run's chart engine resolved it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrintRevision {
    pub symbol: String,
    /// The trading date of the revised print. A revision never crosses dates.
    pub date: chrono::NaiveDate,
    /// The print as it was counted.
    pub removed: RevisedPrint,
    /// The print that replaced it; `None` for a cancel.
    pub added: Option<RevisedPrint>,
    /// The date's newest OHLC-qualifying price after the revision, with its execution time.
    /// `None`: no price of that date is left.
    pub last: Option<(i64, i64)>,
    /// The date's highest OHLC-qualifying price after the revision ([`DayHigh`]).
    pub day_high: Option<DayHigh>,
}

/// What the engine did with the revisions and duplicates it was given. Every number is counted,
/// so an incomplete repair is visible instead of assumed away.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct RevisionTelemetry {
    /// Redelivered prints dropped on identity, both paths together.
    pub duplicates_dropped: u64,
    /// Revisions whose volume and trade count were adjusted exactly.
    pub applied: u64,
    /// Revisions for a symbol or trading date the engine does not hold: nothing to adjust.
    pub unknown_target: u64,
    /// Revisions the engine applied to the volume side but could not prove a day high for, so
    /// its day high (and the `below_high_bp` and setup levels derived from it) may still reflect
    /// a print that is gone. Counted, never silently accepted.
    pub day_high_unproven: u64,
    /// Revisions that arrived after their trading date had rolled over in the engine: the facts
    /// they refer to are no longer the current ones and prior decisions are immutable.
    pub too_late: u64,
}

impl RevisionTelemetry {
    /// Nothing was left unrepaired: every revision the engine was given was applied in full.
    pub fn complete(&self) -> bool {
        self.unknown_target == 0 && self.day_high_unproven == 0 && self.too_late == 0
    }
}
