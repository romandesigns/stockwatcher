//! Detection lifecycle through the engine: replay determinism, run isolation, bounded output,
//! clock-driven expiry without prints, observation gaps, coverage, segment ends, restores and
//! warmup marking. Synthetic fixtures, fictional symbols.

use market_events::UnixNanos;
use stock_context::detect::fixture::{self, FixtureStep, Script, cutoff, end};
use stock_context::detect::policy::DETECTION_V0;
use stock_context::*;

const S: i64 = 1_000_000_000;

fn json<T: serde::Serialize + ?Sized>(v: &T) -> String {
    serde_json::to_string(v).unwrap()
}

fn micro_at(s: &mut Script, sym: &str, k0: i64) {
    for k in 0..6 {
        s.bar(sym, k0 + k, [300, 300, 300, 300], 100);
    }
    for (k, b) in [
        [300, 302, 300, 302],
        [302, 310, 302, 310],
        [310, 325, 310, 325],
        [325, 340, 325, 338],
        [338, 338, 333, 335],
        [335, 336, 332, 334],
    ]
    .into_iter()
    .enumerate()
    {
        s.bar(sym, k0 + 6 + k as i64, b, 200);
    }
}

#[test]
fn replays_reproduce_records_and_runs_never_share_state() {
    let steps = fixture::script("quiet_build_ignition").unwrap();
    let mut a = fixture::engine("replay-1");
    let mut b = fixture::engine("replay-1");
    let mut other = fixture::engine("live-7");
    let ta = fixture::drive(&mut a, &steps);
    let tb = fixture::drive(&mut b, &steps);
    assert_eq!(
        json(&ta),
        json(&tb),
        "a rebuilt run reproduces every record"
    );
    assert_eq!(json(&a.detection_snapshot()), json(&b.detection_snapshot()));
    // Another run on the same input: same semantics, its own ids, nothing leaks across.
    let to = fixture::drive(&mut other, &steps);
    assert_eq!(ta.len(), to.len());
    assert!(to.iter().all(|t| t.episode_id.starts_with("live-7:")));
    assert!(ta.iter().all(|t| t.episode_id.starts_with("replay-1:")));
    let strip = |ts: &[DetectionTransition], run: &str| json(ts).replace(run, "RUN");
    assert_eq!(strip(&ta, "replay-1"), strip(&to, "live-7"));
    // Ids and sequence are unique and the snapshot cursor is the last one.
    let mut ids: Vec<_> = ta.iter().map(|t| t.transition_id.clone()).collect();
    ids.dedup();
    assert_eq!(ids.len(), ta.len());
    assert!(ta.windows(2).all(|w| w[1].seq == w[0].seq + 1));
    assert_eq!(
        a.detection_snapshot().unwrap().cursor,
        ta.last().unwrap().seq
    );
}

#[test]
fn a_full_transition_journal_drops_the_oldest_and_says_so() {
    let small = DETECTION_V0.replace("\"transition_log_max\": 8192", "\"transition_log_max\": 2");
    let mut e = Engine::new(Policy::current(), fixture::engine("t").run_info().clone())
        .with_detection_policy(DetectionPolicy::parse(&small).unwrap());
    // Drive without taking, so the bound bites.
    let steps = fixture::script("micro_valid").unwrap();
    for s in &steps {
        if let FixtureStep::Print {
            symbol,
            exec,
            admit,
            price,
            size,
            seq,
        } = s
        {
            let t = market_events::Trade::new(
                symbol.as_str(),
                UnixNanos(*exec),
                market_events::Price::from_micros(*price),
                *size,
                *seq,
            );
            e.apply(
                &t,
                Verdict {
                    ohlc: true,
                    volume: true,
                },
                UnixNanos(*admit),
            );
        }
    }
    e.advance_to(UnixNanos(cutoff(20)));
    let tel = e.detection_telemetry();
    assert_eq!(tel.transitions_logged, 4);
    assert_eq!(tel.transitions_dropped, 2);
    assert!(!tel.complete());
    let held = e.take_detection_transitions();
    assert_eq!(held.len(), 2);
    assert_eq!(held[0].event, "crossing_provisional", "the newest are kept");
    let tel = e.detection_telemetry();
    assert_eq!(
        tel.transitions_logged,
        tel.transitions_taken + tel.transitions_dropped + tel.transitions_held as u64
    );
}

