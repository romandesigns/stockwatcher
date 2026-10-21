//! `micro_pullback_v0` through the real engine: causal anchors, formation before arming, fixed
//! levels after it, post-arm trade-driven crossings, full-bar confirmation, retractions,
//! failures, ambiguity and corrections. Synthetic fixtures, fictional symbols.

use stock_context::detect::fixture::{self, FixtureStep, Script, cutoff, end};
use stock_context::*;

const S: i64 = 1_000_000_000;
const MS: i64 = 1_000_000;

fn micro(ts: &[DetectionTransition]) -> Vec<&DetectionTransition> {
    ts.iter()
        .filter(|t| t.family == Family::MicroPullback)
        .collect()
}

fn run(steps: &[FixtureStep]) -> Vec<DetectionTransition> {
    let mut e = fixture::engine("t");
    fixture::drive(&mut e, steps)
}

fn events(ts: &[&DetectionTransition]) -> Vec<(&'static str, State)> {
    ts.iter().map(|t| (t.event, t.to)).collect()
}

/// The impulse and two-bar retreat shared by the scenarios (see `fixture::micro_base`).
fn base(s: &mut Script, sym: &str) {
    for k in 0..6 {
        s.bar(sym, k, [300, 300, 300, 300], 100);
    }
    s.bar(sym, 6, [300, 302, 300, 302], 200);
    s.bar(sym, 7, [302, 310, 302, 310], 300);
    s.bar(sym, 8, [310, 325, 310, 325], 300);
    s.bar(sym, 9, [325, 340, 325, 338], 400);
    s.bar(sym, 10, [338, 338, 333, 335], 150);
    s.bar(sym, 11, [335, 336, 332, 334], 120);
}

#[test]
fn a_valid_micro_pullback_forms_from_history_alone_and_confirms_on_a_full_post_arm_bar() {
    let r = fixture::run("micro_valid").unwrap();
    // No ignition, Ross membership, news or float: none was needed.
    assert!(
        r.transitions
            .iter()
            .all(|t| t.family == Family::MicroPullback)
    );
    let m = micro(&r.transitions);
    assert_eq!(
        events(&m),
        vec![
            ("impulse_frozen_retreat_started", State::PullbackForming),
            ("armed", State::Armed),
            ("crossing_provisional", State::CrossingProvisional),
            ("trigger_observed", State::TriggerObserved),
        ]
    );
    // Anchors frozen at the retreat, before H0's own extension could be judged.
    let a = m[0].anchors.as_ref().unwrap();
    assert_eq!((a.l0, a.l0_bar_end), (3_000_000, end(6)));
    assert_eq!((a.h0, a.h0_bar_end), (3_400_000, end(9)));
    assert_eq!(a.frozen_at, cutoff(10));
    assert_eq!(a.pullback_low, 3_330_000);
    assert_eq!(a.depth_milli, 175);
    // The forming low evolved to 3.32 before arming (d = 20 %, the handoff's geometry example).
    let armed = m[1].anchors.as_ref().unwrap();
    assert_eq!((armed.pullback_low, armed.depth_milli), (3_320_000, 200));
    assert_eq!(armed.retreat_bp, 235);
    assert_eq!(m[1].observed_at, cutoff(11));
    let t = m[1].level.as_ref().unwrap();
    assert_eq!(
        (t.kind, t.price, t.version, t.armed_at),
        ("trigger", 3_400_000, 1, Some(cutoff(11)))
    );
    // The crossing is a post-arm print, timed by its admission.
    let x = m[2].trade.as_ref().unwrap();
    assert!(x.exec >= cutoff(11));
    assert_eq!(m[2].observed_at, end(11) + 4 * S + 100 * MS);
    // Bar 12 straddles armed_at: its close 3.42 cannot confirm. Bar 13 is the first full bar.
    assert_eq!(m[3].observed_at, cutoff(13));
    assert_eq!(
        m[3].level.as_ref().unwrap().version,
        1,
        "the level tested is the armed version"
    );
}

#[test]
fn negative_returns_during_the_retreat_do_not_erase_the_impulse() {
    let r = fixture::run("micro_valid").unwrap();
    let m = micro(&r.transitions);
    // Bars 10 and 11 both closed lower (3.38 → 3.35 → 3.34) and the episode formed and armed.
    assert_eq!(m[0].bar_end, Some(end(10)));
    assert_eq!(m[1].to, State::Armed);
}

