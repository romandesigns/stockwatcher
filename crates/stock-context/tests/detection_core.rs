//! Detection families A (quiet context, momentum building, 1.15× diagnostics, ignition, range
//! breakout) through the real engine: timing, causality and the handoff's acceptance cases.
//! Synthetic fixtures with fictional symbols; nothing here is evidence of usefulness.

use stock_context::detect::fixture::{self, Script, cutoff, end};
use stock_context::detect::{ReturnClass, policy::DETECTION_V0};
use stock_context::*;

const S: i64 = 1_000_000_000;

fn run(steps: &[fixture::FixtureStep]) -> (Engine, Vec<DetectionTransition>) {
    let mut e = fixture::engine("t");
    let ts = fixture::drive(&mut e, steps);
    (e, ts)
}

fn of(ts: &[DetectionTransition], fam: Family) -> Vec<&DetectionTransition> {
    ts.iter().filter(|t| t.family == fam).collect()
}

/// 40 quiet adjacent bars, then bar 40 opened/high/low without its close print.
fn ignition_without_close(s: &mut Script) {
    s.quiet("IGNB", 0..40, 500, 501);
    let st = end(40) - 15 * S;
    s.print("IGNB", st + S, 500, 100);
    s.print("IGNB", st + 4 * S, 510, 100);
    s.print("IGNB", st + 8 * S, 499, 100);
}

#[test]
fn ignition_without_precursor_is_recognized_at_its_cutoff_and_quietness_stays_unavailable() {
    let r = fixture::run("ignition_no_precursor").unwrap();
    let ig = of(&r.transitions, Family::Ignition);
    assert_eq!(ig[0].event, "ignition_recognized");
    assert_eq!(
        ig[0].observed_at,
        cutoff(40),
        "recognized at bar end + settle"
    );
    assert_eq!(ig[0].bar_end, Some(end(40)));
    assert_eq!(ig[0].certainty, Certainty::Confirmed);
    let f = ig[0].features.as_ref().unwrap();
    assert_eq!(f.return_class, ReturnClass::Adjacent);
    assert!(f.z_milli.unwrap() >= 4000, "z {:?}", f.z_milli);
    assert_eq!(f.return_ge_1pct, Some(true));
    assert!(f.range.e_adj_milli.unwrap() >= 3000);
    assert_eq!(
        f.range.floor_binding,
        Some(true),
        "1-cent ranges: the 2-increment floor binds"
    );
    // No quiet reference: context unavailable, never quiet or already moving, and the parent
    // ignition is still evaluated.
    assert_eq!(f.window.context, ContextClass::Unavailable);
    assert!(of(&r.transitions, Family::MomentumBuilding).is_empty());
    assert!(of(&r.transitions, Family::UpwardAcceleration).is_empty());
    // Experimental, never an alert.
    assert!(r.snapshot.experimental && !r.snapshot.alerts_enabled);
}

#[test]
fn the_candidate_is_excluded_from_its_own_baseline() {
    let r = fixture::run("ignition_no_precursor").unwrap();
    let f = of(&r.transitions, Family::Ignition)[0]
        .features
        .clone()
        .unwrap();
    // Bars 1..39 give 39 adjacent returns; the candidate's own return would make 40.
    assert_eq!(f.baseline.returns_n, 39);
    // 39 alternating returns, 20 up and 19 down: the median is the up move, ln(5.01/5.00).
    assert_eq!(f.baseline.median_ppm, Some(1998));
    assert_eq!(
        f.window.end,
        end(39),
        "W ends at the slot before the candidate"
    );
    assert_eq!(f.window.slots_expected, 20);
}

#[test]
fn a_print_admitted_one_nanosecond_before_the_cutoff_counts_and_at_or_after_it_does_not() {
    let detect = |admit: i64| {
        let mut s = Script::new();
        ignition_without_close(&mut s);
        s.print_admitted("IGNB", end(40) - 2 * S, admit, 5_080_000, 100);
        s.advance(cutoff(44));
        let (_, ts) = run(&s.steps);
        of(&ts, Family::Ignition)
            .first()
            .map(|t| (t.observed_at, t.features.as_ref().unwrap().close))
    };
    assert_eq!(detect(cutoff(40) - 1), Some((cutoff(40), 5_080_000)));
    // Exactly at tau the clock decides the barrier first (the engine's inherited rule), and the
    // late print never edits the completed bar.
    assert_eq!(detect(cutoff(40)), None);
    assert_eq!(detect(cutoff(40) + 1), None);
}