#[test]
fn expiry_needs_no_prints_and_no_membership() {
    let mut s = Script::new();
    s.quiet("SILN", 0..40, 500, 501);
    s.bar("SILN", 40, [500, 510, 499, 508], 400);
    s.advance(cutoff(60));
    let mut e = fixture::engine("t");
    let ts = fixture::drive(&mut e, &s.steps);
    let ig: Vec<_> = ts.iter().filter(|t| t.family == Family::Ignition).collect();
    assert_eq!(ig.len(), 2);
    assert_eq!(
        (ig[1].to, ig[1].observed_at),
        (State::Completed, cutoff(48))
    );
    assert!(
        e.panel().is_none_or(|p| p.rows.is_empty()),
        "not a Ross member"
    );
}

#[test]
fn a_link_gap_interrupts_everything_and_resumption_requalifies() {
    let mut s = Script::new();
    micro_at(&mut s, "LNKA", 0);
    s.quiet("LNKB", 0..12, 500, 501);
    s.steps.push(FixtureStep::Interrupt {
        symbol: None,
        at: end(11) + 5 * S,
        reason: "link_down",
    });
    let mut e = fixture::engine("t");
    let ts = fixture::drive(&mut e, &s.steps);
    let int: Vec<_> = ts.iter().filter(|t| t.to == State::Interrupted).collect();
    assert_eq!(int.len(), 1);
    assert_eq!(
        (int[0].reason, int[0].symbol.as_str()),
        ("link_down", "LNKA")
    );
    let snap = e.detection_snapshot().unwrap();
    assert_eq!(
        snap.data_status.global_interruption.unwrap().reason,
        "link_down"
    );
    assert!(snap.active.is_empty());

    // After the resume, nothing anchors or baselines on pre-gap history.
    let mut r = Script::new();
    r.steps.push(FixtureStep::Resume {
        symbol: None,
        at: end(30),
    });
    for k in 31..34 {
        r.bar("LNKB", k, [500, 501, 500, 501], 100);
    }
    r.bar("LNKB", 34, [501, 520, 500, 519], 400);
    r.advance(cutoff(36));
    let ts = fixture::drive(&mut e, &r.steps);
    assert!(ts.is_empty());
    let f = e.latest_features("LNKB").unwrap();
    assert_eq!(f.baseline.status, "sparse_baseline");
    assert_eq!(f.window.context_reason, "data_gap");
    let snap = e.detection_snapshot().unwrap();
    assert!(snap.data_status.global_interruption.is_none());
    let gap = &snap.data_status.recent_gaps[0];
    assert_eq!(
        (gap.from, gap.to, gap.reason),
        (end(11) + 5 * S, Some(end(30)), "link_down")
    );
}

#[test]
fn lost_coverage_interrupts_and_suppresses_new_detections() {
    let mut s = Script::new();
    micro_at(&mut s, "COVA", 0);
    s.advance(cutoff(11));
    let mut e = fixture::engine("t");
    let ts = fixture::drive(&mut e, &s.steps);
    assert_eq!(ts.last().unwrap().to, State::Armed);
    e.set_coverage("COVA", Some((Coverage::Partial, "stream_gap")));
    let mut r = Script::new();
    r.bar("COVA", 12, [335, 341, 335, 342], 300);
    r.bar("COVA", 13, [342, 344, 339, 343], 300);
    r.advance(cutoff(15));
    let ts = fixture::drive(&mut e, &r.steps);
    // The provisional crossing is trade-driven and still recorded; the barrier then interrupts
    // the episode on the coverage loss instead of confirming it.
    let last = ts.last().unwrap();
    assert_eq!(
        (last.to, last.reason),
        (State::Interrupted, "coverage_partial")
    );
    assert!(ts.iter().all(|t| t.to != State::TriggerObserved));
}

#[test]
fn an_episode_ends_with_its_segment() {
    // Armed at 15:59:32; the regular session closes at 16:00.
    let k0 = 1546;
    let mut s = Script::new();
    micro_at(&mut s, "SEGX", k0);
    s.print("SEGX", end(k0 + 11) + 5 * S, 335, 100);
    s.print("SEGX", end(k0 + 11) + 20 * S, 335, 100);
    s.advance(cutoff(k0 + 16));
    let mut e = fixture::engine("t");
    let ts = fixture::drive(&mut e, &s.steps);
    let armed = ts.iter().find(|t| t.to == State::Armed).unwrap();
    assert_eq!(armed.observed_at, cutoff(k0 + 11));
    let last = ts.last().unwrap();
    assert_eq!((last.to, last.reason), (State::Interrupted, "segment_end"));
    assert_eq!(last.bar_end, Some(fixture::at(16, 0, 15)));
}

