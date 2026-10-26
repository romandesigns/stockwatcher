//! The Ross Panel's per-row pullback cue through the engine (owner decision 2026-10-21): ONE
//! pullback detector everywhere. Each member's cue is the product detection policy's
//! `micro_pullback` (detection-v2, variant MP5) — the same episodes, levels and states the
//! Detection Panel shows — and it runs for members only while the Detection Panel is off.
//! Every fixture is synthetic: one trading day, 2026-10-16, previous close 2026-10-15.

use chrono::NaiveDate;
use market_events::session::et;
use market_events::{Price, Trade, UnixNanos};
use stock_context::*;

const VOL: Verdict = Verdict {
    ohlc: true,
    volume: true,
};
const S: i64 = 1_000_000_000;

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 16).unwrap()
}
fn prev() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 15).unwrap()
}
fn t(h: u32, m: u32, s: u32) -> i64 {
    et(day(), h, m, s, 0).unwrap().nanos()
}
fn inst(sym: &str) -> InstrumentId {
    InstrumentId::symbol(sym)
}
/// End of the k-th 15 s bar after 09:30.
fn end(k: i64) -> i64 {
    t(9, 30, 0) + (k + 1) * 15 * S
}
fn cutoff(k: i64) -> i64 {
    end(k) + 2 * S
}

fn engine(policy: Policy) -> Engine {
    Engine::new(
        policy,
        RunInfo {
            run_id: "fixture".into(),
            source: "replay".into(),
            fidelity: Fidelity::SyntheticFixture,
            numerator_coverage: Coverage::Complete,
        },
    )
}

fn print(e: &mut Engine, sym: &str, exec: i64, micros: i64, size: u64) {
    e.apply(
        &Trade::new(sym, UnixNanos(exec), Price::from_micros(micros), size, 1),
        VOL,
        UnixNanos(exec + 100_000_000),
    );
}

/// Evidence for a pass on every pillar: 30 sessions of 100 shares (5x RVOL = 500 shares).
fn evidence(e: &mut Engine, sym: &str, close: i64) {
    e.add_prev_close(PrevClose {
        id: format!("close:{sym}"),
        version: 1,
        instrument: inst(sym),
        date: prev(),
        close_micros: close,
        basis: CloseBasis::SplitAdjusted,
        actions_checked_through: Some(day()),
        actions: Default::default(),
        observed_at: t(4, 0, 0),
    });
    let mut sessions = Vec::new();
    let mut d = prev();
    while sessions.len() < 30 {
        if market_events::is_trading_day(d) {
            sessions.push(SessionVolume {
                date: d,
                volume: 100,
                complete: true,
                comparable: true,
            });
        }
        d = d.pred_opt().unwrap();
    }
    e.add_daily(DailyBaseline {
        id: format!("daily:{sym}"),
        version: 1,
        instrument: inst(sym),
        sessions,
        basis: Default::default(),
        observed_at: t(4, 0, 0),
    });
    e.add_float(FloatObservation {
        id: format!("float:{sym}"),
        version: 1,
        instrument: inst(sym),
        shares: 5_000_000,
        provider: "fixture".into(),
        effective_as_of: None,
        observed_at: t(4, 0, 0),
        quality: vec![],
    });
    e.add_news(NewsItem {
        id: format!("news:{sym}"),
        version: 1,
        instrument: inst(sym),
        event_type: "other".into(),
        source: "issuer".into(),
        credible_source: true,
        relevant: true,
        substantive: true,
        authenticity: Authenticity::IssuerAnnounced,
        favorability: Favorability::Favorable,
        published_at: Some(t(7, 0, 0)),
        observed_at: t(7, 0, 1),
        tag: Tag::Present,
        content_key: "fixture".into(),
        revision_at: Some(0),
        rules: "fixture".into(),
    });
}

/// Prints making bar `k` exactly `[open, high, low, close]` (cents) with `vol` shares, at +1 s,
/// +3 s, +5 s and +13 s into the bar.
fn bar(e: &mut Engine, sym: &str, k: i64, ohlc: [i64; 4], vol: u64) {
    let e0 = end(k) - 15 * S;
    let [o, h, l, c] = ohlc.map(|x| x * 10_000);
    print(e, sym, e0 + S, o, vol / 4);
    print(e, sym, e0 + 3 * S, h, vol / 4);
    print(e, sym, e0 + 5 * S, l, vol / 4);
    print(e, sym, e0 + 13 * S, c, vol - 3 * (vol / 4));
}

