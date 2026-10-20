//! Stored daily bars and corporate-action announcements → stock-context evidence:
//! [`stock_context::DailyBaseline`] (RVOL's 30-session denominator) and
//! [`stock_context::PrevClose`] (the gain's reference), for one symbol and one evaluated trading
//! date D. Pure: the caller reads the files and passes them in; nothing here touches the disk, a
//! clock or a provider. Not wired into the server. Full rules and their reasons:
//! `docs/stock-context/daily-baseline-plan.md` §6.
//!
//! Metric: the sessions' volume is **Alpaca's SIP daily-bar volume** (`provider_daily_volume_v1`
//! in every record id), not our condition-filtered 04:00 → end aggregation. Whether the two agree
//! is the volume study of plan §9 (`swimport volume-compat`).
//!
//! # Sessions
//!
//! Only dates before D; missing is missing (never a zero session); a date whose raw file was
//! fetched before its last session ended is not covered; `comparable` = a full trading day in the
//! calendar (early closes and years outside the calendar are not).
//!
//! # Split consistency ([`split_consistency`])
//!
//! For a date x with a raw bar R and a split-adjusted bar A from one snapshot, the adjustment
//! multiplier is `m = A.c / R.c` (`P_adj = P_raw · m`). A split adjustment moves volume by the
//! reciprocal (`V_adj = V_raw / m`), so `P·V` is preserved. For a split of `old → new` shares,
//! `m = old/new`: a 1-for-10 reverse split (10 → 1) gives m = 10, $0.08 → $0.80 and
//! 1,000,000 → 100,000 shares. Two splits after x multiply. The check allows 1 % (min 1 share) on
//! volume and never compares floats for equality; trade counts must match. **It checks the
//! consistency of the adjustment Alpaca supplied, not that every action is covered**, and it is
//! only available after the fact: a final daily-bar adjustment never certifies an earlier live
//! intraday decision. Records keep their basis date, `observed_at` (retrieval time) and source
//! so a replay labels them `reconstructed_history`.
//!
//! # `actions_checked_through`
//!
//! 1. **Bars**: E = the latest date in P..=D such that every trading date from P to E is
//!    consistent (both sets cover it, agree on presence, consistent bars) with bars on E. When
//!    E = D the record is `BarVerified`.
//! 2. **Announcements** (live; [`coverage_for`], coverage policy `corporate-actions-coverage-v1`):
//!    otherwise the record rests on the provider's announcements for `(P, D]`:
//!    - `ConfirmedAction`: complete, consistent split terms → close × Π(old/new), prior volumes
//!      × Π(new/old), `AnnouncedAction`, checked through D;
//!    - `NoKnownActionProviderChecked`: a fresh, fully paginated check that covered the symbol
//!      found nothing relevant → unchanged basis is **assumed** under the policy
//!      (`ProviderCheckedNoKnownAction`, `assumption = unchanged_share_basis_assumed`), checked
//!      through D;
//!    - `UnresolvedAction`, `StaleCoverage`, `UnavailableCoverage` → not checked through D (the
//!      bars' E, or `None`); the state is still recorded.
//!
//!    The baseline uses announcements only on top of bars consistent through P (44 of 45 dates
//!    stay bar-checked).

use crate::corporate_actions::{ActionRetrieval, CorporateAction, CoveragePolicy};
use crate::daily_bars::{DailyBar, RawDayBars, SplitSnapshot};
use chrono::{DateTime, NaiveDate, Utc};
use market_events::session::{
    DayKind, calendar_covers, day_kind, is_trading_day, previous_trading_day, sessions_for,
};
use market_events::{Price, UnixNanos};
use std::collections::{BTreeMap, BTreeSet};
/// Re-exported so callers need no direct stock-context dependency for the provenance types.
pub use stock_context::{ActionsEvidence, ActionsSource, CoverageState, SplitRatio};
use stock_context::{CloseBasis, DailyBaseline, InstrumentId, PrevClose, SessionVolume};