#[test]
fn restores_bring_facts_never_episodes_and_need_the_same_detection_policy() {
    let steps = fixture::script("micro_valid").unwrap();
    let cut = steps
        .iter()
        .position(|s| matches!(s, FixtureStep::Print { exec, .. } if *exec >= end(11)))
        .unwrap();
    // The restore engine replays the day up to bar 11 and has an armed episode of its own.
    let mut restore = fixture::engine("live");
    fixture::drive(&mut restore, &steps[..cut]);
    restore.advance_to(UnixNanos(cutoff(11)));
    assert_eq!(restore.detection_snapshot().unwrap().active.len(), 1);
    let mut live = fixture::engine("live");
    live.advance_to(UnixNanos(cutoff(11)));
    assert_eq!(
        live.adopt_facts(&mut restore, "MPXA"),
        Adoption::Adopted { published: false }
    );
    let snap = live.detection_snapshot().unwrap();
    assert!(
        snap.active.is_empty(),
        "the restore's episode is not this run's claim"
    );
    assert_eq!(live.restore_telemetry().episodes_discarded, 1);
    // A different detection policy is refused, like any other policy mismatch.
    let mut odd = Engine::new(Policy::current(), live.run_info().clone()).with_detection_policy(
        DetectionPolicy::parse(&DETECTION_V0.replace("\"expiry_slots\": 8", "\"expiry_slots\": 9"))
            .unwrap(),
    );
    fixture::drive(&mut odd, &steps[..cut]);
    assert_eq!(
        live.adopt_facts(&mut odd, "MPXA"),
        Adoption::Refused("detection_policy")
    );
}

#[test]
fn recognitions_reading_a_restored_prefix_are_marked_warmup() {
    let steps = fixture::script("ignition_no_precursor").unwrap();
    let split = steps
        .iter()
        .position(|s| matches!(s, FixtureStep::Print { exec, .. } if *exec >= end(39)))
        .unwrap();
    let mut e = fixture::engine("t");
    fixture::drive(&mut e, &steps[..split]);
    e.set_restored_through("IGNB", Some(end(30)));
    let ts = fixture::drive(&mut e, &steps[split..]);
    let ig = ts.iter().find(|t| t.family == Family::Ignition).unwrap();
    assert!(ig.warmup, "its window reached into the restored prefix");
    let plain = fixture::run("ignition_no_precursor").unwrap();
    assert!(plain.transitions.iter().all(|t| !t.warmup));
}

#[test]
fn the_snapshot_names_one_dominant_episode_and_proximity_never_moves_seq() {
    let mut s = Script::new();
    micro_at(&mut s, "DOMX", 0);
    s.advance(cutoff(11));
    let mut e = fixture::engine("t");
    fixture::drive(&mut e, &s.steps);
    let before = e.detection_snapshot().unwrap();
    // Prints between I and T move the current distance, never the sequence.
    let mut r = Script::new();
    r.print("DOMX", end(11) + 4 * S, 336, 100);
    r.print("DOMX", end(11) + 6 * S, 339, 100);
    fixture::drive(&mut e, &r.steps);
    let after = e.detection_snapshot().unwrap();
    assert_eq!(before.cursor, after.cursor);
    let ep = &after.active[0];
    assert_eq!(
        after.symbols[0].dominant_episode_id.as_deref(),
        Some(ep.episode_id.as_str())
    );
    assert_eq!(ep.last_material_seq, before.active[0].last_material_seq);
    let cur = ep.current.as_ref().unwrap();
    assert_eq!(cur.price, 3_390_000);
    assert_eq!(cur.distance_to_trigger_micros, Some(10_000));
    assert_eq!(cur.distance_to_trigger_bp, Some(29));
    assert_eq!(cur.distance_to_invalidation_micros, Some(70_000));
    assert_eq!(cur.at_or_above_trigger, Some(false));
}

#[test]
fn descriptors_are_marked_optional_and_rules_required() {
    let mut all = Vec::new();
    for name in fixture::scenario_names() {
        let r = fixture::run(name).unwrap();
        for t in &r.transitions {
            all.extend(t.checks.iter().map(|c| (c.name, c.required)));
        }
    }
    let req = |n: &str| all.iter().find(|(x, _)| *x == n).map(|(_, r)| *r);
    assert_eq!(req("volume_condition"), Some(false));
    assert_eq!(req("dollar_volume_rate"), Some(false));
    assert_eq!(req("below_range_high"), Some(false));
    assert_eq!(req("extended"), Some(false));
    assert_eq!(req("robust_surprise"), Some(true));
    assert_eq!(req("range_expansion"), Some(true));
    assert_eq!(req("pullback_bars"), Some(true));
    assert_eq!(req("close_above_trigger"), Some(true));
}

