//! Breakout correctness through production chart admission, ZoneBoard and RocketBoard APIs.
//! Prices keep their original micro units; only eligible executions strictly after the tick
//! that first made the Run row known may establish a new momentum high. Whole-bar volume,
//! historical pace, and high-event (including spike/retreat) semantics stay unchanged.

use std::sync::Arc;

use chart_engine::{ChartEngine, wire::ChartUpdate};
use market_events::{MarketEvent, Price, Trade, UnixNanos, session::et};
use serde_json::Value;
use stockwatcher_server::{
    quotes::Counted,
    rocket::{
        AlertKind, Features, Notice, Policy, RocketBoard, RocketScorer, log::RocketLog,
        track::MState,
    },
    zones::ZoneBoard,
};

const S: i64 = 1_000_000_000;
const BAR: i64 = 15 * S;

struct Silent;
impl RocketScorer for Silent {
    fn id(&self) -> &str {
        "breakout-correctness-no-score"
    }
    fn version(&self) -> &str {
        "1"
    }
    fn score(&self, _: &Features) -> Option<f64> {
        None
    }
}

#[derive(Clone)]
struct Print {
    exec: i64,
    arrival: i64,
    micros: i64,
    shares: u64,
    condition: Option<&'static str>,
}
fn print(exec: i64, arrival: i64, micros: i64, shares: u64) -> Print {
    Print {
        exec,
        arrival,
        micros,
        shares,
        condition: None,
    }
}
fn on_time(exec: i64, micros: i64, shares: u64) -> Print {
    print(exec, exec, micros, shares)
}
fn price(micros: i64) -> f64 {
    Price::from_micros(micros).to_f64()
}

struct Scenario {
    flag_micros: i64,
    preflag_high: Option<i64>,
    early: bool,
    phase: i64,
    history_shares: u64,
    mark_shares: u64,
    extra: Vec<Print>,
}
impl Default for Scenario {
    fn default() -> Self {
        Self {
            flag_micros: 1_250_000,
            preflag_high: None,
            early: true,
            phase: 0,
            history_shares: 100,
            mark_shares: 1000,
            extra: Vec::new(),
        }
    }
}

struct Admission {
    exec: i64,
    arrival: i64,
    counted: Counted,
}
struct Observed {
    board: RocketBoard,
    base: i64,
    known: i64,
    admissions: Vec<Admission>,
    // (barrier label, tick that observed/emitted it), both relative to base.
    breakouts: Vec<(i64, i64)>,
    breakout_notices: Vec<Notice>,
}

fn run(c: Scenario, until_seconds: i64) -> Observed {
    let base = et("2026-10-26".parse().unwrap(), 10, 0, 0, 0).unwrap().0;
    let mut chart = ChartEngine::default();
    let mut zones = ZoneBoard::new(true);
    zones.set_early(c.early);
    let mut board = RocketBoard::new(
        true,
        Arc::new(Silent),
        Policy {
            fast: false,
            ..Policy::default()
        },
    );
    let mut prints = Vec::new();
    // Positive, constant historical pace; enough history to keep all denominator slots real.
    for slot in -132..0 {
        prints.push(on_time(
            slot * BAR + S,
            c.flag_micros * 4 / 5,
            c.history_shares,
        ));
    }
    // Distinct identities and a genuine liquid +25% Run. Its close is the flag price even
    // when the first print is an older, higher preflag peak.
    for i in 0..25 {
        prints.push(on_time(
            100_000_000 + i * 20_000_000,
            if i == 0 {
                c.preflag_high.unwrap_or(c.flag_micros)
            } else {
                c.flag_micros
            },
            c.mark_shares,
        ));
    }
    prints.extend(c.extra);
    prints.sort_by_key(|p| p.arrival);
    let mut next = 0;
    let mut known = None;
    let mut admissions = Vec::new();
    let mut breakouts = Vec::new();
    let mut breakout_notices = Vec::new();
    for sec in -1981..=until_seconds {
        let now = sec * S + c.phase;
        while next < prints.len() && prints[next].arrival <= now {
            let p = &prints[next];
            chart.advance_clock(UnixNanos(base + p.arrival));
            let mut trade = Trade::new(
                "BRKREG",
                UnixNanos(base + p.exec),
                Price::from_micros(p.micros),
                p.shares,
                next as u64 + 1,
            )
            .with_exchange("Q");
            trade.receive_ts = Some(UnixNanos(base + p.arrival));
            if let Some(condition) = p.condition {
                trade.conditions = vec![condition.into()];
            }
            let (updates, _) = chart.apply_revising(&MarketEvent::Trade(trade.clone()));
            let Some(ChartUpdate::Tape(row)) = updates.first() else {
                panic!("each distinct print must receive the real chart's leading tape verdict");
            };
            let counted = Counted::from_tape(&row.counted);
            zones.apply_trade(&trade, counted);
            board.apply_trade(&trade, counted);
            if p.exec >= S || p.arrival >= S {
                admissions.push(Admission {
                    exec: p.exec,
                    arrival: p.arrival,
                    counted,
                });
            }
            next += 1;
        }
        chart.advance_clock(UnixNanos(base + now));
        zones.tick(base + now);
        board.add_runs(zones.take_entries());
        board.tick(base + now, |_| None);
        if !board.alerts().is_empty() && known.is_none() {
            known = Some(now);
        }
        for notice in board.take_notices() {
            if notice.kind.as_str() == "breakout" {
                breakouts.push((notice.at_ns - base, now));
                breakout_notices.push(notice);
            }
        }
    }
    assert_eq!(
        next,
        prints.len(),
        "fixture must deliver every specified print"
    );
    assert_eq!(
        board.alerts().len(),
        1,
        "exactly one real ZoneBoard Run row"
    );
    assert_eq!(board.alerts()[0].kind, AlertKind::Run);
    assert_eq!(board.alerts()[0].price, price(c.flag_micros));
    let known = known.expect("real Run must become known");
    assert_eq!(
        known,
        if c.early {
            S + c.phase
        } else {
            16 * S + c.phase
        }
    );
    Observed {
        board,
        base,
        known,
        admissions,
        breakouts,
        breakout_notices,
    }
}

