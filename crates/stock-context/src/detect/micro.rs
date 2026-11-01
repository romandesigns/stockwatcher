//! `micro_pullback_v0`: a brief retreat inside an established upward impulse, recognised from
//! causal history alone (no prior ignition, Ross membership, news or float gate).
//!
//! Anchors: `H0` is the latest bar holding the lookback's highest high, `L0` the lowest low in a
//! strictly earlier bar (intrabar order is never assumed, so the two are always distinct bars).
//! They are frozen at the barrier the retreat is first seen; the pullback low `Lp` then evolves.
//! Arming fixes `T = H0` and `I = Lp` as version 1 of each level, `armed_at` = that cutoff. After
//! arming only prints executed at or after `armed_at` can cross or breach them, and only a full
//! bar starting at or after `armed_at` can confirm. v0 never revises an armed level.

use super::features::{bar_at, bps};
use super::math::milli;
use super::{
    Barrier, Certainty, Change, Check, Detector, Ep, Extra, Family, LevelVersion, MicroAnchors,
    MicroWick, Seg, State, SymDetect, TradeEvidence, emit, open,
};
use crate::pillar::Tri;
use market_events::TradeId;

#[derive(Clone, Debug)]
pub(crate) struct Inner {
    pub trigger: Option<LevelVersion>,
    pub invalidation: Option<LevelVersion>,
    pub armed_bar_end: Option<i64>,
    /// The print that made the current provisional crossing, and that transition's id.
    pub crossing: Option<(TradeId, i64, String)>,
}

fn is_micro(e: &Ep) -> bool {
    e.view.family == Family::MicroPullback && !e.view.state.is_terminal()
}

fn level(
    ep: &Ep,
    kind: &'static str,
    price: i64,
    tau: i64,
    from: (i64, i64),
    reason: &'static str,
) -> LevelVersion {
    LevelVersion {
        level_id: format!("{}:{}", ep.view.episode_id, kind),
        kind,
        version: 1,
        price,
        known_at: tau,
        armed_at: Some(tau),
        formed_from: from,
        reason,
    }
}

