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
    let f: Value =
        serde_json::from_str(&b.frame("rocket.snapshot", Some(5), |_| (Some(1.1), Some(1.0))))
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