fn no_breakout(o: &Observed, why: &str) {
    assert!(
        o.breakouts.is_empty(),
        "{why}: unexpected notices {:?}",
        o.breakouts
    );
    assert_ne!(
        o.board.alerts()[0].track.state,
        MState::BreakingOut,
        "{why}"
    );
}
fn accepted(o: &Observed, exec: i64, arrival: i64) {
    let a = o
        .admissions
        .iter()
        .find(|a| a.exec == exec && a.arrival == arrival)
        .unwrap();
    assert!(
        a.counted.ohlc && a.counted.volume,
        "causal control must really be admitted"
    );
}

#[test]
fn equal_decimal_early_run_is_not_a_new_high() {
    let o = run(
        Scenario {
            flag_micros: 1_830_000,
            extra: vec![on_time(2 * S, 1_830_000, 100)],
            ..Scenario::default()
        },
        16,
    );
    no_breakout(
        &o,
        "an unchanged $1.83 execution is equal in original price units",
    );
    assert_eq!(o.board.alerts()[0].track.high(), price(1_830_000));
}

#[test]
fn equal_decimal_completed_run_is_not_a_new_high() {
    let o = run(
        Scenario {
            flag_micros: 1_830_000,
            early: false,
            extra: vec![on_time(17 * S, 1_830_000, 1000)],
            ..Scenario::default()
        },
        31,
    );
    no_breakout(
        &o,
        "completed Run must compare the same original $1.83 price units",
    );
}

#[test]
fn preflag_peak_without_any_postflag_execution_cannot_break_out() {
    let o = run(
        Scenario {
            preflag_high: Some(1_350_000),
            ..Scenario::default()
        },
        16,
    );
    no_breakout(
        &o,
        "a candle completing after the flag does not make its preflag peak new",
    );
    assert_eq!(o.board.alerts()[0].track.high(), price(1_250_000));
}

#[test]
fn late_arrival_does_not_turn_preflag_execution_into_postflag_evidence() {
    let o = run(
        Scenario {
            extra: vec![print(S - 1, 5 * S, 1_350_000, 1000)],
            ..Scenario::default()
        },
        16,
    );
    accepted(&o, S - 1, 5 * S);
    no_breakout(&o, "admitted late print executed before F_known");
}

#[test]
fn execution_exactly_at_known_time_is_excluded_even_if_it_arrives_later() {
    let o = run(
        Scenario {
            extra: vec![print(S, 5 * S, 1_350_000, 1000)],
            ..Scenario::default()
        },
        16,
    );
    accepted(&o, S, 5 * S);
    no_breakout(
        &o,
        "strict execution-time boundary excludes equality to F_known",
    );
}