/// Flat bars 0-3 at $5.00, the impulse (bars 4-5, L0 $5.00 → H0 $5.30, 6 %), and a two-bar
/// retreat (bars 6-7, low $5.20: 33 % of the impulse, inside MP5's 38.2 %). The episode forms at
/// bar 6 and arms at bar 7: trigger H0 $5.30, invalidation the pullback low $5.20.
fn impulse_and_pullback(e: &mut Engine, sym: &str, flat: Option<&str>) {
    const BARS: [([i64; 4], u64); 8] = [
        ([500, 500, 500, 500], 400),
        ([500, 500, 500, 500], 400),
        ([500, 500, 500, 500], 400),
        ([500, 500, 500, 500], 400),
        ([500, 510, 500, 510], 2_000),
        ([510, 530, 510, 528], 2_000),
        ([528, 528, 521, 522], 500),
        ([522, 524, 520, 521], 500),
    ];
    evidence(e, sym, 4_000_000);
    if let Some(f) = flat {
        evidence(e, f, 4_000_000);
    }
    for (k, (ohlc, vol)) in BARS.into_iter().enumerate() {
        bar(e, sym, k as i64, ohlc, vol);
        if let Some(f) = flat {
            bar(e, f, k as i64, [500, 500, 500, 500], 400);
        }
    }
}

/// The row of `sym` in the latest panel at bar end `b`.
fn at(panels: &[PanelSnapshot], b: i64, sym: &str) -> Option<PanelRow> {
    panels
        .iter()
        .rfind(|p| p.bar_end <= b)
        .and_then(|p| p.rows.iter().find(|r| r.symbol == sym).cloned())
}

fn decide_through(e: &mut Engine, k: i64) -> Vec<PanelSnapshot> {
    e.advance_to(UnixNanos(end(k) + 3 * S));
    e.take_panels()
}

/// Pullback → at trigger → crossing → broke trigger, then flat.
fn walk(e: &mut Engine) -> Vec<PanelSnapshot> {
    impulse_and_pullback(e, "PULL", None);
    bar(e, "PULL", 8, [521, 526, 521, 526], 400); // $5.26: within 1 % below $5.30
    // Bar 9 does not trade. Bar 10 opens at $5.32 (a post-arm print over $5.30, admitted before
    // barrier 9 is decided: crossing at that barrier) and closes at $5.33, a full post-arm bar
    // closing over the trigger: trigger observed at barrier 10.
    bar(e, "PULL", 10, [532, 534, 531, 533], 2_000);
    for k in 11..30 {
        bar(e, "PULL", k, [533, 533, 533, 533], 400);
    }
    decide_through(e, 30)
}

