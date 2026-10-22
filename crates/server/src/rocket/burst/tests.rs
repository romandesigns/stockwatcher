use super::*;
use market_events::session::et;
use market_events::{Price, Trade};
use std::collections::BTreeMap;

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 21).unwrap()
}

fn t0() -> i64 {
    et(day(), 4, 0, 0, 0).unwrap().0
}

fn trade(sym: &str, exec: i64, p: f64, size: u64, ex: &str) -> Trade {
    let mut t = Trade::new(
        sym,
        UnixNanos(exec),
        Price::from_micros((p * 1e6).round() as i64),
        size,
        1,
    );
    t.exchange = ex.into();
    t
}

const BOTH: Counted = Counted {
    ohlc: true,
    volume: true,
};

fn pc(_: &str) -> Option<f64> {
    Some(1.0)
}

/// 30 min of quiet $100 prints every 15 s from 05:00, then a 10-print burst at rising prices
/// over two venues: the research's own gate test, on the live detector.
fn burst_tape(sym: &str, start: i64) -> Vec<Trade> {
    let mut v: Vec<Trade> = (0..120)
        .map(|k| trade(sym, start + k * 15 * SEC, 1.00, 100, "V"))
        .collect();
    let b0 = start + 120 * 15 * SEC;
    for n in 0..10 {
        let ex = if n % 2 == 0 { "Q" } else { "P" };
        v.push(trade(
            sym,
            b0 + n * SEC / 2,
            1.00 + 0.02 * (n as f64 + 1.0),
            5_000,
            ex,
        ));
    }
    v
}

/// Drives a detector over prints in execution order, checking every 5 s boundary at `w + 1 s`
/// with the prints executed by then (what the live tick does, without a lag).
fn drive(d: &mut BurstDetector, prints: &[Trade], until: i64) -> Vec<BurstAlert> {
    let mut out = Vec::new();
    let mut i = 0;
    let mut w = prints
        .first()
        .map_or(t0(), |p| p.event_ts.0)
        .div_euclid(5 * SEC)
        * 5
        * SEC;
    while w <= until {
        let now = w + SETTLE_NS;
        while i < prints.len() && prints[i].event_ts.0 <= now {
            d.apply_trade(&prints[i], BOTH);
            i += 1;
        }
        out.extend(d.tick(now, &pc));
        w += 5 * SEC;
    }
    out
}

#[test]
fn the_frozen_model_parses() {
    let m = BurstModel::frozen();
    assert_eq!(m.trees(), 300);
    assert_eq!(m.max_feature_idx + 1, NF);
    // All missing: a finite probability.
    let p = m.predict(&[f64::NAN; NF]);
    assert!(p > 0.0 && p < 1.0, "{p}");
}

/// The pure-Rust evaluator against LightGBM's own `predict` on 442 research rows from five TEST
/// days (`burst_parity.csv`, written by `burst_export.py` from the research's export, features
/// as float32 exactly as the research scored them).
#[test]
fn model_matches_lightgbm() {
    let txt = include_str!("burst_parity.csv");
    let mut lines = txt.lines();
    let header: Vec<&str> = lines.next().unwrap().split(',').collect();
    assert_eq!(&header[3..3 + NF], &FEATS[..]);
    let (mut n, mut worst, mut missing) = (0, 0f64, 0);
    for l in lines {
        let c: Vec<&str> = l.split(',').collect();
        let mut x = [f64::NAN; NF];
        for (i, v) in c[3..3 + NF].iter().enumerate() {
            if !v.is_empty() {
                x[i] = v.parse().unwrap();
                // Every input is a float32, as the research fed the model.
                assert_eq!(f64::from(x[i] as f32), x[i]);
            } else {
                missing += 1;
            }
        }
        let want: f64 = c[3 + NF].parse().unwrap();
        let got = BurstModel::frozen().predict(&x);
        worst = worst.max((got - want).abs());
        n += 1;
    }
    assert!(n >= 300, "{n} rows");
    assert!(missing > 0, "the vectors exercise missing values");
    assert!(worst <= 1e-12, "max |diff| {worst:e}");
}

/// The research's `num`: how a feature was written to its CSV.
pub(super) fn num(x: f64) -> String {
    if x.is_nan() {
        String::new()
    } else if x == x.trunc() && x.abs() < 1e9 {
        format!("{}", x as i64)
    } else {
        format!("{x:.5}")
    }
}