#[test]
fn execution_one_nanosecond_after_known_time_is_included() {
    let o = run(
        Scenario {
            extra: vec![print(S + 1, 5 * S, 1_350_000, 1000)],
            ..Scenario::default()
        },
        16,
    );
    accepted(&o, S + 1, 5 * S);
    assert_eq!(o.breakouts, vec![(BAR, BAR)]);
}

#[test]
fn a_real_postflag_spike_can_qualify_after_the_close_retreats() {
    let o = run(
        Scenario {
            extra: vec![
                on_time(5 * S, 1_350_000, 1000),
                on_time(6 * S, 1_250_000, 1000),
            ],
            ..Scenario::default()
        },
        16,
    );
    assert_eq!(o.breakouts, vec![(BAR, BAR)]);
    assert!(o.board.alerts()[0].track.high() >= price(1_350_000));
    assert_eq!(o.board.alerts()[0].track.price, price(1_250_000));
}

#[test]
fn postflag_high_below_an_older_preflag_peak_still_qualifies() {
    let o = run(
        Scenario {
            preflag_high: Some(1_500_000),
            extra: vec![
                on_time(5 * S, 1_350_000, 1000),
                on_time(6 * S, 1_250_000, 1000),
            ],
            ..Scenario::default()
        },
        16,
    );
    assert_eq!(o.breakouts, vec![(BAR, BAR)]);
    assert_eq!(
        o.board.alerts()[0].track.high(),
        price(1_350_000),
        "preflag peak cannot poison H_previous"
    );
}

#[test]
fn equal_peak_on_both_sides_retains_the_postflag_occurrence() {
    let o = run(
        Scenario {
            preflag_high: Some(1_350_000),
            extra: vec![
                on_time(5 * S, 1_350_000, 100),
                on_time(6 * S, 1_250_000, 100),
            ],
            ..Scenario::default()
        },
        16,
    );
    assert_eq!(o.breakouts, vec![(BAR, BAR)]);
    assert!(o.board.alerts()[0].track.high() >= price(1_350_000));
}

#[test]
fn unchanged_binary_exact_price_is_a_negative_control() {
    let o = run(
        Scenario {
            extra: vec![on_time(2 * S, 1_250_000, 100)],
            ..Scenario::default()
        },
        16,
    );
    no_breakout(&o, "binary-exact unchanged price");
}

#[test]
fn source_conditions_i_and_z_cannot_supply_high_evidence() {
    for condition in ["I", "Z"] {
        let mut high = on_time(5 * S, 1_500_000, 100);
        high.condition = Some(condition);
        let o = run(
            Scenario {
                extra: vec![high],
                ..Scenario::default()
            },
            16,
        );
        assert_eq!(o.admissions.len(), 1);
        assert!(!o.admissions[0].counted.ohlc && o.admissions[0].counted.volume);
        no_breakout(&o, condition);
    }
}

#[test]
fn first_partial_bar_keeps_whole_bar_volume_context() {
    let o = run(
        Scenario {
            extra: vec![on_time(5 * S, 1_350_000, 1)],
            ..Scenario::default()
        },
        16,
    );
    assert_eq!(
        o.breakouts,
        vec![(BAR, BAR)],
        "preflag volume still supplies the unchanged whole-bar numerator"
    );
}

#[test]
fn thin_full_postflag_bar_fails_but_sufficient_volume_passes() {
    for (shares, expected) in [(100, vec![]), (200, vec![(2 * BAR, 2 * BAR)])] {
        let o = run(
            Scenario {
                extra: vec![on_time(16 * S, 1_350_000, shares)],
                ..Scenario::default()
            },
            31,
        );
        assert_eq!(
            o.breakouts, expected,
            "2x unchanged 100-share historical pace; shares={shares}"
        );
    }
}

#[test]
fn zero_historical_pace_keeps_the_positive_volume_fallback() {
    for (shares, expected) in [(0, vec![]), (1, vec![(2 * BAR, 2 * BAR)])] {
        let o = run(
            Scenario {
                history_shares: 0,
                extra: vec![on_time(16 * S, 1_350_000, shares)],
                ..Scenario::default()
            },
            31,
        );
        accepted(&o, 16 * S, 16 * S);
        assert_eq!(
            o.breakouts, expected,
            "zero-base fallback needs positive whole-bar volume; shares={shares}"
        );
    }
}