#[test]
fn a_member_walks_pullback_at_trigger_crossing_broke_trigger() {
    let mut e = engine(Policy::v1());
    let panels = walk(&mut e);
    let st = |k| at(&panels, end(k), "PULL").unwrap().setup;

    // Bar 5 makes the impulse; nothing has retreated yet.
    let s5 = st(5);
    assert_eq!((s5.state, s5.reason), (CueState::Watching, "no_pullback"));
    assert!(s5.trigger.is_none() && s5.episode_id.is_none());

    // Bar 6 freezes the anchors: a pullback forming, no levels until it arms.
    let s6 = st(6);
    assert_eq!(s6.state, CueState::Forming);
    assert_eq!(s6.stage, Some(State::PullbackForming));
    assert_eq!(s6.family, "micro_pullback");
    assert_eq!(s6.variant, "micro_pullback_mp5");
    assert!(s6.policy.starts_with("detection-v2@"));
    assert_eq!(
        s6.episode_id.as_deref(),
        Some("fixture:2026-10-16:PULL:micro_pullback:1")
    );
    assert_eq!((s6.impulse_bp, s6.depth_milli), (Some(600), Some(300)));
    assert!(s6.trigger.is_none() && s6.meter_milli.is_none());

    // Bar 7 arms: T = H0, I = the pullback low, both version 1, known at the cutoff.
    let s7 = st(7);
    assert_eq!(
        (s7.state, s7.stage),
        (CueState::Forming, Some(State::Armed))
    );
    assert_eq!(s7.state_since, Some(cutoff(7)));
    let trig = s7.trigger.clone().unwrap();
    let inv = s7.invalidation.clone().unwrap();
    assert_eq!((trig.price, inv.price), (5_300_000, 5_200_000));
    assert_eq!(
        trig.version,
        "fixture:2026-10-16:PULL:micro_pullback:1:trigger:v1"
    );
    assert_eq!((trig.set_at, trig.reason), (cutoff(7), "impulse_high"));
    assert_eq!(s7.pullback_bars, Some(2));
    // Judged on bar 8's opening print ($5.21, admitted before the barrier was decided):
    // (5.30 − 5.21) / 5.21 = 172 bp below; meter (5.21 − 5.20) / 0.10 = 0.1.
    assert_eq!(s7.price, Some(5_210_000));
    assert_eq!(s7.distance_to_trigger_bp, Some(172));
    assert_eq!(s7.meter_milli, Some(100));
    assert_eq!(s7.to_invalidation_bp, Some(19));
    assert_eq!(s7.near_trigger, Some(false));

    // Bar 8 closes at $5.26, within 1 % of $5.30: at trigger (the Detection Panel's geometry).
    let s8 = st(8);
    assert_eq!((s8.state, s8.stage), (CueState::Near, Some(State::Armed)));
    assert_eq!(s8.near_trigger, Some(true));

    // Crossing: provisional, a full bar still has to close above.
    let s9 = st(9);
    assert_eq!(
        (s9.state, s9.stage),
        (CueState::Crossing, Some(State::CrossingProvisional))
    );
    let pend: Vec<_> = s9.pending.iter().map(|p| (p.condition, p.state)).collect();
    assert_eq!(pend, vec![("close_above_trigger", Tri::Unknown)]);
    assert_eq!(
        s9.trigger.as_ref().unwrap(),
        &trig,
        "an armed level never moves"
    );

    // Broke trigger: bar 10 closed at $5.33 over $5.30. Descriptive, held for 120 s.
    let s10 = st(10);
    assert_eq!(
        (s10.state, s10.stage),
        (CueState::Triggered, Some(State::TriggerObserved))
    );
    let r = s10.resolution.clone().unwrap();
    assert_eq!(
        (r.reason, r.at, r.bar_end),
        (
            "full_post_arm_bar_closed_above_trigger",
            cutoff(10),
            Some(end(10))
        )
    );
    assert_eq!(s10.trigger.as_ref().unwrap(), &trig);
    assert!(!s10.extended);
    assert_eq!(st(17).state, CueState::Triggered);
    let s18 = st(18);
    assert_eq!(s18.state, CueState::Watching);
    assert!(s18.trigger.is_none() && s18.resolution.is_none());

    // The Ross-only detector's transitions are the setup log: one episode, in order.
    let log = e.take_setup_log();
    let states: Vec<_> = log.iter().map(|t| (t.to, t.observed_at)).collect();
    assert_eq!(
        states.iter().map(|x| x.0).collect::<Vec<_>>(),
        vec![
            State::PullbackForming,
            State::Armed,
            State::CrossingProvisional,
            State::TriggerObserved
        ]
    );
    assert_eq!(states[3].1, cutoff(10));
    assert!(log.iter().all(|t| t.family == Family::MicroPullback));
    let tel = e.output_telemetry();
    assert_eq!(tel.setup_entries_logged, 4);
    assert!(tel.complete());
}

#[test]
fn the_ross_cue_and_the_detection_panel_are_one_detector() {
    // The same day with the Detection Panel on: every family runs for every symbol, and the
    // Ross rows read its episodes. Rows, levels and the micro-pullback's transitions agree.
    let mut off = engine(Policy::v1());
    let mut on = engine(Policy::v1()).with_detection_policy(DetectionPolicy::current());
    let (p_off, p_on) = (walk(&mut off), walk(&mut on));
    assert_eq!(p_off.len(), p_on.len());
    for (a, b) in p_off.iter().zip(&p_on) {
        assert_eq!(a.rows, b.rows, "panel at bar end {}", a.bar_end);
        assert_eq!(a.setup_policy, b.setup_policy);
    }
    let key = |t: &DetectionTransition| {
        (
            t.episode_id.clone(),
            t.from,
            t.to,
            t.event,
            t.reason,
            t.observed_at,
            t.level.clone(),
            t.anchors.clone(),
        )
    };
    let ross: Vec<_> = off.take_setup_log().iter().map(key).collect();
    let panel: Vec<_> = on
        .take_detection_transitions()
        .iter()
        .filter(|t| t.family == Family::MicroPullback)
        .map(key)
        .collect();
    assert_eq!(ross, panel);
    assert!(
        on.take_setup_log().is_empty(),
        "with the panel on they are detection transitions"
    );
    assert_eq!(off.detection_snapshot(), None, "the Detection Panel is off");
    assert_eq!(off.detection_policy(), None);
    assert_eq!(off.setup_policy(), on.setup_policy());
}