#[test]
fn quiet_build_then_ignition_are_separate_linked_recognitions() {
    let r = fixture::run("quiet_build_ignition").unwrap();
    let mb = of(&r.transitions, Family::MomentumBuilding);
    let ig = of(&r.transitions, Family::Ignition);
    assert_eq!(mb[0].to, State::Building);
    assert_eq!(mb[0].observed_at, cutoff(42));
    assert_eq!(ig[0].observed_at, cutoff(45));
    assert!(
        mb[0].observed_at < ig[0].observed_at,
        "the precursor came first"
    );
    let precursor = mb[0].episode_id.clone();
    assert!(ig[0].related_episode_ids.contains(&precursor));
    assert_eq!(mb[1].to, State::Ignited);
    assert!(mb[1].related_episode_ids.contains(&ig[0].episode_id));
    let ep = r
        .snapshot
        .recent
        .iter()
        .find(|e| e.episode_id == ig[0].episode_id)
        .unwrap();
    assert_eq!(ep.parent_episode_id.as_deref(), Some(precursor.as_str()));
    // The precursor's context came from a synthetic reference, and says so.
    let rec = r
        .snapshot
        .recent
        .iter()
        .find(|e| e.episode_id == precursor)
        .unwrap();
    assert_eq!(rec.context, Some(ContextClass::ActiveConsolidation));
    // The frozen context window is a range_high / range_low pair, version 1.
    let kinds: Vec<_> = rec.levels.iter().map(|l| (l.kind, l.version)).collect();
    assert_eq!(kinds, vec![("range_high", 1), ("range_low", 1)]);
    // The ignition rule itself never read quietness: its window carries the reference only as
    // a stratum label.
    let fi = ig[0].features.as_ref().unwrap();
    assert_eq!(fi.window.quiet_reference_synthetic, Some(true));
}

#[test]
fn ignition_and_momentum_summaries_copy_their_own_checks() {
    let r = fixture::run("quiet_build_ignition").unwrap();
    let all: Vec<&DetectionEpisode> = r
        .snapshot
        .active
        .iter()
        .chain(r.snapshot.recent.iter())
        .collect();
    let check = |e: &DetectionEpisode, name: &str| {
        e.checks
            .iter()
            .find(|c| c.name == name)
            .and_then(|c| c.value)
    };
    let ig = all
        .iter()
        .find(|e| e.family == Family::Ignition)
        .expect("ignition");
    let facts: Vec<_> = ig
        .summary
        .iter()
        .map(|f| (f.name, f.value, f.units, f.window_secs))
        .collect();
    let f = ig.recognition.as_ref().unwrap();
    assert_eq!(
        facts,
        vec![
            (
                "range_expansion",
                check(ig, "range_expansion").unwrap(),
                "milli",
                None
            ),
            (
                "return",
                check(ig, "return_at_least_1pct").unwrap(),
                "log_ppm",
                Some(15)
            ),
        ]
    );
    assert_eq!(f.elapsed_secs, Some(15), "an adjacent bar: a 15 s return");
    let mb = all
        .iter()
        .find(|e| e.family == Family::MomentumBuilding)
        .expect("momentum");
    let step = DetectionPolicy::current().body.momentum.step_slots * 15;
    let mut want = Vec::new();
    if let Some(dv) = check(mb, "dollar_volume_rate") {
        want.push(("dollar_volume_rate", dv, "milli", Some(step)));
    }
    want.push(("rise", check(mb, "rise").unwrap(), "log_ppm", Some(step)));
    let got: Vec<_> = mb
        .summary
        .iter()
        .map(|f| (f.name, f.value, f.units, f.window_secs))
        .collect();
    assert_eq!(got, want);
    // Families whose note is their levels carry none.
    assert!(
        all.iter()
            .filter(|e| matches!(e.family, Family::MicroPullback | Family::RangeBreakout))
            .all(|e| e.summary.is_empty())
    );
    // On the wire it is an additive key of the episode record.
    let json = serde_json::to_value(ig).unwrap();
    assert_eq!(json["summary"][0]["name"], "range_expansion");
    assert_eq!(json["summary"][1]["window_secs"], 15);
}

#[test]
fn a_reference_observed_after_the_cutoff_or_from_today_cannot_label_quietness() {
    for late in [true, false] {
        let mut steps = fixture::script("quiet_build_ignition").unwrap();
        for s in steps.iter_mut() {
            if let fixture::FixtureStep::QuietReference(r) = s {
                if late {
                    r.observed_at = fixture::at(11, 0, 0);
                } else {
                    r.prior_through = fixture::day();
                }
            }
        }
        let (_, ts) = run(&steps);
        assert!(of(&ts, Family::MomentumBuilding).is_empty(), "late={late}");
        let acc = of(&ts, Family::UpwardAcceleration);
        assert_eq!(
            acc[0].observed_at,
            cutoff(42),
            "same rise, separately named"
        );
        assert!(!acc[0].default_visible);
        assert_eq!(of(&ts, Family::Ignition)[0].observed_at, cutoff(45));
    }
}

