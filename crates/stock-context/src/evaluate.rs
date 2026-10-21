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

    // Same-time and recent-window diagnostics: never the pillar.
    if let Some(b) = store.intraday(inst, cutoff) {
        let prior: Vec<_> = b
            .sessions
            .iter()
            .filter(|s| s.date < date && s.complete)
            .collect();
        let min = policy.body.diagnostics.same_time_min_sessions;
        diag.same_time_sessions = prior.len();
        if prior.len() < min {
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
        if prior.len() < policy.body.diagnostics.recent_min_sessions {
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
    // Missing prints only lower the numerator, so an incomplete count that already reaches the
    // threshold is a valid PASS; one that falls short proves nothing.
    let (state, reason) = match (passes, vol.coverage) {
        (true, _) => (Tri::Pass, "at_or_above_min"),
        (false, Coverage::Complete) => (Tri::Fail, "below_min"),
        (false, _) => (Tri::Unknown, "numerator_coverage_unverified"),
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
    // One current version per story (`news_current`), never every version ever observed: a
    // version a later edit disqualified, or whose symbol tag was removed, must stop qualifying.
    let qualifying: Vec<_> = store
        .news_current(inst, cutoff)
        .into_iter()
        .filter(|n| {
            n.relevant
                && n.substantive
                && n.credible_source
                && n.authenticity != Authenticity::Unsupported
        })
        .filter(|n| {
            n.published_at
                .is_some_and(|p| p <= cutoff && cutoff - p <= fresh)
        })
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
