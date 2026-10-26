//! The pre-registered outcome (protocol section 6) and the reference runs (section 7), from the
//! symbol-day's chart-counted prints and engine bars. Exact integer comparisons on micro-dollars.

use crate::replay::{BAR, BarRec, Print, RawSignal, SEC, in_band, scored_session};
use market_events::UnixNanos;
use market_events::session::{classify, session_bounds};
use serde::Serialize;

pub const HORIZON: i64 = 600 * SEC;

#[derive(Clone, Debug, Serialize)]
pub struct Scored {
    pub day: String,
    pub sym: String,
    pub det: &'static str,
    pub var: String,
    pub kind: &'static str,
    pub ep: String,
    pub t0: i64,
    pub bar_end: i64,
    pub session: &'static str,
    pub p0: Option<i64>,
    pub in_band: bool,
    /// `success`, `failure`, `timeout`, `censored`, or `no_price`.
    pub outcome: &'static str,
    /// When it resolved (success completion bar end, or the failure print's execution).
    pub t_res: Option<i64>,
    pub t_first3: Option<i64>,
    pub mfe_bp: Option<i64>,
    pub mae_bp: Option<i64>,
    pub cue_t: Option<i64>,
    pub armed_t: Option<i64>,
}

fn bp(p: i64, p0: i64) -> i64 {
    ((p - p0) as i128 * 10_000 / p0 as i128) as i64
}

/// `P0`: the latest OHLC-counted print executed before `bar_end` and admitted by `t0`.
pub fn p0(prints: &[Print], t0: i64, bar_end: i64) -> Option<i64> {
    let lim = bar_end.min(t0 + 1);
    let n = prints.partition_point(|p| p.exec < lim);
    prints[..n]
        .iter()
        .rev()
        .take(100_000)
        .find(|p| p.admit <= t0)
        .map(|p| p.price)
}

pub fn score(prints: &[Print], bars: &[BarRec], s: &RawSignal, day: &str, sym: &str) -> Scored {
    let session = crate::replay::session_of(s.bar_end - 1);
    let mut out = Scored {
        day: day.to_string(),
        sym: sym.to_string(),
        det: s.det,
        var: s.var.clone(),
        kind: s.kind,
        ep: s.ep.clone(),
        t0: s.t0,
        bar_end: s.bar_end,
        session,
        p0: None,
        in_band: false,
        outcome: "no_price",
        t_res: None,
        t_first3: None,
        mfe_bp: None,
        mae_bp: None,
        cue_t: s.cue_t,
        armed_t: s.armed_t,
    };
    let Some(p0) = p0(prints, s.t0, s.bar_end) else {
        return out;
    };
    out.p0 = Some(p0);
    out.in_band = in_band(p0);
    let seg_end = classify(UnixNanos(s.bar_end - 1))
        .and_then(session_bounds)
        .map_or(s.t0, |(_, e)| e.nanos());
    let horizon = s.t0 + HORIZON;
    let stop = horizon.min(seg_end);
    let up = |p: i64| p as i128 * 10_000 >= p0 as i128 * 10_300;
    let down = |p: i64| p as i128 * 10_000 <= p0 as i128 * 9_850;
    let start = prints.partition_point(|p| p.exec <= s.t0);
    let (mut fail, mut first3) = (None, None);
    let (mut hi, mut lo) = (p0, p0);
    for p in prints[start..].iter().take_while(|p| p.exec < stop) {
        hi = hi.max(p.price);
        lo = lo.min(p.price);
        if fail.is_none() && down(p.price) {
            fail = Some(p.exec);
        }
        if first3.is_none() && up(p.price) {
            first3 = Some(p.exec);
        }
    }
    out.mfe_bp = Some(bp(hi, p0));
    out.mae_bp = Some(bp(lo, p0));
    out.t_first3 = first3;
    // Two adjacent completed bars (consecutive slots) after t0's bar, both closing >= +3 %.
    let k0 = bars.partition_point(|b| b.end - BAR < s.bar_end);
    let mut success = None;
    let mut prev: Option<&BarRec> = None;
    for b in &bars[k0..] {
        if b.end > stop {
            break;
        }
        let ok = b.ohlc.is_some_and(|o| up(o.3));
        if ok && prev.is_some_and(|q| q.end + BAR == b.end && q.ohlc.is_some_and(|o| up(o.3))) {
            success = Some(b.end);
            break;
        }
        prev = Some(b);
    }
    (out.outcome, out.t_res) = match (success, fail) {
        (Some(sx), Some(f)) if f < sx => ("failure", Some(f)),
        (Some(sx), _) => ("success", Some(sx)),
        (None, Some(f)) => ("failure", Some(f)),
        (None, None) if horizon > seg_end => ("censored", None),
        (None, None) => ("timeout", None),
    };
    out
}

