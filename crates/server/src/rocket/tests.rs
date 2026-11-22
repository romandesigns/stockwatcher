use super::features::{Barrier, Tape, features, settle};
use super::*;
use crate::quotes::Counted;
use market_events::session::et;
use market_events::{Price, Trade};

const BOTH: Counted = Counted {
    ohlc: true,
    volume: true,
};
const S: i64 = 1_000_000_000;

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 16).unwrap()
}

fn at(h: u32, m: u32, s: u32) -> i64 {
    et(day(), h, m, s, 0).unwrap().0
}

fn tr(sym: &str, ts: i64, dollars: f64, size: u64) -> Trade {
    Trade::new(
        sym,
        UnixNanos(ts),
        Price::from_micros((dollars * 1e6).round() as i64),
        size,
        1,
    )
}

/// A scorer for the policy tests: a fixed score for the named symbols, nothing for the rest.
struct Fixed(Vec<(&'static str, f64)>);

impl RocketScorer for Fixed {
    fn id(&self) -> &str {
        "test"
    }
    fn version(&self) -> &str {
        "t1"
    }
    fn score(&self, f: &Features) -> Option<f64> {
        self.0
            .iter()
            .find(|(s, _)| *s == &*f.symbol)
            .map(|(_, x)| *x)
    }
}

fn board(scores: Vec<(&'static str, f64)>, policy: Policy) -> RocketBoard {
    RocketBoard::new(true, Arc::new(Fixed(scores)), policy)
}

fn fixed(t: f64, budget: u32) -> Policy {
    Policy {
        budget,
        threshold: Some(Threshold::Fixed(t)),
        ..Policy::default()
    }
}

/// One print every 15 s slot of `[from, to)` at `price`.
fn flat(sym: &str, from: i64, to: i64, price: f64, size: u64) -> Vec<Trade> {
    let mut out = Vec::new();
    let mut t = from + S;
    while t < to {
        out.push(tr(sym, t, price, size));
        t += 15 * S;
    }
    out
}

/// Plays `trades` in time order with a tick 1 s after every slot boundary in `[from, to)`, as
/// the quotes task would (it ticks every wall second; one per slot is enough here).
fn play(b: &mut RocketBoard, mut trades: Vec<Trade>, from: i64, to: i64) {
    trades.sort_by_key(|t| t.event_ts.0);
    let mut i = 0;
    let mut t = from - from.rem_euclid(15 * S) + S;
    while t < to {
        while i < trades.len() && trades[i].event_ts.0 < t {
            b.apply_trade(&trades[i], BOTH);
            i += 1;
        }
        b.tick(t, |_| None);
        t += 15 * S;
    }
}

fn features_at(tape: &mut Tape, sym: &str, slot: i64) -> Option<Features> {
    let (t0, ord) = tape.day(day());
    let b = Barrier::new(t0, ord, slot);
    let i = *tape.index.get(sym)?;
    let s = &mut tape.syms[i];
    settle(s, &b);
    features(s, &b, || Some(1.0))
}

#[test]
fn accepted_older_execution_keeps_legacy_feature_price_but_candidate_close_unknown() {
    use chart_engine::{ChartEngine, Timeframe, wire::ChartUpdate};
    use market_events::MarketEvent;

    let symbol = "LATECLOSE";
    let base = at(10, 0, 0);
    let mut chart = ChartEngine::default();
    let mut b = board(vec![(symbol, 1.0)], fixed(0.5, 10));
    let mut admit = |board: &mut RocketBoard, mut trade: Trade, arrival: i64| {
        trade.receive_ts = Some(UnixNanos(arrival));
        chart.advance_clock(UnixNanos(arrival));
        let updates = chart.apply(&MarketEvent::Trade(trade.clone()));
        let Some(ChartUpdate::Tape(row)) = updates.first() else {
            panic!("trade admission");
        };
        let counted = Counted::from_tape(&row.counted);
        assert_eq!(counted, BOTH);
        board.apply_trade(&trade, counted);
    };
    admit(&mut b, tr(symbol, base + S, 1.0, 100), base + S);
    b.tick(base + 15 * S, |_| None);
    b.take_notices();
    let mut high = tr(symbol, base + 25 * S, 1.2, 1_000);
    high.source_seq = 2;
    admit(&mut b, high, base + 25 * S);
    let mut late = tr(symbol, base + 24 * S, 1.1, 1_000);
    late.source_seq = 3;
    admit(&mut b, late, base + 25 * S + S / 2);
    b.tick(base + 30 * S, |_| None);
    let notice = b
        .take_notices()
        .into_iter()
        .find(|n| n.kind == NoticeKind::Breakout)
        .unwrap();
    let Some(NoticeDetail::Breakout(evidence)) = notice.detail else {
        panic!("source evidence");
    };
    assert_eq!(evidence.high_price_micros, 1_200_000);
    assert_eq!(evidence.high_exec_ns, (base + 25 * S).to_string());
    assert_eq!(evidence.close_price_micros, None);
    let legacy_price = f64::from(1.1f32);
    assert_eq!(notice.price.to_bits(), legacy_price.to_bits());
    let slot = (base - b.t0).div_euclid(features::BAR_NS) + 1;
    let f = features_at(&mut b.tape, symbol, slot).unwrap();
    assert_eq!(f.price.to_bits(), legacy_price.to_bits());
    assert_eq!(f.x[ix::LOGP].to_bits(), legacy_price.ln().to_bits());
    assert_eq!(
        features::exact_bar_close(b.tape.sym(symbol).unwrap(), features::key_of(b.ord, slot)),
        Some(1_100_000)
    );
    // The accepted chronological chart close differs from the preserved feature close.
    let snapshot = chart.snapshot(symbol, Timeframe::S15, None).unwrap();
    assert_eq!(snapshot.candles.last().unwrap().c, 1.2);
}

fn same(a: &Features, b: &Features) -> bool {
    a.price == b.price
        && a.x
            .iter()
            .zip(b.x.iter())
            .all(|(x, y)| (x.is_nan() && y.is_nan()) || x == y)
}

/// Features at barrier k read only slots <= k: a print in slot k+1 that is already on the tape
/// when the barrier is processed changes nothing in them (module docs, "Causality").
#[test]
fn features_are_causal() {
    let feed = |with_next: bool| {
        let mut tape = Tape::default();
        let mut t = at(9, 0, 1);
        while t < at(9, 40, 0) {
            tape.apply_trade(&tr("CAUS", t, 1.0, 200), BOTH);
            t += 15 * S;
        }
        // Slot ending 09:40:15 is the barrier; 09:40:16 is the next slot.
        tape.apply_trade(&tr("CAUS", at(9, 40, 5), 1.02, 500), BOTH);
        if with_next {
            tape.apply_trade(&tr("CAUS", at(9, 40, 16), 1.50, 90_000), BOTH);
        }
        tape
    };
    let (t0, _) = Tape::default().day(day());
    let k = (at(9, 40, 5) - t0) / features::BAR_NS;
    let a = features_at(&mut feed(false), "CAUS", k).expect("a unit barrier");
    let b = features_at(&mut feed(true), "CAUS", k).expect("a unit barrier");
    assert!(same(&a, &b), "{a:?}\n{b:?}");
    assert_eq!(a.price, 1.02f32 as f64);
    // The next barrier does see it.
    let c = features_at(&mut feed(true), "CAUS", k + 1).unwrap();
    assert!(c.price > 1.4);
}

/// `vr3`, `grind` and `ret*` by the research's formulas (setup-research rocket.rs `features`).
#[test]
fn features_follow_the_research_formulas() {
    let mut tape = Tape::default();
    let mut t = at(9, 0, 1);
    while t < at(9, 30, 0) {
        tape.apply_trade(&tr("FORM", t, 1.0, 100), BOTH);
        t += 15 * S;
    }
    for i in 0..12 {
        tape.apply_trade(
            &tr(
                "FORM",
                at(9, 30, 1) + i * 15 * S,
                1.0 + 0.01 * (i + 1) as f64,
                1000,
            ),
            BOTH,
        );
    }
    let (t0, _) = Tape::default().day(day());
    let k = (at(9, 32, 50) - t0) / features::BAR_NS;
    let f = features_at(&mut tape, "FORM", k).unwrap();
    // Recent 3 min: 12,000 shares = 4,000/min; prior 30 min (slots k-131..k-12): 120 x 100 =
    // 400/min.
    let vr3 = (4001.0f64 / 401.0).ln();
    assert!((f.x[ix::VR3] - vr3).abs() < 1e-9, "{}", f.x[ix::VR3]);
    assert_eq!(f.x[ix::GRIND], 12.0);
    assert_eq!(f.x[ix::UPC3], 1.0);
    assert!((f.x[ix::RET3] - (1.12f32 as f64 / 1.0).ln()).abs() < 1e-6);
    assert!(f.x[ix::IDLE30].abs() < 1e-12, "a print every slot");
    assert_eq!(f.x[ix::SESS], 1.0, "regular session");
    assert!(f.x[ix::PDH].is_nan() && f.x[ix::RVOL].is_nan() && f.x[ix::N15].is_nan());
    let reasons = standard_reasons(&f);
    assert!(
        reasons.iter().any(|r| r.starts_with("volume 10.0×")),
        "{reasons:?}"
    );
    assert!(
        reasons.iter().any(|r| r == "grinding: 12 higher closes"),
        "{reasons:?}"
    );
    // The placeholder: vr3, gated on up vs 30 minutes ago.
    let sc = PlaceholderB2.score(&f).unwrap();
    assert!((sc - vr3).abs() < 1e-9);
}

#[test]
fn outside_the_band_or_idle_is_not_a_unit() {
    let mut tape = Tape::default();
    tape.apply_trade(&tr("PRCY", at(9, 0, 1), 26.0, 100), BOTH);
    tape.apply_trade(&tr("IDLE", at(8, 0, 1), 1.0, 100), BOTH);
    let (t0, _) = Tape::default().day(day());
    let k = (at(9, 0, 1) - t0) / features::BAR_NS;
    assert!(features_at(&mut tape, "PRCY", k).is_none());
    assert!(
        features_at(&mut tape, "IDLE", k).is_none(),
        "no print in 30 min"
    );
}

#[test]
fn precursor_scoring_starts_at_ten_cents_and_keeps_earlier_sub_dime_history() {
    let mut tape = Tape::default();
    for (sym, price) in [
        ("LOW", 0.0999),
        ("TEN", 0.10),
        ("MID", 0.12),
        ("OLD", 0.1499),
        ("TOP", 25.0),
        ("OVER", 25.0001),
    ] {
        tape.apply_trade(&tr(sym, at(9, 0, 1), price, 100), BOTH);
    }
    let (t0, _) = tape.day(day());
    let k = (at(9, 0, 1) - t0) / features::BAR_NS;
    for sym in ["TEN", "MID", "OLD", "TOP"] {
        assert!(
            features_at(&mut tape, sym, k).is_some(),
            "{sym} is eligible"
        );
    }
    for sym in ["LOW", "OVER"] {
        assert!(features_at(&mut tape, sym, k).is_none(), "{sym} is outside");
    }

    // The first in-band score must still see the earlier sub-dime move.
    tape.apply_trade(&tr("CROSS", at(9, 0, 1), 0.08, 100), BOTH);
    assert!(features_at(&mut tape, "CROSS", k).is_none());
    tape.apply_trade(&tr("CROSS", at(9, 0, 16), 0.10, 100), BOTH);
    let f = features_at(&mut tape, "CROSS", k + 1).unwrap();
    assert!((f.x[ix::RANGE60] - (0.10 / 0.08 - 1.0)).abs() < 1e-6);
}

/// The budget is a cap; at most one alert per symbol per 30 minutes.
#[test]
fn budget_caps_and_one_alert_per_symbol_per_30_minutes() {
    let syms = ["AAA", "BBB", "CCC", "DDD", "EEE"];
    let mut b = board(
        syms.iter().map(|s| (*s, 1.0)).collect(),
        Policy {
            budget: 3,
            threshold: Some(Threshold::Budget),
            ..Policy::default()
        },
    );
    let trades = syms
        .iter()
        .flat_map(|s| flat(s, at(9, 0, 0), at(10, 0, 0), 2.0, 100))
        .collect();
    play(&mut b, trades, at(9, 0, 0), at(10, 0, 0));
    assert_eq!(b.alerts().len(), 3, "the budget caps the day");

    let mut b = board(vec![("AAA", 1.0)], fixed(0.5, 10));
    play(
        &mut b,
        flat("AAA", at(9, 30, 0), at(10, 40, 0), 2.0, 100),
        at(9, 30, 0),
        at(10, 40, 0),
    );
    let times: Vec<i64> = b.alerts().iter().map(|a| a.at_ns).collect();
    assert_eq!(times.len(), 3, "09:30:15, 10:00:15, 10:30:15: {times:?}");
    assert!(times.windows(2).all(|w| w[1] - w[0] >= DEDUP_NS));
    let lines = b.take_log();
    assert_eq!(
        lines
            .iter()
            .filter(|(_, l)| l.contains("\"kind\":\"alert\""))
            .count(),
        3
    );
}

/// The adaptive threshold: no prior means no alert in the first hour; then the day's own block
/// maxima; never below the floor.
#[test]
fn the_adaptive_threshold_waits_for_data_without_a_prior() {
    let mut b = board(
        vec![("AAA", 1.0)],
        Policy {
            budget: 10,
            threshold: Some(Threshold::Budget),
            ..Policy::default()
        },
    );
    let trades = flat("AAA", at(4, 0, 0), at(5, 30, 0), 2.0, 100);
    play(&mut b, trades.clone(), at(4, 0, 0), at(4, 59, 0));
    assert!(b.alerts().is_empty(), "first hour, no prior");
    play(&mut b, trades, at(4, 59, 0), at(5, 1, 0));
    assert_eq!(b.alerts().len(), 1);
    assert_eq!(
        b.alerts()[0].at_ns,
        at(5, 0, 15),
        "slot 240, the first after 05:00"
    );
}

/// Outcomes are filled after the fact from the bars that complete after the alert: a +40 % leg
/// starting ~2 minutes after the alert is a hit; a flat symbol fades after 45 minutes. Both go to
/// the forward log, and the log restores them.
#[test]
fn outcomes_fill_after_the_fact_and_the_forward_log_restores_them() {
    // Scores only from 10:00 (the alert time), so the alerts are at 10:00:00.
    let mut b = board(vec![("ROCK", 1.0), ("FADE", 1.0)], fixed(0.5, 10));
    let mut trades = flat("ROCK", at(9, 30, 0), at(10, 2, 15), 1.0, 2000);
    trades.extend(flat("FADE", at(9, 30, 0), at(11, 0, 0), 3.0, 2000));
    let (before, after): (Vec<Trade>, Vec<Trade>) = trades
        .into_iter()
        .partition(|t| t.event_ts.0 < at(10, 0, 0));
    for t in &before {
        b.apply_trade(t, BOTH);
    }
    b.tick(at(10, 0, 1), |_| None);
    assert_eq!(b.alerts().len(), 2);
    let alert_ns = at(10, 0, 0);
    assert!(b.alerts().iter().all(|a| a.at_ns == alert_ns));
    assert!(b.alerts().iter().all(|a| a.state() == "pending"));
    // The leg: 10:02:15 → 10:06:15, +2.5 % a slot, 5 prints of 1,000 a slot.
    let mut trades = after;
    let mut p = 1.0;
    let mut t = at(10, 2, 16);
    while t < at(10, 6, 15) {
        p *= 1.025;
        for j in 0..5 {
            trades.push(tr("ROCK", t + j * S, p, 1000));
        }
        t += 15 * S;
    }
    trades.extend(flat("ROCK", at(10, 6, 15), at(11, 0, 0), p, 500));
    play(&mut b, trades, at(10, 0, 15), at(10, 50, 0));
    let rock = b.alerts().iter().find(|a| &*a.symbol == "ROCK").unwrap();
    assert_eq!(rock.state(), "rocketed", "{rock:?}");
    let m = rock.outcome.mark.unwrap();
    assert_eq!(
        m.start_ns - alert_ns,
        135 * S,
        "the last flat bar before the leg"
    );
    assert!(rock.max_gain_pct().unwrap() > 40.0);
    assert!(rock.outcome.settled);
    let fade = b.alerts().iter().find(|a| &*a.symbol == "FADE").unwrap();
    assert_eq!(fade.state(), "faded");
    assert_eq!(fade.max_gain_pct(), Some(0.0));

    let lines = b.take_log();
    let outcome = lines
        .iter()
        .map(|(_, l)| serde_json::from_str::<Value>(l).unwrap())
        .find(|v| v["kind"] == "outcome" && v["symbol"] == "ROCK")
        .unwrap();
    assert_eq!(outcome["rocket10"], true);
    assert_eq!(outcome["rocket30"], true);
    assert_eq!(outcome["partial"], false);

    let tmp = tempfile::tempdir().unwrap();
    let log = RocketLog::new(tmp.path().join("rocket-watch"), 30);
    let yesterday = NaiveDate::from_ymd_opt(2026, 10, 15).unwrap();
    log.append(
        yesterday,
        &[
            r#"{"kind":"alert","v":1,"id":"y1","symbol":"OLD","alert_ns":1}"#.to_string(),
            r#"{"kind":"outcome","v":1,"id":"y1","rocket10":true,"rocket30":true}"#.to_string(),
            r#"{"kind":"day","v":1,"strategy":"test","version":"t1","ideal_threshold":0.75}"#
                .to_string(),
        ],
    );
    log.append(
        NaiveDate::from_ymd_opt(2026, 1, 2).unwrap(),
        &["{}".to_string()],
    );
    let today: Vec<String> = lines.into_iter().map(|(_, l)| l).collect();
    log.append(day(), &today);
    assert_eq!(
        log.prune(day()),
        vec![NaiveDate::from_ymd_opt(2026, 1, 2).unwrap()]
    );
    let loaded = log.load(day());
    assert_eq!(
        loaded.today.len(),
        4,
        "10:00 and again at 10:30, after the dedup window"
    );
    assert_eq!(
        loaded.days,
        vec![DaySummary {
            date: yesterday,
            alerts: 1,
            hits10: 1,
            hits30: 1
        }]
    );
    let mut r = board(vec![], fixed(0.5, 10));
    r.restore(day(), loaded);
    let again = r.alerts().iter().find(|a| &*a.symbol == "ROCK").unwrap();
    assert_eq!(again.state(), "rocketed");
    assert_eq!(r.alerts().len(), 4);
    assert_eq!(r.prior(), Some(0.75));
    // The restored dedup holds: no second ROCK alert inside 30 minutes of the first.
    assert!(r.take_log().is_empty(), "a restore writes nothing");
    assert!(
        r.take_notices().is_empty(),
        "a restart re-announces nothing"
    );
}

#[test]
fn the_panel_frame() {
    let mut b = board(vec![("ROCK", 1.0)], fixed(0.5, 10));
    play(
        &mut b,
        flat("ROCK", at(9, 30, 0), at(10, 0, 0), 1.0, 2000),
        at(9, 59, 50),
        at(10, 0, 5),
    );
    let levels = json!({"state": "forming", "trigger": 1_050_000, "invalidation": 950_000, "meter_milli": 500});
    // A provisional high needs an eligible post-creation source print, not an injected quote.
    b.apply_trade(&tr("ROCK", at(10, 0, 2), 1.1, 100), BOTH);
    let f: Value = serde_json::from_str(&b.frame(
        "rocket.snapshot",
        Some(5),
        |_| (Some(1.1), Some(1.0)),
        |s| (s == "ROCK").then(|| levels.clone()),
    ))
    .unwrap();
    assert_eq!(f["type"], "rocket.snapshot");
    assert_eq!(f["t"], 5);
    assert_eq!(f["enabled"], true);
    assert_eq!(f["strategy"]["id"], "test");
    assert_eq!(f["mode"], "fixed");
    assert_eq!(f["today"]["alerts"], 1);
    let row = &f["rows"][0];
    assert_eq!(row["s"], "ROCK");
    assert_eq!(row["at"], at(9, 59, 45) / 1_000_000);
    assert_eq!(row["last"], 1.1);
    assert_eq!(row["pc"], 1.0);
    assert_eq!(row["outcome"]["state"], "pending");
    // The additive momentum and later-move fields.
    assert_eq!(row["outcome"]["rocket_later"], false);
    assert!(row["outcome"].get("gain_2h").is_some());
    assert_eq!(f["today"]["ran_later"], 0);
    // The live price is above the high since the flag: a provisional breakout.
    assert_eq!(row["state"], "breaking_out");
    assert_eq!(row["provisional"], true);
    assert_eq!(row["state_since"], at(9, 59, 45) / 1_000_000);
    assert_eq!(row["from_flag_pct"], 10.0);
    assert_eq!(row["from_high_pct"], 0.0);
    assert_eq!(row["move_1m_pct"], 10.0);
    assert_eq!(row["pullback_depth_pct"], Value::Null);
    assert_eq!(row["levels"], levels);
    let off: Value = serde_json::from_str(&disabled_frame("rocket.snapshot")).unwrap();
    assert_eq!(off["enabled"], false);
    assert_eq!(off["rows"], json!([]));
    // A disabled board takes nothing.
    let mut d = RocketBoard::disabled();
    d.apply_trade(&tr("ROCK", at(9, 30, 1), 1.0, 100), BOTH);
    assert!(!d.tick(at(10, 0, 1), |_| None));
    assert!(d.tape.syms.is_empty());
}

#[test]
fn the_policy_from_the_environment() {
    let p = Policy::from_lookup(|_| None).unwrap();
    assert_eq!(p, Policy::default());
    let p = Policy::from_lookup(|k| match k {
        "ROCKET_ALERTS_PER_DAY" => Some("5".into()),
        "ROCKET_THRESHOLD" => Some("1.25".into()),
        _ => None,
    })
    .unwrap();
    assert_eq!(p.budget, 5);
    assert_eq!(p.threshold, Some(Threshold::Fixed(1.25)));
    let p = Policy::from_lookup(|k| (k == "ROCKET_THRESHOLD").then(|| "budget".into())).unwrap();
    assert_eq!(p.threshold, Some(Threshold::Budget));
    assert!(Policy::from_lookup(|k| (k == "ROCKET_ALERTS_PER_DAY").then(|| "0".into())).is_err());
    assert!(Policy::from_lookup(|k| (k == "ROCKET_THRESHOLD").then(|| "x".into())).is_err());
    // RUNNERS_SCORER picks the strategy; r0 until the judge confirms R2.
    assert_eq!(p.scorer, ScorerChoice::R0);
    for (v, want, id) in [
        ("r0", ScorerChoice::R0, "precursors-R0"),
        ("R2", ScorerChoice::R2, "precursors-R2"),
        ("placeholder", ScorerChoice::Placeholder, "placeholder-b2"),
    ] {
        let p = Policy::from_lookup(|k| (k == "RUNNERS_SCORER").then(|| v.into())).unwrap();
        assert_eq!(p.scorer, want);
        assert_eq!(production_scorer(p.scorer).id(), id);
    }
    assert!(Policy::from_lookup(|k| (k == "RUNNERS_SCORER").then(|| "r9".into())).is_err());
    // R2 runs at its frozen B10 threshold.
    let b = RocketBoard::new(
        true,
        Arc::from(production_scorer(ScorerChoice::R2)),
        Policy::default(),
    );
    assert_eq!(b.mode(), Threshold::Fixed(r2::model().theta_b10));
    // Without an override the placeholder has no frozen threshold: budget mode.
    let b = RocketBoard::new(true, Arc::new(PlaceholderB2), Policy::default());
    assert_eq!(b.mode(), Threshold::Budget);
}

/// The cost on ~7k in-band symbols with a print in every slot (worst case: every one a unit,
/// every ring full). Prints the measurement; the bounds are loose so a debug build passes.
#[test]
fn cost_per_trade_and_per_barrier() {
    const N: usize = 7000;
    const SLOTS: i64 = 260;
    let names: Vec<String> = (0..N).map(|i| format!("S{i:04}")).collect();
    let mut b = RocketBoard::new(true, Arc::new(PlaceholderB2), Policy::default());
    let start = at(9, 0, 0);
    let trades: Vec<Trade> = (0..SLOTS)
        .flat_map(|k| {
            names.iter().enumerate().map(move |(i, s)| {
                let p = 1.0 + (i % 100) as f64 * 0.1 + (k % 7) as f64 * 0.001;
                tr(s, start + k * 15 * S + S, p, 100 + (k as u64 % 5) * 10)
            })
        })
        .collect();
    let t = Instant::now();
    for x in &trades {
        b.apply_trade(x, BOTH);
    }
    let per_trade = t.elapsed() / trades.len() as u32;
    let now = start + SLOTS * 15 * S;
    let t = Instant::now();
    b.tick(now, |_| Some(1.0));
    let per_barrier = t.elapsed();
    let c = b.last_cost().unwrap();
    eprintln!(
        "rocket cost: {} prints, {per_trade:?}/print; barrier over {} symbols ({} scored): {per_barrier:?}",
        trades.len(),
        c.symbols,
        c.scored
    );
    assert_eq!(c.symbols, N);
    assert!(per_trade < Duration::from_micros(50), "{per_trade:?}");
    assert!(per_barrier < Duration::from_secs(2), "{per_barrier:?}");
}

/// R2's share of a barrier: the LightGBM walk for every unit symbol, on ~7k symbols' real
/// feature vectors. The bound is loose for a debug build; the release figure is printed.
#[test]
fn r2_cost_per_barrier() {
    const N: usize = 7000;
    const SLOTS: i64 = 260;
    let names: Vec<String> = (0..N).map(|i| format!("S{i:04}")).collect();
    let mut b = RocketBoard::new(
        true,
        Arc::from(production_scorer(ScorerChoice::R2)),
        Policy::default(),
    );
    let start = at(9, 0, 0);
    let trades: Vec<Trade> = (0..SLOTS)
        .flat_map(|k| {
            names.iter().enumerate().map(move |(i, s)| {
                let p = 1.0 + (i % 100) as f64 * 0.1 + ((k * (i as i64 % 13)) % 7) as f64 * 0.01;
                tr(s, start + k * 15 * S + S, p, 100 + (k as u64 % 5) * 10)
            })
        })
        .collect();
    for x in &trades {
        b.apply_trade(x, BOTH);
    }
    let now = start + SLOTS * 15 * S;
    let t = Instant::now();
    b.tick(now, |_| Some(1.0));
    let per_barrier = t.elapsed();
    let c = b.last_cost().unwrap();
    // The model alone on the same vectors.
    let mut tape = std::mem::take(&mut b.tape);
    let bar = Barrier::new(b.t0, b.ord, (now - b.t0).div_euclid(15 * S) - 1);
    let xs: Vec<[f64; NF]> = tape
        .syms
        .iter_mut()
        .filter_map(|s| {
            settle(s, &bar);
            features(s, &bar, || Some(1.0)).map(|f| f.x)
        })
        .collect();
    let m = r2::model();
    let t = Instant::now();
    let mut acc = 0.0;
    for x in &xs {
        acc += m.raw(x);
    }
    let model_only = t.elapsed();
    eprintln!(
        "r2 cost: barrier over {} symbols ({} scored): {per_barrier:?}; model alone on {}: {model_only:?} (sum {acc:.3})",
        c.symbols,
        c.scored,
        xs.len()
    );
    assert_eq!(c.scored, N, "R2 scores every unit symbol");
    assert!(per_barrier < Duration::from_secs(2), "{per_barrier:?}");
}

/// Scores `symbol` only at the barrier ending at `at_ns` (one alert, at a known time).
struct Once(&'static str, i64);

impl RocketScorer for Once {
    fn id(&self) -> &str {
        "once"
    }
    fn version(&self) -> &str {
        "t1"
    }
    fn score(&self, f: &Features) -> Option<f64> {
        (&*f.symbol == self.0 && f.at_ns == self.1).then_some(1.0)
    }
}

/// `n` slots of a +`step` compounding climb from `from`, one print of `size` a slot.
fn climb(sym: &str, from: i64, n: i64, p0: f64, step: f64, size: u64) -> (Vec<Trade>, f64) {
    let mut p = p0;
    let mut out = Vec::new();
    for k in 0..n {
        p *= 1.0 + step;
        out.push(tr(sym, from + k * 15 * S + S, p, size));
    }
    (out, p)
}

/// The SHFS shape the owner saw (2026-10-22): flagged, nothing for the strict 45 minutes (the
/// strict tag says faded), then a climb to +43 % within the hour after. The later-move outcome
/// catches it, the momentum states walk with it, both reach the log, and a restart restores them.
#[test]
fn a_later_run_is_caught_and_states_are_logged_and_restored() {
    let alert_ns = at(10, 0, 15);
    let mut b = RocketBoard::new(true, Arc::new(Once("SLOW", alert_ns)), fixed(0.5, 10));
    // A quiet base from 09:00 at $0.21 (the alert at 10:00:15).
    play(
        &mut b,
        flat("SLOW", at(9, 0, 0), at(10, 50, 0), 0.21, 20_000),
        at(9, 0, 0),
        at(10, 50, 0),
    );
    assert_eq!(b.alerts().len(), 1);
    let a = &b.alerts()[0];
    assert_eq!(a.at_ns, alert_ns);
    assert_eq!(a.state(), "faded", "strict: nothing in 45 minutes");
    assert!(!a.ran_later());
    assert_eq!(a.track.state, MState::Watching);
    let a_id = a.id.clone();
    // The flag is one "new" notice for the phone push (crate::alerts::runners).
    let n = b.take_notices();
    assert_eq!(n.len(), 1, "{n:?}");
    assert_eq!(n[0].kind, NoticeKind::New);
    assert_eq!(n[0].symbol, "SLOW");
    assert_eq!(n[0].alert_id, a_id);
    assert_eq!(n[0].at_ns, alert_ns);
    assert!(b.take_notices().is_empty(), "taken once");
    // 10:50–11:00: +0.91 % a slot on 10× volume (new highs, past 2× pace), ~+44 %.
    let (mut run, top) = climb("SLOW", at(10, 50, 0), 40, 0.21, 0.0091, 200_000);
    run.extend(flat("SLOW", at(11, 0, 0), at(11, 30, 0), top, 20_000));
    play(&mut b, run, at(10, 50, 0), at(11, 30, 0));
    let a = b.alerts()[0].clone();
    let g = a.max_gain_2h_pct().unwrap();
    assert!(g > 40.0, "{g}");
    assert!(a.ran_later());
    assert_eq!(a.state(), "faded", "the strict tag never changes");
    assert!(a.track.later.is_some(), "a +30 % mark later that day");
    let lines: Vec<Value> = b
        .take_log()
        .into_iter()
        .map(|(_, l)| serde_json::from_str(&l).unwrap())
        .collect();
    let states: Vec<&str> = lines
        .iter()
        .filter(|v| v["kind"] == "state")
        .map(|v| v["to"].as_str().unwrap())
        .collect();
    assert!(states.contains(&"breaking_out"), "{states:?}");
    // Each turn into breaking_out is one "breakout" notice, with its % from the flag.
    let notices = b.take_notices();
    let outs = states.iter().filter(|s| **s == "breaking_out").count();
    assert_eq!(notices.len(), outs, "{notices:?}");
    assert!(notices.iter().all(|n| n.kind == NoticeKind::Breakout));
    assert!(notices[0].from_flag_pct.unwrap() > 0.0, "{notices:?}");
    assert!(notices[0].at_ns > at(10, 50, 0));
    assert!(states.contains(&"running"), "{states:?}");
    // Causal: every transition at a barrier after the flag, at or after the run began.
    for v in lines.iter().filter(|v| v["kind"] == "state") {
        assert!(v["at_ns"].as_i64().unwrap() > at(10, 50, 0), "{v}");
    }
    let body = b.body(&|_| (Some(top), Some(0.2)), &|_| None);
    assert_eq!(body["today"]["ran_later"], 1);
    let row = &body["rows"][0];
    assert!(row["outcome"]["gain_2h"].as_f64().unwrap() > 40.0);
    assert_eq!(row["outcome"]["rocket_later"], true);

    // The 2-hour line once 12:00:15 passed, the final one at 20:00.
    b.tick(at(12, 1, 0), |_| None);
    b.tick(at(20, 0, 1), |_| None);
    let later: Vec<Value> = b
        .take_log()
        .into_iter()
        .map(|(_, l)| serde_json::from_str::<Value>(&l).unwrap())
        .filter(|v| v["kind"] == "later")
        .collect();
    assert_eq!(later.len(), 2, "{later:?}");
    assert_eq!(later[0]["final"], false);
    assert_eq!(later[0]["complete_2h"], true);
    assert_eq!(later[1]["final"], true);
    assert_eq!(later[1]["rocket_later"], true);
    assert!(later[1]["max_gain_session_pct"].as_f64().unwrap() > 40.0);

    // A restart mid-day restores the later outcome and the state from the log.
    let tmp = tempfile::tempdir().unwrap();
    let log = RocketLog::new(tmp.path().join("rocket-watch"), 30);
    let all: Vec<String> = lines
        .iter()
        .chain(later.iter().take(1))
        .map(|v| v.to_string())
        .collect();
    log.append(day(), &all);
    let mut r = board(vec![], fixed(0.5, 10));
    r.restore(day(), log.load(day()));
    let a2 = &r.alerts()[0];
    assert!(a2.track.resumed);
    assert!(a2.ran_later());
    assert!(a2.track.later.is_some());
    assert_eq!(a2.track.state, a.track.state);
    assert_eq!(a2.track.since_ns, a.track.since_ns);
    assert!(r.take_log().is_empty(), "a restore writes nothing");
}

/// A halt from the chart engine shows at once and is a logged transition at the next barrier.
#[test]
fn a_halt_is_a_state() {
    let mut b = board(vec![("HALT", 1.0)], fixed(0.5, 10));
    play(
        &mut b,
        flat("HALT", at(9, 30, 0), at(10, 0, 0), 2.0, 2000),
        at(9, 59, 50),
        at(10, 0, 5),
    );
    b.take_log();
    b.set_halted("HALT", true, None);
    let body = b.body(&|_| (Some(2.0), None), &|_| None);
    assert_eq!(body["rows"][0]["state"], "halted");
    b.tick(at(10, 0, 31), |_| None);
    assert_eq!(b.alerts()[0].track.state, MState::Halted);
    let log: Vec<String> = b.take_log().into_iter().map(|(_, l)| l).collect();
    assert!(
        log.iter().any(|l| l.contains(r#""to":"halted""#)),
        "{log:?}"
    );
    b.set_halted("HALT", false, Some(at(10, 0, 31)));
    b.tick(at(10, 0, 46), |_| None);
    assert_ne!(b.alerts()[0].track.state, MState::Halted);
}

/// A busy runner logs its states all day (owner, 2026-10-22: SHFS and POM hit the old cap of
/// 60 state lines by about 12:15 ET and logged nothing while SHFS ran on). 150 halts and resumes
/// (immediate both ways, one a barrier) are at least 300 transitions (a resume can settle into
/// another state a barrier later), every one of them logged.
#[test]
fn a_busy_row_logs_every_transition_past_the_old_cap() {
    let mut b = board(vec![("BUSY", 1.0)], fixed(0.5, 10));
    play(
        &mut b,
        flat("BUSY", at(9, 30, 0), at(10, 0, 0), 2.0, 2000),
        at(9, 59, 50),
        at(10, 0, 5),
    );
    b.take_log();
    let mut t = at(10, 0, 16);
    for _ in 0..150 {
        for halted in [true, false] {
            b.set_halted("BUSY", halted, (!halted).then_some(t));
            b.tick(t, |_| None);
            t += 15 * S;
        }
    }
    let states = b
        .take_log()
        .into_iter()
        .filter(|(_, l)| l.contains(r#""kind":"state""#))
        .count();
    assert!(states >= 300, "{states}");
    // Fixed scores re-flag the symbol after each dedup window: every row's transitions count.
    let made: u32 = b.alerts().iter().map(|a| a.track.transitions).sum();
    assert!(b.alerts()[0].track.transitions > 60);
    assert_eq!(states as u32, made, "none unlogged");
}

/// The momentum states' and later outcomes' share of a barrier: 50 flagged rows (a busy day)
/// among 7,000 active symbols, each followed every barrier. Prints the measurement.
#[test]
fn momentum_cost_for_fifty_flagged_rows() {
    const N: usize = 7000;
    const SLOTS: i64 = 260;
    let names: Vec<&'static str> = (0..N)
        .map(|i| &*Box::leak(format!("S{i:04}").into_boxed_str()))
        .collect();
    let flagged: Vec<(&'static str, f64)> = names.iter().take(50).map(|s| (*s, 1.0)).collect();
    let mut b = board(flagged, fixed(0.5, 50));
    let start = at(9, 0, 0);
    let trades: Vec<Trade> = (0..SLOTS)
        .flat_map(|k| {
            names.iter().enumerate().map(move |(i, s)| {
                let p = 1.0 + (i % 100) as f64 * 0.1 + (k % 7) as f64 * 0.001;
                tr(s, start + k * 15 * S + S, p, 100 + (k as u64 % 5) * 10)
            })
        })
        .collect();
    let mut i = 0;
    // Flag at the first barrier, then feed the rest and time one barrier.
    let first = start + 15 * S + S;
    while i < trades.len() && trades[i].event_ts.0 < first {
        b.apply_trade(&trades[i], BOTH);
        i += 1;
    }
    b.tick(first, |_| Some(1.0));
    assert_eq!(b.alerts().len(), 50);
    for x in &trades[i..] {
        b.apply_trade(x, BOTH);
    }
    let now = start + SLOTS * 15 * S;
    let t = Instant::now();
    b.tick(now, |_| Some(1.0));
    let with = t.elapsed();
    let mut plain = board(vec![], fixed(0.5, 50));
    for x in &trades {
        plain.apply_trade(x, BOTH);
    }
    let t = Instant::now();
    plain.tick(now, |_| Some(1.0));
    let without = t.elapsed();
    eprintln!("momentum cost: barrier with 50 flagged rows {with:?}, without {without:?}");
    assert!(b.alerts().iter().all(|a| a.outcome.checked > 0));
    assert!(with < Duration::from_secs(2), "{with:?}");
}

/// A burst tape on a board with the detector on: one burst row (kind `burst`) beside the Runners
/// rows, a `burst` log line, a `Burst` notice, the outcome settled 30 minutes later into a
/// `burst_outcome` line, and the row back after a restart. Runners counts never include it.
#[test]
fn a_burst_becomes_a_tracked_row() {
    // This tape is also a fast move (+25 % in 15 s); the fast entry has its own test.
    let mut b = board(
        vec![],
        Policy {
            fast: false,
            ..fixed(0.5, 10)
        },
    );
    b.enable_burst(burst::BurstPolicy {
        threshold: 0.0001,
        ..burst::BurstPolicy::default()
    });
    let start = at(9, 0, 0);
    let mut trades = flat("BRST", start, start + 30 * 60 * S, 1.0, 100);
    let b0 = start + 30 * 60 * S + 3 * S;
    for n in 0..10i64 {
        let mut t = tr("BRST", b0 + n * S / 2, 1.0 + 0.02 * (n + 1) as f64, 5000);
        t.exchange = if n % 2 == 0 { "Q".into() } else { "P".into() };
        trades.push(t);
    }
    // The run goes on to +25 % over the next seconds.
    for n in 0..20i64 {
        trades.push(tr("BRST", b0 + (10 + n) * S, 1.25 + 0.005 * n as f64, 5000));
    }
    trades.sort_by_key(|t| t.event_ts.0);
    // Tick every second like the quotes task.
    let mut i = 0;
    let mut t = start;
    let end = b0 + 40 * 60 * S;
    while t < end {
        while i < trades.len() && trades[i].event_ts.0 <= t {
            b.apply_trade(&trades[i], BOTH);
            i += 1;
        }
        b.tick(t, |_| Some(1.0));
        t += S;
    }
    assert_eq!(b.alerts().len(), 1);
    let a = &b.alerts()[0];
    assert_eq!(a.kind, AlertKind::Burst);
    // The burst notice first; the row is tracked like a flag, so its breakout may follow (the
    // push's per-stock cooldown holds that one back).
    let notices = b.take_notices();
    assert_eq!(notices[0].kind, NoticeKind::Burst, "{notices:?}");
    assert_eq!(
        notices
            .iter()
            .filter(|n| n.kind == NoticeKind::Burst)
            .count(),
        1
    );
    assert!(notices[0].from_flag_pct.unwrap() > 10.0);
    assert!(notices[0].since_start_min.is_some());
    let log: Vec<Value> = b
        .take_log()
        .into_iter()
        .map(|(_, l)| serde_json::from_str(&l).unwrap())
        .collect();
    let kinds: Vec<&str> = log.iter().map(|v| v["kind"].as_str().unwrap()).collect();
    assert!(kinds.contains(&"burst"), "{kinds:?}");
    assert!(kinds.contains(&"burst_outcome"), "{kinds:?}");
    assert!(
        !kinds.contains(&"outcome"),
        "no Runners strict outcome for a burst"
    );
    let bl = log.iter().find(|v| v["kind"] == "burst").unwrap();
    assert_eq!(bl["features"].as_object().unwrap().len(), burst::NF);
    assert_eq!(bl["strategy"], burst::STRATEGY);
    let ol = log.iter().find(|v| v["kind"] == "burst_outcome").unwrap();
    assert_eq!(ol["cont10"], true);
    assert_eq!(ol["partial"], false);
    // The frame: a burst row with its facts, the panel's burst block, Runners counts untouched.
    let body = b.body(&|_| (Some(1.3), Some(1.0)), &|_| None);
    let row = &body["rows"][0];
    assert_eq!(row["kind"], "burst");
    assert!(row["burst"]["gain_pct"].as_f64().unwrap() > 10.0);
    assert!(row["burst"]["rate_x"].as_f64().unwrap() >= 3.0);
    assert_eq!(row["burst"]["outcome"]["settled"], true);
    assert_eq!(row["burst"]["outcome"]["c10"], true);
    assert!(row["state"].is_string(), "tracked like a flag");
    assert_eq!(body["burst"]["today"], 1);
    assert_eq!(body["burst"]["per_day"], 20);
    assert_eq!(body["today"]["alerts"], 0);
    // A restart: the row comes back from the log, settled, with nothing re-logged or re-pushed.
    let tmp = tempfile::tempdir().unwrap();
    let rl = RocketLog::new(tmp.path().join("rocket-watch"), 30);
    let lines: Vec<String> = log.iter().map(|v| v.to_string()).collect();
    rl.append(day(), &lines);
    let mut r = board(vec![], fixed(0.5, 10));
    r.enable_burst(burst::BurstPolicy::default());
    r.restore(day(), rl.load(day()));
    assert_eq!(r.alerts().len(), 1);
    let a2 = &r.alerts()[0];
    assert_eq!(a2.kind, AlertKind::Burst);
    let o = &a2.burst.as_ref().unwrap().outcome;
    assert!(o.settled && o.cont()[0]);
    assert!(r.take_log().is_empty() && r.take_notices().is_empty());
    let body = r.body(&|_| (None, None), &|_| None);
    assert_eq!(body["rows"][0]["kind"], "burst");
    assert_eq!(body["today"]["alerts"], 0);
}

/// A board without the detector (a replay, `BURST_ALERTS=0`) sends no burst block and makes no
/// burst rows whatever the tape.
#[test]
fn no_burst_rows_without_the_detector() {
    let mut b = board(vec![], fixed(0.5, 10));
    let start = at(9, 0, 0);
    let mut trades = flat("BRST", start, start + 30 * 60 * S, 1.0, 100);
    for n in 0..10i64 {
        trades.push(tr(
            "BRST",
            start + 30 * 60 * S + n * S / 2,
            1.0 + 0.02 * (n + 1) as f64,
            5000,
        ));
    }
    play(&mut b, trades, start, start + 35 * 60 * S);
    assert!(b.alerts().is_empty());
    assert!(b.take_notices().is_empty());
    let body = b.body(&|_| (None, None), &|_| None);
    assert!(body.get("burst").is_none());
}

/// These fixtures would qualify both admission paths on a fully restored tape. Provisional
/// prints must not create rows/notices or contaminate the adaptive calibration population.
#[test]
fn peer_regression_unrestored_precursor_and_running_are_withheld() {
    for scorer in [vec![("COLD", 1.0)], vec![]] {
        let mut p = fixed(0.5, 10);
        p.fast = false;
        let mut b = board(scorer, p);
        b.tick(at(8, 59, 0), |_| None);
        b.set_running_facts(day(), HashMap::new());
        let mut tape = flat("COLD", at(9, 0, 0), at(10, 0, 0), 8.0, 3_000);
        tape.extend(climb("COLD", at(10, 0, 0), 40, 8.0, 0.01, 20_000).0);
        for t in tape {
            b.apply_unrestored_trade(&t, BOTH);
            b.tick(t.event_ts.0 + 15 * S, |_| None);
        }
        assert!(
            b.alerts().is_empty(),
            "live-only prefixes cannot admit rows"
        );
        assert!(b.take_notices().is_empty());
        assert!(
            b.block_max.is_empty() && b.top.is_empty(),
            "invalid scores cannot calibrate"
        );
    }
}

#[test]
fn peer_regression_running_waits_for_facts_then_known_absent_keeps_fallback() {
    let mut p = fixed(0.5, 10);
    p.fast = false;
    let mut b = board(vec![], p);
    let mut tape = flat("FACT", at(9, 0, 0), at(10, 0, 0), 8.0, 3_000);
    tape.extend(climb("FACT", at(10, 0, 0), 40, 8.0, 0.01, 20_000).0);
    play(&mut b, tape, at(9, 0, 0), at(10, 10, 1));
    assert!(b.alerts().is_empty(), "not loaded is not known absent");
    assert!(b.take_notices().is_empty());
    b.set_running_facts(day(), HashMap::new());
    b.apply_trade(&tr("FACT", at(10, 10, 1), 12.1, 20_000), BOTH);
    b.tick(at(10, 10, 16), |_| None);
    assert_eq!(
        b.alerts().len(),
        1,
        "an explicitly empty facts table uses existing fallback"
    );
    assert_eq!(b.alerts()[0].kind, AlertKind::Running);
}

#[test]
fn peer_regression_known_halt_blocks_new_precursor_and_running_rows() {
    for scorer in [vec![("STOP", 1.0)], vec![]] {
        let mut p = fixed(0.5, 10);
        p.fast = false;
        let mut b = board(scorer, p);
        b.tick(at(8, 59, 0), |_| None);
        b.set_running_facts(day(), HashMap::new());
        // Direct state setup also lets this regression run unchanged on the pre-fix tree.
        b.halted.insert("STOP".to_string());
        let mut tape = flat("STOP", at(9, 0, 0), at(10, 0, 0), 8.0, 3_000);
        tape.extend(climb("STOP", at(10, 0, 0), 40, 8.0, 0.01, 20_000).0);
        play(&mut b, tape, at(9, 0, 0), at(10, 10, 1));
        assert!(
            b.alerts().is_empty(),
            "known halts block new admissions even with delayed prints"
        );
        assert!(b.take_notices().is_empty());
        assert_eq!(b.last_cost.as_ref().unwrap().symbols, 1);
    }
}

#[test]
fn restored_tape_matches_full_prefix_and_empty_adoption_discards_provisional_bars() {
    let mut b = board(vec![("COLD", 1.0)], fixed(0.5, 10));
    let prefix = flat("COLD", at(9, 0, 0), at(9, 59, 30), 2.0, 1000);
    let held = flat("COLD", at(9, 59, 30), at(10, 0, 0), 2.1, 2000);
    let mut restored = Tape::default();
    for t in prefix.iter().chain(held.iter()) {
        restored.apply_trade(t, BOTH);
    }
    for t in &held {
        b.apply_unrestored_trade(t, BOTH);
    }
    b.tick(at(10, 0, 1), |_| None);
    assert!(b.alerts().is_empty());
    b.adopt_tape("COLD", &restored, false, day());
    assert_eq!(b.readiness()["unrestored"], 0);
    let slot = (at(10, 0, 0) - at(4, 0, 0)) / (15 * S) - 1;
    assert!(same(
        &features_at(&mut b.tape, "COLD", slot).unwrap(),
        &features_at(&mut restored, "COLD", slot).unwrap()
    ));
    b.adopt_tape("COLD", &Tape::default(), false, day());
    assert_eq!(b.readiness()["withheld"], 0);
    assert!(
        features_at(&mut b.tape, "COLD", slot).is_none(),
        "empty authoritative tape removes the old nonempty state"
    );
    b.tick(at(10, 0, 16), |_| None);
    assert!(b.alerts().is_empty() && b.take_notices().is_empty());
}

#[test]
fn pending_adoption_survives_date_roll_until_explicit_authoritative_adoption() {
    let mut b = board(vec![("COLD", 1.0)], fixed(0.5, 10));
    b.apply_unrestored_trade(&tr("COLD", at(10, 0, 1), 2.0, 1000), BOTH);
    b.tick(at(10, 0, 16), |_| None);
    assert_eq!(b.readiness()["unrestored"], 1);
    let next = et(NaiveDate::from_ymd_opt(2026, 10, 19).unwrap(), 10, 0, 1, 0)
        .unwrap()
        .0;
    // The ordinary tape reset must preserve readiness too, even without re-marking it.
    b.apply_trade(&tr("COLD", next, 2.0, 1000), BOTH);
    b.tick(next + 15 * S, |_| None);
    assert_eq!(
        b.readiness()["unrestored"],
        1,
        "reset_day must not make the provisional prefix ready"
    );
    let rebuilt = b.tape.clone();
    b.adopt_tape(
        "COLD",
        &rebuilt,
        false,
        NaiveDate::from_ymd_opt(2026, 10, 19).unwrap(),
    );
    assert_eq!(b.readiness()["withheld"], 0);
    b.tick(next + 30 * S, |_| None);
    assert_eq!(b.alerts().len(), 1);
}

#[test]
fn unavailable_adoption_blocks_only_its_date_including_an_empty_rebuild() {
    for empty in [false, true] {
        let mut b = board(vec![("COLD", 1.0)], fixed(0.5, 10));
        b.apply_unrestored_trade(&tr("COLD", at(10, 0, 1), 2.0, 1000), BOTH);
        let restored = if empty {
            Tape::default()
        } else {
            b.tape.clone()
        };
        b.adopt_tape("COLD", &restored, true, day());
        assert_eq!(b.readiness()["unrestored"], 0);
        assert_eq!(b.readiness()["unavailable"], 1);
        // Initializing an empty restored symbol on the SAME date cannot clear the guard.
        b.apply_trade(&tr("COLD", at(10, 0, 2), 2.0, 1000), BOTH);
        b.tick(at(10, 0, 16), |_| None);
        assert_eq!(b.readiness()["unavailable"], 1);
        assert!(b.alerts().is_empty());
        let next = et(NaiveDate::from_ymd_opt(2026, 10, 19).unwrap(), 10, 0, 1, 0)
            .unwrap()
            .0;
        b.apply_trade(&tr("COLD", next, 2.0, 1000), BOTH);
        b.tick(next + 15 * S, |_| None);
        assert_eq!(
            b.readiness()["unavailable"],
            0,
            "old structural loss does not describe a new date"
        );
        assert_eq!(b.alerts().len(), 1);
    }
}

#[test]
fn halt_resume_requires_a_completed_bar_opened_after_resume_for_both_paths() {
    for scorer in [vec![("STOP", 1.0)], vec![]] {
        let mut p = fixed(0.5, 10);
        p.fast = false;
        let mut b = board(scorer, p);
        b.tick(at(8, 59, 0), |_| None);
        b.set_running_facts(day(), HashMap::new());
        b.set_halted("STOP", true, None);
        let mut tape = flat("STOP", at(9, 0, 0), at(10, 0, 0), 8.0, 3_000);
        tape.extend(climb("STOP", at(10, 0, 0), 40, 8.0, 0.01, 20_000).0);
        play(&mut b, tape, at(9, 0, 0), at(10, 10, 1));
        let resume = at(10, 10, 7);
        b.set_halted("STOP", false, Some(resume));
        b.apply_trade(&tr("STOP", at(10, 10, 8), 12.1, 20_000), BOTH);
        b.tick(at(10, 10, 16), |_| None);
        assert!(
            b.alerts().is_empty(),
            "bar containing resume is not fully post-resume"
        );
        b.apply_trade(&tr("STOP", at(10, 10, 16), 12.2, 20_000), BOTH);
        b.tick(at(10, 10, 31), |_| None);
        assert_eq!(
            b.alerts().len(),
            1,
            "10:10:15–30 is the first completed eligible bar"
        );
    }
}

#[test]
fn open_halts_survive_date_roll_until_an_official_resume() {
    let mut b = board(vec![("STOP", 1.0)], fixed(0.5, 10));
    b.tick(at(10, 0, 0), |_| None);
    b.set_halted("STOP", true, None);
    let next = et(NaiveDate::from_ymd_opt(2026, 10, 19).unwrap(), 10, 0, 1, 0)
        .unwrap()
        .0;
    b.apply_trade(&tr("STOP", next, 2.0, 1000), BOTH);
    b.tick(next + 15 * S, |_| None);
    assert!(b.halted.contains("STOP") && b.alerts().is_empty());
    b.set_halted("STOP", false, Some(next + 16 * S));
    b.tick(next + 30 * S, |_| None);
    assert!(b.alerts().is_empty());
    b.tick(next + 45 * S, |_| None);
    assert_eq!(b.alerts().len(), 1);
}

/// The running entry (owner, 2026-10-22: TANH ran $8 → $28 and the forecast flagged it at
/// $24.61): a stock that IS running gets a row, tracked like a flag, its own log line and push
/// kind, once a day, and comes back from the log after a restart. Its required move is the
/// symbol's own: the same climb enters a quiet stock early and a wild one not at all.
#[test]
fn a_running_stock_gets_a_running_row() {
    let run = |facts: Option<running::Fact>| {
        let mut b = board(vec![], fixed(0.5, 10));
        // Roll to the day first, so the facts are for it.
        b.tick(at(8, 59, 0), |_| None);
        assert_eq!(b.take_running_facts_request(), Some(day()));
        if let Some(f) = facts {
            b.set_running_facts(day(), HashMap::from([("TNH".to_string(), f)]));
        }
        // A $8 base for an hour on $100k a minute, then +1 % a slot on heavier volume.
        let mut tape = flat("TNH", at(9, 0, 0), at(10, 0, 0), 8.0, 3_000);
        let (run, _) = climb("TNH", at(10, 0, 0), 40, 8.0, 0.01, 20_000);
        tape.extend(run);
        play(&mut b, tape, at(9, 0, 0), at(10, 12, 0));
        b
    };
    let quiet = running::Fact {
        vol: Some(0.05),
        adv: Some(500_000.0),
    };
    let mut b = run(Some(quiet));
    let rows: Vec<&Alert> = b.alerts().iter().collect();
    assert_eq!(rows.len(), 1, "one row a day");
    let a = rows[0];
    assert_eq!(a.kind, AlertKind::Running);
    // 2.5 × 5 % = 12.5 % off the 60-minute low: the 12th +1 % slot, about $9.01.
    assert!(a.price > 9.0 && a.price < 9.1, "{}", a.price);
    assert_eq!(a.at_ns, at(10, 3, 0));
    assert_eq!(a.strategy, running::STRATEGY);
    let body = b.body(&|_| (Some(9.0), Some(8.0)), &|_| None);
    assert_eq!(body["rows"][0]["kind"], "running");
    // The push: its own kind (a per-phone switch, on by default). Its later breakouts are the
    // breakout kind, under that switch (off by default).
    let n = b.take_notices();
    assert_eq!(n[0].kind, NoticeKind::Running);
    assert!(
        n[1..].iter().all(|n| n.kind == NoticeKind::Breakout),
        "{n:?}"
    );
    // The log line, and the row back from it after a restart (no notice on a restore).
    let lines: Vec<String> = b.take_log().into_iter().map(|(_, l)| l).collect();
    let entry: Vec<&String> = lines
        .iter()
        .filter(|l| l.contains(r#""kind":"running""#))
        .collect();
    assert_eq!(entry.len(), 1);
    let tmp = tempfile::tempdir().unwrap();
    let log = RocketLog::new(tmp.path().join("rocket-watch"), 30);
    log.append(day(), &lines);
    let mut r = board(vec![], fixed(0.5, 10));
    r.restore(day(), log.load(day()));
    assert_eq!(r.alerts().len(), 1);
    assert_eq!(r.alerts()[0].kind, AlertKind::Running);
    assert!(r.take_notices().is_empty());
    // Not in the Runners day record.
    assert_eq!(r.body(&|_| (None, None), &|_| None)["today"]["alerts"], 0);

    // A wild stock (its days span 25 %) needs the cap: +25 %, which this climb reaches later.
    let wild = running::Fact {
        vol: Some(0.25),
        adv: Some(500_000.0),
    };
    let b = run(Some(wild));
    assert_eq!(b.alerts().len(), 1);
    assert!(b.alerts()[0].price >= 10.0, "{}", b.alerts()[0].price);
    // A symbol that already has a row gets no running row.
    let mut c = board(vec![("TNH", 1.0)], fixed(0.5, 10));
    let mut tape = flat("TNH", at(9, 0, 0), at(10, 0, 0), 8.0, 3_000);
    tape.extend(climb("TNH", at(10, 0, 0), 40, 8.0, 0.01, 20_000).0);
    play(&mut c, tape, at(9, 0, 0), at(10, 12, 0));
    assert!(c.alerts().iter().all(|a| a.kind == AlertKind::Runner));
}

/// The "Run +20%" entry (owner, 2026-10-22): a +20 % run from the volume-shelf trigger becomes a
/// row at its mark, tracked like the others, with its own log line, push kind and day count;
/// once a symbol a day, never for a symbol that already has a row, and back after a restart.
#[test]
fn a_plus20_run_gets_a_run_row() {
    let run = |sym: &str, start: i64, mark: i64, p0: f64, close: f64| {
        (
            Arc::<str>::from(sym),
            crate::zones::Run {
                start_ns: start,
                start_price: p0,
                plus20_ns: mark,
                close,
                exec_ns: mark - 500_000_000,
                early: false,
            },
        )
    };
    // FLG is flagged by the strategy at 10:03 (as in the running test), RUNX is not.
    let mut b = board(vec![("FLG", 1.0)], fixed(0.5, 10));
    let mut tape = flat("FLG", at(9, 0, 0), at(10, 0, 0), 8.0, 3_000);
    tape.extend(climb("FLG", at(10, 0, 0), 40, 8.0, 0.01, 20_000).0);
    tape.extend(flat("RUNX", at(9, 0, 0), at(10, 0, 0), 1.0, 30_000));
    tape.extend(climb("RUNX", at(10, 0, 0), 40, 1.0, 0.01, 50_000).0);
    play(&mut b, tape, at(9, 0, 0), at(10, 5, 0));
    assert!(b.alerts().iter().any(|a| &*a.symbol == "FLG"));
    let before = b.alerts().len();
    let _ = b.take_notices();
    b.add_runs(vec![
        run("RUNX", at(10, 0, 0), at(10, 5, 0), 1.0, 1.22),
        // Already has a row (its R2 flag): none.
        run("FLG", at(10, 0, 0), at(10, 5, 0), 8.0, 9.7),
        // A second run the same day: none (one row per symbol a day).
        run("RUNX", at(10, 1, 0), at(10, 5, 0), 1.0, 1.25),
    ]);
    b.tick(at(10, 5, 1), |_| None);
    let runs: Vec<&Alert> = b
        .alerts()
        .iter()
        .filter(|a| a.kind == AlertKind::Run)
        .collect();
    assert_eq!(runs.len(), 1);
    assert_eq!(b.alerts().len(), before + 1);
    let a = runs[0];
    assert_eq!(&*a.symbol, "RUNX");
    assert_eq!(a.at_ns, at(10, 5, 0), "at the end of the mark slot");
    assert_eq!(a.price, 1.22);
    assert_eq!(a.strategy, run::STRATEGY);
    assert_eq!(a.reasons[0], "+22% in 5 min off $1.00");
    let body = b.body(&|_| (Some(1.22), Some(1.0)), &|_| None);
    let row = body["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["s"] == "RUNX")
        .unwrap();
    assert_eq!(row["kind"], "run");
    // FLG's run is new evidence on its existing row (owner, 2026-10-23): a secondary chip and a
    // follow-up notice, never a second row; RUNX's second run adds nothing (its own path).
    let flg = body["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["s"] == "FLG")
        .unwrap();
    assert_eq!(flg["also"][0]["kind"], "run");
    assert!(row.get("also").is_none(), "no follow-up on RUNX");
    // Its push kind: a per-phone switch, on by default.
    let n = b.take_notices();
    assert_eq!(n.len(), 2, "{n:?}");
    assert_eq!(n[0].kind, NoticeKind::Run);
    assert_eq!(n[0].from_flag_pct, Some(22.0));
    assert_eq!(n[1].kind, NoticeKind::FollowUp);
    assert_eq!(n[1].symbol, "FLG");
    let (title, text, data) = crate::alerts::runners::message(&n[1]);
    assert_eq!(title, "FLG · Run +20% (after Flag 5 min ago) · $9.70");
    assert!(
        text.starts_with("+21.2% in 5 min off $8.00 · 25+ trades, $25k+\n"),
        "{text}"
    );
    assert!(text.ends_with("Alert $9.70 · +5% = $10.19"), "{text}");
    assert_eq!(data["kind"], "runners.followup");
    // The same path again on FLG (its 15 s confirmation, a restart's re-fire): nothing.
    b.add_runs(vec![run("FLG", at(10, 1, 0), at(10, 5, 15), 8.0, 9.8)]);
    b.tick(at(10, 5, 16), |_| None);
    assert!(
        b.take_notices()
            .iter()
            .all(|n| n.kind != NoticeKind::FollowUp),
        "one follow-up per path per row"
    );
    // The fire-time spread (crate::spread, display only): absent until the spread stage priced
    // the row's notice, then on that row alone; an id that is no row of today's board is ignored.
    let runx_id = b
        .alerts()
        .iter()
        .find(|a| a.kind == AlertKind::Run)
        .unwrap()
        .id
        .clone();
    let row_of = |b: &RocketBoard, s: &str| {
        b.body(&|_| (Some(1.22), Some(1.0)), &|_| None)["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["s"] == s)
            .unwrap()
            .clone()
    };
    assert!(row_of(&b, "RUNX").get("spread_bp").is_none());
    b.set_spread(&runx_id, 157);
    b.set_spread("2026-01-01-NOPE-r1", 999);
    assert_eq!(row_of(&b, "RUNX")["spread_bp"], 157);
    assert!(row_of(&b, "FLG").get("spread_bp").is_none());
    // No live quote (crate::spread::Priced::NoQuote): `no_quote: true` on that row only, absent
    // everywhere else, and an id that is no row of today's board is ignored.
    // (The stage sets one or the other for a real row; the board just shows what it is told.)
    assert!(row_of(&b, "RUNX").get("no_quote").is_none());
    b.set_no_quote(&runx_id);
    b.set_no_quote("2026-01-01-NOPE-r1");
    assert_eq!(row_of(&b, "RUNX")["no_quote"], true);
    assert!(row_of(&b, "FLG").get("no_quote").is_none());
    // Tracked like the others: the climb goes on, the row's momentum follows.
    let more = climb("RUNX", at(10, 5, 0), 20, 1.01f64.powi(20), 0.01, 50_000).0;
    play(&mut b, more, at(10, 5, 0), at(10, 12, 0));
    let a = b
        .alerts()
        .iter()
        .find(|a| a.kind == AlertKind::Run)
        .unwrap();
    assert!(a.track.high() > 1.22, "{}", a.track.high());
    // The log line, the day's count, and the row back after a restart (no notice).
    b.tick(at(20, 0, 1), |_| None);
    let lines: Vec<String> = b.take_log().into_iter().map(|(_, l)| l).collect();
    let entry: Vec<&String> = lines
        .iter()
        .filter(|l| l.contains(r#""kind":"run""#))
        .collect();
    assert_eq!(entry.len(), 1);
    let day_line: Value = lines
        .iter()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .find(|v| v["kind"] == "day")
        .unwrap();
    assert_eq!(day_line["rows"]["run"], 1);
    let flags = b
        .alerts()
        .iter()
        .filter(|a| a.kind == AlertKind::Runner)
        .count();
    assert!(flags >= 1);
    assert_eq!(day_line["rows"]["runner"], flags);
    assert_eq!(day_line["alerts"], flags, "the run row is not a flag");
    let tmp = tempfile::tempdir().unwrap();
    let log = RocketLog::new(tmp.path().join("rocket-watch"), 30);
    log.append(day(), &lines);
    let mut r = board(vec![], fixed(0.5, 10));
    r.restore(day(), log.load(day()));
    let back: Vec<&Alert> = r
        .alerts()
        .iter()
        .filter(|a| a.kind == AlertKind::Run)
        .collect();
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].at_ns, at(10, 5, 0));
    assert!(r.take_notices().is_empty());
    // A run handed over after a restart for a symbol it restored: still one row.
    r.tick(at(10, 30, 0), |_| None);
    r.add_runs(vec![run("RUNX", at(10, 20, 0), at(10, 30, 0), 1.3, 1.6)]);
    r.tick(at(10, 30, 1), |_| None);
    assert_eq!(
        r.alerts()
            .iter()
            .filter(|a| a.kind == AlertKind::Run)
            .count(),
        1
    );
    // Not in the Runners day record.
    assert_eq!(
        r.body(&|_| (None, None), &|_| None)["today"]["alerts"],
        flags
    );
}

/// Plays `trades` second by second over `[from, to)` with a tick 300 ms into every second, as
/// the quotes task does: the prints of a second's first 300 ms are already in when the second
/// before it is read (`fast::FastTape::inputs_settled`).
fn play_secs(b: &mut RocketBoard, mut trades: Vec<Trade>, from: i64, to: i64) {
    trades.sort_by_key(|t| t.event_ts.0);
    let mut i = 0;
    let mut t = from - from.rem_euclid(S) + 300_000_000;
    while t < to {
        while i < trades.len() && trades[i].event_ts.0 < t {
            b.apply_trade(&trades[i], BOTH);
            i += 1;
        }
        b.tick(t, |_| None);
        t += S;
    }
}

/// The "Fast move" entry, end to end: a replayed spike (+10 % in 50 s on 100 prints and
/// $200k+) makes exactly one fast row and one push decision; a symbol that already has a row
/// gets none; the row is logged, counted in the day line and back after a restart.
#[test]
fn a_fast_spike_gets_one_fast_row_and_one_push() {
    let mut b = board(vec![], fixed(0.5, 10));
    let mut tape = Vec::new();
    for sym in ["ZAP", "HAS"] {
        // A quiet minute at $2.00, then two prints a second climbing 0.2 % a second for 50 s,
        // then holding at the top for two minutes.
        for k in 0..60 {
            tape.push(tr(sym, at(9, 59, 0) + k * S + 100_000_000, 2.00, 100));
        }
        for k in 0..170i64 {
            let p = 2.00 + 0.004 * (k.min(50) as f64);
            for off in [100_000_000, 600_000_000] {
                tape.push(tr(sym, at(10, 0, 0) + k * S + off, p, 1_000));
            }
        }
    }
    // HAS already has a row (a run entry, say): none for it.
    b.tick(at(9, 0, 1), |_| None);
    b.alerts.push(test_row(
        "HAS",
        at(9, 30, 0),
        AlertKind::Run,
        MState::Watching,
    ));
    play_secs(&mut b, tape, at(9, 59, 0), at(10, 3, 0));
    let fast: Vec<&Alert> = b
        .alerts()
        .iter()
        .filter(|a| a.kind == AlertKind::Fast)
        .collect();
    assert_eq!(fast.len(), 1, "{fast:?}");
    let a = fast[0].clone();
    assert_eq!(&*a.symbol, "ZAP");
    // +8 % held over the minute's low ($2.16) is reached about 40 s in.
    assert!(
        a.at_ns > at(10, 0, 30) && a.at_ns <= at(10, 0, 50),
        "{}",
        (a.at_ns - at(10, 0, 0)) / S
    );
    assert!(a.price >= 2.16 && a.score >= 1.0, "{a:?}");
    assert_eq!(a.strategy, fast::STRATEGY);
    assert!(a.reasons[0].contains("in 60 s"), "{:?}", a.reasons);
    let body = b.body(&|_| (Some(2.2), Some(2.0)), &|_| None);
    let row = body["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["s"] == "ZAP")
        .unwrap();
    assert_eq!(row["kind"], "fast");
    assert_eq!(row["outcome"]["state"], "pending", "no strict outcome tag");
    // One notice, and one push decision through the Runners delivery (on by default).
    let n: Vec<Notice> = b
        .take_notices()
        .into_iter()
        .filter(|n| n.kind == NoticeKind::Fast)
        .collect();
    assert_eq!(n.len(), 1, "{n:?}");
    assert_eq!(n[0].exec_ns, Some(a.at_ns - S), "the second it fired");
    let mut delivery = crate::alerts::runners::RunnersDelivery::in_memory();
    let prefs = crate::alerts::model::Prefs::default();
    let sent = n
        .iter()
        .filter(|x| {
            let ms = x.at_ns / 1_000_000;
            delivery.decide(&x.event_id(), 1, &prefs, ms, ms, day(), 600)
                == crate::alerts::runners::Decision::Send
        })
        .count();
    assert_eq!(sent, 1);
    let (title, text, _) = crate::alerts::runners::message(&n[0]);
    assert!(title.starts_with("ZAP · Fast move · $"), "{title}");
    assert!(
        text.contains(" in 60 s · ") && text.contains(" trades, $"),
        "{text}"
    );
    // The log line, the day's count, and the row back after a restart (no notice).
    b.tick(at(20, 0, 1), |_| None);
    let lines: Vec<String> = b.take_log().into_iter().map(|(_, l)| l).collect();
    let entry: Vec<Value> = lines
        .iter()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|v| v["kind"] == "fast")
        .collect();
    assert_eq!(entry.len(), 1);
    assert_eq!(entry[0]["sec"], (a.at_ns - S) / S);
    assert_eq!(entry[0]["secs"], 60);
    let day_line: Value = lines
        .iter()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .find(|v| v["kind"] == "day")
        .unwrap();
    assert_eq!(day_line["rows"]["fast"], 1);
    let tmp = tempfile::tempdir().unwrap();
    let log = RocketLog::new(tmp.path().join("rocket-watch"), 30);
    log.append(day(), &lines);
    let mut r = board(vec![], fixed(0.5, 10));
    r.restore(day(), log.load(day()));
    let back: Vec<&Alert> = r
        .alerts()
        .iter()
        .filter(|a| a.kind == AlertKind::Fast)
        .collect();
    assert_eq!(back.len(), 1);
    assert_eq!(&*back[0].symbol, "ZAP");
    assert!(r.take_notices().is_empty());
}

/// A row as the board would hold it, in momentum state `state`.
fn test_row(sym: &str, at_ns: i64, kind: AlertKind, state: MState) -> Alert {
    let mut track = Track::new(at_ns, 1.0);
    track.state = state;
    Alert {
        id: format!("{sym}-{at_ns}"),
        symbol: Arc::from(sym),
        at_ns,
        price: 1.0,
        score: 1.0,
        threshold: 1.0,
        reasons: vec![
            "+9% in 60 s (held)".into(),
            "48 trades, $64k in 60 s".into(),
        ],
        strategy: fast::STRATEGY.into(),
        version: fast::VERSION.into(),
        outcome: Outcome {
            checked: 0,
            high: f64::NAN,
            low: f64::NAN,
            mark: None,
            settled: true,
            partial: false,
        },
        track,
        kind,
        burst: None,
        also: Vec::new(),
    }
}

/// Past [`ROWS`] rows the panel keeps the active ones (breaking out, running, pullback) first,
/// then the newest; fading rows go first. Sent newest first. Also measures the payload.
#[test]
fn a_shelf_entry_of_a_runners_stock_is_one_notice() {
    let mut b = board(vec![], fixed(0.5, 10));
    b.tick(at(9, 0, 1), |_| None);
    b.alerts.push(test_row(
        "SHLF",
        at(9, 0, 0),
        AlertKind::Run,
        MState::Running,
    ));
    let _ = b.take_notices();
    let e = crate::zones::Event::test(at(9, 30, 0), 0.40, 0.38, 0.42);
    b.add_shelf_entries(vec![
        (Arc::from("SHLF"), e),
        // Not on Runners: nothing.
        (Arc::from("OTHER"), e),
    ]);
    b.tick(at(9, 30, 1), |_| None);
    let n = b.take_notices();
    assert_eq!(n.len(), 1, "{n:?}");
    assert_eq!(n[0].kind, NoticeKind::Shelf);
    assert_eq!(n[0].symbol, "SHLF");
    assert_eq!(n[0].at_ns, at(9, 30, 0));
    let (title, body, _) = crate::alerts::runners::message(&n[0]);
    assert_eq!(title, "SHLF · Volume shelf · $0.4000");
    assert!(
        body.starts_with("In volume shelf $0.38–0.42 · about 1 in 3 hold, 1 in 2 fail\n"),
        "{body}"
    );
    // Taken once: the next tick has nothing.
    b.tick(at(9, 30, 2), |_| None);
    assert!(b.take_notices().is_empty());
}

#[test]
fn the_row_limit_keeps_active_rows_then_the_newest() {
    let mut b = board(vec![], fixed(0.5, 10));
    b.tick(at(9, 0, 1), |_| None);
    // 10 old running rows, 80 quiet ones, 10 new fading ones.
    for i in 0..100usize {
        let state = match i {
            0..10 => MState::Running,
            90.. => MState::Fading,
            _ => MState::Watching,
        };
        let kind = [AlertKind::Fast, AlertKind::Run, AlertKind::Running][i % 3];
        b.alerts.push(test_row(
            &format!("S{i:03}"),
            at(9, 0, 0) + i as i64 * MIN_NS,
            kind,
            state,
        ));
    }
    let body = b.body(&|_| (None, None), &|_| None);
    let rows = body["rows"].as_array().unwrap();
    assert_eq!(rows.len(), ROWS);
    let syms: Vec<&str> = rows.iter().map(|r| r["s"].as_str().unwrap()).collect();
    assert_eq!(syms[0], "S089", "newest first, the fading ones dropped");
    for i in 0..10 {
        let s = format!("S{i:03}");
        assert!(syms.contains(&s.as_str()), "active {s} kept");
    }
    assert!(!syms.iter().any(|s| *s >= "S090"), "fading dropped first");
    assert!(
        !syms.contains(&"S019") && syms.contains(&"S020"),
        "then the oldest"
    );
    let ats: Vec<i64> = rows.iter().map(|r| r["at"].as_i64().unwrap()).collect();
    assert!(ats.windows(2).all(|w| w[0] >= w[1]), "newest first");
    // Under the limit nothing is dropped.
    b.alerts.truncate(30);
    let body = b.body(&|_| (None, None), &|_| None);
    assert_eq!(body["rows"].as_array().unwrap().len(), 30);
    // Payload: ROWS rows with prices (no levels, no day lines: the market adds those).
    b.alerts.clear();
    for i in 0..ROWS {
        b.alerts.push(test_row(
            &format!("SYM{i}"),
            at(9, 0, 0) + i as i64 * MIN_NS,
            AlertKind::Fast,
            MState::Running,
        ));
    }
    let body = b.body(&|_| (Some(2.41), Some(1.87)), &|_| None);
    let bytes = body.to_string().len();
    eprintln!("rocket body with {ROWS} rows: {bytes} B");
    assert!(bytes < 100_000, "{bytes}");
}

/// The fast-move state's memory per symbol (reported with the integration).
#[test]
fn fast_state_memory_per_symbol() {
    // The map entry (key + value inline, the filtered tape's struct included) plus both rings on
    // the heap (the second one only with FAST_BATCH_FILTER shadow or on) and the symbol's name.
    let per =
        std::mem::size_of::<(Arc<str>, FastSym)>() + 2 * fast::FastTape::heap_bytes() + 16 + 8;
    eprintln!(
        "fast tapes: ~{per} B per symbol, ~{:.1} MB for 13k symbols",
        (per * 13_000) as f64 / 1e6
    );
    assert!(per < 10_000, "{per}");
}

/// The fast-move CPU cost at whole-market scale: 13,000 symbols printing twice every second
/// (far busier than any real second). Reports the cost per print and per tick.
#[test]
fn fast_cost_at_market_scale() {
    // Off, the fast rule alone, and the rule with the batch filter's second tape (shadow).
    for (fast, batch) in [
        (false, FastBatch::Off),
        (true, FastBatch::Off),
        (true, FastBatch::Shadow),
    ] {
        fast_cost(fast, batch);
    }
}

fn fast_cost(fast: bool, fast_batch: FastBatch) {
    const N: usize = 13_000;
    let mut b = board(
        vec![],
        Policy {
            fast,
            fast_batch,
            ..fixed(0.5, 10)
        },
    );
    b.tick(at(9, 0, 1), |_| None);
    let syms: Vec<String> = (0..N).map(|i| format!("S{i}")).collect();
    let t0 = at(10, 0, 0);
    let (mut apply, mut tick, mut prints, mut ticks) = (Duration::ZERO, Duration::ZERO, 0u32, 0u32);
    let mut worst = Duration::ZERO;
    for k in 0..70i64 {
        let trades: Vec<Trade> = syms
            .iter()
            .enumerate()
            .flat_map(|(i, s)| {
                let p = 1.0 + (i % 50) as f64 * 0.1;
                [
                    tr(s, t0 + k * S + 100_000_000, p, 100),
                    tr(s, t0 + k * S + 600_000_000, p * 1.001, 100),
                ]
            })
            .collect();
        let t = Instant::now();
        for x in &trades {
            b.apply_trade(x, BOTH);
        }
        apply += t.elapsed();
        prints += trades.len() as u32;
        let t = Instant::now();
        b.tick(t0 + (k + 1) * S + 300_000_000, |_| None);
        let e = t.elapsed();
        tick += e;
        worst = worst.max(e);
        ticks += 1;
    }
    eprintln!(
        "fast={fast} batch={fast_batch:?}: {:.0} ns per print (the whole board), tick {:?} mean / {:?} worst with {N} symbols printing",
        apply.as_nanos() as f64 / f64::from(prints),
        tick / ticks,
        worst
    );
    assert!(worst < Duration::from_secs(1), "{worst:?}");
}

/// A venue-D Form T print (a TRF report in extended hours).
fn trf(sym: &str, ts: i64, dollars: f64, size: u64) -> Trade {
    let mut t = tr(sym, ts, dollars, size);
    t.exchange = "D".into();
    t.conditions = vec!["@".into(), "T".into()];
    t
}

/// The old 08:00 regime replayed through the board: exchange trading from 07:50 with the TRF
/// silent, then its opening batch (2,000 reports a second market-wide for 30 s) in which SPK's
/// reports span $2.30–$2.40 against a $2.00 exchange price, then a genuine exchange spike at
/// 09:00 on ZAP. Returns the board's fast rows and its log lines.
fn trf_day(mode: FastBatch) -> (Vec<Alert>, Vec<serde_json::Value>) {
    let mut b = board(
        vec![],
        Policy {
            fast_batch: mode,
            ..fixed(0.5, 10)
        },
    );
    b.tick(at(7, 0, 1), |_| None);
    let mut tape = Vec::new();
    for k in 0..(16 * 60) {
        tape.push(tr("SPK", at(7, 50, 0) + k * S + 100_000_000, 2.00, 100));
        tape.push(tr("BG", at(7, 50, 0) + k * S + 200_000_000, 5.00, 100));
    }
    for k in 0..30i64 {
        for j in 0..2_000i64 {
            tape.push(trf("BG", at(8, 0, 0) + k * S + j * 400_000, 5.00, 100));
        }
        if k < 3 {
            for j in 0..60i64 {
                let p = 2.30 + 0.10 * (j as f64 / 60.0);
                tape.push(trf(
                    "SPK",
                    at(8, 0, 0) + k * S + 1_000_000 + j * 10_000_000,
                    p,
                    1_000,
                ));
            }
        }
    }
    play_secs(&mut b, tape, at(7, 50, 0), at(8, 6, 0));
    // A genuine move on an exchange: +10 % in 50 s on two prints a second.
    let mut spike = Vec::new();
    for k in 0..60 {
        spike.push(tr("ZAP", at(8, 59, 0) + k * S + 100_000_000, 2.00, 100));
    }
    for k in 0..120i64 {
        let p = 2.00 + 0.004 * (k.min(50) as f64);
        for off in [100_000_000, 600_000_000] {
            spike.push(tr("ZAP", at(9, 0, 0) + k * S + off, p, 1_000));
        }
    }
    play_secs(&mut b, spike, at(8, 59, 0), at(9, 2, 0));
    let rows = b
        .alerts()
        .iter()
        .filter(|a| a.kind == AlertKind::Fast)
        .cloned()
        .collect();
    let lines = b
        .take_log()
        .into_iter()
        .map(|(_, l)| serde_json::from_str(&l).unwrap())
        .collect();
    (rows, lines)
}

fn log_kind<'a>(lines: &'a [serde_json::Value], kind: &str) -> Vec<&'a serde_json::Value> {
    lines.iter().filter(|l| l["kind"] == kind).collect()
}

/// `FAST_BATCH_FILTER=shadow` (the default): alerts exactly as before, the batch fire on SPK
/// included, while the filtered tape's verdict is logged beside it: no SPK fire, the same ZAP
/// fire. The batch window is logged once.
#[test]
fn the_trf_batch_filter_in_shadow_alerts_as_before_and_logs_both_tapes() {
    let (rows, lines) = trf_day(FastBatch::Shadow);
    let syms: Vec<&str> = rows.iter().map(|a| &*a.symbol).collect();
    assert_eq!(syms, ["SPK", "ZAP"], "unchanged alerting");
    let shadow = log_kind(&lines, "fast_shadow");
    let got: Vec<(String, String, bool, bool)> = shadow
        .iter()
        .map(|l| {
            (
                l["symbol"].as_str().unwrap().to_string(),
                l["variant"].as_str().unwrap().to_string(),
                l["alerting"].as_bool().unwrap(),
                l["in_batch"].as_bool().unwrap(),
            )
        })
        .collect();
    let want: Vec<(String, String, bool, bool)> = vec![
        ("SPK".into(), "unfiltered".into(), true, true),
        ("ZAP".into(), "unfiltered".into(), true, false),
        ("ZAP".into(), "filtered".into(), false, false),
    ];
    assert_eq!(got, want, "{shadow:?}");
    // The two ZAP fires are one move: the filtered line points at the unfiltered fire.
    assert_eq!(shadow[2]["other_fired_sec"], shadow[1]["sec"]);
    let batches = log_kind(&lines, "trf_batch");
    assert_eq!(batches.len(), 1, "{batches:?}");
    assert_eq!(batches[0]["start_ns"], at(8, 0, 0));
    assert_eq!(batches[0]["reason"], "rate");
    assert!(batches[0]["form_t_prints"].as_u64().unwrap() >= 60_000);
}

/// `on`: the filtered tape alerts, so the batch makes no row; the genuine move still does.
#[test]
fn the_trf_batch_filter_on_drops_the_batch_fire_only() {
    let (rows, lines) = trf_day(FastBatch::On);
    let syms: Vec<&str> = rows.iter().map(|a| &*a.symbol).collect();
    assert_eq!(syms, ["ZAP"]);
    let spk = log_kind(&lines, "fast_shadow")
        .into_iter()
        .find(|l| l["symbol"] == "SPK")
        .unwrap();
    assert_eq!(spk["variant"], "unfiltered");
    assert_eq!(spk["alerting"], false);
}

/// `off`: the rule as before, no second tape, nothing extra logged.
#[test]
fn the_trf_batch_filter_off_is_the_old_rule() {
    let (rows, lines) = trf_day(FastBatch::Off);
    assert_eq!(rows.len(), 2);
    assert!(log_kind(&lines, "fast_shadow").is_empty());
    assert!(log_kind(&lines, "trf_batch").is_empty());
}

#[test]
fn the_trf_batch_filter_switch() {
    let get = |v: &'static str| move |k: &str| (k == "FAST_BATCH_FILTER").then(|| v.to_string());
    assert_eq!(Policy::default().fast_batch, FastBatch::Shadow);
    assert_eq!(
        Policy::from_lookup(get("ON")).unwrap().fast_batch,
        FastBatch::On
    );
    assert_eq!(
        Policy::from_lookup(get("off")).unwrap().fast_batch,
        FastBatch::Off
    );
    assert!(Policy::from_lookup(get("maybe")).is_err());
}

/// WFF, 2026-10-26: a stock that spent the hour below the $0.10 floor and rose through it. Its
/// first Runners entry carries the crossing line (notice, forward log, restore); a later path
/// on the same stock does not, and neither does a second entry notice.
#[test]
fn the_first_entry_after_rising_through_the_floor_carries_the_crossing_line() {
    let mut r = board(vec![], fixed(0.5, 10));
    let mut seq = 0;
    let mut print = |r: &mut RocketBoard, ts: i64, px: f64| {
        seq += 1;
        let mut t = tr("WFF", ts, px, 50_000);
        t.source_seq = seq;
        r.apply_trade(&t, BOTH);
    };
    for m in 0..56 {
        print(&mut r, at(13, m, 0), 0.0755 + f64::from(m) * 0.0004);
    }
    print(&mut r, at(13, 58, 0), 0.1005);
    print(&mut r, at(13, 59, 0), 0.1019);
    r.tick(at(14, 0, 1), |_| None);
    let _ = r.take_notices();
    let _ = r.take_log();
    let entry = Notice {
        spread_bp: None,
        no_quote: false,
        at_ns: at(14, 0, 0),
        price: 0.1019,
        ..Notice::test(NoticeKind::Running, "WFF")
    };
    r.inject_notice(entry.clone());
    r.inject_notice(Notice {
        spread_bp: None,
        no_quote: false,
        alert_id: "WFF-2".into(),
        ..entry.clone()
    });
    r.inject_notice(Notice::test(NoticeKind::FollowUp, "WFF"));
    let n = r.take_notices();
    let line = "Crossed $0.10 2 min ago · 60-min low $0.0755 at 1:00 PM (+35.0%) · $242k traded below $0.10";
    assert!(
        matches!(&n[0].detail, Some(NoticeDetail::Crossing(t)) if t == line),
        "{:?}",
        n[0].detail
    );
    assert!(n[1].detail.is_none(), "decided once per stock and day");
    assert!(n[2].detail.is_none(), "a follow-up never carries it");
    let body = crate::alerts::runners::message(&n[0]).1;
    assert_eq!(body.lines().nth(1), Some(line), "{body}");
    assert!(
        body.lines().nth(2).unwrap().starts_with("Alert $0.1019"),
        "{body}"
    );

    // Logged once, and a restart keeps it.
    let logged: Vec<String> = r.take_log().into_iter().map(|(_, l)| l).collect();
    assert_eq!(
        logged.iter().filter(|l| l.contains("\"crossing\"")).count(),
        1
    );
    let tmp = tempfile::tempdir().unwrap();
    let log = RocketLog::new(tmp.path().join("rocket-watch"), 30);
    log.append(day(), &logged);
    let loaded = log.load(day());
    assert_eq!(loaded.crossings.get("WFF").map(String::as_str), Some(line));
    let mut back = board(vec![], fixed(0.5, 10));
    back.restore(day(), loaded);
    back.inject_notice(entry);
    assert!(
        back.take_notices()[0].detail.is_none(),
        "the restored decision stands"
    );
}

/// Scores `symbol` only at the barriers ending at `at_ns` (one alert per time, at known times).
struct AtTimes(&'static str, Vec<i64>);

impl RocketScorer for AtTimes {
    fn id(&self) -> &str {
        "at-times"
    }
    fn version(&self) -> &str {
        "t1"
    }
    fn score(&self, f: &Features) -> Option<f64> {
        (&*f.symbol == self.0 && self.1.contains(&f.at_ns)).then_some(1.0)
    }
}

/// CHSN 2026-11-03 (and PARA, RDAC, UFG, CNSY, BIVI the same day): a stock with two rows (here a
/// flag and a second flag 40 minutes later, live a Fast row and then a Buying-burst row) had
/// every breakout pushed once per row, same barrier, same price and body, under two event ids
/// (`...-f1793725140-<s>` and `...-b36070-<s>`), so the delivery's per-event dedupe let both
/// through. A breakout is the stock's: one notice per stock per barrier, on its newest row.
#[test]
fn a_stock_with_two_rows_breaks_out_once_per_barrier() {
    let first = at(10, 0, 15);
    let second = at(10, 40, 15);
    let mut b = RocketBoard::new(
        true,
        Arc::new(AtTimes("TWO", vec![first, second])),
        fixed(0.5, 10),
    );
    play(
        &mut b,
        flat("TWO", at(9, 0, 0), at(10, 50, 0), 0.21, 20_000),
        at(9, 0, 0),
        at(10, 50, 0),
    );
    assert_eq!(b.alerts().len(), 2, "two rows on one stock");
    let newest = b.alerts()[1].id.clone();
    assert_eq!(b.alerts()[1].at_ns, second);
    b.take_notices();
    b.take_log();
    let (mut run, top) = climb("TWO", at(10, 50, 0), 40, 0.21, 0.0091, 200_000);
    run.extend(flat("TWO", at(11, 0, 0), at(11, 30, 0), top, 20_000));
    play(&mut b, run, at(10, 50, 0), at(11, 30, 0));
    // Both rows' trackers did turn breaking_out at the same barriers (the precondition).
    let lines: Vec<Value> = b
        .take_log()
        .into_iter()
        .map(|(_, l)| serde_json::from_str(&l).unwrap())
        .collect();
    let turns = |id: &str| -> Vec<i64> {
        lines
            .iter()
            .filter(|v| v["kind"] == "state" && v["to"] == "breaking_out" && v["id"] == id)
            .map(|v| v["at_ns"].as_i64().unwrap())
            .collect()
    };
    let older = b.alerts()[0].id.clone();
    let shared: Vec<i64> = turns(&older)
        .into_iter()
        .filter(|t| turns(&newest).contains(t))
        .collect();
    assert!(!shared.is_empty(), "both rows broke out together");
    let outs: Vec<Notice> = b
        .take_notices()
        .into_iter()
        .filter(|n| n.kind == NoticeKind::Breakout)
        .collect();
    assert!(!outs.is_empty());
    for t in &shared {
        let at_t: Vec<&Notice> = outs.iter().filter(|n| n.at_ns == *t).collect();
        assert_eq!(
            at_t.len(),
            1,
            "one breakout per stock per barrier: {at_t:?}"
        );
        assert_eq!(at_t[0].alert_id, newest, "on the newest row");
    }
    let mut ats: Vec<i64> = outs.iter().map(|n| n.at_ns).collect();
    ats.dedup();
    assert_eq!(ats.len(), outs.len(), "{outs:?}");
}

/// Engine A T2-c4's prune POST<1 (`crate::c4_live::prunes_post1`): with the additions on, a Run
/// +20 % notice under $1 is not sent (its row is untouched); at $1 it is, and without the
/// additions nothing is pruned.
#[test]
fn t2_c4_prunes_post_cross_notices_under_a_dollar_only_when_on() {
    let run = |price: f64| Notice {
        spread_bp: None,
        no_quote: false,
        price,
        ..Notice::test(NoticeKind::Run, "SUBD")
    };
    let mut b = board(vec![], Policy::default());
    b.inject_notice(run(0.8));
    assert_eq!(b.take_notices().len(), 1, "off: nothing pruned");
    b.enable_c4();
    b.inject_notice(run(0.8));
    b.inject_notice(run(1.0));
    b.inject_notice(Notice {
        spread_bp: None,
        no_quote: false,
        price: 0.8,
        ..Notice::test(NoticeKind::Fast, "SUBD")
    });
    let kept: Vec<(NoticeKind, f64)> = b.take_notices().iter().map(|n| (n.kind, n.price)).collect();
    assert_eq!(kept, vec![(NoticeKind::Run, 1.0), (NoticeKind::Fast, 0.8)]);
}