/// Feature parity without the research disk: five symbols of TEST day 2026-09-02 (four of
/// them alerted that day), their counted prints as the archive replay produced them, through the
/// live detector; every gate pass and every feature must equal the research's export
/// (`burst_fixture.csv`, written by `replay_parity` with `BURST_PARITY_FIXTURE`).
#[test]
fn fixture_replays_like_the_research() {
    let txt = include_str!("burst_fixture.csv");
    let mut prints: BTreeMap<String, Vec<(Trade, Counted)>> = BTreeMap::new();
    let mut ctx: HashMap<String, (Option<f64>, Option<f64>)> = HashMap::new();
    let mut research: BTreeMap<(String, i64), Vec<String>> = BTreeMap::new();
    for l in txt.lines().filter(|l| !l.starts_with('#')) {
        let c: Vec<&str> = l.split(',').collect();
        let opt = |s: &str| (!s.is_empty()).then(|| s.parse::<f64>().unwrap());
        match c[0] {
            "c" => {
                ctx.insert(c[1].into(), (opt(c[2]), opt(c[3])));
            }
            "p" => {
                let mut t = Trade::new(
                    c[1],
                    UnixNanos(c[2].parse().unwrap()),
                    Price::from_micros(c[3].parse().unwrap()),
                    c[4].parse().unwrap(),
                    1,
                );
                t.exchange = c[7].into();
                let k = Counted {
                    ohlc: c[5] == "1",
                    volume: c[6] == "1",
                };
                prints.entry(c[1].into()).or_default().push((t, k));
            }
            "r" => {
                research.insert(
                    (c[1].into(), c[2].parse().unwrap()),
                    c[3..].iter().map(|s| s.to_string()).collect(),
                );
            }
            _ => panic!("bad fixture line {l}"),
        }
    }
    assert!(research.len() >= 80, "{} rows", research.len());
    let day = NaiveDate::from_ymd_opt(2026, 9, 2).unwrap();
    let t0 = et(day, 4, 0, 0, 0).unwrap().0;
    let mut d = BurstDetector::new(BurstPolicy::default());
    d.capture();
    let mut adv = HashMap::new();
    for (s, (_, a)) in &ctx {
        if let Some(a) = a {
            adv.insert(s.clone(), *a);
        }
    }
    let pcs: HashMap<String, f64> = ctx
        .iter()
        .filter_map(|(s, (p, _))| p.map(|p| (s.clone(), p)))
        .collect();
    // One detector, all five symbols interleaved in execution order, as the live tape is.
    let mut all: Vec<(Trade, Counted)> = prints.into_values().flatten().collect();
    all.sort_by_key(|x| x.0.event_ts.0);
    let pc = |s: &str| pcs.get(s).copied();
    let mut alerts = Vec::new();
    let (mut j, mut w) = (0, t0);
    d.tick(t0, &pc);
    d.set_adv(day, adv);
    while w <= t0 + NSEC * SEC {
        let now = w + SETTLE_NS;
        while j < all.len() && all[j].0.event_ts.0 <= now {
            d.apply_trade(&all[j].0, all[j].1);
            j += 1;
        }
        alerts.extend(d.tick(now, &pc));
        w += STEP_S * SEC;
    }
    let mine: BTreeMap<(String, i64), Pass> = d
        .take_captured()
        .into_iter()
        .map(|p| ((p.symbol.to_string(), p.t_ns), p))
        .collect();
    assert_eq!(
        mine.keys().collect::<Vec<_>>(),
        research.keys().collect::<Vec<_>>(),
        "gate passes"
    );
    for (k, f) in &research {
        let p = &mine[k];
        for (n, name) in FEATS.iter().enumerate() {
            assert_eq!(num(p.x[n]), f[n], "{} {} {name}", k.0, k.1);
        }
    }
    // The research's alerts that day for these symbols (test/alerts-model-theta-B20.csv).
    let got: Vec<(String, i64)> = alerts
        .iter()
        .map(|a| (a.pass.symbol.to_string(), a.at_ns))
        .collect();
    assert_eq!(
        got,
        sorted_by_time(&[
            ("JEM", 1788338286000000000),
            ("GCTK", 1788354911000000000),
            ("UCL", 1788379231000000000),
            ("FGL", 1788380081000000000),
            ("LGHL", 1788391456000000000),
        ])
    );
}

fn sorted_by_time(v: &[(&str, i64)]) -> Vec<(String, i64)> {
    let mut v: Vec<(String, i64)> = v.iter().map(|(s, t)| (s.to_string(), *t)).collect();
    v.sort_by_key(|x| x.1);
    v
}

#[test]
fn model_input_is_the_csv_round_trip() {
    assert_eq!(model_input(3.0), 3.0);
    assert!(model_input(f64::NAN).is_nan());
    assert_eq!(model_input(0.123456789), f64::from(0.12346f32));
    assert_eq!(model_input(-0.0000001), 0.0);
}

