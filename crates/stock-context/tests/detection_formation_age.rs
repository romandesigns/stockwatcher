//! Late discovery must obey the same MP5 formation age as a continuously observed setup.
//! Synthetic input through the real engine, including valid boundary controls.

use stock_context::detect::fixture::{self, Script, cutoff, end};
use stock_context::{Engine, Family, Policy, State};

const SEC: i64 = 1_000_000_000;

fn late_discovery(age: i64, pending_history: bool) -> Vec<stock_context::DetectionTransition> {
    let mut engine = if pending_history {
        let mut e = fixture::engine("late-history");
        e.set_history_pending("LATE", true);
        e
    } else {
        // Panel-only scope still accumulates facts for a symbol that is not yet watched.
        Engine::new(
            Policy::current(),
            fixture::engine("late-watch").run_info().clone(),
        )
    };
    let mut script = Script::new();
    for k in 0..6 {
        script.bar("LATE", k, [300, 300, 300, 300], 100);
    }
    script.bar("LATE", 6, [300, 302, 300, 302], 200);
    script.bar("LATE", 7, [302, 310, 302, 310], 300);
    script.bar("LATE", 8, [310, 325, 310, 325], 300);
    script.bar("LATE", 9, [325, 340, 325, 338], 400);
    for k in 10..(9 + age) {
        script.bar("LATE", k, [335, 336, 333, 335], 120);
    }
    script.advance(cutoff(8 + age));
    assert!(fixture::drive(&mut engine, &script.steps).is_empty());
    if pending_history {
        engine.set_history_pending("LATE", false);
    } else {
        engine.set_watched(["LATE".to_string()]);
    }
    script.steps.clear(); // Keep the unique print sequence, without replaying the prefix.
    let k = 9 + age;
    let start = end(k) - 15 * SEC;
    for (offset, cents) in [(3, 335), (4, 336), (8, 333), (12, 335)] {
        script.print("LATE", start + offset * SEC, cents, 120);
    }
    script.advance(cutoff(k));
    let transitions = fixture::drive(&mut engine, &script.steps);
    // Panel-only scope deliberately exposes the shared detector through the setup log.
    let transitions = if pending_history {
        transitions
    } else {
        engine.take_setup_log()
    };
    transitions
        .into_iter()
        .filter(|t| t.family == Family::MicroPullback)
        .collect()
}

fn assert_age_boundary(pending_history: bool) {
    for age in [7, 8, 9, 12] {
        let transitions = late_discovery(age, pending_history);
        if age <= 8 {
            let armed = transitions.iter().find(|t| t.to == State::Armed);
            assert!(armed.is_some(), "valid {age}-slot formation must still arm");
            assert_eq!(armed.unwrap().anchors.as_ref().unwrap().pullback_slots, age);
        } else {
            assert!(
                transitions.is_empty(),
                "decline {age}-slot formation before opening an episode: {transitions:?}"
            );
        }
    }
}

#[test]
fn clearing_pending_history_does_not_make_an_expired_formation_young() {
    assert_age_boundary(true);
}

#[test]
fn watching_a_symbol_late_respects_the_same_formation_age_boundary() {
    assert_age_boundary(false);
}