#[test]
fn exactly_1_15x_range_passes_while_body_is_judged_on_its_own() {
    let mk = |close: i64| {
        let mut s = Script::new();
        for k in 0..30 {
            let st = end(k) - 15 * S;
            s.print_admitted("DIAG", st + S, st + S + 1, 10_000_000, 100);
            s.print_admitted("DIAG", st + 4 * S, st + 4 * S + 1, 10_200_000, 100);
            s.print_admitted("DIAG", st + 12 * S, st + 12 * S + 1, 10_100_000, 100);
        }
        let st = end(30) - 15 * S;
        // Range exactly 0.23 = 1.15 × 0.20; body close − 10.10.
        s.print_admitted("DIAG", st + S, st + S + 1, 10_100_000, 100);
        s.print_admitted("DIAG", st + 4 * S, st + 4 * S + 1, 10_330_000, 100);
        s.print_admitted("DIAG", st + 12 * S, st + 12 * S + 1, close, 100);
        s.advance(cutoff(30));
        let (e, ts) = run(&s.steps);
        let f = e.latest_features("DIAG").unwrap().clone();
        (f, ts)
    };
    let (f, ts) = mk(10_215_000); // body 0.115 = 1.15 × 0.10
    assert_eq!(f.range.median, Some(200_000));
    assert_eq!(f.range_expansion_15pct, Tri::Pass);
    assert_eq!(f.body_expansion_15pct, Tri::Pass);
    assert!(
        of(&ts, Family::Ignition).is_empty(),
        "1.15x alone never ignites"
    );
    let (f, _) = mk(10_214_999);
    assert_eq!(f.range_expansion_15pct, Tri::Pass);
    assert_eq!(f.body_expansion_15pct, Tri::Fail);
}

#[test]
fn zero_medians_and_empty_windows_are_unknown_not_infinite() {
    let mut s = Script::new();
    for k in 0..35 {
        s.bar("FLAT", k, [500, 500, 500, 500], 100);
    }
    s.bar("FLAT", 35, [500, 520, 500, 520], 100);
    s.advance(cutoff(36));
    let (e, ts) = run(&s.steps);
    let f = e.latest_features("FLAT").unwrap();
    assert_eq!(f.range.median, Some(0));
    assert_eq!(f.range.e_milli, None);
    assert_eq!(f.range.e_adj_milli, None, "zero raw median: E unavailable");
    assert_eq!(f.range_expansion_15pct, Tri::Unknown);
    assert_eq!(f.body_expansion_15pct, Tri::Unknown);
    assert!(of(&ts, Family::Ignition).is_empty());

    // Segment start: fewer than 10 valid slots.
    let mut s = Script::new();
    for k in 0..5 {
        s.bar("OPEN", k, [500, 501, 499, 500], 100);
    }
    s.advance(cutoff(6));
    let (e, _) = run(&s.steps);
    let f = e.latest_features("OPEN").unwrap();
    assert_eq!(f.window.context, ContextClass::Insufficient);
    assert_eq!(f.window.context_reason, "segment_start");
    assert_eq!(f.baseline.status, "sparse_baseline");
    assert_eq!(f.range.median, None);
}

#[test]
fn a_large_upper_wick_shows_its_shape_and_does_not_ignite() {
    let r = fixture::run("wick_only").unwrap();
    assert!(r.transitions.is_empty());
    // Bar 40 alone, on a run that stops at its cutoff.
    let mut s = Script::new();
    s.quiet("WCKX", 0..40, 500, 501);
    s.bar("WCKX", 40, [501, 520, 500, 502], 400);
    s.advance(cutoff(40));
    let (e, _) = run(&s.steps);
    let f = e.latest_features("WCKX").unwrap();
    assert_eq!(f.range_expansion_15pct, Tri::Pass);
    assert_eq!(f.upper_wick, 180_000);
    assert!(f.close_location_milli.unwrap() < 200);
    assert_eq!(f.return_ge_1pct, Some(false));
}