/// Looks for a new impulse with a retreat when no micro episode is active.
pub(crate) fn search(det: &mut Detector, sd: &mut SymDetect, x: &Barrier, seg: Seg) {
    if sd.active.iter().any(is_micro) {
        return;
    }
    let pol = det.policy.clone();
    let m = &pol.body.micro;
    let from = x.b - (m.lookback_slots - 1) * x.bar_ns;
    // Bars of the lookback that traded, in this segment, after any interruption.
    let w: Vec<(i64, crate::setup::Ohlc)> = x
        .bars
        .iter()
        .filter(|t| t.end >= from && t.end <= x.b)
        .filter(|t| seg.holds(t.end, x.bar_ns) && t.end - x.bar_ns >= sd.requalify_from)
        .filter_map(|t| t.ohlc.map(|o| (t.end, o)))
        .collect();
    if w.len() < 3 {
        return;
    }
    // H0: the latest bar with the highest high.
    let j = (0..w.len()).fold(0, |a, k| if w[k].1.high >= w[a].1.high { k } else { a });
    let (h0_end, h0) = (w[j].0, w[j].1.high);
    if j == w.len() - 1 || j == 0 || h0_end <= sd.micro_search_from {
        return;
    }
    // Discovery may start after history adoption or late watch admission. The impulse high's
    // age still starts at its bar, not at discovery; silently decline an already-expired setup.
    if (x.b - h0_end) / x.bar_ns > m.formation_max_slots {
        return;
    }
    // L0: the lowest low strictly before H0's bar (latest on ties).
    let i = (0..j).fold(0, |a, k| if w[k].1.low <= w[a].1.low { k } else { a });
    let (l0_end, l0) = (w[i].0, w[i].1.low);
    if det.gap_overlaps(sd, l0_end - x.bar_ns, x.b) || l0 <= 0 {
        return;
    }
    let u = h0 - l0;
    let inc = det.policy.increment(l0);
    if (u as i128) * 10_000 < l0 as i128 * m.impulse_min_bp as i128
        || u < m.impulse_min_increments * inc
    {
        return;
    }
    let impulse_slots = (h0_end - l0_end) / x.bar_ns + 1;
    let traded = w
        .iter()
        .filter(|(e, _)| *e >= l0_end && *e <= h0_end)
        .count() as i64;
    let coverage = milli(traded, impulse_slots);
    if coverage < m.impulse_min_coverage_milli {
        return;
    }
    // The retreat: every bar after H0 (all lower highs by construction).
    let after = &w[j + 1..];
    let (lp_end, lp) = after
        .iter()
        .fold((after[0].0, after[0].1.low), |a, (e, o)| {
            if o.low <= a.1 { (*e, o.low) } else { a }
        });
    if lp <= l0 || milli(h0 - lp, u) > m.max_depth_milli {
        return;
    }
    // Warm-up is decided by WHEN the setup would have been announced, not where its impulse
    // started: a restart restores the day's bars, so an impulse that began before the restart
    // but retreats and arms on bars this process saw live is a live setup (INO 2026-10-30: a
    // 16:38 ET deploy silenced two of them because their L0 sat in the restored prefix). The
    // recognition itself is warm-up only when this barrier is inside the restored prefix;
    // `try_arm` decides again for the arming, the moment that notifies.
    let wu = x.restored_through.is_some_and(|r| r >= x.b);
    let mut ep = open(
        det,
        sd,
        x,
        Family::MicroPullback,
        &m.id,
        seg,
        Some(h0_end),
        wu,
        Extra::Micro(Box::new(Inner {
            trigger: None,
            invalidation: None,
            armed_bar_end: None,
            crossing: None,
        })),
    );
    ep.view.anchors = Some(MicroAnchors {
        wick: Some(MicroWick::measure(
            Some([w[j].1.open, w[j].1.high, w[j].1.low, w[j].1.close]),
            h0_end,
            x.tau,
        )),
        l0,
        l0_bar_end: l0_end,
        h0,
        h0_bar_end: h0_end,
        impulse_bp: bps(u, l0).unwrap_or(0),
        impulse_slots,
        impulse_coverage_milli: coverage,
        frozen_at: x.tau,
        pullback_low: lp,
        pullback_low_bar_end: lp_end,
        depth_milli: milli(h0 - lp, u),
        retreat_bp: bps(h0 - lp, h0).unwrap_or(0),
        pullback_bars: after.len() as u32,
        pullback_slots: (x.b - h0_end) / x.bar_ns,
        dv_rate_ratio_milli: dv_ratio(x, l0_end, h0_end, x.b),
        armed_at: None,
    });
    sd.micro_search_from = h0_end;
    let c = Change::at_barrier(
        State::PullbackForming,
        "impulse_frozen_retreat_started",
        "retreat_below_impulse_high",
        Certainty::Provisional,
        x.b,
        x.tau,
    );
    emit(det, &mut ep, c);
    try_arm(det, sd, x, &mut ep);
    sd.active.push(ep);
}

/// Dollar-volume rate of the pullback (after H0 through `b`) over the impulse (L0..=H0), per
/// covered (traded) bar; `None` when either side has no dollar volume.
fn dv_ratio(x: &Barrier, l0_end: i64, h0_end: i64, b: i64) -> Option<i64> {
    let side = |a: i64, z: i64| {
        let (mut dv, mut n) = (0u128, 0i64);
        for t in x
            .bars
            .iter()
            .filter(|t| t.end >= a && t.end <= z && t.ohlc.is_some())
        {
            dv += t.dollars_micros;
            n += 1;
        }
        (n > 0 && dv > 0).then_some(((dv / 1_000_000) as i64, n))
    };
    let (di, ni) = side(l0_end, h0_end)?;
    let (dp, np) = side(h0_end + x.bar_ns, b)?;
    (di > 0).then(|| milli(dp * ni, di * np))
}