#[test]
fn gate_fires_on_a_burst_and_not_on_a_quiet_tape() {
    let start = t0() + 3600 * SEC;
    let mut d = BurstDetector::new(BurstPolicy {
        threshold: 0.0001,
        ..BurstPolicy::default()
    });
    d.capture();
    let tape = burst_tape("BRST", start);
    let end = tape.last().unwrap().event_ts.0 + 60 * SEC;
    let alerts = drive(&mut d, &tape, end);
    let passes = d.take_captured();
    assert!(!passes.is_empty(), "the burst passes the gate");
    assert_eq!(alerts.len(), 1, "one alert per burst (10-minute dedup)");
    let a = &alerts[0];
    assert_eq!(&*a.pass.symbol, "BRST");
    assert_eq!(a.at_ns, a.pass.t_ns + SEC);
    assert!(a.gain_pct().unwrap() > 10.0, "{:?}", a.gain_pct());
    assert!(a.reasons()[0].starts_with("$ rate "), "{:?}", a.reasons());
    // Quiet: nothing.
    let mut q = BurstDetector::new(BurstPolicy::default());
    q.capture();
    let quiet: Vec<Trade> = (0..200)
        .map(|k| trade("QUIET", start + k * 15 * SEC, 1.0, 100, "V"))
        .collect();
    let end = quiet.last().unwrap().event_ts.0 + 60 * SEC;
    assert!(drive(&mut q, &quiet, end).is_empty());
    assert!(q.take_captured().is_empty());
}

#[test]
fn features_only_see_boundaries_before_the_check() {
    // Causality: prints executed after the boundary (but before the check at w + 1 s) change
    // nothing the boundary scores; the same tape with a wild print added at w + 0.5 s gives the
    // same features at w.
    let start = t0() + 3600 * SEC;
    let tape = burst_tape("CAUS", start);
    let run = |extra: Option<Trade>| {
        let mut d = BurstDetector::new(BurstPolicy::default());
        d.capture();
        let mut v = tape.clone();
        if let Some(x) = extra {
            v.push(x);
            v.sort_by_key(|t| t.event_ts.0);
        }
        let end = v.last().unwrap().event_ts.0 + 20 * SEC;
        drive(&mut d, &v, end);
        d.take_captured()
    };
    let base = run(None);
    let w = base[0].t_ns;
    let spiked = run(Some(trade("CAUS", w + SEC / 2, 9.0, 100_000, "Z")));
    assert_eq!(base[0].t_ns, spiked[0].t_ns);
    assert_eq!(
        base[0].x.map(f64::to_bits),
        spiked[0].x.map(f64::to_bits),
        "a print after the boundary leaked into its features"
    );
}

#[test]
fn dedup_and_outcomes() {
    let start = t0() + 3600 * SEC;
    let mut d = BurstDetector::new(BurstPolicy {
        threshold: 0.0001,
        ..BurstPolicy::default()
    });
    let mut tape = burst_tape("DEDU", start);
    let b_end = tape.last().unwrap().event_ts.0;
    // The price runs to +25 % over the alert price within the 30 minutes, then a second burst
    // 5 minutes after the first: deduplicated.
    for n in 0..20 {
        tape.push(trade(
            "DEDU",
            b_end + (10 + n) * SEC,
            1.30 + 0.01 * n as f64,
            5_000,
            "Q",
        ));
    }
    let end = b_end + 40 * MIN;
    let alerts = drive(&mut d, &tape, end);
    assert_eq!(alerts.len(), 1);
    let settled = d.take_settled();
    assert_eq!(settled.len(), 1);
    let o = &settled[0].outcome;
    assert!(o.settled && !o.partial);
    let pa = alerts[0].price;
    assert!((o.mfe_p - 1.49).abs() < 1e-9, "{}", o.mfe_p);
    assert_eq!(o.cont()[0], 1.49 >= 1.1 * pa);
    assert_eq!(o.cont()[1], 1.49 >= 1.2 * pa - 1e-12);
}

#[test]
fn outcome_needs_the_target_before_the_stop() {
    let mut o = Outcome::new(1, 0, 1.0);
    o.print(10 * SEC, 0.94);
    o.print(20 * SEC, 1.30);
    o.settled = true;
    assert_eq!(o.cont(), [false, false, false]);
    assert!(o.stopped());
    assert_eq!(o.state(), "stopped");
    assert!((o.mfe_pct() - 30.0).abs() < 1e-9);
    assert!((o.mae_pct() + 6.0).abs() < 1e-9);
    let mut o = Outcome::new(1, 0, 1.0);
    o.print(10 * SEC, 1.30);
    o.print(20 * SEC, 0.94);
    assert_eq!(o.cont(), [true, true, true]);
    assert_eq!(o.state(), "cont30");
    assert_eq!(o.t20_s(), Some(10.0));
}

