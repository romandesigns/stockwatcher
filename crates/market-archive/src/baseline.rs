//! Stored daily bars → stock-context evidence: [`stock_context::DailyBaseline`] (RVOL's 30-session
//! denominator) and [`stock_context::PrevClose`] (the gain's reference), for one symbol and one
//! evaluated trading date D. Pure: the caller reads the files ([`crate::daily_bars`]) and passes
//! them in; nothing here touches the disk, a clock or a provider. Not wired into the server.
//!
//! Metric: the sessions' volume is **Alpaca's SIP daily-bar volume** (`provider_daily_volume_v1`
//! in every record id), not our condition-filtered 04:00 → end aggregation. Whether the two are
//! comparable is the open compatibility check of `docs/stock-context/daily-baseline-plan.md` §9;
//! until it passes, these records must not feed the strict pillar.
//!
//! # Rules
//!
//! - **Only dates before D** are sessions. D's own bar (and anything later) is used for one thing
//!   only, finding D's share basis, never as volume.
//! - **Missing is missing.** A date with no covered bar is left out, never a zero session. A date
//!   whose raw file was fetched before the date's last session ended is not covered.
//! - **`complete`** = a raw bar with a volume, covered after the session ended. **`comparable`** =
//!   a full trading day in the calendar (`market_events::session`): an early close (13:00) is not
//!   comparable, and neither is a date outside the calendar's years, whose early closes are
//!   unknown.
//! - **Share basis.** Volumes must be on D's share basis (a 1:10 reverse split the morning of D
//!   makes every raw prior volume 10× too large). Daily bars can only establish that after the
//!   fact, from a split-adjusted snapshot that contains D's bar. [`basis_factors`] has the rule.
//!   No basis through D → **no baseline** (`share_basis_unverified`), so RVOL stays UNKNOWN.
//!
//! # `actions_checked_through` (precise rule)
//!
//! Let P = the trading day before D, raw `R(x)` and split-adjusted `A(x)` the bars of date x
//! (one snapshot, one retrieval), `f(x) = R(x).c / A(x).c` the cumulative split factor from x to
//! the snapshot's retrieval. A date x is **verified** when:
//!
//! 1. both sets cover x (raw: requested and fetched after x's last session ended; snapshot: symbol
//!    requested, x inside `from..=to`, retrieved after x's last session ended), and
//! 2. they agree on presence (both have a bar, or neither), and when both have one:
//! 3. both closes are finite and positive, the trade counts `n` are equal when both are present
//!    (adjustment never changes a count), and the volume moved by the same factor as the price
//!    (`|A.v − R.v·f| ≤ max(1, 1 % of R.v·f)`), which is what a split-only adjustment does and a
//!    dividend or other price-only adjustment does not.
//!
//! `actions_checked_through` = **E, the latest date in P..=D such that every trading date from P
//! to E is verified and both sets have a bar on E**; `None` when P itself is not verified with a
//! bar on both sides. The close is then restated onto E's basis: `R(P).c · f(E) / f(P)`, with
//! basis `Raw` when that ratio is 1 (within 0.2 %, then the raw close is used exactly) and
//! `SplitAdjusted` otherwise. The gain pillar needs E ≥ D, which bars alone can only give once
//! D's bar exists in a snapshot fetched after D: replay or research of past dates. Such a record
//! is labelled `ActionsSource::BarVerified`.
//!
//! # Live: announcement coverage (weaker evidence, labelled `ActionsSource::Announced`)
//!
//! When bars do not reach D, [`announced`] consults the corporate-action retrievals
//! ([`crate::corporate_actions`]). It uses the **latest** retrieval that covered the symbol (its
//! request chunk completed), whose range includes `(P, D]` (`start ≤ P+1`, `end ≥ D`) and that was
//! made **after P's last session ended and before D's 09:30 open**. Then:
//!
//! - a name change touching the symbol in `(P, D]` → unknown (`symbol_changed`: Alpaca files
//!   actions under the ticker in force, so a rename makes the lookup ambiguous);
//! - a unit split touching it in `(P, D]` (or undated) → unknown (`unit_split`);
//! - no forward/reverse split for it with `ex_date` in `(P, D]` → **no split**: the raw close of P
//!   (on P's basis by definition) is D's reference, `actions_checked_through = D`;
//! - exactly one, with integral rates → the close is restated by `old_rate/new_rate` and prior
//!   volumes by `new_rate/old_rate`, the ratio and action id recorded (`applied_split`);
//! - several, or a fractional rate → unknown.
//!
//! No covering retrieval (none made, fetch failed for that chunk, made too late) → unknown, and
//! the record stays as bars alone allow (`checked_before_evaluated_date` or `None`). The baseline
//! uses the announcement only on top of bars that verify P (factors relative to P), so the prior
//! history is still bar-verified and only the last day is announced; the record says
//! `bar_verified_through = P`. `observed_at` includes the retrieval's time.
//!
//! Without a snapshot and without an announcement the raw close is still emitted with `None`, so
//! the engine reports `gain_vs_stored_close_bp` and the pillar stays UNKNOWN
//! (`adjustment_coverage_unknown`).
//!
//! "Actions" here means what Alpaca's `adjustment=split` restates: splits and reverse splits.
//! Spin-offs, cash dividends and symbol changes are not covered (`adjustment=all` would fold
//! dividends into the same factor and is never used for this split-only rule).
//!
//! `observed_at` of each record is the latest retrieval among the files it used, in nanoseconds:
//! evidence becomes visible to the engine only from then, so a snapshot fetched after D is
//! future knowledge to a live decision on D (the engine's leakage rule handles that, not this).