fn try_arm(det: &mut Detector, sd: &mut SymDetect, x: &Barrier, ep: &mut Ep) {
    let m = &det.policy.body.micro;
    let Some(a) = ep.view.anchors.clone() else {
        return;
    };
    if ep.view.state != State::PullbackForming
        || a.pullback_bars < m.min_pullback_bars
        || (x.b - a.h0_bar_end) / x.bar_ns > m.formation_max_slots
    {
        return;
    }
    let fresh = det.policy.body.timing.price_fresh_secs * super::SEC;
    let Some(seen) = sd.last_seen.filter(|s| x.b - s.exec <= fresh) else {
        return;
    };
    if seen.price > a.h0 {
        // The price crossed before any level existed: never a retroactive trigger.
        let c = Change::at_barrier(
            State::Superseded,
            "superseded",
            "crossed_before_arming",
            Certainty::Terminal,
            x.b,
            x.tau,
        )
        .checks(vec![Check::new(
            "price_at_arming",
            Tri::Fail,
            Some(seen.price),
            Some(a.h0),
            "micros",
            "above_impulse_high",
        )]);
        emit(det, ep, c);
        return;
    }
    if seen.price < a.pullback_low {
        return; // still forming: the next bar lowers Lp
    }
    let t = level(
        ep,
        "trigger",
        a.h0,
        x.tau,
        (a.l0_bar_end, a.h0_bar_end),
        "impulse_high",
    );
    let i = level(
        ep,
        "invalidation",
        a.pullback_low,
        x.tau,
        (a.h0_bar_end + x.bar_ns, a.pullback_low_bar_end),
        "pullback_low_at_arming",
    );
    // The arming's warm-up (see `search`): the earliest barrier a continuous run could have
    // armed this setup at is the end of its `min_pullback_bars`-th retreat bar. When that moment
    // is inside the restored prefix, the previous process may already have armed (and announced)
    // it, so this arming and everything after it is warm-up; otherwise it is this run's own
    // observation and may notify, however early its impulse began.
    ep.view.warmup = x.restored_through.is_some_and(|r| {
        let need = (m.min_pullback_bars as usize).max(1);
        let could_arm_at = x
            .bars
            .iter()
            .filter(|t| t.end > a.h0_bar_end && t.end <= x.b && t.ohlc.is_some())
            .nth(need - 1)
            .map_or(x.b, |t| t.end);
        r >= could_arm_at
    });
    ep.view.levels.push(t.clone());
    ep.view.levels.push(i.clone());
    if let Some(an) = ep.view.anchors.as_mut() {
        an.armed_at = Some(x.tau);
    }
    if let Extra::Micro(inner) = &mut ep.extra {
        inner.trigger = Some(t.clone());
        inner.invalidation = Some(i.clone());
        inner.armed_bar_end = Some(x.b);
    }
    let checks = vec![
        Check::new(
            "pullback_bars",
            Tri::Pass,
            Some(a.pullback_bars as i64),
            Some(m.min_pullback_bars as i64),
            "bars",
            "formation",
        ),
        Check::new(
            "depth",
            Tri::Pass,
            Some(a.depth_milli),
            Some(m.max_depth_milli),
            "milli",
            "fraction_of_impulse",
        ),
        Check::new(
            "price_fresh",
            Tri::Pass,
            Some((x.b - seen.exec) / 1_000_000),
            Some(fresh / 1_000_000),
            "ms",
            "latest_admitted_print",
        ),
        Check::new(
            "volume_condition",
            Tri::Unknown,
            a.dv_rate_ratio_milli,
            None,
            "milli",
            "measured_not_required",
        )
        .optional(),
    ];
    let c = Change::at_barrier(
        State::Armed,
        "armed",
        "levels_fixed",
        Certainty::Provisional,
        x.b,
        x.tau,
    )
    .level(Some(t))
    .checks(checks);
    emit(det, ep, c);
    let _ = sd;
}

