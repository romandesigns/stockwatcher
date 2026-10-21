//! Regressions for Codex core audit v2: the current checklist on snapshot episodes, the latest
//! price after a revision of the print it came from, and the quiet-reference admissibility guards.
//! Synthetic fixtures, fictional symbols.

use market_events::{TradeId, UnixNanos};
use stock_context::detect::fixture::{self, FixtureStep, Script, cutoff, end};
use stock_context::*;

const S: i64 = 1_000_000_000;

fn armed(sym: &str) -> Script {
    let mut s = Script::new();
    for k in 0..6 {
        s.bar(sym, k, [300, 300, 300, 300], 100);
    }
    s.bar(sym, 6, [300, 302, 300, 302], 200);
    s.bar(sym, 7, [302, 310, 302, 310], 300);
    s.bar(sym, 8, [310, 325, 310, 325], 300);
    s.bar(sym, 9, [325, 340, 325, 338], 400);
    s.bar(sym, 10, [338, 338, 333, 335], 150);
    s.bar(sym, 11, [335, 336, 332, 334], 120);
    s.advance(cutoff(11));
    s
}

#[test]
fn the_active_armed_episode_carries_its_current_checklist() {
    let mut e = fixture::engine("t");
    fixture::drive(&mut e, &armed("CHKA").steps);
    let snap = e.detection_snapshot().unwrap();
    let ep = &snap.active[0];
    assert_eq!(ep.state, State::Armed);
    let names: Vec<_> = ep
        .checks
        .iter()
        .map(|c| (c.name, c.required, c.state))
        .collect();
    assert!(
        names.contains(&("pullback_bars", true, Tri::Pass)),
        "{names:?}"
    );
    assert!(names.contains(&("depth", true, Tri::Pass)));
    assert!(names.contains(&("volume_condition", false, Tri::Unknown)));
    // An interruption with no checks of its own keeps the last evaluated rules in history.
    e.interrupt_detection(Some("CHKA"), UnixNanos(end(11) + 3 * S), "halt");
    let gone = e.detection_snapshot().unwrap();
    let rec = gone
        .recent
        .iter()
        .find(|r| r.episode_id == ep.episode_id)
        .unwrap();
    assert_eq!(rec.state, State::Interrupted);
    assert_eq!(rec.checks, ep.checks);
}

fn fixture_id(seq: u64) -> TradeId {
    TradeId {
        source: market_events::Source::Fixture,
        exchange: String::new(),
        seq,
    }
}

#[test]
fn revising_the_latest_print_revises_the_current_price_and_never_resurrects_it() {
    let mut s = armed("REVA");
    let seq = s.print("REVA", end(11) + 4 * S, 339, 100);
    let mut e = fixture::engine("t");
    fixture::drive(&mut e, &s.steps);
    let cur = |e: &Engine| {
        e.detection_snapshot().unwrap().active[0]
            .current
            .clone()
            .unwrap()
    };
    assert_eq!(
        (cur(&e).price, cur(&e).near_trigger),
        (3_390_000, Some(true))
    );
    // Reprice the latest (still queued) print: the current price follows the correction.
    e.apply_revision(
        &PrintRevision {
            symbol: "REVA".into(),
            target: RevisionTarget {
                id: fixture_id(seq),
                price: 3_390_000,
                size: 100,
            },
            corrected: Some(CorrectedPrint {
                price: 3_360_000,
                size: 100,
                verdict: Verdict {
                    ohlc: true,
                    volume: true,
                },
            }),
            last: None,
            day_high: None,
        },
        UnixNanos(end(11) + 5 * S),
    );
    assert_eq!(
        (cur(&e).price, cur(&e).near_trigger),
        (3_360_000, Some(false))
    );
    // Cancel it: back to the latest standing print (bar 11's close), never the cancelled one.
    fixture::drive(
        &mut e,
        &[FixtureStep::Cancel {
            symbol: "REVA".into(),
            seq,
            price: 3_360_000,
            size: 100,
            admit: end(11) + 6 * S,
        }],
    );
    let c = cur(&e);
    assert_eq!(c.price, 3_340_000);
    assert_eq!(c.price_exec, end(11) - 3 * S);
    assert_eq!(
        e.detection_snapshot().unwrap().active[0].state,
        State::Armed
    );
}

fn with_reference(edit: impl Fn(&mut QuietReference)) -> Vec<FixtureStep> {
    let mut steps = fixture::script("quiet_build_ignition").unwrap();
    for s in steps.iter_mut() {
        if let FixtureStep::QuietReference(r) = s {
            edit(r);
        }
    }
    steps
}

