//! Family A: momentum building / upward acceleration, the primary experimental ignition, its
//! gap events, and range breakout with retention or rejection. Each family is independent: no
//! precursor is required for an ignition, and quietness never gates the parent ignition.

use super::features::{FeatureSnapshot, ReturnClass, bar_at, bps, window};
use super::math::{ln_ratio_ppm, milli};
use super::{
    Barrier, Certainty, Change, Check, ContextClass, Detector, Ep, Extra, Family, LevelVersion,
    Seg, State, SymDetect, emit, open,
};
use crate::pillar::Tri;

/// Advances the non-micro episodes on the bar completing at this barrier.
pub(crate) fn on_barrier(
    det: &mut Detector,
    sd: &mut SymDetect,
    x: &Barrier,
    f: Option<&FeatureSnapshot>,
) {
    let pol = det.policy.clone();
    let p = &pol.body;
    for i in 0..sd.active.len() {
        let ep = &mut sd.active[i];
        if ep.view.state.is_terminal() {
            continue;
        }
        let slots = ep.view.first_bar_end.map_or(0, |b0| (x.b - b0) / x.bar_ns);
        let at = |to, event, reason, c| Change::at_barrier(to, event, reason, c, x.b, x.tau);
        match (ep.view.family, &mut ep.extra) {
            (
                Family::MomentumBuilding | Family::UpwardAcceleration,
                Extra::Momentum { range_low },
            ) => {
                if f.is_some_and(|f| f.close < *range_low) {
                    emit(
                        det,
                        ep,
                        at(
                            State::Faded,
                            "faded",
                            "returned_below_range",
                            Certainty::Terminal,
                        )
                        .features(f.cloned()),
                    );
                    sd.mom_resolved_at = Some(x.b);
                } else if slots >= p.momentum.expiry_slots {
                    emit(
                        det,
                        ep,
                        at(
                            State::Faded,
                            "faded",
                            "expired_without_ignition",
                            Certainty::Terminal,
                        ),
                    );
                    sd.mom_resolved_at = Some(x.b);
                }
            }
            (Family::Ignition | Family::IgnitionGapEvent, _) => {
                if slots >= p.ignition.display_slots {
                    emit(
                        det,
                        ep,
                        at(
                            State::Completed,
                            "completed",
                            "observation_window_elapsed",
                            Certainty::Terminal,
                        ),
                    );
                }
            }
            (
                Family::RangeBreakout,
                Extra::Breakout {
                    level,
                    extended,
                    seen_after,
                },
            ) => {
                let level = *level;
                if let Some(f) = f {
                    *seen_after += 1;
                    if f.close <= level {
                        let c = at(
                            State::Rejected,
                            "rejected",
                            "returned_into_range",
                            Certainty::Retracted,
                        )
                        .features(Some(f.clone()))
                        .level(ep.view.levels.last().cloned());
                        emit(det, ep, c);
                        sd.brk_resolved_at = Some(x.b);
                        continue;
                    }
                    if !*extended
                        && f.close as i128 * 10_000
                            > level as i128 * (10_000 + p.breakout.extended_bp as i128)
                    {
                        // Descriptive, not a state: the move is extended beyond the range high.
                        *extended = true;
                        ep.view.checks.push(Check::new(
                            "extended_above_range_high",
                            Tri::Pass,
                            bps(f.close - level, level),
                            Some(p.breakout.extended_bp),
                            "bp",
                            "close_above_level_by_more_than_extended_bp",
                        ));
                    }
                }
                if slots >= p.breakout.retention_slots {
                    let seen =
                        matches!(ep.extra, Extra::Breakout { seen_after, .. } if seen_after > 0);
                    let lvl = ep.view.levels.last().cloned();
                    let c = if seen {
                        at(
                            State::Retained,
                            "retained",
                            "closes_held_above_range_high",
                            Certainty::Confirmed,
                        )
                    } else {
                        at(
                            State::Expired,
                            "expired",
                            "no_followthrough_prints",
                            Certainty::Terminal,
                        )
                    };
                    emit(det, ep, c.level(lvl));
                    sd.brk_resolved_at = Some(x.b);
                }
            }
            _ => {}
        }
    }
}