/// The metric these records measure, carried in every id.
pub const METRIC: &str = "provider_daily_volume_v1";
/// Trading dates before D looked at for sessions (owner-accepted 2026-10-20).
pub const DEFAULT_LOOKBACK: usize = 45;
/// Volume/price reciprocity tolerance (owner-accepted 2026-10-20).
pub const VOLUME_FACTOR_TOL: f64 = 0.01;
/// A multiplier ratio this close to 1 is "same basis" (absorbs 4-decimal rounding of adjusted
/// prices; the smallest real splits are ~5 %). Owner-accepted 2026-10-20.
pub const SAME_BASIS_TOL: f64 = 0.002;
/// The assumption label of a record resting on "no known action".
pub const UNCHANGED_BASIS_ASSUMED: &str = "unchanged_share_basis_assumed";

/// The ids of the evidence records for `symbol` on every trading date in `from..=through`: what
/// a late-discovered action may have affected.
pub fn record_ids(symbol: &str, from: NaiveDate, through: NaiveDate) -> Vec<String> {
    let mut out = Vec::new();
    for d in from.iter_days().take_while(|d| *d <= through) {
        if is_trading_day(d) {
            out.push(format!("{METRIC}:prev_close:{symbol}:{d}"));
            out.push(format!("{METRIC}:baseline:{symbol}:{d}"));
        }
    }
    out
}

/// Why a record was or was not produced, for logs and tests.
#[derive(Clone, Debug, PartialEq)]
pub struct Loaded {
    pub prev_close: Option<PrevClose>,
    pub prev_close_reason: &'static str,
    pub baseline: Option<DailyBaseline>,
    pub baseline_reason: &'static str,
    /// What `actions_checked_through` holds.
    pub checked_through: Option<NaiveDate>,
    /// What the announcements said, whether or not it was needed.
    pub coverage: ActionCoverage,
}

/// The inputs for one symbol and one evaluated date.
pub struct Inputs<'a> {
    /// Raw date files in range.
    pub raw: &'a BTreeMap<NaiveDate, RawDayBars>,
    /// At most one split snapshot (one snapshot is one basis:
    /// [`crate::Archive::latest_snapshot_for`] of the history symbol).
    pub split: Option<&'a SplitSnapshot>,
    /// Announcement retrievals made for D (`Archive::read_action_retrievals(D)`; may be empty).
    pub announcements: &'a [ActionRetrieval],
    pub policy: &'a CoveragePolicy,
    /// The decision time the records are built for: nothing retrieved later is used, and
    /// announcement freshness is measured against it.
    pub as_of: DateTime<Utc>,
}

/// Instant the date's last session ends; `None` for a non-trading day.
fn session_end(date: NaiveDate) -> Option<DateTime<Utc>> {
    sessions_for(date).last().map(|s| s.2.to_utc())
}

fn nanos(t: DateTime<Utc>) -> i64 {
    UnixNanos::from_utc(t).map_or(i64::MAX, |n| n.0)
}

/// Raw coverage of `(symbol, date)`, honouring "fetched after the session ended".
fn raw_cov<'a>(
    raw: &'a BTreeMap<NaiveDate, RawDayBars>,
    symbol: &str,
    date: NaiveDate,
) -> Option<Option<&'a DailyBar>> {
    let day = raw.get(&date)?;
    let end = session_end(date)?;
    if day.first_retrieved().is_none_or(|t| t < end) {
        return None;
    }
    day.coverage(symbol)
}

fn split_cov<'a>(
    snap: &'a SplitSnapshot,
    symbol: &str,
    date: NaiveDate,
) -> Option<Option<&'a DailyBar>> {
    let end = session_end(date)?;
    if snap.fetch.retrieved_at < end {
        return None;
    }
    snap.coverage(symbol, date)
}

