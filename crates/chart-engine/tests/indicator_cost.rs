//! What the indicators cost under the server's core lock (plan 2.2 "Core-lock hold time").
//! A measurement, not a check; run in release:
//!
//!   cargo test --release -p chart-engine --test indicator_cost -- --ignored --nocapture

use std::time::Instant;

use chart_engine::indicators::{Bar, IndicatorSeries, IndicatorSpec};
use chrono::NaiveDate;

fn bars(n: usize) -> Vec<Bar> {
    let day = NaiveDate::from_ymd_opt(2026, 10, 15).unwrap();
    let mut p = 10.0f64;
    (0..n)
        .map(|i| {
            p = (p * (1.0 + ((i * 7919 % 101) as f64 - 50.0) * 0.0002) * 100.0).round() / 100.0;
            Bar {
                t_ms: i as i64 * 15_000,
                day,
                h: p + 0.02,
                l: p - 0.02,
                c: p,
                v: 1000.0,
            }
        })
        .collect()
}

#[test]
#[ignore]
fn cost_of_the_presets_at_the_engine_cap() {
    // The menus' presets: what a chart with everything on computes.
    let presets = [
        IndicatorSpec::Vwap,
        IndicatorSpec::Ema { period: 9 },
        IndicatorSpec::Ema { period: 20 },
        IndicatorSpec::Sma { period: 50 },
        IndicatorSpec::Bb {
            period: 20,
            mult: 2.0,
        },
        IndicatorSpec::Rsi { period: 14 },
        IndicatorSpec::Macd {
            fast: 12,
            slow: 26,
            signal: 9,
        },
    ];
    let n = 4000;
    let input = bars(n);
    let reps = 200;
    let t = Instant::now();
    let mut series = Vec::new();
    for _ in 0..reps {
        series = presets
            .iter()
            .map(|s| IndicatorSeries::build(*s, input.clone(), n))
            .collect();
    }
    let build = t.elapsed() / reps;

    let mut forming = input[n - 1];
    let reps = 100_000u32;
    let t = Instant::now();
    for k in 0..reps {
        forming.c = 10.0 + (k % 50) as f64 * 0.01;
        for s in series.iter_mut() {
            s.upsert(forming);
        }
    }
    let per_trade = t.elapsed() / reps;

    let reps = 200u32;
    let t = Instant::now();
    for k in 0..reps {
        let mut b = input[0];
        b.c = 9.0 + (k % 7) as f64 * 0.01;
        for s in series.iter_mut() {
            s.upsert(b);
        }
    }
    let worst_revision = t.elapsed() / reps;
    println!(
        "{} presets over {n} bars: build {build:?}, forming-bar trade {per_trade:?}, revision of the oldest bar {worst_revision:?}",
        presets.len()
    );
}