#[test]
fn only_members_are_watched_when_the_detection_panel_is_off() {
    // OUTS prints the same impulse and pullback but has no evidence: never a member, so the
    // Ross-only detector never steps it. The Detection Panel would.
    let mut off = engine(Policy::v1());
    let mut on = engine(Policy::v1()).with_detection_policy(DetectionPolicy::current());
    for e in [&mut off, &mut on] {
        evidence(e, "PULL", 4_000_000);
        const BARS: [[i64; 4]; 8] = [
            [500, 500, 500, 500],
            [500, 500, 500, 500],
            [500, 500, 500, 500],
            [500, 500, 500, 500],
            [500, 510, 500, 510],
            [510, 530, 510, 528],
            [528, 528, 521, 522],
            [522, 524, 520, 521],
        ];
        for (k, o) in BARS.into_iter().enumerate() {
            bar(e, "PULL", k as i64, o, 2_000);
            bar(e, "OUTS", k as i64, o, 2_000);
        }
        decide_through(e, 8);
    }
    let syms = |ts: Vec<DetectionTransition>| {
        ts.into_iter()
            .filter(|t| t.family == Family::MicroPullback)
            .map(|t| t.symbol)
            .collect::<std::collections::BTreeSet<_>>()
    };
    assert_eq!(syms(off.take_setup_log()), ["PULL".to_string()].into());
    assert_eq!(
        syms(on.take_detection_transitions()),
        ["OUTS".to_string(), "PULL".to_string()].into()
    );
    let p = off.panel().unwrap();
    assert_eq!(p.rows.len(), 1, "OUTS is not on the panel");
}

#[test]
fn a_print_below_the_pullback_low_fails_the_cue() {
    let mut e = engine(Policy::v1());
    impulse_and_pullback(&mut e, "PULL", None);
    bar(&mut e, "PULL", 8, [521, 521, 518, 519], 400);
    let panels = decide_through(&mut e, 8);
    let s = at(&panels, end(8), "PULL").unwrap().setup;
    assert_eq!(
        (s.state, s.stage, s.reason),
        (
            CueState::Invalidated,
            Some(State::Invalidated),
            "print_below_invalidation"
        )
    );
    assert_eq!(s.invalidation.unwrap().price, 5_200_000);
    assert_eq!(
        panels.last().unwrap().attention,
        0,
        "a failure needs no look"
    );
}

#[test]
fn a_bar_closing_back_below_retracts_the_crossing() {
    let mut e = engine(Policy::v1());
    impulse_and_pullback(&mut e, "PULL", None);
    // Trades to $5.32 over the $5.30 trigger and closes back at $5.27: crossing, then retracted
    // when the full bar closes at or below the trigger. Never "broke trigger".
    bar(&mut e, "PULL", 8, [522, 532, 521, 527], 2_000);
    bar(&mut e, "PULL", 9, [527, 528, 526, 527], 300);
    let panels = decide_through(&mut e, 9);
    let s8 = at(&panels, end(8), "PULL").unwrap().setup;
    assert_eq!((s8.state, s8.stage), (CueState::Near, Some(State::Armed)));
    let log = e.take_setup_log();
    let ev: Vec<_> = log.iter().map(|t| t.event).collect();
    assert_eq!(
        ev,
        vec![
            "impulse_frozen_retreat_started",
            "armed",
            "crossing_provisional",
            "crossing_retracted"
        ]
    );
    assert!(
        panels
            .iter()
            .flat_map(|p| &p.rows)
            .all(|r| r.setup.state != CueState::Triggered)
    );
}