/// Checks that `adj` is `raw` under a split adjustment and returns its price multiplier
/// `m = adj.c / raw.c` (module docs). Errors name the failed check.
pub fn split_consistency(raw: &DailyBar, adj: &DailyBar) -> Result<f64, &'static str> {
    if !(raw.c.is_finite() && adj.c.is_finite() && raw.c > 0.0 && adj.c > 0.0) {
        return Err("nonpositive_close");
    }
    if let (Some(rn), Some(an)) = (raw.n, adj.n)
        && rn != an
    {
        return Err("trade_count_differs");
    }
    let m = adj.c / raw.c;
    if let (Some(rv), Some(av)) = (raw.v, adj.v) {
        // V_adj = V_raw / m (P·V preserved); 1 % or one share, whichever is larger.
        let want = rv as f64 / m;
        if (av as f64 - want).abs() > (want * VOLUME_FACTOR_TOL).max(1.0) {
            return Err("volume_not_reciprocal");
        }
    }
    Ok(m)
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Verified {
    /// Both sides have a consistent bar; `m` = adjusted / raw close.
    Bar {
        m: f64,
    },
    NoBar,
    Unverified,
}

fn verify(inputs: &Inputs<'_>, symbol: &str, date: NaiveDate) -> Verified {
    let Some(snap) = inputs.split else {
        return Verified::Unverified;
    };
    let (Some(r), Some(a)) = (
        raw_cov(inputs.raw, symbol, date),
        split_cov(snap, symbol, date),
    ) else {
        return Verified::Unverified;
    };
    match (r, a) {
        (None, None) => Verified::NoBar,
        (Some(r), Some(a)) => match split_consistency(r, a) {
            Ok(m) => Verified::Bar { m },
            Err(_) => Verified::Unverified,
        },
        _ => Verified::Unverified,
    }
}

fn trading_dates(from: NaiveDate, to: NaiveDate) -> Vec<NaiveDate> {
    from.iter_days()
        .take_while(|d| *d <= to)
        .filter(|d| is_trading_day(*d))
        .collect()
}

fn snap_one(r: f64) -> f64 {
    if (r - 1.0).abs() <= SAME_BASIS_TOL {
        1.0
    } else {
        r
    }
}

/// Volume multiplier onto `anchor`'s basis, `m(anchor) / m(x)`, for every consistent date with
/// bars in `dates`; `None` when `anchor` itself is not consistent with bars on both sides. Each x
/// stands alone: one snapshot is one basis, so a gap does not hide a split.
fn volume_factors(
    inputs: &Inputs<'_>,
    symbol: &str,
    dates: &[NaiveDate],
    anchor: NaiveDate,
) -> Option<BTreeMap<NaiveDate, f64>> {
    let Verified::Bar { m: ma } = verify(inputs, symbol, anchor) else {
        return None;
    };
    let mut out = BTreeMap::new();
    for &x in dates {
        if let Verified::Bar { m } = verify(inputs, symbol, x) {
            out.insert(x, snap_one(ma / m));
        }
    }
    Some(out)
}

/// E of the module rule over P..=D, with the price multiplier `m(P)/m(E)` onto E's basis.
fn bars_through(
    inputs: &Inputs<'_>,
    symbol: &str,
    p: NaiveDate,
    d: NaiveDate,
) -> Option<(NaiveDate, f64)> {
    let Verified::Bar { m: mp } = verify(inputs, symbol, p) else {
        return None;
    };
    let mut best = (p, 1.0);
    for x in trading_dates(p, d).into_iter().skip(1) {
        match verify(inputs, symbol, x) {
            Verified::Bar { m } => best = (x, snap_one(mp / m)),
            Verified::NoBar => {}
            Verified::Unverified => break,
        }
    }
    Some(best)
}

fn latest_retrieval(it: impl Iterator<Item = Option<DateTime<Utc>>>) -> i64 {
    it.flatten().map(nanos).max().unwrap_or(0)
}