/// The first +10 % run within 30 min of the symbol-day (protocol section 7).
#[derive(Clone, Debug, Serialize)]
pub struct RunRef {
    pub low: i64,
    pub t_l: i64,
    pub t_m: i64,
    pub t_c: i64,
}

pub fn first_run(bars: &[BarRec]) -> Option<RunRef> {
    let bars: Vec<(i64, i64, i64)> = bars
        .iter()
        .filter(|b| scored_session(b.end - 1))
        .filter_map(|b| b.ohlc.map(|o| (b.end, o.1, o.2)))
        .collect();
    let mut from = 0;
    for j in 0..bars.len() {
        let (ej, hj, _) = bars[j];
        while bars[from].0 <= ej - 1800 * SEC {
            from += 1;
        }
        // Lowest low in (end_j − 30 min, end_j], latest on ties.
        let (mut l, mut tl) = (i64::MAX, 0);
        for &(e, _, lo) in &bars[from..=j] {
            if lo <= l {
                l = lo;
                tl = e;
            }
        }
        if l > 0 && in_band(l) && hj as i128 * 100 >= l as i128 * 110 {
            let t_m = bars[..=j]
                .iter()
                .find(|&&(e, h, _)| e > tl && h as i128 * 100 >= l as i128 * 105)
                .map_or(ej, |b| b.0);
            return Some(RunRef {
                low: l,
                t_l: tl,
                t_m,
                t_c: ej,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: i64 = 1_791_800_000 * SEC; // some instant; sessions resolved from it

    fn sig(t0: i64, bar_end: i64) -> RawSignal {
        RawSignal {
            det: "b0",
            var: "B0".into(),
            kind: "trigger",
            ep: "x".into(),
            t0,
            bar_end,
            cue_t: None,
            armed_t: None,
        }
    }

    fn bar(end: i64, c: i64) -> BarRec {
        BarRec {
            end,
            ohlc: Some((c, c, c, c)),
        }
    }

    fn pr(exec: i64, price: i64) -> Print {
        Print {
            exec,
            admit: exec,
            price,
        }
    }

    /// A regular-session bar end (2026-10-12 11:00:00 ET = 15:00 UTC).
    fn reg() -> i64 {
        market_events::session::et(
            chrono::NaiveDate::from_ymd_opt(2026, 10, 12).unwrap(),
            11,
            0,
            0,
            0,
        )
        .unwrap()
        .nanos()
    }

    #[test]
    fn success_needs_two_adjacent_closes_and_failure_first_wins() {
        let b = reg();
        let _ = T;
        let prints = vec![pr(b - SEC, 1_000_000), pr(b + 20 * SEC, 1_031_000)];
        let bars = vec![bar(b + BAR, 1_031_000), bar(b + 2 * BAR, 1_031_000)];
        let s = score(&prints, &bars, &sig(b + 2 * SEC, b), "d", "S");
        assert_eq!(s.p0, Some(1_000_000));
        assert_eq!(s.outcome, "success");
        // A gap between the two closes is not adjacent.
        let bars = vec![bar(b + BAR, 1_031_000), bar(b + 3 * BAR, 1_031_000)];
        assert_eq!(
            score(&prints, &bars, &sig(b + 2 * SEC, b), "d", "S").outcome,
            "timeout"
        );
        // A -1.5 % print inside the second bar comes first.
        let mut p2 = prints.clone();
        p2.push(pr(b + 2 * BAR - SEC, 985_000));
        p2.sort_by_key(|p| p.exec);
        let bars = vec![bar(b + BAR, 1_031_000), bar(b + 2 * BAR, 1_031_000)];
        assert_eq!(
            score(&p2, &bars, &sig(b + 2 * SEC, b), "d", "S").outcome,
            "failure"
        );
    }

    #[test]
    fn runs_are_found_from_the_window_low() {
        let b = reg();
        let bars: Vec<BarRec> = (0..10)
            .map(|k| {
                let p = 1_000_000 + k * 20_000;
                BarRec {
                    end: b + k * BAR,
                    ohlc: Some((p, p, p, p)),
                }
            })
            .collect();
        let r = first_run(&bars).unwrap();
        assert_eq!((r.low, r.t_l), (1_000_000, b));
        assert_eq!(r.t_m, b + 3 * BAR);
        assert_eq!(r.t_c, b + 5 * BAR);
    }
}
