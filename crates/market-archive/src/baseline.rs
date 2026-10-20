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
//! `SplitAdjusted` otherwise. The gain pillar needs E ≥ D, which this can only give once D's bar
//! exists in a snapshot fetched after D: replay or research of past dates, never live. Live gain
//! stays UNKNOWN until an announcements source can vouch for D (plan §11). Without a snapshot the
//! raw close is still emitted with `None`, so the engine reports `gain_vs_stored_close_bp` and the
//! pillar stays UNKNOWN (`adjustment_coverage_unknown`).
//!
//! "Actions" here means what Alpaca's `adjustment=split` restates: splits and reverse splits.
//! Spin-offs, cash dividends and symbol changes are not covered (`adjustment=all` would fold
//! dividends into the same factor and is never used for this split-only rule).
//!
//! `observed_at` of each record is the latest retrieval among the files it used, in nanoseconds:
//! evidence becomes visible to the engine only from then, so a snapshot fetched after D is
//! future knowledge to a live decision on D (the engine's leakage rule handles that, not this).

use crate::daily_bars::{DailyBar, RawDayBars, SplitSnapshot};
use chrono::{DateTime, NaiveDate, Utc};
use market_events::session::{
    DayKind, calendar_covers, day_kind, is_trading_day, previous_trading_day, sessions_for,
};
use market_events::{Price, UnixNanos};
use std::collections::BTreeMap;
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
    /// E in the module rule, for the P..=D chain (what `actions_checked_through` holds).
    pub checked_through: Option<NaiveDate>,
}

/// The inputs for one symbol: every raw date file in range and at most one split snapshot.
pub struct Inputs<'a> {
    pub raw: &'a BTreeMap<NaiveDate, RawDayBars>,
    pub split: Option<&'a SplitSnapshot>,
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

/// Builds the gain and RVOL evidence for `symbol` on evaluated date `d` (module docs), looking at
/// up to `lookback` trading dates before `d`.
pub fn load(inputs: &Inputs<'_>, symbol: &str, d: NaiveDate, lookback: usize) -> Loaded {
    let instrument = InstrumentId::symbol(symbol);
    let p = previous_trading_day(d);
    let snap_at = inputs.split.map(|s| s.fetch.retrieved_at);

    // ---- previous close
    let mut out = Loaded {
        prev_close: None,
        prev_close_reason: "",
        baseline: None,
        baseline_reason: "",
        checked_through: None,
    };
    match raw_cov(inputs.raw, symbol, p) {
        None => out.prev_close_reason = "previous_day_not_covered",
        Some(None) => out.prev_close_reason = "no_bar_on_previous_day",
        Some(Some(bar)) => match Price::from_f64_rounded(bar.c).filter(|p| p.micros() > 0) {
            None => out.prev_close_reason = "nonpositive_close",
            Some(raw_close) => {
                let raw_seen = inputs.raw.get(&p).and_then(|r| r.last_retrieved());
                let ct = checked_through(inputs, symbol, p, d);
                let (close_micros, basis, through, observed) = match ct {
                    // `snap_one` makes a same-basis ratio exactly 1.0.
                    Some((e, 1.0)) => (
                        raw_close.micros(),
                        CloseBasis::Raw,
                        Some(e),
                        [raw_seen, snap_at],
                    ),
                    Some((e, ratio)) => (
                        Price::from_f64_rounded(bar.c / ratio).map_or(0, |p| p.micros()),
                        CloseBasis::SplitAdjusted,
                        Some(e),
                        [raw_seen, snap_at],
                    ),
                    None => (raw_close.micros(), CloseBasis::Raw, None, [raw_seen, None]),
                };
                out.checked_through = through;
                out.prev_close_reason = match through {
                    Some(e) if e >= d => "checked_through_evaluated_date",
                    Some(_) => "checked_before_evaluated_date",
                    None if inputs.split.is_none() => "no_split_snapshot",
                    None => "split_and_raw_disagree",
                };
                out.prev_close = Some(PrevClose {
                    id: format!("{METRIC}:prev_close:{symbol}:{d}"),
                    version: 1,
                    instrument: instrument.clone(),
                    date: p,
                    close_micros,
                    basis,
                    actions_checked_through: through,
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
    let Some(factors) = basis_factors(inputs, symbol, &dates, d) else {
        out.baseline_reason = if inputs.split.is_none() {
            "no_split_snapshot"
        } else {
            "share_basis_unverified"
        };
        return out;
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
        observed_at: latest_retrieval(seen.into_iter()),
    });
    out
}
