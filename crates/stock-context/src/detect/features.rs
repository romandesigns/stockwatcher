//! Causal feature snapshots of one completed candidate bar: return and its robust surprise,
//! range/body expansion, the prior window's context, and the 1.15× diagnostics.
//!
//! The prior window `W` is the 20 slots immediately before the candidate, same segment; the
//! candidate is never in `W` or in the return baseline (it joins the baseline only after it was
//! tested). Empty slots stay empty: they are counted as slots, never as zero-range bars.

use super::math::{bp, ln_ratio_ppm, median_mad4, median2, milli};
use super::{Barrier, ContextClass, Detector, RetEntry, Seg, SymDetect};
use crate::pillar::Tri;
use crate::setup::Bar;
use market_events::UnixNanos;
use market_events::session::{SessionId, classify};
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReturnClass {
    /// The previous slot traded in the same segment: an ordinary 15 s return.
    Adjacent,
    /// The previous traded bar is further back: not a 15 s return (`elapsed_secs` says how far).
    GapEvent,
    /// No earlier traded bar in the segment.
    FirstInSegment,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RangeRelation {
    Inside,
    CrossedWick,
    ClosedAbove,
    Unavailable,
}

/// A prior window of slots and its past-only context.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WindowStats {
    /// Bar end of the first and last slot.
    pub start: i64,
    pub end: i64,
    pub slots_expected: i64,
    pub slots_in_segment: i64,
    /// Slots with at least one price-qualifying print.
    pub valid_slots: i64,
    /// `valid_slots / slots_expected`, thousandths.
    pub a_milli: i64,
    /// Price-qualifying prints in the window.
    pub n_base: u64,
    pub volume: u64,
    /// Dollar volume of the volume-counted prints, whole dollars.
    pub dv_base: u64,
    pub high: Option<i64>,
    pub low: Option<i64>,
    /// The last valid close in the window (`Q`'s denominator).
    pub last_close: Option<i64>,
    /// `(max H − min L) / last close`, ppm.
    pub q_ppm: Option<i64>,
    /// `|ln(last close / first open)|`, ppm.
    pub d_ppm: Option<i64>,
    pub context: ContextClass,
    pub context_reason: &'static str,
    /// `id@version` of the quiet reference read, when one was admissible.
    pub quiet_reference: Option<String>,
    pub quiet_reference_synthetic: Option<bool>,
    pub q_low_ppm: Option<i64>,
    pub d_low_ppm: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BaselineStats {
    /// Valid same-segment adjacent returns used (newest first, at most `returns_max`).
    pub returns_n: usize,
    pub median_ppm: Option<i64>,
    pub mad_ppm: Option<i64>,
    pub s_floor_ppm: Option<i64>,
    pub floor_binding: Option<bool>,
    /// `ready`, `sparse_baseline`, `not_adjacent`.
    pub status: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RangeStats {
    pub r: i64,
    pub median: Option<i64>,
    /// `R / median R`, thousandths, only for a nondegenerate baseline (median > 0).
    pub e_milli: Option<i64>,
    /// `R / max(median R, floor)`, thousandths: the primary ignition's E. `None` when the
    /// baseline is insufficient or its raw median is zero.
    pub e_adj_milli: Option<i64>,
    /// Two increments of the resolution proxy at `C_prev`.
    pub floor: Option<i64>,
    pub floor_binding: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BodyStats {
    pub b: i64,
    pub median: Option<i64>,
    pub e_milli: Option<i64>,
}

/// Everything known about one completed candidate bar at its cutoff. Immutable once logged.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FeatureSnapshot {
    /// `<run>:<symbol>:<bar_end>`.
    pub id: String,
    pub bar_end: i64,
    pub cutoff: i64,
    pub segment: SessionId,
    pub open: i64,
    pub high: i64,
    pub low: i64,
    pub close: i64,
    pub volume: u64,
    pub dollar_volume: u64,
    pub trades: u32,
    pub c_prev: Option<i64>,
    pub c_prev_bar_end: Option<i64>,
    pub return_class: ReturnClass,
    pub elapsed_secs: Option<i64>,
    /// `ln(C / C_prev)`, ppm (also for a gap event: then not a 15 s return).
    pub log_return_ppm: Option<i64>,
    /// `C ≥ 1.01 × C_prev` (exact: `100 C ≥ 101 C_prev`).
    pub return_ge_1pct: Option<bool>,
    pub upward_close: Option<bool>,
    pub bullish_body: bool,
    /// Robust surprise `(r − m) / max(1.4826 MAD, s_floor)`, thousandths; adjacent only.
    pub z_milli: Option<i64>,
    pub baseline: BaselineStats,
    pub range: RangeStats,
    pub body: BodyStats,
    /// Roman's 1.15× observation, literal (unregularized) ratio; diagnostics only.
    pub range_expansion_15pct: Tri,
    pub body_expansion_15pct: Tri,
    pub upper_wick: i64,
    /// Close location within the bar, thousandths (0 = low, 1000 = high); `None` for a flat bar.
    pub close_location_milli: Option<i64>,
    pub window: WindowStats,
    pub prior_range_relation: RangeRelation,
    pub in_active_band: bool,
    pub in_research_band: bool,
    pub increment: i64,
    pub resolution_proxy: String,
}

/// The bar ending exactly at `end`, if one is stored.
pub(crate) fn bar_at(bars: &std::collections::VecDeque<Bar>, end: i64) -> Option<&Bar> {
    let i = bars.partition_point(|b| b.end < end);
    bars.get(i).filter(|b| b.end == end)
}

pub(crate) fn segment_of(end: i64) -> Option<SessionId> {
    classify(UnixNanos(end - 1))
}

/// Stats of the `slots` slots ending at `last_end` (inclusive), restricted to `seg`.
pub(crate) fn window(
    det: &Detector,
    sd: &SymDetect,
    x: &Barrier,
    seg: Seg,
    last_end: i64,
    slots: i64,
) -> WindowStats {
    let p = &det.policy.body;
    let start = last_end - (slots - 1) * x.bar_ns;
    let (mut in_seg, mut valid, mut n, mut vol, mut dv) = (0i64, 0i64, 0u64, 0u64, 0u128);
    let (mut hi, mut lo): (Option<i64>, Option<i64>) = (None, None);
    let (mut first_open, mut last_close) = (None, None);
    let mut end = start;
    while end <= last_end {
        if seg.holds(end, x.bar_ns) {
            in_seg += 1;
            if let Some(bar) = bar_at(x.bars, end) {
                vol += bar.volume;
                dv += bar.dollars_micros;
                if let Some(o) = bar.ohlc {
                    valid += 1;
                    n += bar.trades as u64;
                    hi = Some(hi.map_or(o.high, |h| h.max(o.high)));
                    lo = Some(lo.map_or(o.low, |l| l.min(o.low)));
                    first_open.get_or_insert(o.open);
                    last_close = Some(o.close);
                }
            }
        }
        end += x.bar_ns;
    }
    let a_milli = milli(valid, slots);
    let q_ppm = match (hi, lo, last_close) {
        (Some(h), Some(l), Some(c)) if c > 0 => {
            Some(((h - l) as i128 * 1_000_000 / c as i128) as i64)
        }
        _ => None,
    };
    let d_ppm = match (first_open, last_close) {
        (Some(o), Some(c)) if o > 0 && c > 0 => Some(ln_ratio_ppm(c, o).abs()),
        _ => None,
    };
    let gap = det.gap_overlaps(sd, start - x.bar_ns, last_end);
    let before_requalify = start - x.bar_ns < sd.requalify_from;
    let mut w = WindowStats {
        start,
        end: last_end,
        slots_expected: slots,
        slots_in_segment: in_seg,
        valid_slots: valid,
        a_milli,
        n_base: n,
        volume: vol,
        dv_base: (dv / 1_000_000) as u64,
        high: hi,
        low: lo,
        last_close,
        q_ppm,
        d_ppm,
        context: ContextClass::Insufficient,
        context_reason: "",
        quiet_reference: None,
        quiet_reference_synthetic: None,
        q_low_ppm: None,
        d_low_ppm: None,
    };
    // Precedence: insufficient → sparse → active consolidation → already moving.
    let (ctx, why) = if gap || before_requalify {
        (ContextClass::Insufficient, "data_gap")
    } else if valid < p.baseline.min_valid_slots {
        (
            ContextClass::Insufficient,
            if in_seg < slots {
                "segment_start"
            } else {
                "few_valid_slots"
            },
        )
    } else if a_milli < p.context.a_min_milli || n < p.context.n_base_min {
        (ContextClass::Sparse, "sparse_activity")
    } else {
        // Only a reference observed by this decision's cutoff, computed from sessions strictly
        // before this date and for this segment, may label quietness.
        match det
            .reference(x.symbol, seg.id, x.tau)
            .filter(|r| r.prior_through < seg.id.date && r.segment == seg.id.kind)
        {
            None => (ContextClass::Unavailable, "quiet_reference_unavailable"),
            Some(r) => {
                w.quiet_reference = Some(format!("{}@{}", r.id, r.version));
                w.quiet_reference_synthetic = Some(r.synthetic);
                w.q_low_ppm = Some(r.q_low_ppm);
                w.d_low_ppm = Some(r.d_low_ppm);
                match (q_ppm, d_ppm) {
                    (Some(q), Some(d)) if q <= r.q_low_ppm && d <= r.d_low_ppm => (
                        ContextClass::ActiveConsolidation,
                        if r.short_history {
                            "quiet_short_history"
                        } else {
                            "quiet"
                        },
                    ),
                    (Some(_), Some(_)) => (
                        ContextClass::AlreadyMoving,
                        "range_or_drift_above_reference",
                    ),
                    _ => (ContextClass::Insufficient, "no_price"),
                }
            }
        }
    };
    w.context = ctx;
    w.context_reason = why;
    w
}

/// Feature snapshot of the bar completing at this barrier, if it traded.
pub(crate) fn candidate(
    det: &Detector,
    sd: &SymDetect,
    x: &Barrier,
    seg: Seg,
) -> Option<FeatureSnapshot> {
    let bar = x.bars.back().filter(|t| t.end == x.b)?;
    let o = bar.ohlc?;
    let p = &det.policy.body;
    // C_prev: the last traded bar before the candidate, same segment.
    let prev = x
        .bars
        .iter()
        .rev()
        .skip(1)
        .take_while(|t| seg.holds(t.end, x.bar_ns))
        .find_map(|t| t.ohlc.map(|po| (t.end, po.close)));
    let return_class = match prev {
        Some((e, _)) if e == x.b - x.bar_ns => ReturnClass::Adjacent,
        Some(_) => ReturnClass::GapEvent,
        None => ReturnClass::FirstInSegment,
    };
    let c_prev = prev.map(|(_, c)| c).filter(|&c| c > 0);
    let inc = det.policy.increment(c_prev.unwrap_or(o.close));
    let log_return_ppm = c_prev.map(|c| ln_ratio_ppm(o.close, c));
    let return_ge_1pct = c_prev.map(|c| {
        o.close as i128 * p.ignition.return_min_den as i128
            >= c as i128 * p.ignition.return_min_num as i128
    });
    let w = window(det, sd, x, seg, x.b - x.bar_ns, p.baseline.window_slots);

    // Return baseline: the newest valid same-segment adjacent returns, candidate excluded.
    let sample: Vec<i64> = sd
        .returns
        .iter()
        .rev()
        .filter(|r| r.seg == seg.id && r.end < x.b && r.end - x.bar_ns >= sd.requalify_from)
        .take(p.baseline.returns_max)
        .map(|r| r.ppm)
        .collect();
    let s_floor_ppm = c_prev
        .map(|c| ((p.baseline.s_floor_increments * inc) as i128 * 1_000_000 / c as i128) as i64);
    let mut baseline = BaselineStats {
        returns_n: sample.len(),
        median_ppm: None,
        mad_ppm: None,
        s_floor_ppm,
        floor_binding: None,
        status: "ready",
    };
    let mut z_milli = None;
    if return_class != ReturnClass::Adjacent {
        baseline.status = "not_adjacent";
    } else if sample.len() < p.baseline.returns_min {
        baseline.status = "sparse_baseline";
    } else if let (Some((m2, mad4)), Some(r), Some(sf)) =
        (median_mad4(&sample), log_return_ppm, s_floor_ppm)
    {
        baseline.median_ppm = Some(m2 / 2);
        baseline.mad_ppm = Some(mad4 / 4);
        // Z = ((2r − m2)/2) / max(1.4826·mad4/4, sf), all in exact integers:
        // denominator scaled by 4·10⁴.
        let scaled_mad = p.baseline.mad_scale_e4 as i128 * mad4 as i128;
        let scaled_floor = 40_000i128 * sf as i128;
        baseline.floor_binding = Some(scaled_floor > scaled_mad);
        let den = scaled_mad.max(scaled_floor);
        if den > 0 {
            z_milli = Some(((2 * r as i128 - m2 as i128) * 20_000_000 / den) as i64);
        }
    }

    // Range and body against the window's traded bars.
    let (mut ranges, mut bodies) = (Vec::new(), Vec::new());
    let mut end = w.start;
    while end <= w.end {
        if seg.holds(end, x.bar_ns)
            && let Some(po) = bar_at(x.bars, end).and_then(|t| t.ohlc)
        {
            ranges.push(po.high - po.low);
            bodies.push((po.close - po.open).abs());
        }
        end += x.bar_ns;
    }
    let enough =
        w.valid_slots >= p.baseline.min_valid_slots && !matches!(w.context_reason, "data_gap");
    let r = o.high - o.low;
    let b = (o.close - o.open).abs();
    let med_r = enough.then(|| median2(&ranges)).flatten();
    let med_b = enough.then(|| median2(&bodies)).flatten();
    let floor = c_prev.map(|_| p.baseline.range_floor_increments * inc);
    // With median2 = 2·median: R / median = 2R / median2.
    let e_milli = med_r.filter(|&m| m > 0).map(|m| milli(2 * r, m));
    let (e_adj_milli, floor_binding) = match (med_r, floor) {
        (Some(m), Some(fl)) if m > 0 => {
            let den2 = m.max(2 * fl);
            (Some(milli(2 * r, den2)), Some(2 * fl > m))
        }
        _ => (None, None),
    };
    let eb_milli = med_b.filter(|&m| m > 0).map(|m| milli(2 * b, m));
    let diag = |x: i64, m: Option<i64>, k: i64| match m {
        // x / median ≥ k/1000 ⇔ 2000·x ≥ k·median2.
        Some(m) if m > 0 => {
            if 2000 * x as i128 >= k as i128 * m as i128 {
                Tri::Pass
            } else {
                Tri::Fail
            }
        }
        _ => Tri::Unknown,
    };
    let range_expansion_15pct = diag(r, med_r, p.expansion.range_diag_milli);
    let body_expansion_15pct = diag(b, med_b, p.expansion.body_diag_milli);
    let prior_range_relation = match (w.high, enough) {
        (Some(h), true) if o.close > h => RangeRelation::ClosedAbove,
        (Some(h), true) if o.high > h => RangeRelation::CrossedWick,
        (Some(_), true) => RangeRelation::Inside,
        _ => RangeRelation::Unavailable,
    };
    let u = &p.universe;
    let in_band = |lo: i64, hi: i64| o.close >= lo && o.close <= hi;
    Some(FeatureSnapshot {
        id: format!("{}:{}:{}", x.run_id, x.symbol, x.b),
        bar_end: x.b,
        cutoff: x.tau,
        segment: seg.id,
        open: o.open,
        high: o.high,
        low: o.low,
        close: o.close,
        volume: bar.volume,
        dollar_volume: (bar.dollars_micros / 1_000_000) as u64,
        trades: bar.trades,
        c_prev,
        c_prev_bar_end: prev.map(|(e, _)| e),
        return_class,
        elapsed_secs: prev.map(|(e, _)| (x.b - e) / super::SEC),
        log_return_ppm,
        return_ge_1pct,
        upward_close: c_prev.map(|c| o.close > c),
        bullish_body: o.close > o.open,
        z_milli,
        baseline,
        range: RangeStats {
            r,
            median: med_r.map(|m| m / 2),
            e_milli,
            e_adj_milli,
            floor,
            floor_binding,
        },
        body: BodyStats {
            b,
            median: med_b.map(|m| m / 2),
            e_milli: eb_milli,
        },
        range_expansion_15pct,
        body_expansion_15pct,
        upper_wick: o.high - o.open.max(o.close),
        close_location_milli: (r > 0).then(|| milli(o.close - o.low, r)),
        window: w,
        prior_range_relation,
        in_active_band: in_band(u.active_band_min_micros, u.active_band_max_micros),
        in_research_band: in_band(u.research_band_min_micros, u.research_band_max_micros),
        increment: inc,
        resolution_proxy: p.baseline.resolution_proxy.clone(),
    })
}

/// Appends a tested adjacent return to the bounded baseline ring.
pub(crate) fn push_return(det: &Detector, sd: &mut SymDetect, end: i64, seg: SessionId, ppm: i64) {
    if sd.returns.back().is_some_and(|r| r.end >= end) {
        return;
    }
    sd.returns.push_back(RetEntry { end, seg, ppm });
    while sd.returns.len() > det.policy.body.baseline.returns_max {
        sd.returns.pop_front();
    }
}

/// Recomputes the ring's entries a revision may have changed, from the rebuilt bars. Entries
/// older than the bars still held are left as they were (their bars were not touched by a
/// revision the engine could rebuild; otherwise the engine has marked the window untrusted).
pub(crate) fn rebuild_returns(
    det: &Detector,
    sd: &mut SymDetect,
    bars: &std::collections::VecDeque<Bar>,
    bar_ns: i64,
) {
    let Some(first) = bars.front().map(|b| b.end) else {
        return;
    };
    sd.returns.retain(|r| r.end <= first);
    for w in bars.iter().collect::<Vec<_>>().windows(2) {
        let (a, b) = (w[0], w[1]);
        if b.end - a.end != bar_ns {
            continue;
        }
        let (Some(ao), Some(bo), Some(sa), Some(sb)) =
            (a.ohlc, b.ohlc, segment_of(a.end), segment_of(b.end))
        else {
            continue;
        };
        if sa != sb || ao.close <= 0 {
            continue;
        }
        sd.returns.push_back(RetEntry {
            end: b.end,
            seg: sb,
            ppm: ln_ratio_ppm(bo.close, ao.close),
        });
    }
    while sd.returns.len() > det.policy.body.baseline.returns_max {
        sd.returns.pop_front();
    }
}

/// Basis points of `num / den` for display.
pub(crate) fn bps(num: i64, den: i64) -> Option<i64> {
    (den > 0).then(|| bp(num, den))
}