use crate::corporate_actions::{ActionRetrieval, SplitAction};
use crate::daily_bars::{DailyBar, RawDayBars, SplitSnapshot};
use chrono::{DateTime, NaiveDate, Utc};
use market_events::session::{
    DayKind, calendar_covers, day_kind, is_trading_day, previous_trading_day, sessions_for,
};
use market_events::{Price, SessionKind, UnixNanos};
use std::collections::BTreeMap;
/// Re-exported so callers need no direct stock-context dependency for the provenance types.
pub use stock_context::{ActionsEvidence, ActionsSource, SplitRatio};
use stock_context::{CloseBasis, DailyBaseline, InstrumentId, PrevClose, SessionVolume};

/// The metric these records measure, carried in every id.
pub const METRIC: &str = "provider_daily_volume_v1";
/// Trading dates before D looked at for sessions: 30 needed, plus room for early closes,
/// holidays-adjacent gaps and days without a bar. An engineering choice, not validated.
pub const DEFAULT_LOOKBACK: usize = 45;
/// Price/volume factor agreement (see the module rules).
const VOLUME_FACTOR_TOL: f64 = 0.01;
/// A factor ratio this close to 1 is "no split in between" (absorbs 4-decimal price rounding on
/// sub-dollar stocks; the smallest real splits are ~5 %).
const SAME_BASIS_TOL: f64 = 0.002;

/// Why a record was or was not produced, for logs and tests.
#[derive(Clone, Debug, PartialEq)]
pub struct Loaded {
    pub prev_close: Option<PrevClose>,
    pub prev_close_reason: &'static str,
    pub baseline: Option<DailyBaseline>,
    pub baseline_reason: &'static str,
    /// What `actions_checked_through` holds (E from bars, or D from an announcement).
    pub checked_through: Option<NaiveDate>,
    /// What the announcements said, whether or not it was needed.
    pub announced: Announced,
}

/// The inputs for one symbol: every raw date file in range, at most one split snapshot (one
/// snapshot is one basis: [`crate::Archive::latest_snapshot_for`]) and the announcement
/// retrievals for D (`Archive::read_action_retrievals(D)`; may be empty).
pub struct Inputs<'a> {
    pub raw: &'a BTreeMap<NaiveDate, RawDayBars>,
    pub split: Option<&'a SplitSnapshot>,
    pub announcements: &'a [ActionRetrieval],
}

/// Instant the date's last session (after hours, or the regular session's end on a day without
/// one) ends; `None` for a non-trading day.
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

