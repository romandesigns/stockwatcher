//! The evidence-backed pillars (gain, RVOL, float, news) and the RVOL diagnostics, each a pure
//! function of the cutoff, the trade-derived state and the evidence visible at that cutoff.

use crate::evidence::{Authenticity, CloseBasis, EvidenceStore, Favorability, InstrumentId};
use crate::pillar::{
    PillarId, PillarResult, Tri, gain_bp, gain_passes, ratio_passes, ratio_vs_mean,
};
use crate::policy::Policy;
use chrono::NaiveDate;
use market_events::session::previous_trading_day;
use serde::Serialize;

const SEC: i64 = 1_000_000_000;

/// How completely today's own trades were collected (the RVOL numerator).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Coverage {
    Complete,
    Partial,
    /// No coverage record exists (today's live recorder keeps none).
    Unverified,
}

/// Today's price as the decision sees it.
#[derive(Clone, Copy, Debug)]
pub struct PriceView {
    pub micros: Option<i64>,
    pub stale: bool,
}

pub struct GainOut {
    pub pillar: PillarResult,
    /// Gain against the close as stored, whatever its basis: a named diagnostic, never the pillar.
    pub vs_stored_close_bp: Option<i64>,
}

pub fn gain(
    policy: &Policy,
    store: &EvidenceStore,
    inst: &InstrumentId,
    date: NaiveDate,
    cutoff: i64,
    price: PriceView,
) -> GainOut {
    let min = policy.body.reference_profile.gain_min_bp;
    let rule = format!(">= {min}");
    let unknown = |reason| {
        PillarResult::new(
            PillarId::Gain,
            Tri::Unknown,
            None,
            "bp",
            rule.clone(),
            reason,
        )
    };
    let Some(pc) = store.prev_close(inst, cutoff) else {
        return GainOut {
            pillar: unknown("no_previous_close"),
            vs_stored_close_bp: None,
        };
    };
    let ev = vec![format!("{}@{}", pc.id, pc.version)];
    let stored = match price.micros {
        Some(p) if pc.close_micros > 0 => Some(gain_bp(p, pc.close_micros)),
        _ => None,
    };
    let out = |pillar: PillarResult| GainOut {
        pillar: pillar.with_evidence(ev.clone()).as_of(Some(pc.observed_at)),
        vs_stored_close_bp: stored,
    };
    if pc.date != previous_trading_day(date) {
        return out(unknown("previous_close_date_mismatch"));
    }
    if pc.close_micros <= 0 {
        return out(unknown("nonpositive_close"));
    }
    // A raw close is only usable once corporate actions are known to be absent through today; an
    // adjusted one only once they are known to be applied through today. Never fall back to an
    // unchecked raw close: on a reverse-split day it fabricates a huge gain.
    let checked = pc.actions_checked_through.is_some_and(|d| d >= date);
    if !checked {
        let reason = match pc.basis {
            CloseBasis::Raw => "adjustment_coverage_unknown",
            CloseBasis::SplitAdjusted => "adjustment_not_current",
        };
        return out(unknown(reason));
    }
    // "A check found no known action" is an assumption the coverage policy permits only while
    // that check is fresh. Past its horizon nothing is verifying today's share basis any more, so
    // the pillar goes UNKNOWN rather than keeping a PASS alive through a provider outage.
    if !pc.actions.valid_at(cutoff) {
        return out(unknown("actions_coverage_expired"));
    }
    let Some(p) = price.micros else {
        return out(unknown("price_unknown"));
    };
    if price.stale {
        let mut r = unknown("price_stale");
        r.value = stored;
        return out(r);
    }
    let g = gain_bp(p, pc.close_micros);
    let state = if gain_passes(p, pc.close_micros, min) {
        Tri::Pass
    } else {
        Tri::Fail
    };
    out(PillarResult::new(
        PillarId::Gain,
        state,
        Some(g),
        "bp",
        rule,
        if state == Tri::Pass {
            "at_or_above_min"
        } else {
            "below_min"
        },
    ))
}