#[test]
fn a_print_after_long_silence_is_a_gap_return_not_15s_momentum() {
    let mut e = fixture::engine("t");
    let ts = fixture::drive(&mut e, &fixture::script("sparse_prints").unwrap());
    assert!(of(&ts, Family::Ignition).is_empty());
    let f = e.latest_features("SPRS").unwrap();
    assert_eq!(f.bar_end, end(70));
    assert_eq!(f.return_class, ReturnClass::GapEvent);
    assert_eq!(f.elapsed_secs, Some(15 * 14));
    assert_eq!(f.z_milli, None);
    assert_eq!(f.baseline.status, "not_adjacent");
    assert!(matches!(
        f.window.context,
        ContextClass::Sparse | ContextClass::Insufficient
    ));
}

#[test]
fn a_later_cancel_appends_nothing_to_the_past_and_changes_only_current_state() {
    let base = fixture::script("ignition_no_precursor").unwrap();
    let (_, plain) = run(&base);
    let mut with = base.clone();
    // The ignition bar's close print is fixture seq 160 (40 bars × 4 prints).
    with.insert(
        with.len() - 1,
        fixture::FixtureStep::Cancel {
            symbol: "IGNB".into(),
            seq: 164,
            price: 5_080_000,
            size: 400,
            admit: cutoff(41) + S,
        },
    );
    let (_, revised) = run(&with);
    let ig_plain = of(&plain, Family::Ignition)[0];
    let ig_rev = of(&revised, Family::Ignition)[0];
    assert_eq!(
        serde_json::to_string(ig_plain).unwrap(),
        serde_json::to_string(ig_rev).unwrap(),
        "the logged recognition is immutable"
    );
}

#[test]
fn ross_membership_and_panels_are_untouched_by_detection() {
    let steps = fixture::script("quiet_build_ignition").unwrap();
    let mut on = fixture::engine("t");
    let mut off = Engine::new(Policy::current(), on.run_info().clone());
    let ts = fixture::drive(&mut on, &steps);
    fixture::drive(&mut off, &steps);
    assert!(
        !ts.is_empty(),
        "detections exist for a nonmember with no evidence"
    );
    let a: Vec<String> = on
        .current()
        .map(|d| serde_json::to_string(d).unwrap())
        .collect();
    let b: Vec<String> = off
        .current()
        .map(|d| serde_json::to_string(d).unwrap())
        .collect();
    assert_eq!(a, b, "decisions identical with detection on or off");
    assert_eq!(on.panel(), off.panel());
    assert!(off.detection_snapshot().is_none());
}

#[test]
fn breakout_from_a_quiet_range_records_its_level_and_retention() {
    let r = fixture::run("quiet_build_ignition").unwrap();
    let bo = of(&r.transitions, Family::RangeBreakout);
    assert_eq!(bo[0].to, State::Confirmed);
    let lvl = bo[0].level.as_ref().unwrap();
    assert_eq!(
        (lvl.kind, lvl.price, lvl.version),
        ("range_high", 3_000_000, 1)
    );
    assert_eq!(bo.last().unwrap().to, State::Retained);
}

#[test]
fn identical_input_gives_identical_records_however_the_clock_is_ticked() {
    let steps = fixture::script("quiet_build_ignition").unwrap();
    let (_, a) = run(&steps);
    // Extra timer ticks between prints (a live server's heartbeat) change nothing.
    let mut ticked = Vec::new();
    for s in &steps {
        if let fixture::FixtureStep::Print { admit, .. } = s {
            ticked.push(fixture::FixtureStep::Advance(*admit - 1));
        }
        ticked.push(s.clone());
    }
    let (_, b) = run(&ticked);
    assert_eq!(
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap()
    );
}

#[test]
fn the_policy_is_experimental_hashed_and_alerts_cannot_be_enabled() {
    let p = DetectionPolicy::v0();
    assert!(p.tag().starts_with("detection-v0@"));
    assert!(
        DetectionPolicy::parse(
            &DETECTION_V0.replace("\"alerts_enabled\": false", "\"alerts_enabled\": true")
        )
        .is_err()
    );
    // A grid that is not the pillar grid disables detection with a reason instead of guessing.
    let off = DETECTION_V0.replace("\"bar_secs\": 15", "\"bar_secs\": 30");
    let mut e = Engine::new(Policy::current(), fixture::engine("t").run_info().clone())
        .with_detection_policy(DetectionPolicy::parse(&off).unwrap());
    fixture::drive(&mut e, &fixture::script("ignition_no_precursor").unwrap());
    let snap = e.detection_snapshot().unwrap();
    assert_eq!(snap.data_status.disabled_reason, Some("grid_mismatch"));
    assert!(e.take_detection_transitions().is_empty());
}