/// One date's verification (module rules 1–3).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Verified {
    /// Both sides have a consistent bar; `f` = raw close / adjusted close.
    Bar { f: f64 },
    /// Both covered, neither has a bar.
    NoBar,
    /// Not covered by one side, presence disagrees, or the bars are inconsistent.
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
        (Some(r), Some(a)) => {
            if !(r.c.is_finite() && a.c.is_finite() && r.c > 0.0 && a.c > 0.0) {
                return Verified::Unverified;
            }
            if let (Some(rn), Some(an)) = (r.n, a.n)
                && rn != an
            {
                return Verified::Unverified;
            }
            let f = r.c / a.c;
            if let (Some(rv), Some(av)) = (r.v, a.v) {
                let want = rv as f64 * f;
                if (av as f64 - want).abs() > (want * VOLUME_FACTOR_TOL).max(1.0) {
                    return Verified::Unverified;
                }
            }
            Verified::Bar { f }
        }
        _ => Verified::Unverified,
    }
}

/// Trading dates in `from..=to`, ascending.
fn trading_dates(from: NaiveDate, to: NaiveDate) -> Vec<NaiveDate> {
    from.iter_days()
        .take_while(|d| *d <= to)
        .filter(|d| is_trading_day(*d))
        .collect()
}

/// `r(x) = f(x) / f(D)` for every verified-with-bar date x in `dates`, or `None` when D itself is
/// not verified with a bar on both sides (its basis cannot be named). `r = 1` exactly when within
/// [`SAME_BASIS_TOL`]. Each x stands alone: one snapshot is one basis, so a gap between x and D
/// does not hide a split (it still shows in `f`).
pub fn basis_factors(
    inputs: &Inputs<'_>,
    symbol: &str,
    dates: &[NaiveDate],
    d: NaiveDate,
) -> Option<BTreeMap<NaiveDate, f64>> {
    let Verified::Bar { f: fd } = verify(inputs, symbol, d) else {
        return None;
    };
    let mut out = BTreeMap::new();
    for &x in dates {
        if let Verified::Bar { f } = verify(inputs, symbol, x) {
            out.insert(x, snap_one(f / fd));
        }
    }
    Some(out)
}

fn snap_one(r: f64) -> f64 {
    if (r - 1.0).abs() <= SAME_BASIS_TOL {
        1.0
    } else {
        r
    }
}

/// E of the module rule over P..=D, with `f(P)/f(E)`.
fn checked_through(
    inputs: &Inputs<'_>,
    symbol: &str,
    p: NaiveDate,
    d: NaiveDate,
) -> Option<(NaiveDate, f64)> {
    let Verified::Bar { f: fp } = verify(inputs, symbol, p) else {
        return None;
    };
    let mut best = (p, 1.0);
    for x in trading_dates(p, d).into_iter().skip(1) {
        match verify(inputs, symbol, x) {
            Verified::Bar { f } => best = (x, snap_one(fp / f)),
            Verified::NoBar => {}
            Verified::Unverified => break,
        }
    }
    Some(best)
}

fn latest_retrieval<'a>(it: impl Iterator<Item = Option<DateTime<Utc>>> + 'a) -> i64 {
    it.flatten().map(nanos).max().unwrap_or(0)
}

/// Start of D's regular session (09:30 ET).
fn regular_open(date: NaiveDate) -> Option<DateTime<Utc>> {
    sessions_for(date)
        .into_iter()
        .find(|s| s.0.kind == SessionKind::Regular)
        .map(|s| s.1.to_utc())
}