#[test]
fn a_halt_ends_the_episode_and_shows_halted() {
    let mut e = engine(Policy::v1());
    impulse_and_pullback(&mut e, "PULL", None);
    bar(&mut e, "PULL", 8, [521, 526, 521, 526], 400);
    let mut panels = decide_through(&mut e, 8);
    assert_eq!(
        at(&panels, end(8), "PULL").unwrap().setup.state,
        CueState::Near
    );
    e.set_halted("PULL", true);
    panels.extend(decide_through(&mut e, 9));
    let s9 = at(&panels, end(9), "PULL").unwrap().setup;
    assert_eq!((s9.state, s9.reason), (CueState::Halted, "halted"));
    assert!(s9.trigger.is_none(), "no level is carried through a halt");
    assert_eq!(panels.last().unwrap().attention, 0);
    e.set_halted("PULL", false);
    bar(&mut e, "PULL", 10, [526, 527, 525, 526], 400);
    panels.extend(decide_through(&mut e, 10));
    let s10 = at(&panels, end(10), "PULL").unwrap().setup;
    assert_eq!(s10.state, CueState::Watching, "the halted episode ended");
    let log = e.take_setup_log();
    assert_eq!(
        log.last().map(|t| (t.to, t.reason)),
        Some((State::Interrupted, "halted"))
    );
}

#[test]
fn no_prints_for_a_minute_ends_the_episode() {
    let mut e = engine(Policy::v1());
    impulse_and_pullback(&mut e, "PULL", None);
    // Nothing after bar 7: the price goes stale and the episode is interrupted, never kept on
    // old levels.
    let panels = decide_through(&mut e, 12);
    let s = at(&panels, end(12), "PULL").unwrap().setup;
    assert_eq!((s.state, s.reason), (CueState::Watching, "stale_price"));
    assert!(s.trigger.is_none() && s.invalidation.is_none() && s.meter_milli.is_none());
}

#[test]
fn attention_counts_at_trigger_and_crossing_members() {
    let mut e = engine(Policy::v1());
    impulse_and_pullback(&mut e, "PULL", Some("FLAT"));
    bar(&mut e, "PULL", 8, [521, 526, 521, 526], 400);
    bar(&mut e, "FLAT", 8, [500, 500, 500, 500], 400);
    let panels = decide_through(&mut e, 8);
    let p = panels.iter().rfind(|p| p.bar_end == end(8)).unwrap();
    assert_eq!(p.rows.len(), 2);
    assert_eq!(p.attention, 1);
    let flat = p.rows.iter().find(|r| r.symbol == "FLAT").unwrap();
    assert_eq!(flat.setup.state, CueState::Watching);
    assert!(p.setup_policy.starts_with("detection-v2@"));
    // A held "broke trigger" is history, not a call for attention.
    let mut e = engine(Policy::v1());
    let panels = walk(&mut e);
    let p10 = panels.iter().rfind(|p| p.bar_end == end(10)).unwrap();
    assert_eq!(p10.rows[0].setup.state, CueState::Triggered);
    assert_eq!(p10.attention, 0);
}

#[test]
fn rows_carry_spark_and_pillar_context() {
    let mut e = engine(Policy::v1());
    let panels = walk(&mut e);
    let r = at(&panels, end(8), "PULL").unwrap();
    // Bars 0-8: nine closes, oldest first.
    assert_eq!(
        r.spark,
        vec![
            5_000_000, 5_000_000, 5_000_000, 5_000_000, 5_100_000, 5_280_000, 5_220_000, 5_210_000,
            5_260_000
        ]
    );
    let r = at(&panels, end(29), "PULL").unwrap();
    assert_eq!(r.spark.len(), 20);
    assert_eq!(r.float_shares, Some(5_000_000));
    assert_eq!(r.news_first_seen, Some(t(7, 0, 1)));
    // The same story's headline class rides along, for the row's news mark.
    assert_eq!(r.news_kind.as_deref(), Some("other"));
    assert_eq!(e.news_kind("PULL"), Some("other"));
    assert!(r.rvol_milli.unwrap() >= 5_000);
    assert!(r.missing.is_empty());
    assert_eq!(r.move_1m_bp, r.momentum.r_primary_bp);
    // Serialized snake_case for the UI: the coarse state clients already decode, plus `stage`.
    let j = serde_json::to_value(&r).unwrap();
    assert_eq!(j["setup"]["state"], "watching");
    assert!(j["setup"]["stage"].is_null());
    assert!(j["setup"]["meter_milli"].is_null());
    let j9 = serde_json::to_value(at(&panels, end(9), "PULL").unwrap()).unwrap();
    assert_eq!(j9["setup"]["state"], "crossing");
    assert_eq!(j9["setup"]["stage"], "crossing_provisional");
    assert_eq!(j9["setup"]["trigger"]["price"], 5_300_000);
    assert_eq!(
        j9["setup"]["trigger"]["version"],
        "fixture:2026-10-16:PULL:micro_pullback:1:trigger:v1"
    );
}