/// What the announcements establish for `(P, D]` and one symbol (module docs).
#[derive(Clone, Debug, PartialEq)]
pub enum ActionCoverage {
    Confirmed {
        splits: Vec<(SplitRatio, String)>,
        /// Π old/new.
        price_multiplier: f64,
        retrieval: String,
        at: DateTime<Utc>,
        history_symbol: Option<String>,
    },
    NoKnownAction {
        retrieval: String,
        at: DateTime<Utc>,
    },
    Unresolved {
        reason: &'static str,
        retrieval: String,
        at: DateTime<Utc>,
    },
    Stale {
        last: String,
        at: DateTime<Utc>,
    },
    Unavailable(&'static str),
}

impl ActionCoverage {
    pub fn state(&self) -> CoverageState {
        match self {
            ActionCoverage::Confirmed { .. } => CoverageState::ConfirmedAction,
            ActionCoverage::NoKnownAction { .. } => CoverageState::NoKnownActionProviderChecked,
            ActionCoverage::Unresolved { .. } => CoverageState::UnresolvedAction,
            ActionCoverage::Stale { .. } => CoverageState::StaleCoverage,
            ActionCoverage::Unavailable(_) => CoverageState::UnavailableCoverage,
        }
    }

    pub fn reason(&self) -> &'static str {
        match self {
            ActionCoverage::Confirmed { .. } => "confirmed_action",
            ActionCoverage::NoKnownAction { .. } => "no_known_action_provider_checked",
            ActionCoverage::Unresolved { reason, .. } => reason,
            ActionCoverage::Stale { .. } => "stale_coverage",
            ActionCoverage::Unavailable(why) => why,
        }
    }

    fn at(&self) -> Option<DateTime<Utc>> {
        match self {
            ActionCoverage::Confirmed { at, .. }
            | ActionCoverage::NoKnownAction { at, .. }
            | ActionCoverage::Unresolved { at, .. }
            | ActionCoverage::Stale { at, .. } => Some(*at),
            ActionCoverage::Unavailable(_) => None,
        }
    }
}