fn context_of(steps: &[FixtureStep], engine: Engine) -> (Vec<DetectionTransition>, Engine) {
    let mut e = engine;
    let ts = fixture::drive(&mut e, steps);
    (ts, e)
}

type Edit = Box<dyn Fn(&mut QuietReference)>;

#[test]
fn inadmissible_references_leave_quietness_unavailable_with_the_reason() {
    let cases: Vec<(&str, Edit)> = vec![
        (
            "quiet_reference_too_few_sessions",
            Box::new(|r| r.prior_sessions = 3),
        ),
        (
            "quiet_reference_too_few_sessions",
            Box::new(|r| r.prior_sessions = 0),
        ),
        (
            "quiet_reference_wrong_percentile",
            Box::new(|r| r.percentile = 25),
        ),
        (
            "quiet_reference_basis_unsupported",
            Box::new(|r| r.basis = "raw".into()),
        ),
        (
            "quiet_reference_basis_unsupported",
            Box::new(|r| r.basis = String::new()),
        ),
    ];
    for (why, edit) in cases {
        let (ts, _) = context_of(&with_reference(edit), fixture::engine("t"));
        assert!(
            ts.iter().all(|t| t.family != Family::MomentumBuilding),
            "{why}"
        );
        let acc = ts
            .iter()
            .find(|t| t.family == Family::UpwardAcceleration)
            .unwrap();
        assert_eq!(acc.reason, why);
        assert!(
            ts.iter().all(|t| t.family != Family::RangeBreakout),
            "{why}"
        );
    }
    // The proposal's explicit pooled fallback is usable, and says so.
    let (ts, _) = context_of(
        &with_reference(|r| {
            r.prior_sessions = 3;
            r.short_history = true;
        }),
        fixture::engine("t"),
    );
    let mb = ts
        .iter()
        .find(|t| t.family == Family::MomentumBuilding)
        .unwrap();
    assert_eq!(mb.reason, "quiet_short_history");
    // A synthetic reference outside a fixture run is refused by the core itself.
    let real = Engine::new(
        Policy::current(),
        RunInfo {
            run_id: "replay".into(),
            source: "replay".into(),
            fidelity: Fidelity::ReconstructedHistory,
            numerator_coverage: Coverage::Unverified,
        },
    )
    .with_detection_policy(DetectionPolicy::current());
    let (ts, _) = context_of(&with_reference(|_| {}), real);
    let acc = ts
        .iter()
        .find(|t| t.family == Family::UpwardAcceleration)
        .unwrap();
    assert_eq!(acc.reason, "quiet_reference_synthetic_outside_fixture");
}

/// The same script with every print price multiplied by `num / den`.
fn scaled(mut steps: Vec<FixtureStep>, num: i64, den: i64) -> Vec<FixtureStep> {
    for s in steps.iter_mut() {
        match s {
            FixtureStep::Print { price, .. } | FixtureStep::Cancel { price, .. } => {
                *price = *price * num / den
            }
            _ => {}
        }
    }
    steps
}

fn families(steps: &[FixtureStep]) -> Vec<(Family, State, &'static str)> {
    let mut e = fixture::engine("t");
    fixture::drive(&mut e, steps)
        .iter()
        .map(|t| (t.family, t.to, t.reason))
        .collect()
}

#[test]
fn micro_and_momentum_stay_inside_the_active_band() {
    // In band ($3), the scenarios detect; outside ($30 and $0.03) they detect nothing at all.
    let micro = fixture::script("micro_valid").unwrap();
    let build = fixture::script("quiet_build_ignition").unwrap();
    assert!(
        families(&micro)
            .iter()
            .any(|f| f.0 == Family::MicroPullback)
    );
    assert!(
        families(&build)
            .iter()
            .any(|f| f.0 == Family::MomentumBuilding)
    );
    for (num, den) in [(10, 1), (1, 100)] {
        assert!(
            families(&scaled(micro.clone(), num, den)).is_empty(),
            "micro x{num}/{den}"
        );
        assert!(
            families(&scaled(build.clone(), num, den)).is_empty(),
            "build x{num}/{den}"
        );
    }
}