#[test]
fn the_monitored_count_includes_symbols_without_episodes() {
    let mut s = Script::new();
    micro_at(&mut s, "MONA", 0);
    s.quiet("MONB", 0..12, 500, 501);
    s.quiet("MONC", 0..12, 700, 701);
    s.advance(cutoff(11));
    let mut e = fixture::engine("t");
    fixture::drive(&mut e, &s.steps);
    e.interrupt_detection(Some("MONC"), UnixNanos(end(11) + 3 * S), "halt");
    let m = e.detection_snapshot().unwrap().monitored;
    assert_eq!(m.symbols_seen, 3);
    assert_eq!(m.symbols_today, 3);
    assert_eq!(m.symbols_active, 1);
    assert_eq!(m.symbols_interrupted, 1);
}

#[test]
fn an_armed_micro_stays_armed_with_an_optional_unknown_volume_check() {
    let mut s = Script::new();
    micro_at(&mut s, "VOLX", 0);
    s.advance(cutoff(11));
    let mut e = fixture::engine("t");
    let ts = fixture::drive(&mut e, &s.steps);
    let armed = ts.last().unwrap();
    assert_eq!(armed.to, State::Armed);
    let v = armed.checks.iter().find(|c| c.name == "volume_condition").unwrap();
    assert!(!v.required);
    assert!(armed.checks.iter().filter(|c| c.required).all(|c| c.state == Tri::Pass));
    let ep = &e.detection_snapshot().unwrap().active[0];
    assert_eq!(ep.state, State::Armed);
}

#[test]
fn one_monitored_symbol_without_a_setup_counts_as_one() {
    let mut s = Script::new();
    s.quiet("ONLY", 0..5, 500, 501);
    s.advance(cutoff(5));
    let mut e = fixture::engine("t");
    fixture::drive(&mut e, &s.steps);
    let snap = e.detection_snapshot().unwrap();
    assert!(snap.active.is_empty());
    assert_eq!(snap.monitored.symbols_seen, 1);
    assert_eq!(snap.monitored.symbols_today, 1);
}

#[test]
fn near_trigger_is_geometry_at_the_policy_boundary_and_absent_without_a_rule() {
    let near = |micros: i64, at: i64| {
        let mut s = Script::new();
        micro_at(&mut s, "NEAR", 0);
        s.advance(cutoff(11));
        s.print_admitted("NEAR", at, at + 1, micros, 100);
        let mut e = fixture::engine("t");
        fixture::drive(&mut e, &s.steps);
        let cur = e.detection_snapshot().unwrap().active[0].current.clone().unwrap();
        (cur.near_trigger, cur.near_trigger_bp, e.detection_snapshot().unwrap().cursor)
    };
    let t = end(11) + 3 * S;
    // T = 3.40: 3.3664 is 0.0336 below, 99.8 bp of the price.
    assert_eq!(near(3_366_400, t).0, Some(true));
    assert_eq!(near(3_350_000, t).0, Some(false), "149 bp below");
    assert_eq!(near(3_390_000, t).1, Some(100));
    // Proximity never sequences anything.
    assert_eq!(near(3_390_000, t).2, near(3_350_000, t).2);
    // Exactly at the boundary: (T − P)·10⁴ = 100·P ⇔ P = T / 1.01.
    let p = 3_400_000 * 100 / 101; // 3.366336…, truncated down: one micro outside
    assert_eq!(near(p, t).0, Some(false));
    assert_eq!(near(p + 1, t).0, Some(true));
    // v0 has no near rule at all.
    let mut s = Script::new();
    micro_at(&mut s, "NEAR", 0);
    s.advance(cutoff(11));
    let mut e = Engine::new(Policy::current(), fixture::engine("t").run_info().clone())
        .with_detection_policy(DetectionPolicy::v0());
    fixture::drive(&mut e, &s.steps);
    let cur = e.detection_snapshot().unwrap().active[0].current.clone().unwrap();
    assert_eq!((cur.near_trigger, cur.near_trigger_bp), (None, None));
}