/// Tests armed levels against the bar completing now (old levels first), then updates forming
/// episodes and arms them.
pub(crate) fn on_barrier(det: &mut Detector, sd: &mut SymDetect, x: &Barrier, _seg: Seg) {
    let pol = det.policy.clone();
    let m = &pol.body.micro;
    let fresh = det.policy.body.timing.price_fresh_secs * super::SEC;
    let bar = bar_at(x.bars, x.b).and_then(|t| t.ohlc);
    let mut eps = std::mem::take(&mut sd.active);
    for ep in eps.iter_mut().filter(|e| is_micro(e)) {
        if sd.last_seen.is_none_or(|s| x.b - s.exec > fresh) {
            // No trades alone is not a data gap: only this episode ends, and a new impulse must
            // peak after it.
            emit(
                det,
                ep,
                Change::at_barrier(
                    State::Interrupted,
                    "interrupted",
                    "stale_price",
                    Certainty::Terminal,
                    x.b,
                    x.tau,
                ),
            );
            sd.micro_search_from = sd.micro_search_from.max(x.b);
            continue;
        }
        match ep.view.state {
            State::PullbackForming => {
                let a = ep.view.anchors.clone().expect("micro anchors");
                if let Some(o) = bar.filter(|_| x.b > a.h0_bar_end + (a.pullback_slots * x.bar_ns))
                {
                    if o.high > a.h0 {
                        emit(
                            det,
                            ep,
                            Change::at_barrier(
                                State::Superseded,
                                "superseded",
                                "new_high_before_arming",
                                Certainty::Terminal,
                                x.b,
                                x.tau,
                            ),
                        );
                        continue;
                    }
                    let an = ep.view.anchors.as_mut().expect("micro anchors");
                    an.pullback_bars += 1;
                    if o.low < an.pullback_low {
                        // A new forming low before arming is a formation update, not a failure.
                        an.pullback_low = o.low;
                        an.pullback_low_bar_end = x.b;
                    }
                    let u = an.h0 - an.l0;
                    an.depth_milli = milli(an.h0 - an.pullback_low, u);
                    an.retreat_bp = bps(an.h0 - an.pullback_low, an.h0).unwrap_or(0);
                    an.pullback_slots = (x.b - an.h0_bar_end) / x.bar_ns;
                    an.dv_rate_ratio_milli = dv_ratio(x, an.l0_bar_end, an.h0_bar_end, x.b);
                    let (lp, l0, depth) = (an.pullback_low, an.l0, an.depth_milli);
                    if lp <= l0 {
                        emit(
                            det,
                            ep,
                            Change::at_barrier(
                                State::Invalidated,
                                "invalidated",
                                "impulse_origin_breached",
                                Certainty::Terminal,
                                x.b,
                                x.tau,
                            ),
                        );
                        continue;
                    }
                    if depth > m.max_depth_milli {
                        emit(
                            det,
                            ep,
                            Change::at_barrier(
                                State::Invalidated,
                                "invalidated",
                                "depth_exceeded",
                                Certainty::Terminal,
                                x.b,
                                x.tau,
                            )
                            .checks(vec![Check::new(
                                "depth",
                                Tri::Fail,
                                Some(depth),
                                Some(m.max_depth_milli),
                                "milli",
                                "fraction_of_impulse",
                            )]),
                        );
                        continue;
                    }
                } else if let Some(an) = ep.view.anchors.as_mut() {
                    an.pullback_slots = (x.b - an.h0_bar_end) / x.bar_ns;
                }
                let slots = (x.b - a.h0_bar_end) / x.bar_ns;
                if slots > m.formation_max_slots {
                    emit(
                        det,
                        ep,
                        Change::at_barrier(
                            State::Expired,
                            "expired",
                            "formation_timeout",
                            Certainty::Terminal,
                            x.b,
                            x.tau,
                        ),
                    );
                    continue;
                }
                try_arm(det, sd, x, ep);
            }
            State::Armed | State::CrossingProvisional => {
                let Extra::Micro(inner) = &ep.extra else {
                    continue;
                };
                let (t, i) = (
                    inner.trigger.clone().expect("armed"),
                    inner.invalidation.clone().expect("armed"),
                );
                let armed_at = t.armed_at.expect("armed");
                let armed_bar = inner.armed_bar_end.expect("armed");
                let crossing = inner.crossing.clone();
                if let Some(o) = bar.filter(|_| x.b > armed_bar) {
                    let full = x.b - x.bar_ns >= armed_at;
                    if full && o.low < i.price {
                        // A full post-arm bar that breached I cannot confirm, whatever its close.
                        let ambiguous = o.high > t.price && o.high_exec == o.low_exec;
                        let c = Change::at_barrier(
                            State::Invalidated,
                            "invalidated",
                            "bar_low_below_invalidation",
                            Certainty::Terminal,
                            x.b,
                            x.tau,
                        )
                        .level(Some(i.clone()))
                        .checks(vec![
                            Check::new(
                                "ordering_ambiguous",
                                if ambiguous { Tri::Pass } else { Tri::Fail },
                                None,
                                None,
                                "flag",
                                if ambiguous {
                                    "equal_timestamps_conservative_invalidation_first"
                                } else {
                                    "order_established_or_not_needed"
                                },
                            )
                            .optional(),
                        ]);
                        emit(det, ep, c);
                        continue;
                    }
                    if full && o.close > t.price {
                        let ext = o.close as i128 * 10_000
                            > t.price as i128 * (10_000 + m.extended_bp as i128);
                        let c = Change::at_barrier(
                            State::TriggerObserved,
                            "trigger_observed",
                            "full_post_arm_bar_closed_above_trigger",
                            Certainty::Confirmed,
                            x.b,
                            x.tau,
                        )
                        .level(Some(t.clone()))
                        .checks(vec![
                            Check::new(
                                "close_above_trigger",
                                Tri::Pass,
                                Some(o.close),
                                Some(t.price),
                                "micros",
                                "completed_full_bar",
                            ),
                            Check::new(
                                "extended",
                                if ext { Tri::Pass } else { Tri::Fail },
                                bps(o.close - t.price, t.price),
                                Some(m.extended_bp),
                                "bp",
                                "close_vs_trigger",
                            )
                            .optional(),
                        ]);
                        emit(det, ep, c);
                        sd.micro_search_from = sd.micro_search_from.max(x.b);
                        continue;
                    }
                    if ep.view.state == State::CrossingProvisional && o.close <= t.price {
                        let c = Change::at_barrier(
                            State::Armed,
                            "crossing_retracted",
                            "bar_closed_at_or_below_trigger",
                            Certainty::Retracted,
                            x.b,
                            x.tau,
                        )
                        .level(Some(t.clone()))
                        .correction_of(crossing.map(|c| c.2));
                        emit(det, ep, c);
                        if let Extra::Micro(inner) = &mut ep.extra {
                            inner.crossing = None;
                        }
                    }
                }
                if (x.b - armed_bar) / x.bar_ns > m.armed_max_slots {
                    emit(
                        det,
                        ep,
                        Change::at_barrier(
                            State::Expired,
                            "expired",
                            "armed_timeout",
                            Certainty::Terminal,
                            x.b,
                            x.tau,
                        )
                        .level(Some(t)),
                    );
                }
            }
            _ => {}
        }
    }
    sd.active = eps;
}