#[test]
fn twenty_five_dollars_is_inside_and_one_cent_above_ends_the_episode() {
    let mut s = Script::new();
    // The impulse is $23.50 → $25.00 (6.4 %): over detection-v2's 5 % micro-pullback minimum.
    for k in 0..6 {
        s.bar("BAND", k, [2350, 2350, 2350, 2350], 100);
    }
    s.bar("BAND", 6, [2350, 2360, 2350, 2360], 200);
    s.bar("BAND", 7, [2360, 2410, 2360, 2410], 300);
    s.bar("BAND", 8, [2410, 2470, 2410, 2470], 300);
    s.bar("BAND", 9, [2470, 2500, 2470, 2496], 400);
    s.bar("BAND", 10, [2496, 2496, 2485, 2490], 150);
    s.bar("BAND", 11, [2490, 2492, 2480, 2488], 120);
    // Closes exactly at $25.00: inside the inclusive band, and not above T = $25.00.
    s.bar("BAND", 12, [2488, 2500, 2488, 2500], 200);
    // Closes at $25.01: outside the band.
    s.bar("BAND", 13, [2500, 2501, 2499, 2501], 200);
    s.advance(cutoff(16));
    let f = families(&s.steps);
    let micro: Vec<_> = f.iter().filter(|x| x.0 == Family::MicroPullback).collect();
    assert_eq!(micro[1].1, State::Armed);
    assert!(micro.iter().all(|x| x.1 != State::TriggerObserved));
    let last = micro.last().unwrap();
    assert_eq!(
        (last.1, last.2),
        (State::Interrupted, "outside_active_band")
    );
}

#[test]
fn momentum_needs_every_slot_of_both_comparison_windows() {
    // Remove bar 39, the interior slot of the earlier window at bar 42. The precursor then waits
    // until bar 44, whose windows (41..44 and endpoint 40) all traded.
    let steps: Vec<FixtureStep> = fixture::script("quiet_build_ignition")
        .unwrap()
        .into_iter()
        .filter(|s| !matches!(s, FixtureStep::Print { exec, .. } if *exec >= end(39) - 15 * S && *exec < end(39)))
        .collect();
    let mut e = fixture::engine("t");
    let ts = fixture::drive(&mut e, &steps);
    let mb = ts
        .iter()
        .find(|t| t.family == Family::MomentumBuilding)
        .unwrap();
    assert_eq!(mb.observed_at, cutoff(44));
    let full = fixture::run("quiet_build_ignition").unwrap();
    let mb_full = full
        .transitions
        .iter()
        .find(|t| t.family == Family::MomentumBuilding)
        .unwrap();
    assert_eq!(mb_full.observed_at, cutoff(42));
}

#[test]
fn a_resume_stamped_before_its_gap_closes_the_gap_at_its_start() {
    let mut s = Script::new();
    s.quiet("GAPX", 0..12, 500, 501);
    s.advance(cutoff(11));
    let mut e = fixture::engine("t");
    fixture::drive(&mut e, &s.steps);
    let from = end(11) + 5 * S;
    e.interrupt_detection(None, UnixNanos(from), "provider_error");
    // A buffered event's receipt time, older than the error.
    e.resume_detection(None, UnixNanos(from - 3 * S));
    let snap = e.detection_snapshot().unwrap();
    let gap = snap.data_status.recent_gaps.last().unwrap();
    assert_eq!(
        (gap.from, gap.to),
        (from, Some(from)),
        "never an inverted interval"
    );
    assert!(snap.data_status.global_interruption.is_none());
}

#[test]
fn upward_acceleration_lists_its_context_as_a_descriptor_not_a_requirement() {
    let mut steps = fixture::script("quiet_build_ignition").unwrap();
    steps.retain(|s| !matches!(s, FixtureStep::QuietReference(_) | FixtureStep::Advance(_)));
    // Stop while the acceleration episode is still active, so the snapshot shows it.
    steps.retain(|s| !matches!(s, FixtureStep::Print { exec, .. } if *exec >= end(43) - 15 * S));
    steps.push(FixtureStep::Advance(cutoff(42)));
    let mut e = fixture::engine("t");
    fixture::drive(&mut e, &steps);
    let snap = e.detection_snapshot().unwrap();
    let ep = snap
        .active
        .iter()
        .find(|e| e.family == Family::UpwardAcceleration)
        .expect("an active acceleration episode");
    let ctx = ep
        .checks
        .iter()
        .find(|c| c.name == "prior_context")
        .unwrap();
    assert_eq!((ctx.state, ctx.required), (Tri::Unknown, false));
    assert!(
        ep.checks
            .iter()
            .filter(|c| c.required)
            .all(|c| c.state == Tri::Pass),
        "nothing required is pending: {:?}",
        ep.checks
    );
    // The quiet family keeps it as its rule.
    let r = fixture::run("quiet_build_ignition").unwrap();
    let mb = r
        .transitions
        .iter()
        .find(|t| t.family == Family::MomentumBuilding)
        .unwrap();
    let c = mb
        .checks
        .iter()
        .find(|c| c.name == "prior_context")
        .unwrap();
    assert_eq!((c.state, c.required), (Tri::Pass, true));
}