fn warmup(x: &Barrier, from: i64) -> bool {
    x.restored_through.is_some_and(|r| r >= from)
}

fn has_active(sd: &SymDetect, fam: &[Family]) -> bool {
    sd.active
        .iter()
        .any(|e| fam.contains(&e.view.family) && !e.view.state.is_terminal())
}

fn range_level(
    ep: &Ep,
    kind: &'static str,
    price: i64,
    x: &Barrier,
    from: (i64, i64),
    reason: &'static str,
) -> LevelVersion {
    LevelVersion {
        level_id: format!("{}:{}", ep.view.episode_id, kind),
        kind,
        version: 1,
        price,
        known_at: x.tau,
        armed_at: None,
        formed_from: from,
        reason,
    }
}

/// Primary ignition, its gap events, and range breakouts on the candidate bar. Returns whether an
/// ignition episode was recognized at this barrier.
pub(crate) fn detect(
    det: &mut Detector,
    sd: &mut SymDetect,
    x: &Barrier,
    seg: Seg,
    f: Option<&FeatureSnapshot>,
) -> bool {
    let Some(f) = f else { return false };
    let pol = det.policy.clone();
    let p = &pol.body;
    let ig = &p.ignition;
    let up = f.upward_close.map(|u| u && f.bullish_body);
    let e_ok = f.range.e_adj_milli.map(|e| e >= ig.range_e_min_milli);
    let z_ok = f.z_milli.map(|z| z >= ig.z_min_milli);
    let adjacent = f.return_class == ReturnClass::Adjacent;
    // Built only when a rule fires (most bars are not candidates).
    let checks = || {
        vec![
            Check::new(
                "adjacent_15s_return",
                Check::tri(Some(adjacent)),
                f.elapsed_secs,
                Some(x.bar_ns / super::SEC),
                "secs",
                match f.return_class {
                    ReturnClass::Adjacent => "previous_slot_traded",
                    ReturnClass::GapEvent => "gap_event",
                    ReturnClass::FirstInSegment => "first_in_segment",
                },
            ),
            Check::new(
                "in_active_band",
                Check::tri(Some(f.in_active_band)),
                Some(f.close),
                None,
                "micros",
                "close_in_active_profile",
            ),
            Check::new(
                "upward_close_and_body",
                Check::tri(up),
                f.log_return_ppm,
                None,
                "log_ppm",
                "close_above_prev_and_open",
            ),
            Check::new(
                "robust_surprise",
                Check::tri(z_ok),
                f.z_milli,
                Some(ig.z_min_milli),
                "milli",
                if z_ok.is_some() {
                    "z_vs_prior_returns"
                } else {
                    f.baseline.status
                },
            ),
            Check::new(
                "return_at_least_1pct",
                Check::tri(f.return_ge_1pct),
                f.log_return_ppm,
                Some(ln_ratio_ppm(ig.return_min_num, ig.return_min_den)),
                "log_ppm",
                "close_vs_prev_close",
            ),
            Check::new(
                "range_expansion",
                Check::tri(e_ok),
                f.range.e_adj_milli,
                Some(ig.range_e_min_milli),
                "milli",
                if e_ok.is_some() {
                    "floor_adjusted_vs_prior_window_median"
                } else {
                    "range_baseline_unavailable"
                },
            ),
        ]
    };
    let mut ignited = false;
    let mut ignition_id = None;
    let trigger = adjacent
        && f.in_active_band
        && up == Some(true)
        && z_ok == Some(true)
        && f.return_ge_1pct == Some(true)
        && e_ok == Some(true);
    if trigger {
        let allowed = sd.ign_t0.is_none_or(|t0| {
            x.b - t0 >= ig.cooldown_secs * super::SEC
                && sd
                    .ign_last_true
                    .is_none_or(|lt| x.b - lt > ig.cooldown_false_bars * x.bar_ns)
        });
        sd.ign_last_true = Some(x.b);
        if allowed && !has_active(sd, &[Family::Ignition]) {
            sd.ign_t0 = Some(x.b);
            let wu = warmup(x, f.window.start - x.bar_ns);
            let mut ep = open(
                det,
                sd,
                x,
                Family::Ignition,
                &ig.id,
                seg,
                Some(x.b),
                wu,
                Extra::Ignition,
            );
            ep.view.context = Some(f.window.context);
            ep.view.recognition = Some(f.clone());
            ep.view.checks = checks();
            if let Some(h) = f.window.high {
                let l = range_level(
                    &ep,
                    "range_high",
                    h,
                    x,
                    (f.window.start, f.window.end),
                    "prior_window_high",
                );
                ep.view.levels.push(l);
            }
            // A precursor still building becomes this ignition's parent.
            let parents: Vec<usize> = (0..sd.active.len())
                .filter(|&i| {
                    let e = &sd.active[i];
                    matches!(
                        e.view.family,
                        Family::MomentumBuilding | Family::UpwardAcceleration
                    ) && e.view.state == State::Building
                })
                .collect();
            for &i in &parents {
                let pid = sd.active[i].view.episode_id.clone();
                ep.view.related_episode_ids.push(pid.clone());
                if sd.active[i].view.family == Family::MomentumBuilding
                    || ep.view.parent_episode_id.is_none()
                {
                    ep.view.parent_episode_id = Some(pid);
                }
            }
            let c = Change::at_barrier(
                State::Recognized,
                "ignition_recognized",
                "all_rules_pass",
                Certainty::Confirmed,
                x.b,
                x.tau,
            )
            .features(Some(f.clone()))
            .checks(checks())
            .level(ep.view.levels.first().cloned());
            emit(det, &mut ep, c);
            let id = ep.view.episode_id.clone();
            for &i in &parents {
                let pe = &mut sd.active[i];
                pe.view.related_episode_ids.push(id.clone());
                emit(
                    det,
                    pe,
                    Change::at_barrier(
                        State::Ignited,
                        "ignited",
                        "ignition_recognized",
                        Certainty::Confirmed,
                        x.b,
                        x.tau,
                    ),
                );
                sd.mom_resolved_at = Some(x.b);
            }
            sd.active.push(ep);
            ignited = true;
            ignition_id = Some(id);
        }
    }

    // Gap events: the same shape across empty slots, reported separately, never confirmed.
    let gap_trigger = f.return_class == ReturnClass::GapEvent
        && f.in_active_band
        && up == Some(true)
        && f.return_ge_1pct == Some(true)
        && e_ok == Some(true);
    if gap_trigger {
        let allowed = sd.gap_t0.is_none_or(|t0| {
            x.b - t0 >= ig.cooldown_secs * super::SEC
                && sd
                    .gap_last_true
                    .is_none_or(|lt| x.b - lt > ig.cooldown_false_bars * x.bar_ns)
        });
        sd.gap_last_true = Some(x.b);
        if allowed && !has_active(sd, &[Family::IgnitionGapEvent]) {
            sd.gap_t0 = Some(x.b);
            let wu = warmup(x, f.window.start - x.bar_ns);
            let mut ep = open(
                det,
                sd,
                x,
                Family::IgnitionGapEvent,
                &ig.id,
                seg,
                Some(x.b),
                wu,
                Extra::Ignition,
            );
            ep.view.context = Some(f.window.context);
            ep.view.recognition = Some(f.clone());
            ep.view.checks = checks();
            let c = Change::at_barrier(
                State::Observed,
                "gap_event_observed",
                "not_a_15s_return",
                Certainty::Provisional,
                x.b,
                x.tau,
            )
            .features(Some(f.clone()))
            .checks(checks());
            emit(det, &mut ep, c);
            sd.active.push(ep);
        }
    }

    // Range breakout from a supported consolidation window.
    if f.window.context == ContextClass::ActiveConsolidation
        && let Some(h) = f.window.high
        && f.high > h
        && f.in_active_band
        && !has_active(sd, &[Family::RangeBreakout])
        && sd
            .brk_resolved_at
            .is_none_or(|r| x.b - r >= p.breakout.cooldown_slots * x.bar_ns)
    {
        let wu = warmup(x, f.window.start - x.bar_ns);
        let mut ep = open(
            det,
            sd,
            x,
            Family::RangeBreakout,
            &p.breakout.id,
            seg,
            Some(x.b),
            wu,
            Extra::Breakout {
                level: h,
                extended: false,
                seen_after: 0,
            },
        );
        ep.view.context = Some(f.window.context);
        ep.view.recognition = Some(f.clone());
        let lvl = range_level(
            &ep,
            "range_high",
            h,
            x,
            (f.window.start, f.window.end),
            "prior_consolidation_high",
        );
        ep.view.levels.push(lvl.clone());
        if let Some(id) = &ignition_id {
            ep.view.related_episode_ids.push(id.clone());
            if let Some(ie) = sd.active.iter_mut().find(|e| &e.view.episode_id == id) {
                ie.view.related_episode_ids.push(ep.view.episode_id.clone());
            }
        }
        let closed = f.close > h;
        let check = Check::new(
            "close_above_range_high",
            Check::tri(Some(closed)),
            Some(f.close),
            Some(h),
            "micros",
            "completed_bar_close_vs_prior_window_high",
        );
        ep.view.checks.push(check.clone());
        let c = if closed {
            Change::at_barrier(
                State::Confirmed,
                "breakout_confirmed",
                "closed_above_range_high",
                Certainty::Confirmed,
                x.b,
                x.tau,
            )
        } else {
            Change::at_barrier(
                State::Rejected,
                "breakout_rejected",
                "crossed_and_closed_inside",
                Certainty::Retracted,
                x.b,
                x.tau,
            )
        };
        emit(
            det,
            &mut ep,
            c.features(Some(f.clone()))
                .level(Some(lvl))
                .checks(vec![check]),
        );
        if !closed {
            sd.brk_resolved_at = Some(x.b);
        }
        sd.active.push(ep);
    }
    ignited
}