/// Trade-driven tests of armed levels: only prints executed at or after `armed_at`.
pub(crate) fn on_print(
    det: &mut Detector,
    sd: &mut SymDetect,
    id: &TradeId,
    exec: i64,
    admit: i64,
    price: i64,
) {
    for ep in sd.active.iter_mut().filter(|e| is_micro(e)) {
        let Extra::Micro(inner) = &ep.extra else {
            continue;
        };
        let (Some(t), Some(i)) = (inner.trigger.clone(), inner.invalidation.clone()) else {
            continue;
        };
        let armed_at = t.armed_at.expect("armed");
        if exec < armed_at || admit < armed_at {
            continue;
        }
        let ev = Some(TradeEvidence {
            trade_id: id.clone(),
            exec,
            admit,
            price,
        });
        if price < i.price {
            // A crossing at the same execution nanosecond: the true market order of the two is
            // not established (only their delivery order is). Conservative: invalidation stands.
            let ambiguous = inner.crossing.as_ref().is_some_and(|c| c.1 == exec);
            let c = Change::at(
                State::Invalidated,
                "invalidated",
                "print_below_invalidation",
                Certainty::Terminal,
                admit,
            )
            .trade(ev)
            .level(Some(i))
            .checks(vec![
                Check::new(
                    "ordering_ambiguous",
                    if ambiguous { Tri::Pass } else { Tri::Fail },
                    None,
                    None,
                    "flag",
                    if ambiguous {
                        "equal_timestamps_conservative_invalidation_first"
                    } else {
                        "order_established_or_not_needed"
                    },
                )
                .optional(),
            ]);
            let tid = emit(det, ep, c);
            // Kept so a later revision of this print appends a correction notice.
            while det.cited.len() >= det.policy.body.lifecycle.recent_max {
                det.cited.pop_front();
            }
            det.cited.push_back((id.clone(), exec, tid, ep.clone()));
        } else if price > t.price && ep.view.state == State::Armed {
            let c = Change::at(
                State::CrossingProvisional,
                "crossing_provisional",
                "post_arm_print_above_trigger",
                Certainty::Provisional,
                admit,
            )
            .trade(ev)
            .level(Some(t));
            let tid = emit(det, ep, c);
            if let Extra::Micro(inner) = &mut ep.extra {
                inner.crossing = Some((id.clone(), exec, tid));
            }
        }
    }
}