#[test]
fn a_crossing_before_the_level_existed_never_becomes_a_trigger() {
    let r = fixture::run("micro_crossing_before_arming").unwrap();
    let m = micro(&r.transitions);
    assert_eq!(m.last().unwrap().to, State::Superseded);
    assert_eq!(m.last().unwrap().reason, "crossed_before_arming");
    assert!(m.iter().all(|t| t.to != State::TriggerObserved));
    assert!(m.iter().all(|t| t.to != State::CrossingProvisional));
}

#[test]
fn a_bar_high_before_arming_is_not_a_later_crossing() {
    // The only print above T executes at +1 s, before armed_at (+2 s), and is admitted after
    // it. Execution, not delivery, places it before the level existed.
    let mut s2 = Script::new();
    base(&mut s2, "MPXG");
    s2.print_admitted("MPXG", end(11) + S, cutoff(11) + 500 * MS, 3_410_000, 100);
    s2.print("MPXG", end(11) + 6 * S, 338, 100);
    s2.advance(cutoff(14));
    let m2 = run(&s2.steps);
    let m2 = micro(&m2);
    assert!(
        m2.iter().all(|t| t.to != State::CrossingProvisional),
        "executed before armed_at, even though admitted after it"
    );
}

#[test]
fn a_provisional_crossing_that_reverses_is_retracted_not_confirmed() {
    let r = fixture::run("micro_retracted").unwrap();
    let m = micro(&r.transitions);
    let ev = events(&m);
    assert_eq!(ev[2], ("crossing_provisional", State::CrossingProvisional));
    assert_eq!(ev[3], ("crossing_retracted", State::Armed));
    assert_eq!(m[3].certainty, Certainty::Retracted);
    assert_eq!(
        m[3].correction_of.as_deref(),
        Some(m[2].transition_id.as_str())
    );
    assert!(m.iter().all(|t| t.to != State::TriggerObserved));
}

#[test]
fn a_print_below_the_armed_invalidation_fails_the_setup_immediately() {
    let r = fixture::run("micro_invalidated").unwrap();
    let m = micro(&r.transitions);
    let last = m.last().unwrap();
    assert_eq!(
        (last.to, last.reason),
        (State::Invalidated, "print_below_invalidation")
    );
    let l = last.level.as_ref().unwrap();
    assert_eq!((l.kind, l.price, l.version), ("invalidation", 3_320_000, 1));
    assert_eq!(last.trade.as_ref().unwrap().price, 3_310_000);
    assert_eq!(last.observed_at, end(11) + 8 * S + 100 * MS);
}

#[test]
fn a_new_low_before_arming_is_formation_and_beyond_the_depth_limit_is_invalidation() {
    let r = fixture::run("micro_depth_exceeded").unwrap();
    let m = micro(&r.transitions);
    let last = m.last().unwrap();
    assert_eq!(
        (last.to, last.reason),
        (State::Invalidated, "depth_exceeded")
    );
    assert_eq!(last.observed_at, cutoff(11));
    assert!(m.iter().all(|t| t.to != State::Armed));
}

#[test]
fn levels_never_move_after_arming_and_a_later_lower_low_invalidates() {
    let mut s = Script::new();
    base(&mut s, "MPXL");
    // Post-arm: 3.325 (above I, below the forming low it would have been) then 3.319 (< I).
    s.print_admitted("MPXL", end(11) + 3 * S, end(11) + 3 * S + 1, 3_325_000, 100);
    s.print_admitted("MPXL", end(11) + 6 * S, end(11) + 6 * S + 1, 3_319_000, 100);
    s.advance(cutoff(14));
    let ts = run(&s.steps);
    let m = micro(&ts);
    let last = m.last().unwrap();
    assert_eq!(last.to, State::Invalidated);
    assert_eq!(
        last.level.as_ref().unwrap().price,
        3_320_000,
        "I was not lowered to rescue it"
    );
    assert!(
        m.iter()
            .filter_map(|t| t.level.as_ref())
            .all(|l| l.version == 1)
    );
}

#[test]
fn equal_timestamp_crossing_and_breach_are_flagged_and_invalidation_wins() {
    let mut s = Script::new();
    base(&mut s, "MPXQ");
    let x = end(11) + 4 * S;
    s.print_admitted("MPXQ", x, x + 100 * MS, 3_410_000, 100);
    s.print_admitted("MPXQ", x, x + 101 * MS, 3_310_000, 100);
    s.bar("MPXQ", 13, [341, 345, 340, 344], 300);
    s.advance(cutoff(16));
    let ts = run(&s.steps);
    let m = micro(&ts);
    let ev = events(&m);
    assert_eq!(ev[2].1, State::CrossingProvisional);
    assert_eq!(ev[3].1, State::Invalidated);
    let flag = m[3]
        .checks
        .iter()
        .find(|c| c.name == "ordering_ambiguous")
        .unwrap();
    assert_eq!(flag.state, Tri::Pass);
    assert!(m.iter().all(|t| t.to != State::TriggerObserved));
}