/// What the pre-session announcement retrievals say about `(P, D]` for one symbol.
#[derive(Clone, Debug, PartialEq)]
pub enum Announced {
    /// A covering retrieval lists no split for the symbol effective in `(P, D]`.
    NoSplit {
        retrieval: String,
        at: DateTime<Utc>,
    },
    /// Exactly one forward or reverse split effective in `(P, D]`, with integral rates.
    Split {
        ratio: SplitRatio,
        action: String,
        retrieval: String,
        at: DateTime<Utc>,
    },
    /// Nothing can be vouched for (reason).
    Unknown(&'static str),
}

/// The announcement rule (module docs, "Live: announcement coverage").
pub fn announced(
    retrievals: &[ActionRetrieval],
    symbol: &str,
    p: NaiveDate,
    d: NaiveDate,
) -> Announced {
    let (Some(after_p), Some(open_d), Some(day_after_p)) =
        (session_end(p), regular_open(d), p.succ_opt())
    else {
        return Announced::Unknown("not_a_trading_day");
    };
    let Some(r) = retrievals
        .iter()
        .filter(|r| {
            r.covers(symbol)
                && r.start <= day_after_p
                && r.end >= d
                && r.retrieved_at >= after_p
                && r.retrieved_at < open_d
        })
        .max_by_key(|r| r.retrieved_at)
    else {
        return Announced::Unknown("no_announcement_coverage");
    };
    let within = |x: NaiveDate| x > p && x <= d;
    // F7: actions are filed under the ticker in force at the time; a rename in the window makes
    // "which ticker" ambiguous, so nothing is vouched for.
    if r.name_changes
        .iter()
        .any(|n| within(n.process_date) && (n.old_symbol == symbol || n.new_symbol == symbol))
    {
        return Announced::Unknown("symbol_changed");
    }
    if r.unit_splits.iter().any(|u| {
        let touches = [&u.old_symbol, &u.new_symbol, &u.alternate_symbol]
            .iter()
            .any(|s| s.as_deref() == Some(symbol));
        touches && u.effective_date.is_none_or(within)
    }) {
        return Announced::Unknown("unit_split");
    }
    let splits: Vec<&SplitAction> = r
        .splits
        .iter()
        .filter(|s| s.symbol == symbol && within(s.ex_date))
        .collect();
    match splits.as_slice() {
        [] => Announced::NoSplit {
            retrieval: r.id(),
            at: r.retrieved_at,
        },
        [s] => {
            let integral = |x: f64| x.is_finite() && x >= 1.0 && x.fract() == 0.0;
            if !(integral(s.old_rate) && integral(s.new_rate)) {
                return Announced::Unknown("split_ratio_not_integral");
            }
            Announced::Split {
                ratio: SplitRatio {
                    old_rate: s.old_rate as u64,
                    new_rate: s.new_rate as u64,
                },
                action: s.id.clone(),
                retrieval: r.id(),
                at: r.retrieved_at,
            }
        }
        _ => Announced::Unknown("multiple_splits"),
    }
}

/// Builds the gain and RVOL evidence for `symbol` on evaluated date `d` (module docs), looking at
/// up to `lookback` trading dates before `d`.
pub fn load(inputs: &Inputs<'_>, symbol: &str, d: NaiveDate, lookback: usize) -> Loaded {
    let instrument = InstrumentId::symbol(symbol);
    let p = previous_trading_day(d);
    let snap_at = inputs.split.map(|s| s.fetch.retrieved_at);
    let ann = announced(inputs.announcements, symbol, p, d);
    let ann_at = match &ann {
        Announced::NoSplit { at, .. } | Announced::Split { at, .. } => Some(*at),
        Announced::Unknown(_) => None,
    };
    let announced_evidence = |bar_through: Option<NaiveDate>| -> Option<ActionsEvidence> {
        match &ann {
            Announced::NoSplit { retrieval, .. } => Some(ActionsEvidence {
                source: ActionsSource::Announced,
                bar_verified_through: bar_through,
                announcement: Some(retrieval.clone()),
                applied_split: None,
            }),
            Announced::Split {
                ratio,
                action,
                retrieval,
                ..
            } => Some(ActionsEvidence {
                source: ActionsSource::Announced,
                bar_verified_through: bar_through,
                announcement: Some(retrieval.clone()),
                applied_split: Some((*ratio, action.clone())),
            }),
            Announced::Unknown(_) => None,
        }
    };
    // Price multiplier onto D's basis from the announcement (old/new), and the volume one (new/old).
    let (ann_price, ann_volume) = match &ann {
        Announced::Split { ratio, .. } => (
            ratio.old_rate as f64 / ratio.new_rate as f64,
            ratio.new_rate as f64 / ratio.old_rate as f64,
        ),
        _ => (1.0, 1.0),
    };

    let mut out = Loaded {
        prev_close: None,
        prev_close_reason: "",
        baseline: None,
        baseline_reason: "",
        checked_through: None,
        announced: ann.clone(),
    };

    // ---- previous close
    match raw_cov(inputs.raw, symbol, p) {
        None => out.prev_close_reason = "previous_day_not_covered",
        Some(None) => out.prev_close_reason = "no_bar_on_previous_day",
        Some(Some(bar)) => match Price::from_f64_rounded(bar.c).filter(|p| p.micros() > 0) {
            None => out.prev_close_reason = "nonpositive_close",
            Some(raw_close) => {
                let raw_seen = inputs.raw.get(&p).and_then(|r| r.last_retrieved());
                let ct = checked_through(inputs, symbol, p, d);
                let bar_through = ct.map(|(e, _)| e);
                let bar_evidence = ActionsEvidence {
                    source: ActionsSource::BarVerified,
                    bar_verified_through: bar_through,
                    ..ActionsEvidence::default()
                };
                let restated =
                    |m: f64| Price::from_f64_rounded(bar.c * m).map_or(0, |p| p.micros());
                // (close, basis, through, evidence, reason, observed)
                let (close_micros, basis, through, actions, reason, observed) = match ct {
                    // Bars verify D itself: the strong kind wins.
                    Some((e, ratio)) if e >= d => (
                        // `snap_one` makes a same-basis ratio exactly 1.0.
                        if ratio == 1.0 {
                            raw_close.micros()
                        } else {
                            restated(1.0 / ratio)
                        },
                        if ratio == 1.0 {
                            CloseBasis::Raw
                        } else {
                            CloseBasis::SplitAdjusted
                        },
                        Some(e),
                        bar_evidence,
                        "checked_through_evaluated_date",
                        [raw_seen, snap_at],
                    ),
                    // Otherwise D rests on the announcement. P's raw close is on P's basis by
                    // definition, so only `(P, D]` needs vouching for.
                    _ => match announced_evidence(bar_through) {
                        Some(ev) => (
                            if ev.applied_split.is_some() {
                                restated(ann_price)
                            } else {
                                raw_close.micros()
                            },
                            if ev.applied_split.is_some() {
                                CloseBasis::SplitAdjusted
                            } else {
                                CloseBasis::Raw
                            },
                            Some(d),
                            ev,
                            "announced_through_evaluated_date",
                            [raw_seen, ann_at],
                        ),
                        None => (
                            raw_close.micros(),
                            CloseBasis::Raw,
                            // Bars through P (or nothing): truthful, short of D.
                            bar_through.filter(|e| *e < d),
                            if bar_through.is_some() {
                                bar_evidence
                            } else {
                                ActionsEvidence::default()
                            },
                            match (&ann, bar_through) {
                                (Announced::Unknown(why), _)
                                    if *why != "no_announcement_coverage" =>
                                {
                                    why
                                }
                                (_, Some(_)) => "checked_before_evaluated_date",
                                (_, None) if inputs.split.is_none() => "no_split_snapshot",
                                (_, None) => "split_and_raw_disagree",
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

    // ---- baseline: the `lookback` trading dates strictly before d
    let mut dates = Vec::with_capacity(lookback);
    let mut x = d;
    while dates.len() < lookback {
        x = previous_trading_day(x);
        dates.push(x);
    }
    dates.reverse();
    // Bars verify D's basis (strong), or verify P's and the announcement covers (P, D] (weaker).
    let (factors, basis, extra_at) = match basis_factors(inputs, symbol, &dates, d) {
        Some(f) => (
            f,
            ActionsEvidence {
                source: ActionsSource::BarVerified,
                bar_verified_through: Some(d),
                ..ActionsEvidence::default()
            },
            None,
        ),
        None => {
            let via_p = basis_factors(inputs, symbol, &dates, p);
            match (via_p, announced_evidence(Some(p))) {
                (Some(f), Some(ev)) => (
                    f.into_iter()
                        .map(|(x, r)| (x, snap_one(r * ann_volume)))
                        .collect(),
                    ev,
                    ann_at,
                ),
                (via_p, _) => {
                    out.baseline_reason = if inputs.split.is_none() {
                        "no_split_snapshot"
                    } else if via_p.is_none() {
                        "share_basis_unverified"
                    } else {
                        match &ann {
                            Announced::Unknown(why) => why,
                            _ => "share_basis_unverified",
                        }
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
        let Some(Some(bar)) = raw_cov(inputs.raw, symbol, x) else {
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