/// Whether a symbol's trade-derived state can be trusted, and for what.
///
/// Separate from [`Coverage`], because the two say different things and only one of them is
/// safe. Coverage is about **missing** prints: an incomplete count can only be lower than the
/// truth, so an RVOL that already clears the threshold is a valid PASS. Integrity is about
/// prints the engine may be counting that should not be there — a cancel it could not place, a
/// print it could not prove was not a redelivery — and that can only be *higher* than the truth.
/// A possibly-overstated numerator is not a lower bound, so it cannot pass anything.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Integrity {
    /// The counted volume may include a print that was revised away: a correction or cancel the
    /// engine could not place against exactly one of its own admissions.
    pub volume_untrusted: bool,
    /// The retained-window state (the cumulative checkpoints, the price history, the setup bars)
    /// may not match the stream, so anything measured over a window is not reportable.
    pub window_untrusted: bool,
    /// The day's high may still be a print that is gone, so the distance below it and the setup
    /// levels taken from it are not reportable.
    pub high_untrusted: bool,
    /// The symbol's day history has not been restored yet (a loader is pending): the facts so
    /// far are a tail, so nothing measured over the day or a window is reportable. Temporary and
    /// effective-only — set at each decision from the symbol's pending flag
    /// ([`crate::Engine::set_history_pending`]), never stored with the permanent doubts above.
    pub history_pending: bool,
}

impl Integrity {
    pub fn trusted(&self) -> bool {
        !self.volume_untrusted
            && !self.window_untrusted
            && !self.high_untrusted
            && !self.history_pending
    }

    /// Every doubt either of the two holds. Used when state from two sources meets in one symbol
    /// (a restore adopted into a live symbol): a doubt about either half is a doubt about the sum.
    pub fn or(self, other: Integrity) -> Integrity {
        Integrity {
            volume_untrusted: self.volume_untrusted || other.volume_untrusted,
            window_untrusted: self.window_untrusted || other.window_untrusted,
            high_untrusted: self.high_untrusted || other.high_untrusted,
            history_pending: self.history_pending || other.history_pending,
        }
    }
}