#[test]
fn subsequent_one_micro_high_survives_an_f32_collision() {
    assert_eq!(price(16_000_001) as f32, price(16_000_002) as f32);
    let o = run(
        Scenario {
            flag_micros: 15_000_000,
            extra: vec![
                on_time(16 * S, 16_000_001, 1),
                on_time(32 * S, 16_000_002, 200),
            ],
            ..Scenario::default()
        },
        46,
    );
    assert_eq!(
        o.breakouts,
        vec![(3 * BAR, 3 * BAR)],
        "the second original-micro high must be recognized after the thin first high"
    );
    assert_eq!(o.board.alerts()[0].track.high(), price(16_000_002));
}

#[test]
fn unchanged_live_quote_at_2_03_is_not_a_provisional_breakout() {
    let o = run(
        Scenario {
            extra: vec![on_time(16 * S, 2_030_000, 1)],
            ..Scenario::default()
        },
        31,
    );
    no_breakout(&o, "the high arrived on insufficient volume");
    let body = o.board.body(&|_| (Some(price(2_030_000)), None), &|_| None);
    assert_eq!(body["rows"][0]["state"], "watching");
    assert_eq!(
        body["rows"][0]["provisional"], false,
        "unchanged full-precision quote equals the tracked high"
    );
}

#[test]
fn interval_edges_assign_the_high_to_its_actual_half_open_candle() {
    for (exec, barrier) in [(BAR - 1, BAR), (BAR, 2 * BAR), (BAR + 1, 2 * BAR)] {
        let o = run(
            Scenario {
                extra: vec![on_time(exec, 1_350_000, 1000)],
                ..Scenario::default()
            },
            31,
        );
        accepted(&o, exec, exec);
        assert_eq!(
            o.breakouts,
            vec![(barrier, barrier)],
            "execution offset {exec}"
        );
    }
}

#[test]
fn phase_shift_uses_actual_known_tick_and_preserves_barrier_label() {
    let phase = 250_000_000;
    let excluded = run(
        Scenario {
            phase,
            extra: vec![print(S + phase, 5 * S, 1_350_000, 1000)],
            ..Scenario::default()
        },
        16,
    );
    assert_eq!(excluded.known, S + phase);
    no_breakout(&excluded, "phase-shifted F_known equality is excluded");
    let included = run(
        Scenario {
            phase,
            extra: vec![print(S + phase + 1, 5 * S, 1_350_000, 1000)],
            ..Scenario::default()
        },
        16,
    );
    assert_eq!(included.breakouts, vec![(BAR, BAR + phase)]);
}

#[test]
fn completed_run_uses_known_tick_instead_of_its_backdated_barrier_label() {
    let before_known = run(
        Scenario {
            early: false,
            extra: vec![on_time(15 * S + S / 2, 1_350_000, 1000)],
            ..Scenario::default()
        },
        31,
    );
    assert_eq!(
        before_known.board.alerts()[0].at_ns - before_known.base,
        BAR
    );
    assert_eq!(before_known.known, 16 * S);
    accepted(&before_known, 15 * S + S / 2, 15 * S + S / 2);
    no_breakout(
        &before_known,
        "execution after flag label but before row became known",
    );
    let after_known = run(
        Scenario {
            early: false,
            extra: vec![on_time(16 * S + 1, 1_350_000, 1000)],
            ..Scenario::default()
        },
        31,
    );
    assert_eq!(after_known.breakouts, vec![(2 * BAR, 2 * BAR)]);
}

// Added after the separately archived 19-test unchanged-source baseline. These exercise
// production serialization/loading, rather than constructing a private Track snapshot.
fn load_and_restore(lines: &[String]) -> RocketBoard {
    let day = "2026-10-26".parse().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let log = RocketLog::new(tmp.path().join("rocket-watch"), 30);
    log.append(day, lines);
    let mut restored = RocketBoard::new(
        true,
        Arc::new(Silent),
        Policy {
            fast: false,
            ..Policy::default()
        },
    );
    restored.restore(day, log.load(day));
    restored
}