#[test]
fn policy_from_env() {
    let p = BurstPolicy::from_lookup(|_| None).unwrap();
    assert_eq!(p, BurstPolicy::default());
    assert!(p.enabled);
    assert_eq!(p.threshold, THETA_B20);
    let p =
        BurstPolicy::from_lookup(|k| (k == "BURST_ALERTS_PER_DAY").then(|| "10".into())).unwrap();
    assert_eq!((p.per_day, p.threshold), (10, THETA_B10));
    let p = BurstPolicy::from_lookup(|k| (k == "BURST_THRESHOLD").then(|| "0.3".into())).unwrap();
    assert_eq!(p.threshold, 0.3);
    let p = BurstPolicy::from_lookup(|k| (k == "BURST_ALERTS").then(|| "0".into())).unwrap();
    assert!(!p.enabled);
    assert!(
        BurstPolicy::from_lookup(|k| (k == "BURST_ALERTS_PER_DAY").then(|| "15".into())).is_err()
    );
    assert!(BurstPolicy::from_lookup(|k| (k == "BURST_THRESHOLD").then(|| "2".into())).is_err());
}

/// Synthetic whole-market cost: ~7k symbols printing all day at a few prints a second each,
/// with bursts in some. Measures the per-print update and the 5 s check.
#[test]
fn cost_per_print_and_per_check() {
    let n_sym = 7000usize;
    let syms: Vec<String> = (0..n_sym).map(|i| format!("S{i:04}")).collect();
    let mut d = BurstDetector::new(BurstPolicy::default());
    let start = t0() + 5 * 3600 * SEC;
    // 10 minutes; every symbol prints every 2 s (≈ 3,500 prints/s across the market).
    let secs = 600i64;
    let mut prints = 0u64;
    let mut apply_ns = 0u128;
    let mut check_ns = 0u128;
    let mut checks = 0u64;
    let mut max_check = Duration::ZERO;
    let mut batch: Vec<Trade> = Vec::with_capacity(n_sym);
    for s in 0..secs {
        batch.clear();
        for (i, sym) in syms.iter().enumerate() {
            if (i as i64 + s) % 2 != 0 {
                continue;
            }
            let burst = i % 500 == 0 && (300..330).contains(&s);
            let p = if burst {
                1.0 + 0.01 * (s - 300) as f64
            } else {
                1.0 + 0.001 * ((s % 7) as f64)
            };
            batch.push(trade(
                sym,
                start + s * SEC + (i as i64 % 1000) * 1000,
                p,
                if burst { 5000 } else { 100 },
                "Q",
            ));
        }
        let t = Instant::now();
        for tr in &batch {
            d.apply_trade(tr, BOTH);
        }
        apply_ns += t.elapsed().as_nanos();
        prints += batch.len() as u64;
        if s % 5 == 0 {
            let t = Instant::now();
            d.tick(start + s * SEC + SETTLE_NS, &pc);
            let e = t.elapsed();
            check_ns += e.as_nanos();
            max_check = max_check.max(e);
            checks += 1;
        }
    }
    let per_print = apply_ns as f64 / prints as f64;
    let per_check_us = check_ns as f64 / checks as f64 / 1e3;
    eprintln!(
        "burst cost: {prints} prints, {per_print:.0} ns/print; {checks} checks, mean {per_check_us:.0} µs, max {max_check:?}; {} symbols with rings",
        d.with_rings()
    );
    // Generous bounds (debug builds on a loaded machine): the point is O(1) and no blow-up.
    assert!(per_print < 20_000.0, "{per_print} ns per print");
    assert!(max_check < Duration::from_millis(500), "{max_check:?}");
}

#[test]
fn idle_rings_are_freed_and_rebuilt() {
    let mut d = BurstDetector::new(BurstPolicy::default());
    let start = t0() + 3600 * SEC;
    d.apply_trade(&trade("IDLE", start, 1.0, 100, "Q"), BOTH);
    d.tick(start + 10 * SEC, &pc);
    assert_eq!(d.with_rings(), 1);
    d.tick(start + 40 * MIN, &pc);
    d.tick(start + 41 * MIN, &pc);
    assert_eq!(d.with_rings(), 0, "freed after 35 idle minutes");
    // The burst after the gap still alerts as the research would (the base is empty).
    let mut d2 = BurstDetector::new(BurstPolicy {
        threshold: 0.0001,
        ..BurstPolicy::default()
    });
    d2.capture();
    let mut tape = vec![trade("IDLE", start, 1.0, 100, "Q")];
    let b0 = start + 50 * MIN;
    for n in 0..10 {
        tape.push(trade(
            "IDLE",
            b0 + n * SEC / 2,
            1.0 + 0.02 * (n + 1) as f64,
            5000,
            "Q",
        ));
    }
    let a = drive(&mut d2, &tape, b0 + 30 * SEC);
    assert_eq!(a.len(), 1);
}