/// Today's volume as the decision sees it.
#[derive(Clone, Copy, Debug)]
pub struct VolumeView {
    /// Eligible shares 04:00 → the bar end.
    pub cum: u64,
    /// Eligible shares in the recent window ending at the bar end, when the window is covered.
    pub recent: Option<u64>,
    /// Seconds from 04:00 to the bar end.
    pub offset: i64,
    pub coverage: Coverage,
    pub integrity: Integrity,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RvolDiagnostics {
    /// Prior sessions the strict daily denominator found (needs `rvol_daily_sessions`).
    pub daily_sessions: usize,
    pub same_time_milli: Option<u64>,
    pub same_time_sessions: usize,
    pub same_time_reason: &'static str,
    pub recent_milli: Option<u64>,
    pub recent_sessions: usize,
    pub recent_reason: &'static str,
}

pub fn rvol(
    policy: &Policy,
    store: &EvidenceStore,
    inst: &InstrumentId,
    date: NaiveDate,
    cutoff: i64,
    vol: VolumeView,
) -> (PillarResult, RvolDiagnostics) {
    let prof = &policy.body.reference_profile;
    let n = prof.rvol_daily_sessions;
    let rule = format!(
        ">= {} (daily denominator, {} sessions)",
        prof.rvol_daily_min_milli, n
    );
    let mut diag = RvolDiagnostics {
        daily_sessions: 0,
        same_time_milli: None,
        same_time_sessions: 0,
        same_time_reason: "no_intraday_baseline",
        recent_milli: None,
        recent_sessions: 0,
        recent_reason: "no_intraday_baseline",
    };

    // History pending: the numerator is a tail awaiting the day's restore. Nothing about the
    // denominators changes that, so the guard comes before every baseline lookup — a missing or
    // expired baseline must not replace the reason a client needs to see.
    if vol.integrity.history_pending {
        diag.same_time_reason = "history_pending";
        diag.recent_reason = "history_pending";
        return (
            PillarResult::new(
                PillarId::Rvol,
                Tri::Unknown,
                None,
                "milli",
                rule,
                "history_pending",
            ),
            diag,
        );
    }

    // Same-time and recent-window diagnostics: never the pillar.
    if let Some(b) = store.intraday(inst, cutoff) {
        // The restatement onto today's share basis rests on evidence with its own expiry, like
        // the strict denominator's. Past it nothing is verifying the basis the prior sessions
        // were restated onto, so the diagnostic reports no number — a stale basis could be a
        // whole split out, which is the direction that invents volume.
        let expired = !b.basis.valid_at(cutoff);
        if expired {
            diag.same_time_reason = "actions_coverage_expired";
            diag.recent_reason = "actions_coverage_expired";
        }
        let prior: Vec<_> = b
            .sessions
            .iter()
            .filter(|s| s.date < date && s.complete)
            .collect();
        let min = policy.body.diagnostics.same_time_min_sessions;
        diag.same_time_sessions = prior.len();
        if vol.integrity.history_pending {
            diag.same_time_reason = "history_pending";
        } else if vol.integrity.volume_untrusted {
            // The numerator may hold a print that was revised away: no ratio is published from
            // it, in either direction.
            diag.same_time_reason = "volume_untrusted";
        } else if expired {
            // Nothing more to say: the sessions are counted, but no number is published from a
            // basis nothing is still verifying.
        } else if prior.len() < min {
            diag.same_time_reason = "insufficient_sessions";
        } else {
            let sum: u64 = prior.iter().map(|s| s.at(vol.offset)).sum();
            diag.same_time_milli = ratio_vs_mean(vol.cum, sum, prior.len());
            diag.same_time_reason = if diag.same_time_milli.is_some() {
                "ok"
            } else {
                "zero_denominator"
            };
        }
        let w = policy.body.diagnostics.recent_window_secs;
        diag.recent_sessions = prior.len();
        if vol.integrity.history_pending {
            diag.recent_reason = "history_pending";
        } else if vol.integrity.volume_untrusted {
            diag.recent_reason = "volume_untrusted";
        } else if vol.integrity.window_untrusted {
            // The checkpoints the window is measured between may not match the stream.
            diag.recent_reason = "window_untrusted";
        } else if expired {
        } else if prior.len() < policy.body.diagnostics.recent_min_sessions {
            diag.recent_reason = "insufficient_sessions";
        } else if let Some(now) = vol.recent {
            let sum: u64 = prior
                .iter()
                .map(|s| s.at(vol.offset).saturating_sub(s.at(vol.offset - w)))
                .sum();
            diag.recent_milli = ratio_vs_mean(now, sum, prior.len());
            diag.recent_reason = if diag.recent_milli.is_some() {
                "ok"
            } else {
                "zero_denominator"
            };
        } else {
            diag.recent_reason = "window_not_covered";
        }
    }

    let unknown = |reason| {
        PillarResult::new(
            PillarId::Rvol,
            Tri::Unknown,
            None,
            "milli",
            rule.clone(),
            reason,
        )
    };
    let Some(b) = store.daily(inst, cutoff) else {
        return (unknown("no_daily_baseline"), diag);
    };
    let ev = vec![format!("{}@{}", b.id, b.version)];
    // The denominator's sessions were restated onto today's share basis. Where that rested on a
    // freshness-dependent "no known action" assumption, it expires with the check behind it
    // (`ActionsEvidence::valid_until`): an unapplied split moves the denominator, so a stale
    // assumption is not a safe lower bound either.
    if !b.basis.valid_at(cutoff) {
        return (
            unknown("actions_coverage_expired")
                .with_evidence(ev)
                .as_of(Some(b.observed_at)),
            diag,
        );
    }
    let mut prior: Vec<_> = b
        .sessions
        .iter()
        .filter(|s| s.date < date && s.complete && s.comparable)
        .collect();
    prior.sort_by_key(|s| s.date);
    diag.daily_sessions = prior.len();
    if prior.len() < n {
        return (
            unknown("insufficient_baseline")
                .with_evidence(ev)
                .as_of(Some(b.observed_at)),
            diag,
        );
    }
    let window = &prior[prior.len() - n..];
    let sum: u64 = window.iter().map(|s| s.volume).sum();
    let Some(milli) = ratio_vs_mean(vol.cum, sum, n) else {
        return (
            unknown("zero_denominator")
                .with_evidence(ev)
                .as_of(Some(b.observed_at)),
            diag,
        );
    };
    let passes = ratio_passes(vol.cum, sum, n, prof.rvol_daily_min_milli);
    // Two different doubts, and only one of them is safe.
    //
    // Missing prints only lower the numerator, so an incomplete count that already reaches the
    // threshold is a valid PASS and one that falls short proves nothing — that is the lower-bound
    // argument, and it holds for every `Coverage`.
    //
    // It does **not** hold for a numerator that may be too high. A correction or cancel the
    // engine could not place against exactly one of its own admissions leaves volume in the
    // count that should not be there, so the figure is neither a lower bound nor a measurement:
    // UNKNOWN, with the reason, until the next decision computes it from a count that is sound.
    // A tail awaiting its day's history is neither a measurement nor a safe lower bound for a
    // decision a client will read as the day's: UNKNOWN until the history is adopted.
    let (state, reason) = match (vol.integrity.volume_untrusted, passes, vol.coverage) {
        _ if vol.integrity.history_pending => (Tri::Unknown, "history_pending"),
        (true, _, _) => (Tri::Unknown, "volume_untrusted"),
        (false, true, _) => (Tri::Pass, "at_or_above_min"),
        (false, false, Coverage::Complete) => (Tri::Fail, "below_min"),
        (false, false, _) => (Tri::Unknown, "numerator_coverage_unverified"),
    };
    (
        PillarResult::new(
            PillarId::Rvol,
            state,
            Some(milli as i64),
            "milli",
            rule,
            reason,
        )
        .with_evidence(ev)
        .as_of(Some(b.observed_at)),
        diag,
    )
}

pub fn float(
    policy: &Policy,
    store: &EvidenceStore,
    inst: &InstrumentId,
    cutoff: i64,
) -> PillarResult {
    let max = policy.body.reference_profile.float_max_exclusive_shares;
    let rule = format!("< {max}");
    let Some(f) = store.float(inst, cutoff) else {
        return PillarResult::new(
            PillarId::Float,
            Tri::Unknown,
            None,
            "shares",
            rule,
            "no_float_evidence",
        );
    };
    let ev = vec![format!("{}@{}", f.id, f.version)];
    let r = |state, reason| {
        PillarResult::new(
            PillarId::Float,
            state,
            Some(f.shares as i64),
            "shares",
            rule.clone(),
            reason,
        )
        .with_evidence(ev.clone())
        .as_of(Some(f.observed_at))
    };
    if f.shares == 0 {
        return r(Tri::Unknown, "nonpositive_float");
    }
    if cutoff - f.observed_at > policy.body.float.stale_after_secs * SEC {
        return r(Tri::Unknown, "float_stale");
    }
    if f.shares < max {
        r(Tri::Pass, "below_max")
    } else {
        r(Tri::Fail, "at_or_above_max")
    }
}

/// The news dimensions kept apart: fresh news present, primary-source verified, favorability.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NewsSummary {
    pub items: Vec<String>,
    pub primary_verified: bool,
    pub favorability: Option<Favorability>,
    pub coverage: Option<String>,
}

