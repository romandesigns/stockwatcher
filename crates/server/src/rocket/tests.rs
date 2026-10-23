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
    b.set_halted("HALT", true);
    let body = b.body(&|_| (Some(2.0), None), &|_| None);
    assert_eq!(body["rows"][0]["state"], "halted");
    b.tick(at(10, 0, 31), |_| None);
    assert_eq!(b.alerts()[0].track.state, MState::Halted);
    let log: Vec<String> = b.take_log().into_iter().map(|(_, l)| l).collect();
    assert!(
        log.iter().any(|l| l.contains(r#""to":"halted""#)),
        "{log:?}"
    );
    b.set_halted("HALT", false);
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
            b.set_halted("BUSY", halted);
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
    let mut b = board(vec![], fixed(0.5, 10));
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
    // Its push kind: a per-phone switch, on by default.
    let n = b.take_notices();
    assert_eq!(n.len(), 1, "{n:?}");
    assert_eq!(n[0].kind, NoticeKind::Run);
    assert_eq!(n[0].from_flag_pct, Some(22.0));
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
