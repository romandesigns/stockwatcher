//! What server-computed indicators cost (plan 2.2/2.4: core-lock hold time and snapshot size).
//! A measurement, not a check, over the synthetic fixture day (FIXTURE DATA, NOT MARKET DATA);
//! run in release:
//!
//!   cargo test --release -p stockwatcher-server --test indicator_bench -- --ignored --nocapture

use std::time::{Duration, Instant};

use chart_engine::{ChartEngine, Timeframe};
use chrono::NaiveDate;
use market_archive::fixture::{DEFAULT_SEED, fixture_symbol, symbol_params};
use market_events::session::et;
use serde_json::json;
use stockwatcher_server::indicators::{IndicatorRegistry, parse_request};

#[test]
#[ignore]
fn cost_and_size_over_the_fixture_day() {
    let date = NaiveDate::from_ymd_opt(2026, 10, 15).unwrap();
    let (seed, scale) = symbol_params(DEFAULT_SEED, 0, "SMPL");
    let events = fixture_symbol(date, "SMPL", seed, scale).unwrap();
    // Every preset on: 3 overlays would be the menu cap, but the registry computes what it is
    // asked; this is the heaviest list a client can send (16 specs are allowed, the caps then
    // keep 3 overlays and 2 panes, so 5 studies / 9 lines).
    let list = json!([
        {"id": "bb", "kind": "bb", "period": 20, "mult": 2},
        {"id": "vwap", "kind": "vwap"},
        {"id": "ema-9", "kind": "ema", "period": 9},
        {"id": "rsi", "kind": "rsi", "period": 14},
        {"id": "macd", "kind": "macd"}
    ]);
    let s15 = parse_request("SMPL", Timeframe::S15, Some(&list), Some(false))
        .unwrap()
        .unwrap();
    let m1 = parse_request("SMPL", Timeframe::M1, Some(&list), Some(true))
        .unwrap()
        .unwrap();

    // The engine alone, then the engine with the registry fed, over the same events.
    let mut plain = ChartEngine::default();
    let t = Instant::now();
    for e in &events {
        plain.apply(e);
    }
    let engine_only = t.elapsed();

    let mut engine = ChartEngine::default();
    let mut reg = IndicatorRegistry::default();
    reg.acquire(&s15);
    reg.acquire(&m1);
    let (mut fed, mut frames, mut bytes, mut worst) =
        (Duration::ZERO, 0usize, 0usize, Duration::ZERO);
    let mut built = false;
    for (n, e) in events.iter().enumerate() {
        let ups = engine.apply(e);
        if !built && n == events.len() / 50 {
            // The charts are opened early in the day; from here on the registry follows.
            reg.snapshot(&engine, &s15);
            reg.snapshot(&engine, &m1);
            built = true;
        }
        let t = Instant::now();
        let out = reg.on_updates(&ups);
        let d = t.elapsed();
        fed += d;
        worst = worst.max(d);
        frames += out.len();
        bytes += out.iter().map(|(_, j)| j.len()).sum::<usize>();
    }
    engine.advance_clock(et(date, 20, 0, 0, 0).unwrap());

    // A snapshot at the end of the day: the 15 s series is at the engine's 4000-bar cap.
    let t = Instant::now();
    let snap15 = reg.snapshot(&engine, &s15);
    let snap15_time = t.elapsed();
    let snap_m1 = reg.snapshot(&engine, &m1);
    // A fresh build of the same (what the first subscriber of a chart pays).
    let mut cold = IndicatorRegistry::default();
    cold.acquire(&s15);
    let t = Instant::now();
    let cold15 = cold.snapshot(&engine, &s15);
    let cold_time = t.elapsed();
    assert_eq!(cold15.len(), snap15.len());
    let candles15 = serde_json::to_string(&engine.snapshot("SMPL", Timeframe::S15, None).unwrap())
        .unwrap()
        .len();
    let bars15 = serde_json::from_str::<serde_json::Value>(&snap15).unwrap()["t"]
        .as_array()
        .unwrap()
        .len();

    println!("fixture day SMPL: {} events", events.len());
    println!(
        "engine.apply alone {engine_only:?}; registry.on_updates (5 studies on 15s all-sessions + 5 on 1m rth) {fed:?} total, {:?}/event mean, worst {worst:?}",
        fed / events.len() as u32
    );
    println!(
        "increments: {frames} frames, {bytes} bytes ({} B/frame mean)",
        bytes / frames.max(1)
    );
    println!(
        "snapshot 15s ({bars15} bars, 9 lines): {} bytes in {snap15_time:?} warm, built cold in {cold_time:?}; candle snapshot of the same chart {candles15} bytes",
        snap15.len()
    );
    println!("snapshot 1m rth: {} bytes", snap_m1.len());
    // The phone's ask (`limit`, indicators.limit.v1): the newest 1500 bars, same values.
    let lim15 = reg.snapshot(&engine, &s15.clone().with_limit(Some(1500)));
    println!(
        "snapshot 15s limited to 1500 bars: {} bytes ({:.0} % of the full {})",
        lim15.len(),
        100.0 * lim15.len() as f64 / snap15.len() as f64,
        snap15.len()
    );
}