pub fn news(
    policy: &Policy,
    store: &EvidenceStore,
    inst: &InstrumentId,
    cutoff: i64,
) -> (PillarResult, NewsSummary) {
    let fresh = policy.body.reference_profile.news_freshness_secs * SEC;
    let rule = format!(
        "relevant substantive credible news published within {}s",
        fresh / SEC
    );
    // What makes a version qualify, applied to one version at a time.
    let qualifies = |n: &crate::evidence::NewsItem| {
        n.tag == crate::evidence::Tag::Present
            && n.relevant
            && n.substantive
            && n.credible_source
            && n.authenticity != Authenticity::Unsupported
            && n.published_at
                .is_some_and(|p| p <= cutoff && cutoff - p <= fresh)
    };
    // One current version per story, never every version ever observed: a version a later edit
    // disqualified, or whose symbol tag was removed, must stop qualifying.
    //
    // A story whose visible versions could not be put in the provider's revision order
    // ([`crate::evidence::Story::ordered`]) cannot *support* anything: which version is current
    // is a guess, so a PASS from it might quote a headline already replaced. It is excluded here
    // and reconsidered below, where the question is whether it could have changed the answer.
    let stories = store.news_stories(inst, cutoff);
    let qualifying: Vec<_> = stories
        .iter()
        .filter(|s| s.ordered)
        .map(|s| s.current)
        .filter(|n| qualifies(n))
        .collect();
    let coverage = store.coverage(inst, cutoff);
    let mut summary = NewsSummary {
        items: qualifying
            .iter()
            .map(|n| format!("{}@{}", n.id, n.version))
            .collect(),
        primary_verified: qualifying.iter().any(|n| {
            matches!(
                n.authenticity,
                Authenticity::IssuerAnnounced | Authenticity::RegulatorConfirmed
            )
        }),
        favorability: None,
        coverage: coverage.map(|c| c.id.clone()),
    };
    if !qualifying.is_empty() {
        // Favorability is reported, never folded into the pillar: adverse news is still news.
        let has = |x| qualifying.iter().any(|n| n.favorability == x);
        let (fav, adv, mixed) = (
            has(Favorability::Favorable),
            has(Favorability::Adverse),
            has(Favorability::Mixed),
        );
        summary.favorability = Some(if mixed || (fav && adv) {
            Favorability::Mixed
        } else if fav {
            Favorability::Favorable
        } else if adv {
            Favorability::Adverse
        } else {
            Favorability::Undetermined
        });
        let first = qualifying.iter().map(|n| n.observed_at).max();
        let r = PillarResult::new(
            PillarId::News,
            Tri::Pass,
            None,
            "",
            rule,
            "fresh_relevant_news",
        )
        .with_evidence(summary.items.clone())
        .as_of(first);
        return (r, summary);
    }
    // Nothing qualifies on the stories whose order is settled. Before that can be a FAIL, the
    // unsettled ones have to be ruled out: if any version of an ambiguous story would itself
    // qualify, then whether this symbol has a catalyst depends on which version is current, and
    // that is exactly what cannot be established.
    //
    // The test is per story and per version, not "is anything ambiguous". The pillar's claim is
    // existential — one qualifying story is enough — so a stale conflicting story outside the
    // freshness window, or an irrelevant one, changes no answer and must not force UNKNOWN over
    // a fresh unambiguous catalyst. An earlier draft asked the broad question and did exactly
    // that.
    if stories
        .iter()
        .filter(|s| !s.ordered)
        .any(|s| s.visible.iter().any(|n| qualifies(n)))
    {
        return (
            PillarResult::new(
                PillarId::News,
                Tri::Unknown,
                None,
                "",
                rule,
                "news_revision_order_unknown",
            ),
            summary,
        );
    }
    // A FAIL needs coverage continuous from the freshness window's start to at least
    // `cutoff - lag` (pillars-v2: 120 s; v1: 0, the cutoff itself). Only coverage records and
    // items observed by the cutoff are visible, so the lag never lets a later record in.
    match coverage {
        Some(c)
            if c.covered_from <= cutoff - fresh
                && c.covered_through + policy.body.news.coverage_max_lag_secs * SEC >= cutoff =>
        {
            let r = PillarResult::new(
                PillarId::News,
                Tri::Fail,
                None,
                "",
                rule,
                "no_matching_news_in_covered_sources",
            )
            .with_evidence(vec![c.id.clone()])
            .as_of(Some(c.observed_at));
            (r, summary)
        }
        _ => (
            PillarResult::new(
                PillarId::News,
                Tri::Unknown,
                None,
                "",
                rule,
                "news_coverage_unknown",
            ),
            summary,
        ),
    }
}