/// A revision of the print behind a provisional crossing retracts it (unless the corrected print
/// still crosses); a revision of a print behind a terminal invalidation appends a correction
/// notice and leaves the outcome as recorded.
pub(crate) fn on_revision(
    det: &mut Detector,
    sd: &mut SymDetect,
    id: &TradeId,
    exec: i64,
    now: Option<i64>,
    admit: i64,
) {
    for ep in sd.active.iter_mut().filter(|e| is_micro(e)) {
        let Extra::Micro(inner) = &ep.extra else {
            continue;
        };
        let Some((cid, cexec, ctid)) = inner.crossing.clone() else {
            continue;
        };
        let t = inner.trigger.clone().expect("armed");
        if cid != *id || cexec != exec || now.is_some_and(|p| p > t.price) {
            continue;
        }
        let c = Change::at(
            State::Armed,
            "crossing_retracted",
            "crossing_print_revised",
            Certainty::Retracted,
            admit,
        )
        .level(Some(t))
        .correction_of(Some(ctid));
        emit(det, ep, c);
        if let Extra::Micro(inner) = &mut ep.extra {
            inner.crossing = None;
        }
    }
    let hits: Vec<usize> = (0..det.cited.len())
        .filter(|&k| det.cited[k].0 == *id && det.cited[k].1 == exec)
        .collect();
    for k in hits {
        let (_, _, tid, mut ep) = det.cited[k].clone();
        let state = ep.view.state;
        let c = Change::at(
            state,
            "basis_print_revised",
            "invalidation_print_revised",
            Certainty::Correction,
            admit,
        )
        .correction_of(Some(tid));
        emit(det, &mut ep, c);
        if let Some(r) = det
            .recent
            .iter_mut()
            .find(|r| r.episode_id == ep.view.episode_id)
        {
            r.last_material_seq = ep.view.last_material_seq;
        }
        det.cited[k].3 = ep;
    }
}