fn replay() -> (Vec<String>, Vec<String>) {
    let mut e = engine(Policy::v1());
    let panels = walk(&mut e);
    let log = e.take_setup_log();
    (
        panels
            .iter()
            .map(|p| serde_json::to_string(p).unwrap())
            .collect(),
        log.iter()
            .map(|x| serde_json::to_string(x).unwrap())
            .collect(),
    )
}

#[test]
fn replay_is_identical() {
    let (a, b) = (replay(), replay());
    assert!(a.0.len() > 20);
    assert_eq!(a, b);
}

#[test]
fn a_42_cent_stock_passes_price_under_v2_and_fails_under_v1() {
    for (policy, want) in [(Policy::v2(), Tri::Pass), (Policy::v1(), Tri::Fail)] {
        let mut e = engine(policy);
        evidence(&mut e, "PENY", 300_000);
        print(&mut e, "PENY", t(9, 30, 1), 420_000, 1_000);
        let ds = e.advance_to(UnixNanos(t(9, 30, 18)));
        let d = ds.iter().rfind(|d| d.symbol == "PENY").unwrap();
        assert_eq!(d.pillars[0].pillar, PillarId::Price);
        assert_eq!(d.pillars[0].state, want);
        if want == Tri::Pass {
            assert_eq!(d.membership.status, Some(MemberStatus::Qualified));
            assert!(d.policy.starts_with("pillars-v2@"));
        } else {
            assert_eq!(d.membership.status, Some(MemberStatus::NotMember));
        }
    }
}

#[test]
fn a_watched_symbol_runs_the_micro_pullback_without_membership() {
    // The server's Runners rows (owner, 2026-10-22): OUTS has no evidence and is never a Ross
    // member, but once watched it gets the same micro-pullback, the same levels a member would.
    let run = |watch: bool| {
        let mut e = engine(Policy::v1());
        evidence(&mut e, "PULL", 4_000_000);
        if watch {
            e.set_watched(["OUTS".to_string()]);
        }
        const BARS: [[i64; 4]; 8] = [
            [500, 500, 500, 500],
            [500, 500, 500, 500],
            [500, 500, 500, 500],
            [500, 500, 500, 500],
            [500, 510, 500, 510],
            [510, 530, 510, 528],
            [528, 528, 521, 522],
            [522, 524, 520, 521],
        ];
        for (k, o) in BARS.into_iter().enumerate() {
            bar(&mut e, "PULL", k as i64, o, 2_000);
            bar(&mut e, "OUTS", k as i64, o, 2_000);
        }
        decide_through(&mut e, 8);
        e
    };
    let syms = |e: &mut Engine| {
        e.take_setup_log()
            .into_iter()
            .filter(|t| t.family == Family::MicroPullback)
            .map(|t| t.symbol)
            .collect::<std::collections::BTreeSet<_>>()
    };
    let mut plain = run(false);
    assert_eq!(syms(&mut plain), ["PULL".to_string()].into());
    assert_eq!(plain.watched_cue("OUTS", cutoff(8)), None, "not watched");
    let mut w = run(true);
    assert_eq!(
        syms(&mut w),
        ["OUTS".to_string(), "PULL".to_string()].into()
    );
    let (a, b) = (
        w.watched_cue("OUTS", cutoff(8)).unwrap(),
        w.watched_cue("PULL", cutoff(8)).unwrap(),
    );
    // Same bars, same detector: the same stage and levels as the member's.
    assert_eq!(a.state, b.state);
    assert_eq!(
        a.trigger.as_ref().map(|l| l.price),
        b.trigger.as_ref().map(|l| l.price)
    );
    assert_eq!(a.meter_milli, b.meter_milli);
    assert!(a.trigger.is_some());
    assert_eq!(
        w.panel().unwrap().rows.len(),
        1,
        "watching is not membership"
    );
    // Unwatched again: its episode is interrupted at the next barrier, nothing more is read.
    w.set_watched(Vec::<String>::new());
    assert_eq!(w.watched_cue("OUTS", cutoff(8)), None);
}