#[test]
fn a_cancelled_crossing_print_retracts_with_a_reference_to_the_crossing() {
    let r = fixture::run("micro_crossing_cancelled").unwrap();
    let m = micro(&r.transitions);
    let cross = m
        .iter()
        .find(|t| t.to == State::CrossingProvisional)
        .unwrap();
    let retract = m.iter().find(|t| t.event == "crossing_retracted").unwrap();
    assert_eq!(retract.reason, "crossing_print_revised");
    assert_eq!(
        retract.correction_of.as_deref(),
        Some(cross.transition_id.as_str())
    );
    assert_eq!(retract.observed_at, end(11) + 6 * S);
}

#[test]
fn revising_the_print_behind_an_invalidation_appends_a_correction_and_keeps_the_outcome() {
    let mut steps = fixture::script("micro_invalidated").unwrap();
    let ts_before = run(&steps);
    // The 3.31 low of bar 12 is that bar's third print: base has 12 bars × 4 prints.
    let seq = 12 * 4 + 3;
    steps.insert(
        steps.len() - 1,
        FixtureStep::Cancel {
            symbol: "MPXD".into(),
            seq,
            price: 3_310_000,
            size: 300,
            admit: cutoff(12) + S,
        },
    );
    let ts = run(&steps);
    let m = micro(&ts);
    let inv = m
        .iter()
        .find(|t| t.to == State::Invalidated && t.certainty == Certainty::Terminal)
        .unwrap();
    let note = m
        .iter()
        .find(|t| t.certainty == Certainty::Correction)
        .unwrap();
    assert_eq!(
        note.correction_of.as_deref(),
        Some(inv.transition_id.as_str())
    );
    assert_eq!(
        (note.from, note.to),
        (Some(State::Invalidated), State::Invalidated)
    );
    // Everything logged before the correction is byte-identical to the run without it.
    let n = ts_before.len();
    assert_eq!(
        serde_json::to_string(&ts_before).unwrap(),
        serde_json::to_string(&ts[..n]).unwrap()
    );
}

#[test]
fn armed_without_a_trigger_expires_and_silence_ends_the_episode_as_stale() {
    let mut s = Script::new();
    base(&mut s, "MPXT");
    for k in 12..24 {
        s.bar("MPXT", k, [334, 336, 333, 335], 100);
    }
    s.advance(cutoff(26));
    let ts = run(&s.steps);
    let last = *micro(&ts).last().unwrap();
    assert_eq!((last.to, last.reason), (State::Expired, "armed_timeout"));
    assert_eq!(last.observed_at, cutoff(20));

    let r = fixture::run("micro_retracted").unwrap();
    let last = *micro(&r.transitions).last().unwrap();
    assert_eq!((last.to, last.reason), (State::Interrupted, "stale_price"));
}

#[test]
fn a_halt_interrupts_and_resumption_requires_fresh_anchors() {
    let r = fixture::run("micro_halt_interrupt").unwrap();
    let m = micro(&r.transitions);
    let int = m.iter().find(|t| t.to == State::Interrupted).unwrap();
    assert_eq!((int.reason, int.observed_at), ("halt", end(11) + 5 * S));
    assert_eq!(
        m.len(),
        3,
        "no new episode from pre-halt anchors after the resume"
    );
    assert!(
        r.snapshot
            .data_status
            .recent_gaps
            .iter()
            .any(|g| g.reason == "halt")
    );
}

#[test]
fn every_transition_cites_levels_known_before_the_crossing_it_tested() {
    for name in fixture::scenario_names() {
        let r = fixture::run(name).unwrap();
        for t in &r.transitions {
            if let (Some(l), Some(tr)) = (&t.level, &t.trade) {
                assert!(l.known_at <= tr.exec, "{name}: {t:?}");
            }
            if t.event == "trigger_observed" {
                let l = t.level.as_ref().unwrap();
                assert!(
                    t.bar_end.unwrap() - 15 * S >= l.armed_at.unwrap(),
                    "{name}: full bar"
                );
            }
        }
    }
}
