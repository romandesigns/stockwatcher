//! `bull_flag_v0` (EXPERIMENTAL, setups-v0) through the engine, and the pillars-v2 price band.
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
    });
}

/// Prints making bar `k` exactly `[open, high, low, close]` (cents) with `vol` shares.
fn bar(e: &mut Engine, sym: &str, k: i64, ohlc: [i64; 4], vol: u64) {
    let e0 = end(k) - 15 * S;
    let [o, h, l, c] = ohlc.map(|x| x * 10_000);
    print(e, sym, e0 + S, o, vol / 4);
    print(e, sym, e0 + 3 * S, h, vol / 4);
    print(e, sym, e0 + 5 * S, l, vol / 4);
    print(e, sym, e0 + 13 * S, c, vol - 3 * (vol / 4));
}

/// Flat bars 0-3, the pole (bars 4-5, $5.00 → $5.30) and a two-bar flag (bars 6-7): at bar 7 the
/// flag forms with trigger $5.25 and invalidation $5.17.
fn pole_and_flag(e: &mut Engine, sym: &str) {
    pole_and_flag_with(e, sym, None);
}

/// The same, with a flat `$5.00` companion symbol printing in the same bars (prints stay in time
/// order across symbols).
fn pole_and_flag_with(e: &mut Engine, sym: &str, flat: Option<&str>) {
    const BARS: [([i64; 4], u64); 8] = [
        ([500, 500, 500, 500], 400),
        ([500, 500, 500, 500], 400),
        ([500, 500, 500, 500], 400),
        ([500, 500, 500, 500], 400),
        ([500, 510, 500, 510], 2_000),
        ([510, 530, 510, 528], 2_000),
        ([525, 525, 518, 520], 500),
        ([520, 522, 517, 519], 500),
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

/// The setup of `sym` in the latest panel at bar end `b`.
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

fn walk(e: &mut Engine) -> Vec<PanelSnapshot> {
    pole_and_flag(e, "FLAG");
    bar(e, "FLAG", 8, [519, 521, 518, 521], 400); // near
    bar(e, "FLAG", 9, [521, 527, 520, 526], 300); // over $5.25, weak volume: crossing
    bar(e, "FLAG", 10, [526, 532, 526, 531], 2_000); // closes over $5.27 with volume
    for k in 11..30 {
        bar(e, "FLAG", k, [531, 531, 531, 531], 400);
    }
    decide_through(e, 30)
}

#[test]
fn pole_flag_breakout_walks_to_triggered_with_its_levels_and_times() {
    let mut e = engine(Policy::v1());
    let panels = walk(&mut e);
    let st = |k| at(&panels, end(k), "FLAG").unwrap().setup;

    // Bar 5 completes the pole: no flag yet, so watching (the pole is shown, no levels).
    let s5 = st(5);
    assert_eq!(
        (s5.state, s5.reason),
        (SetupState::Watching, "impulse_awaiting_flag")
    );
    let pole = s5.pole.unwrap();
    assert_eq!((pole.low, pole.high), (5_000_000, 5_300_000));
    assert_eq!((pole.low_bar_end, pole.high_bar_end), (end(4), end(5)));
    assert!(s5.trigger.is_none() && s5.meter_milli.is_none());

    // Bar 7 forms the flag: trigger = highest flag high, invalidation = lowest flag low.
    let s7 = st(7);
    assert_eq!(s7.state, SetupState::Forming);
    assert_eq!(
        s7.state_since,
        end(7) + 2 * S,
        "the cutoff, not the bar end"
    );
    let trig1 = s7.trigger.clone().unwrap();
    let inv1 = s7.invalidation.clone().unwrap();
    assert_eq!((trig1.price, inv1.price), (5_250_000, 5_170_000));
    assert_eq!(
        trig1.version,
        "fixture:2026-10-16:FLAG:bull_flag_v0:1:trigger:1"
    );
    assert_eq!(trig1.reason, "flag_formed");
    assert_eq!(s7.flag_bars, 2);
    // $5.19: (5.25 − 5.19) / 5.19 = 115 bp below; meter (5.19 − 5.17) / 0.08 = 0.25.
    assert_eq!(s7.distance_to_trigger_bp, Some(115));
    assert_eq!(s7.meter_milli, Some(250));
    assert_eq!(s7.risk_to_stop_bp, Some(38));

    // Bar 8 closes at $5.21, within 1 % of $5.25.
    assert_eq!(st(8).state, SetupState::Near);

    // Bar 9 trades and closes over $5.25 on weak volume: crossing, volume FAIL.
    let s9 = st(9);
    assert_eq!(s9.state, SetupState::Crossing);
    assert_eq!(s9.crossed_version.as_deref(), Some(trig1.version.as_str()));
    let pend: Vec<_> = s9.pending.iter().map(|p| (p.condition, p.state)).collect();
    assert_eq!(
        pend,
        vec![("close_above_trigger", Tri::Pass), ("volume", Tri::Fail)]
    );
    // After the test the bar joined the flag: the trigger is revised, with a new version.
    let trig2 = s9.trigger.clone().unwrap();
    assert_eq!((trig2.price, trig2.reason), (5_270_000, "flag_high_raised"));
    assert_eq!(
        trig2.version,
        "fixture:2026-10-16:FLAG:bull_flag_v0:1:trigger:2"
    );
    assert_eq!(trig2.set_at, end(9) + 2 * S);

    // Bar 10 closes at $5.31 over $5.27 (its own $5.32 high is not in the level) with volume.
    let s10 = st(10);
    assert_eq!(s10.state, SetupState::Triggered);
    let o = s10.outcome.clone().unwrap();
    assert_eq!(
        (o.at, o.bar_end, o.price),
        (end(10) + 2 * S, end(10), 5_310_000)
    );
    assert_eq!(o.level, trig2, "keeps the level version it crossed");
    assert!(!s10.extended);

    // Held for 120 s, then the family re-arms and looks for a new impulse.
    let s17 = st(17);
    assert_eq!(s17.state, SetupState::Triggered);
    assert_eq!(s17.outcome.unwrap().level.version, trig2.version);
    let s18 = st(18);
    assert_eq!(s18.state, SetupState::Watching);
    assert!(s18.trigger.is_none() && s18.outcome.is_none());

    // The log: transitions in order with unique ids, never rewritten; every level version.
    let log = e.take_setup_log();
    let tr: Vec<_> = log
        .iter()
        .filter_map(|x| match x {
            SetupLogEntry::Transition(t) => Some((t.to, t.at)),
            _ => None,
        })
        .collect();
    let states: Vec<_> = tr.iter().map(|x| x.0).collect();
    assert_eq!(
        states,
        vec![
            SetupState::Watching,
            SetupState::Forming,
            SetupState::Near,
            SetupState::Crossing,
            SetupState::Triggered,
            SetupState::Watching
        ]
    );
    assert_eq!(tr[4].1, end(10) + 2 * S);
    let levels: Vec<_> = log
        .iter()
        .filter_map(|x| match x {
            SetupLogEntry::Level(l) => Some(l.level.version.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        levels,
        vec![
            trig1.version.clone(),
            inv1.version.clone(),
            trig2.version.clone()
        ]
    );
    let ids: std::collections::BTreeSet<_> = log
        .iter()
        .filter_map(|x| match x {
            SetupLogEntry::Transition(t) => Some(t.id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(ids.len(), 6);
}

#[test]
fn breakdown_below_the_flag_low_invalidates() {
    let mut e = engine(Policy::v1());
    pole_and_flag(&mut e, "FLAG");
    bar(&mut e, "FLAG", 8, [519, 519, 512, 514], 400);
    let panels = decide_through(&mut e, 8);
    let s = at(&panels, end(8), "FLAG").unwrap().setup;
    assert_eq!(
        (s.state, s.reason),
        (SetupState::Invalidated, "closed_below_flag_low")
    );
    let o = s.outcome.unwrap();
    assert_eq!(o.level.price, 5_170_000);
    assert_eq!(
        o.level.version,
        "fixture:2026-10-16:FLAG:bull_flag_v0:1:invalidation:1"
    );
    // Not counted as attention.
    assert_eq!(panels.last().unwrap().attention, 0);
}

#[test]
fn a_spike_through_that_closes_back_below_is_crossing_then_returns() {
    let mut e = engine(Policy::v1());
    pole_and_flag(&mut e, "FLAG");
    // Trades to $5.27 over the $5.25 trigger, closes back at $5.22 on big volume.
    bar(&mut e, "FLAG", 8, [520, 527, 519, 522], 2_000);
    bar(&mut e, "FLAG", 9, [522, 523, 520, 522], 300);
    let panels = decide_through(&mut e, 9);
    let s8 = at(&panels, end(8), "FLAG").unwrap().setup;
    assert_eq!(s8.state, SetupState::Crossing);
    let pend: Vec<_> = s8.pending.iter().map(|p| (p.condition, p.state)).collect();
    assert_eq!(
        pend,
        vec![("close_above_trigger", Tri::Fail), ("volume", Tri::Pass)]
    );
    assert!(s8.crossed_version.unwrap().ends_with(":trigger:1"));
    let s9 = at(&panels, end(9), "FLAG").unwrap().setup;
    // $5.22 against the revised $5.27 trigger (5.2173 near line): near, never triggered.
    assert_eq!(s9.state, SetupState::Near);
    assert_eq!(s9.trigger.unwrap().price, 5_270_000);
    assert!(
        panels
            .iter()
            .flat_map(|p| &p.rows)
            .all(|r| r.setup.state != SetupState::Triggered)
    );
}

#[test]
fn a_halted_member_pauses_and_resumes() {
    let mut e = engine(Policy::v1());
    pole_and_flag(&mut e, "FLAG");
    bar(&mut e, "FLAG", 8, [519, 521, 518, 521], 400);
    let mut panels = decide_through(&mut e, 8);
    assert_eq!(
        at(&panels, end(8), "FLAG").unwrap().setup.state,
        SetupState::Near
    );
    e.set_halted("FLAG", true);
    panels.extend(decide_through(&mut e, 9));
    let s9 = at(&panels, end(9), "FLAG").unwrap().setup;
    assert_eq!((s9.state, s9.reason), (SetupState::Halted, "halted"));
    // Levels are kept while paused, not invented or dropped.
    assert_eq!(s9.trigger.unwrap().price, 5_250_000);
    assert_eq!(panels.last().unwrap().attention, 0);
    e.set_halted("FLAG", false);
    bar(&mut e, "FLAG", 10, [521, 522, 520, 521], 400);
    panels.extend(decide_through(&mut e, 10));
    assert_eq!(
        at(&panels, end(10), "FLAG").unwrap().setup.state,
        SetupState::Near
    );
}

#[test]
fn missing_prints_degrade_to_watching_without_levels() {
    let mut e = engine(Policy::v1());
    pole_and_flag(&mut e, "FLAG");
    // Nothing for over a minute: the setup is dropped, never kept on old levels.
    let panels = decide_through(&mut e, 12);
    let s = at(&panels, end(12), "FLAG").unwrap().setup;
    assert_eq!(
        (s.state, s.reason),
        (SetupState::Watching, "no_recent_prints")
    );
    assert!(s.trigger.is_none() && s.invalidation.is_none() && s.meter_milli.is_none());
}

#[test]
fn attention_counts_near_crossing_and_triggered_members() {
    let mut e = engine(Policy::v1());
    pole_and_flag_with(&mut e, "FLAG", Some("FLAT"));
    bar(&mut e, "FLAG", 8, [519, 521, 518, 521], 400);
    bar(&mut e, "FLAT", 8, [500, 500, 500, 500], 400);
    let panels = decide_through(&mut e, 8);
    let p = panels.iter().rfind(|p| p.bar_end == end(8)).unwrap();
    assert_eq!(p.rows.len(), 2);
    assert_eq!(p.attention, 1);
    let flat = p.rows.iter().find(|r| r.symbol == "FLAT").unwrap();
    assert_eq!(flat.setup.state, SetupState::Watching);
    assert!(p.setup_policy.starts_with("setups-v0@"));
}

#[test]
fn rows_carry_spark_and_pillar_context() {
    let mut e = engine(Policy::v1());
    let panels = walk(&mut e);
    let r = at(&panels, end(10), "FLAG").unwrap();
    // Bars 0-10: eleven closes, oldest first.
    assert_eq!(
        r.spark,
        vec![
            5_000_000, 5_000_000, 5_000_000, 5_000_000, 5_100_000, 5_280_000, 5_200_000, 5_190_000,
            5_210_000, 5_260_000, 5_310_000
        ]
    );
    let r = at(&panels, end(29), "FLAG").unwrap();
    assert_eq!(r.spark.len(), 20);
    assert!(r.spark.iter().all(|&c| c == 5_310_000));
    assert_eq!(r.float_shares, Some(5_000_000));
    assert_eq!(r.news_first_seen, Some(t(7, 0, 1)));
    assert!(r.rvol_milli.unwrap() >= 5_000);
    assert!(r.missing.is_empty());
    assert_eq!(r.move_1m_bp, r.momentum.r_primary_bp);
    assert_eq!(r.from_high_bp, Some(18), "5.31 vs the 5.32 high");
    // Serialized snake_case for the UI.
    let j = serde_json::to_value(&r).unwrap();
    assert_eq!(j["setup"]["state"], "watching");
    assert!(j["setup"]["meter_milli"].is_null());
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