/// Momentum building (quiet prior context) or the separately named upward acceleration.
pub(crate) fn momentum(
    det: &mut Detector,
    sd: &mut SymDetect,
    x: &Barrier,
    seg: Seg,
    candidate: Option<&FeatureSnapshot>,
) {
    let pol = det.policy.clone();
    let p = &pol.body;
    let m = &p.momentum;
    // A precursor precedes an ignition: none starts while an ignition episode is active.
    if has_active(
        sd,
        &[
            Family::MomentumBuilding,
            Family::UpwardAcceleration,
            Family::Ignition,
        ],
    ) || sd
        .mom_resolved_at
        .is_some_and(|r| x.b - r < m.cooldown_slots * x.bar_ns)
    {
        return;
    }
    let step = m.step_slots * x.bar_ns;
    let close = |end: i64| {
        (seg.holds(end, x.bar_ns))
            .then(|| bar_at(x.bars, end).and_then(|b| b.ohlc))
            .flatten()
            .map(|o| o.close)
    };
    // Endpoints must be traded bars (valid and fresh by construction), and every slot of the
    // buildup window must have traded.
    let (Some(pb), Some(p1), Some(p2)) = (close(x.b), close(x.b - step), close(x.b - 2 * step))
    else {
        return;
    };
    if (0..m.step_slots).any(|k| close(x.b - k * x.bar_ns).is_none()) {
        return;
    }
    let first = x.b - 2 * step - x.bar_ns;
    if first < sd.requalify_from
        || det.gap_overlaps(sd, first, x.b)
        || pb <= 0
        || p1 <= 0
        || p2 <= 0
    {
        return;
    }
    let r = ln_ratio_ppm(pb, p1);
    let r_prev = ln_ratio_ppm(p1, p2);
    let a = r - r_prev;
    let inc = det.policy.increment(p1);
    let disp_ok = pb - p1 >= m.displacement_min_increments * inc;
    if !(r >= m.r_min_ppm && a >= m.a_min_ppm && disp_ok) {
        return;
    }
    // Quiet context strictly before the evaluated buildup (both comparison windows).
    let w = window(det, sd, x, seg, x.b - 2 * step, p.baseline.window_slots);
    let family = match w.context {
        ContextClass::ActiveConsolidation => Family::MomentumBuilding,
        ContextClass::AlreadyMoving | ContextClass::Unavailable => Family::UpwardAcceleration,
        ContextClass::Sparse | ContextClass::Insufficient => return,
    };
    let variant = if family == Family::MomentumBuilding {
        &m.building_id
    } else {
        &m.acceleration_id
    };
    // Rates over equal durations: the buildup's per slot against the window's per slot.
    let mut build_dv: u128 = 0;
    let mut build_n: u64 = 0;
    for k in 0..m.step_slots {
        if let Some(b) = bar_at(x.bars, x.b - k * x.bar_ns) {
            build_dv += b.dollars_micros;
            build_n += b.trades as u64;
        }
    }
    let build_dv = (build_dv / 1_000_000) as i64;
    let dv_ratio = (w.dv_base > 0)
        .then(|| milli(build_dv * w.slots_expected, w.dv_base as i64 * m.step_slots));
    let n_ratio = (w.n_base > 0).then(|| {
        milli(
            build_n as i64 * w.slots_expected,
            w.n_base as i64 * m.step_slots,
        )
    });
    let checks = vec![
        Check::new(
            "prior_context",
            if family == Family::MomentumBuilding {
                Tri::Pass
            } else {
                Tri::Unknown
            },
            w.q_ppm,
            w.q_low_ppm,
            "ppm",
            w.context_reason,
        ),
        Check::new(
            "rise",
            Tri::Pass,
            Some(r),
            Some(m.r_min_ppm),
            "log_ppm",
            "close_vs_one_step_back",
        ),
        Check::new(
            "acceleration",
            Tri::Pass,
            Some(a),
            Some(m.a_min_ppm),
            "log_ppm",
            "rise_vs_previous_equal_step",
        ),
        Check::new(
            "displacement",
            Tri::Pass,
            Some(pb - p1),
            Some(m.displacement_min_increments * inc),
            "micros",
            "resolution_proxy_increments",
        ),
        Check::new(
            "dollar_volume_rate",
            Tri::Unknown,
            dv_ratio,
            None,
            "milli",
            "measured_not_required",
        ),
        Check::new(
            "trade_rate",
            Tri::Unknown,
            n_ratio,
            None,
            "milli",
            "measured_not_required",
        ),
        Check::new(
            "below_range_high",
            Tri::Unknown,
            w.high.and_then(|h| bps(h - pb, h)),
            None,
            "bp",
            "proximity_to_prior_range_high",
        ),
    ];
    let wu = warmup(x, w.start - x.bar_ns);
    let range_low = w.low.unwrap_or(0);
    let mut ep = open(
        det,
        sd,
        x,
        family,
        variant,
        seg,
        Some(x.b),
        wu,
        Extra::Momentum { range_low },
    );
    ep.view.context = Some(w.context);
    ep.view.checks = checks.clone();
    // The context window is frozen at first recognition: later windows never move it.
    if let (Some(h), Some(l)) = (w.high, w.low) {
        let a = range_level(
            &ep,
            "range_high",
            h,
            x,
            (w.start, w.end),
            "frozen_context_window",
        );
        let b = range_level(
            &ep,
            "range_low",
            l,
            x,
            (w.start, w.end),
            "frozen_context_window",
        );
        ep.view.levels.push(a);
        ep.view.levels.push(b);
    }
    let f = candidate.cloned();
    ep.view.recognition = f.clone();
    let event = if family == Family::MomentumBuilding {
        "momentum_building_recognized"
    } else {
        "upward_acceleration_observed"
    };
    let c = Change::at_barrier(
        State::Building,
        event,
        w.context_reason,
        Certainty::Provisional,
        x.b,
        x.tau,
    )
    .features(f)
    .checks(checks)
    .level(ep.view.levels.first().cloned());
    emit(det, &mut ep, c);
    sd.active.push(ep);
}