/// The announcement rule for symbol `s` and `(P, D]`, as known at `as_of`:
///
/// 1. Usable retrievals: made for a process-date window containing the policy's window around D,
///    after P's last session ended, at or before `as_of`, and covering `s`. None →
///    `Unavailable`. The latest one older than the freshness limit → `Stale`.
/// 2. Actions are the union over usable retrievals (latest version per id). An action relevant to
///    the window that an earlier check listed and the latest omits → unresolved.
/// 3. Identity: a name change into or out of `s`, or a reverse split whose `new_symbol` is `s`,
///    effective in the window, adds the other ticker to the alias set (the history symbol when it
///    is the old one); every alias must be covered by the latest check.
/// 4. Relevant = splits on an alias effective in `(P, D]`, plus any undated action on an alias.
///    Unit splits, undated or incomplete actions, fractional rates and same-day conflicts →
///    unresolved. None → `NoKnownAction` (if the policy permits the inference); otherwise the
///    product of the splits → `Confirmed`.
pub fn coverage_for(
    retrievals: &[ActionRetrieval],
    policy: &CoveragePolicy,
    s: &str,
    p: NaiveDate,
    d: NaiveDate,
    as_of: DateTime<Utc>,
) -> ActionCoverage {
    let Some(after_p) = session_end(p) else {
        return ActionCoverage::Unavailable("not_a_trading_day");
    };
    let (want_start, want_end) = policy.process_window(d);
    let usable: Vec<&ActionRetrieval> = retrievals
        .iter()
        .filter(|r| {
            r.process_start <= want_start
                && r.process_end >= want_end
                && r.retrieved_at >= after_p
                && r.retrieved_at <= as_of
                && r.covers(s)
        })
        .collect();
    let Some(latest) = usable.iter().max_by_key(|r| r.retrieved_at).copied() else {
        return ActionCoverage::Unavailable("no_usable_retrieval");
    };
    let (rid, at) = (latest.id(), latest.retrieved_at);
    if (as_of - at).num_seconds() > policy.body.freshness_secs {
        return ActionCoverage::Stale { last: rid, at };
    }
    let unresolved = |reason| ActionCoverage::Unresolved {
        reason,
        retrieval: rid.clone(),
        at,
    };
    // Union by id, the latest retrieval's version winning.
    let mut by_id: BTreeMap<&str, (&CorporateAction, bool)> = BTreeMap::new();
    for r in &usable {
        let is_latest = std::ptr::eq(*r, latest);
        for a in &r.actions {
            let e = by_id.entry(a.id.as_str()).or_insert((a, is_latest));
            if is_latest {
                *e = (a, true);
            }
        }
    }
    let within = |x: NaiveDate| x > p && x <= d;
    // Identity chain (bounded: a ticker rarely changes twice in a day).
    let mut aliases: BTreeSet<String> = BTreeSet::from([s.to_string()]);
    let mut history_symbol: Option<String> = None;
    for _ in 0..3 {
        let before = aliases.len();
        for (a, _) in by_id.values() {
            let Some(eff) = a.effective_date else {
                continue;
            };
            if !within(eff) {
                continue;
            }
            let (old, new) = (a.symbol.as_deref(), a.new_symbol.as_deref());
            let renames = matches!(a.kind, crate::ActionKind::NameChange)
                || (a.kind.is_split() && new.is_some() && new != old);
            if !renames {
                continue;
            }
            if let (Some(o), Some(n)) = (old, new) {
                if aliases.contains(n) && !aliases.contains(o) {
                    aliases.insert(o.to_string());
                    if n == s {
                        history_symbol = Some(o.to_string());
                    }
                } else if aliases.contains(o) && !aliases.contains(n) {
                    aliases.insert(n.to_string());
                }
            } else if a.touches(s) {
                return unresolved("incomplete_name_change");
            }
        }
        if aliases.len() == before {
            break;
        }
    }
    if aliases.iter().any(|x| !latest.covers(x)) {
        return unresolved("rename_chain_not_covered");
    }
    let mut splits: Vec<&CorporateAction> = Vec::new();
    for (a, in_latest) in by_id.values() {
        if !aliases.iter().any(|x| a.touches(x)) {
            continue;
        }
        let relevant = a.effective_date.is_none_or(within);
        if !relevant {
            continue;
        }
        if a.effective_date.is_none() {
            return unresolved("undated_action");
        }
        if !in_latest {
            return unresolved("action_missing_from_latest_check");
        }
        match a.kind {
            crate::ActionKind::UnitSplit => return unresolved("unit_split"),
            crate::ActionKind::NameChange => {}
            _ => {
                if !a.complete {
                    return unresolved("incomplete_action");
                }
                splits.push(a);
            }
        }
    }
    // Duplicates (same ticker, date and terms under two ids) count once; different terms on the
    // same date conflict.
    let mut seen: BTreeMap<(String, NaiveDate), (Option<f64>, Option<f64>)> = BTreeMap::new();
    let mut applied = Vec::new();
    let mut multiplier = 1.0;
    splits.sort_by_key(|a| (a.effective_date, a.id.clone()));
    for a in splits {
        let key = (
            a.symbol.clone().unwrap_or_default(),
            a.effective_date.expect("dated"),
        );
        match seen.get(&key) {
            Some(terms) if *terms == (a.old_rate, a.new_rate) => continue,
            Some(_) => return unresolved("conflicting_actions"),
            None => {
                seen.insert(key, (a.old_rate, a.new_rate));
            }
        }
        let Some((old, new)) = a.integral_rates() else {
            return unresolved("non_integral_ratio");
        };
        multiplier *= old as f64 / new as f64;
        applied.push((
            SplitRatio {
                old_rate: old,
                new_rate: new,
            },
            a.id.clone(),
        ));
    }
    if applied.is_empty() {
        if !policy.body.no_known_action_permits_unchanged_basis {
            return ActionCoverage::Unavailable("policy_forbids_absence_inference");
        }
        if history_symbol.is_some() {
            // A rename without a split: same shares, different ticker.
            return ActionCoverage::Confirmed {
                splits: Vec::new(),
                price_multiplier: 1.0,
                retrieval: rid,
                at,
                history_symbol,
            };
        }
        return ActionCoverage::NoKnownAction { retrieval: rid, at };
    }
    ActionCoverage::Confirmed {
        splits: applied,
        price_multiplier: multiplier,
        retrieval: rid,
        at,
        history_symbol,
    }
}