#[test]
fn current_entry_log_roundtrip_preserves_the_actual_known_time() {
    let mut o = run(
        Scenario {
            early: false,
            phase: 250_000_000,
            ..Scenario::default()
        },
        17,
    );
    let lines: Vec<String> = o
        .board
        .take_log()
        .into_iter()
        .map(|(_, line)| line)
        .collect();
    let entry = lines
        .iter()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|v| v["kind"] == "run")
        .unwrap();
    assert_eq!(entry["alert_ns"], o.base + BAR);
    assert_eq!(
        entry["breakout_track"]["exact"]["flag_known_ns"],
        o.base + o.known
    );
    assert_eq!(
        entry["breakout_track"]["exact"]["flag_price_micros"],
        1_250_000
    );
    let mut restored = load_and_restore(&lines);
    assert_eq!(restored.alerts().len(), 1);
    assert_eq!(restored.alerts()[0].at_ns, o.base + BAR);
    assert!(
        restored.take_notices().is_empty(),
        "restart cannot replay entry or historical breakout delivery"
    );
    let body = restored.body(&|_| (Some(price(1_250_000)), None), &|_| None);
    assert_eq!(
        body["rows"][0]["breakout_tracking"]["status"], "unknown",
        "log provenance alone cannot prove a complete outage interval"
    );
}

#[test]
fn high_without_a_state_transition_is_checkpointed_and_loaded() {
    let mut o = run(
        Scenario {
            flag_micros: 15_000_000,
            extra: vec![on_time(16 * S, 16_000_001, 1)],
            ..Scenario::default()
        },
        31,
    );
    no_breakout(
        &o,
        "thin high establishes an exact comparison reference without transitioning",
    );
    let lines: Vec<String> = o
        .board
        .take_log()
        .into_iter()
        .map(|(_, line)| line)
        .collect();
    let checkpoint = lines
        .iter()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .rfind(|v| v["kind"] == "breakout_track")
        .expect("every changed exact reference must survive a restart, including a quiet state");
    assert_eq!(
        checkpoint["breakout_track"]["exact"]["high_price_micros"],
        16_000_001
    );
    let mut restored = load_and_restore(&lines);
    assert_eq!(
        restored.alerts()[0].track.high(),
        price(16_000_001),
        "actual current log loader must retain the latest exact checkpoint"
    );
    assert!(restored.take_notices().is_empty());
    let body = restored.body(&|_| (Some(price(16_000_001)), None), &|_| None);
    assert_eq!(body["rows"][0]["provisional"], false);
    assert_eq!(body["rows"][0]["breakout_tracking"]["status"], "unknown");
}

#[test]
fn legacy_log_without_exact_provenance_stays_readable_and_unknown() {
    let mut o = run(Scenario::default(), 2);
    let mut entry = o
        .board
        .take_log()
        .into_iter()
        .map(|(_, line)| serde_json::from_str::<Value>(&line).unwrap())
        .find(|v| v["kind"] == "run")
        .unwrap();
    entry.as_object_mut().unwrap().remove("breakout_track");
    let mut restored = load_and_restore(&[entry.to_string()]);
    assert_eq!(restored.alerts().len(), 1);
    assert_eq!(restored.alerts()[0].price, price(1_250_000));
    assert!(restored.take_notices().is_empty());
    let body = restored.body(&|_| (Some(price(1_250_000)), None), &|_| None);
    assert_eq!(
        body["rows"][0]["breakout_tracking"]["status"], "unknown",
        "a legacy float or row timestamp cannot manufacture original-micro provenance"
    );
}

#[test]
fn a_newer_state_line_wins_over_an_older_exact_checkpoint() {
    let mut o = run(
        Scenario {
            extra: vec![on_time(16 * S, 1_350_000, 1)],
            ..Scenario::default()
        },
        31,
    );
    o.board.set_halted("BRKREG", true, None);
    o.board.tick(o.base + 3 * BAR, |_| None);
    assert_eq!(o.board.alerts()[0].track.state, MState::Halted);
    let since = o.board.alerts()[0].track.since_ns;
    let lines: Vec<String> = o
        .board
        .take_log()
        .into_iter()
        .map(|(_, line)| line)
        .collect();
    let mut restored = load_and_restore(&lines);
    assert_eq!(
        restored.alerts()[0].track.state,
        MState::Halted,
        "a quiet exact checkpoint must not overwrite a later state transition"
    );
    assert_eq!(restored.alerts()[0].track.since_ns, since);
    assert!(restored.take_notices().is_empty());
}