/// Builds the gain and RVOL evidence for `symbol` on evaluated date `d` (module docs), looking at
/// up to `lookback` trading dates before `d`.
pub fn load(inputs: &Inputs<'_>, symbol: &str, d: NaiveDate, lookback: usize) -> Loaded {
    let instrument = InstrumentId::symbol(symbol);
    let p = previous_trading_day(d);
    let snap_at = inputs
        .split
        .map(|s| s.fetch.retrieved_at)
        .filter(|t| *t <= inputs.as_of);
    let cov = coverage_for(
        inputs.announcements,
        inputs.policy,
        symbol,
        p,
        d,
        inputs.as_of,
    );
    let hist = match &cov {
        ActionCoverage::Confirmed {
            history_symbol: Some(h),
            ..
        } => h.clone(),
        _ => symbol.to_string(),
    };
    let renamed = hist != symbol;
    // Announcement-based evidence for (P, D], when it permits a calculation.
    let announced = |bar_through: Option<NaiveDate>| -> Option<(ActionsEvidence, f64)> {
        let (source, splits, mult, assumption, retrieval) = match &cov {
            ActionCoverage::Confirmed {
                splits,
                price_multiplier,
                retrieval,
                ..
            } => (
                ActionsSource::AnnouncedAction,
                splits.clone(),
                *price_multiplier,
                None,
                retrieval,
            ),
            ActionCoverage::NoKnownAction { retrieval, .. } => (
                ActionsSource::ProviderCheckedNoKnownAction,
                Vec::new(),
                1.0,
                Some(UNCHANGED_BASIS_ASSUMED.to_string()),
                retrieval,
            ),
            _ => return None,
        };
        Some((
            ActionsEvidence {
                source,
                basis_date: Some(d),
                bar_verified_through: bar_through,
                coverage: Some(cov.state()),
                coverage_policy: Some(inputs.policy.tag.clone()),
                announcement: Some(retrieval.clone()),
                applied_splits: splits,
                assumption,
                history_symbol: renamed.then(|| hist.clone()),
            },
            mult,
        ))
    };
    let mut out = Loaded {
        prev_close: None,
        prev_close_reason: "",
        baseline: None,
        baseline_reason: "",
        checked_through: None,
        coverage: cov.clone(),
    };

    // ---- previous close (P's raw close, under the history ticker)
    match raw_cov(inputs.raw, &hist, p) {
        None => out.prev_close_reason = "previous_day_not_covered",
        Some(None) => out.prev_close_reason = "no_bar_on_previous_day",
        Some(Some(bar)) => match Price::from_f64_rounded(bar.c).filter(|p| p.micros() > 0) {
            None => out.prev_close_reason = "nonpositive_close",
            Some(raw_close) => {
                let raw_seen = inputs.raw.get(&p).and_then(|r| r.last_retrieved());
                // Bars are only read under one ticker; across a rename they do not apply.
                let bt = if renamed {
                    None
                } else {
                    bars_through(inputs, symbol, p, d)
                };
                let bar_through = bt.map(|(e, _)| e);
                let restated =
                    |m: f64| Price::from_f64_rounded(bar.c * m).map_or(0, |p| p.micros());
                let (close_micros, basis, through, actions, reason, observed) = match bt {
                    Some((e, m)) if e >= d => (
                        if m == 1.0 {
                            raw_close.micros()
                        } else {
                            restated(m)
                        },
                        if m == 1.0 {
                            CloseBasis::Raw
                        } else {
                            CloseBasis::SplitAdjusted
                        },
                        Some(e),
                        ActionsEvidence {
                            source: ActionsSource::BarVerified,
                            basis_date: Some(d),
                            bar_verified_through: Some(e),
                            ..ActionsEvidence::default()
                        },
                        "bar_verified_through_evaluated_date",
                        [raw_seen, snap_at],
                    ),
                    _ => match announced(bar_through) {
                        Some((ev, m)) => (
                            if m == 1.0 {
                                raw_close.micros()
                            } else {
                                restated(m)
                            },
                            if m == 1.0 {
                                CloseBasis::Raw
                            } else {
                                CloseBasis::SplitAdjusted
                            },
                            Some(d),
                            ev,
                            cov.reason(),
                            [raw_seen, cov.at()],
                        ),
                        None => (
                            raw_close.micros(),
                            CloseBasis::Raw,
                            bar_through.filter(|e| *e < d),
                            ActionsEvidence {
                                source: if bar_through.is_some() {
                                    ActionsSource::BarVerified
                                } else {
                                    ActionsSource::Unchecked
                                },
                                basis_date: Some(p),
                                bar_verified_through: bar_through,
                                coverage: Some(cov.state()),
                                coverage_policy: Some(inputs.policy.tag.clone()),
                                ..ActionsEvidence::default()
                            },
                            match (&cov, bar_through) {
                                // Nothing usable from announcements: say what the bars gave.
                                (ActionCoverage::Unavailable(_), Some(_)) => {
                                    "bar_verified_before_evaluated_date"
                                }
                                (ActionCoverage::Unavailable(_), None)
                                    if inputs.split.is_none() =>
                                {
                                    "no_split_snapshot"
                                }
                                (ActionCoverage::Unavailable(_), None) => "split_and_raw_disagree",
                                _ => cov.reason(),
                            },
                            [raw_seen, snap_at.filter(|_| bar_through.is_some())],
                        ),
                    },
                };
                out.checked_through = through;
                out.prev_close_reason = reason;
                out.prev_close = Some(PrevClose {
                    id: format!("{METRIC}:prev_close:{symbol}:{d}"),
                    version: 1,
                    instrument: instrument.clone(),
                    date: p,
                    close_micros,
                    basis,
                    actions_checked_through: through,
                    actions,
                    observed_at: latest_retrieval(observed.into_iter()),
                });
            }
        },
    }

    // ---- baseline: the `lookback` trading dates strictly before d, under the history ticker
    let mut dates = Vec::with_capacity(lookback);
    let mut x = d;
    while dates.len() < lookback {
        x = previous_trading_day(x);
        dates.push(x);
    }
    dates.reverse();
    let bar_d = if renamed {
        None
    } else {
        volume_factors(inputs, symbol, &dates, d)
    };
    let (factors, basis, extra_at) = match bar_d {
        Some(f) => (
            f,
            ActionsEvidence {
                source: ActionsSource::BarVerified,
                basis_date: Some(d),
                bar_verified_through: Some(d),
                ..ActionsEvidence::default()
            },
            None,
        ),
        None => {
            let via_p = volume_factors(inputs, &hist, &dates, p);
            match (via_p, announced(Some(p))) {
                (Some(f), Some((ev, price_m))) => {
                    // Volumes move by the reciprocal of the price multiplier.
                    let vol_m = 1.0 / price_m;
                    (
                        f.into_iter()
                            .map(|(x, r)| (x, snap_one(r * vol_m)))
                            .collect(),
                        ev,
                        cov.at(),
                    )
                }
                (via_p, _) => {
                    out.baseline_reason = if inputs.split.is_none() {
                        "no_split_snapshot"
                    } else if via_p.is_none() {
                        "share_basis_unverified"
                    } else {
                        cov.reason()
                    };
                    return out;
                }
            }
        }
    };
    let mut sessions = Vec::new();
    let mut seen = Vec::new();
    for &x in &dates {
        debug_assert!(x < d);
        let Some(Some(bar)) = raw_cov(inputs.raw, &hist, x) else {
            continue; // missing, not zero
        };
        let (Some(v), Some(&r)) = (bar.v, factors.get(&x)) else {
            continue; // no volume, or this date's basis unverified
        };
        let volume = if r == 1.0 {
            v
        } else {
            (v as f64 * r).round() as u64
        };
        seen.push(inputs.raw.get(&x).and_then(|r| r.last_retrieved()));
        sessions.push(SessionVolume {
            date: x,
            volume,
            complete: true,
            comparable: calendar_covers(x) && day_kind(x) == DayKind::Full,
        });
    }
    seen.push(snap_at);
    seen.push(extra_at);
    let comparable = sessions.iter().filter(|s| s.comparable).count();
    out.baseline_reason = if comparable >= 30 {
        "ok"
    } else {
        "fewer_than_30_comparable"
    };
    out.baseline = Some(DailyBaseline {
        id: format!("{METRIC}:baseline:{symbol}:{d}"),
        version: 1,
        instrument,
        sessions,
        basis,
        observed_at: latest_retrieval(seen.into_iter()),
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(c: f64, v: u64) -> DailyBar {
        DailyBar {
            o: None,
            h: None,
            l: None,
            c,
            v: Some(v),
            n: Some(42),
            vw: None,
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
    }

    #[test]
    fn reverse_split_one_for_ten_multiplies_price_and_divides_volume() {
        // f = old/new = 10/1: $0.08 → $0.80, 1,000,000 → 100,000; P·V preserved.
        let raw = bar(0.08, 1_000_000);
        let adj = bar(0.80, 100_000);
        let m = split_consistency(&raw, &adj).unwrap();
        assert!(close(m, 10.0));
        assert!(close(
            raw.c * raw.v.unwrap() as f64,
            adj.c * adj.v.unwrap() as f64
        ));
        // Volume moved the same way as price: not a split adjustment.
        assert_eq!(
            split_consistency(&raw, &bar(0.80, 10_000_000)),
            Err("volume_not_reciprocal")
        );
    }

    #[test]
    fn forward_split_two_for_one() {
        // f = 1/2: $10.00 → $5.00, 1,000 → 2,000.
        let m = split_consistency(&bar(10.0, 1_000), &bar(5.0, 2_000)).unwrap();
        assert!(close(m, 0.5));
    }

    #[test]
    fn two_intervening_splits_multiply() {
        // 2-for-1 then 1-for-10 after x: f = (1/2)·(10/1) = 5. $1.00 → $5.00, 50,000 → 10,000.
        let m = split_consistency(&bar(1.0, 50_000), &bar(5.0, 10_000)).unwrap();
        assert!(close(m, 5.0));
    }

    #[test]
    fn unchanged_dates_have_multiplier_one() {
        let m = split_consistency(&bar(3.21, 12_345), &bar(3.21, 12_345)).unwrap();
        assert_eq!(snap_one(m), 1.0);
    }

    #[test]
    fn zero_volume_edges() {
        // Zero stays zero; a few raw shares divided by 10 round to zero within one share.
        assert!(split_consistency(&bar(0.1, 0), &bar(1.0, 0)).is_ok());
        assert!(split_consistency(&bar(0.1, 3), &bar(1.0, 0)).is_ok());
        assert_eq!(
            split_consistency(&bar(0.1, 0), &bar(1.0, 500)),
            Err("volume_not_reciprocal")
        );
    }

    #[test]
    fn rounding_of_adjusted_prices_is_tolerated_not_equated() {
        // 3-for-1 forward: $0.31 → 0.103333… published as 0.1033; 99,999 → 299,997 (299,998 ±1).
        let m = split_consistency(&bar(0.31, 99_999), &bar(0.1033, 299_998)).unwrap();
        assert!((m - 1.0 / 3.0).abs() < 0.001);
        assert_ne!(m, 1.0 / 3.0, "never relies on exact float equality");
        // Two dates on the same basis whose rounded multipliers differ slightly snap to 1.
        let m2 = split_consistency(&bar(0.32, 10), &bar(0.1067, 30)).unwrap();
        assert_eq!(snap_one(m / m2), 1.0);
    }
}