#[test]
fn actual_producer_notice_roundtrips_to_phone_desk_and_grouped_history() {
    use stockwatcher_server::{alerts::runners, notices};
    let o = run(
        Scenario {
            flag_micros: 1_830_000,
            phase: 250_000_000,
            extra: vec![
                on_time(5 * S, 2_030_000, 1000),
                on_time(6 * S, 1_830_000, 1000),
            ],
            ..Scenario::default()
        },
        16,
    );
    assert_eq!(o.breakout_notices.len(), 1);
    let n = &o.breakout_notices[0];
    let evidence = n
        .source_evidence()
        .expect("actual confirmed notice must freeze its source measurements");
    assert_eq!(evidence["reference_price_micros"], 1_830_000);
    assert_eq!(evidence["high_price_micros"], 2_030_000);
    assert_eq!(
        evidence["evaluated_ns"],
        (o.base + BAR + 250_000_000).to_string()
    );
    let (title, body, phone_data) = runners::message(n);
    let desk: Value = serde_json::from_str(&runners::desk_frame(n)).unwrap();
    assert_eq!(phone_data["breakout"], evidence);
    assert_eq!(desk["data"], phone_data);
    let decision = serde_json::json!({
        "kind": "decision", "v": 1, "event_id": n.event_id(),
        "notice": "runners.breakout", "symbol": &n.symbol,
        "decided_at": evidence["evaluated_at_ms"], "path": "desk", "decision": "sent",
        "price": n.price, "title": title, "body": body,
        "breakout": phone_data["breakout"], "entry_context": phone_data["entry_context"],
    });
    let (items, truncated) = notices::group(&decision.to_string());
    assert!(!truncated);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["breakout"], evidence);
    if let Some(path) = std::env::var_os("BREAKOUT_PRODUCER_ROUNDTRIP_PATH") {
        let artifact = serde_json::json!({
            "origin": "production ChartEngine verdict -> ZoneBoard -> RocketBoard notice",
            "event_id": n.event_id(), "title": title, "body": body,
            "phone_data": phone_data, "desk": desk,
            "decision": decision, "grouped_notice": items[0],
        });
        std::fs::write(path, serde_json::to_vec_pretty(&artifact).unwrap()).unwrap();
    }
}

fn assert_provisional_surfaces(o: &Observed, quote: i64, state: &str, provisional: bool) {
    let last = Some(price(quote));
    let body = o.board.body(&|_| (last, None), &|_| None);
    let frame: Value = serde_json::from_str(&o.board.frame(
        "rocket.snapshot",
        Some(o.base + 3 * S),
        |_| (last, None),
        |_| None,
    ))
    .unwrap();
    for surface in [&body, &frame] {
        assert_eq!(surface["rows"][0]["state"], state);
        assert_eq!(surface["rows"][0]["provisional"], provisional);
    }
    let context = o.board.symbol_context("BRKREG", last).unwrap();
    assert_eq!(
        context["state"], state,
        "symbol context must use the same source-temporal guard"
    );
}

#[test]
fn provisional_public_surfaces_require_a_genuine_postcreation_source_print() {
    let no_source = run(Scenario::default(), 3);
    assert_provisional_surfaces(&no_source, 1_350_000, "watching", false);

    // The chart accepts this execution-stamped future print before the Run is known. It
    // must never acquire a new admission identity merely because a later quote repeats it.
    let future_before_creation = run(
        Scenario {
            extra: vec![print(5 * S, 750_000_000, 1_350_000, 1000)],
            ..Scenario::default()
        },
        3,
    );
    accepted(&future_before_creation, 5 * S, 750_000_000);
    assert_provisional_surfaces(&future_before_creation, 1_350_000, "watching", false);

    for exec in [S - 1, S] {
        let excluded = run(
            Scenario {
                extra: vec![print(exec, 2 * S, 1_350_000, 1000)],
                ..Scenario::default()
            },
            3,
        );
        accepted(&excluded, exec, 2 * S);
        assert_provisional_surfaces(&excluded, 1_350_000, "watching", false);
    }

    let mut eligible = run(
        Scenario {
            extra: vec![on_time(2 * S, 1_350_000, 1000)],
            ..Scenario::default()
        },
        3,
    );
    accepted(&eligible, 2 * S, 2 * S);
    assert_provisional_surfaces(&eligible, 1_350_000, "breaking_out", true);
    assert_provisional_surfaces(&eligible, 1_400_000, "watching", false);
    assert!(
        eligible.board.take_notices().is_empty(),
        "provisional display cannot send a confirmed notice"
    );
    eligible.board.set_halted("BRKREG", true, None);
    assert_provisional_surfaces(&eligible, 1_350_000, "halted", true);
}
