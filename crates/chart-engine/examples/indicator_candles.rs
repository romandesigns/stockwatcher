//! Writes `tests/fixtures/indicator_candles.json`: the candles this engine builds from the
//! synthetic fixture day (crates/market-archive/src/fixture.rs; FIXTURE DATA, NOT MARKET DATA),
//! with each candle's session. They are the input of the indicator parity fixtures: the TS
//! generator (packages/chart-core/src/indicatorFixtures.ts) runs the client's indicator code over
//! exactly these candles, and tests/indicator_parity.rs runs this crate's over the same ones.
//!
//!   cargo run -p chart-engine --example indicator_candles
//!
//! The candles go out as the wire prints them (`Price::to_f64`, shortest round-trip JSON), so
//! both sides parse the same f64s. The 15 s series keeps the regular session only, to keep the
//! committed file small; the others keep every session so the rth filter and the 20:00 ET VWAP
//! rollover are exercised on engine output.

use std::collections::BTreeMap;

use chart_engine::{ChartEngine, Timeframe};
use chrono::NaiveDate;
use market_archive::fixture::{DEFAULT_SEED, fixture_symbol, symbol_params};
use market_events::session::{SessionKind, et};
use serde_json::{Value, json};

fn main() {
    let day = NaiveDate::from_ymd_opt(2026, 10, 15).expect("valid date");
    let mut engine = ChartEngine::default();
    let symbols = ["SMPL", "FXLO"];
    for (i, sym) in symbols.iter().enumerate() {
        let (seed, scale) = symbol_params(DEFAULT_SEED, i, sym);
        for e in fixture_symbol(day, sym, seed, scale).expect("fixture day") {
            engine.apply(&e);
        }
    }
    // Past the after-hours close, so every candle is final.
    engine.advance_clock(et(day, 20, 0, 0, 0).expect("valid time"));

    let wanted: [(&str, Timeframe, bool); 4] = [
        ("SMPL", Timeframe::M1, false),
        ("SMPL", Timeframe::M5, false),
        ("SMPL", Timeframe::S15, true),
        ("FXLO", Timeframe::M5, false),
    ];
    let mut series: BTreeMap<String, Value> = BTreeMap::new();
    for (sym, tf, regular_only) in wanted {
        let snap = engine.snapshot(sym, tf, None).expect("fixture symbol");
        // `[t, h, l, c, v, session]`, arrays rather than objects to keep the committed file small.
        let candles: Vec<Value> = snap
            .candles
            .iter()
            .filter_map(|c| {
                let session =
                    market_events::session::classify(market_events::UnixNanos::from_millis(c.t))
                        .expect("every candle is in a session");
                if regular_only && session.kind != SessionKind::Regular {
                    return None;
                }
                Some(json!([c.t, c.h, c.l, c.c, c.v, session.to_string()]))
            })
            // The 15 s regular session: its first 90 minutes (360 candles) are enough.
            .take(if regular_only { 360 } else { usize::MAX })
            .collect();
        let tf_name = serde_json::to_value(tf).expect("timeframe serializes");
        let name = format!("{sym}/{}", tf_name.as_str().expect("string"));
        series.insert(name, Value::Array(candles));
    }
    let out = json!({
        "note": "FIXTURE DATA, NOT MARKET DATA: chart-engine candles of the synthetic fixture day. Regenerate: cargo run -p chart-engine --example indicator_candles",
        "day": day.to_string(),
        "series": series,
    });
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/indicator_candles.json"
    );
    std::fs::create_dir_all(std::path::Path::new(path).parent().expect("has a parent"))
        .expect("mkdir");
    std::fs::write(
        path,
        serde_json::to_string(&out).expect("serializes") + "\n",
    )
    .expect("write");
    let n: usize = out["series"]
        .as_object()
        .expect("object")
        .values()
        .map(|v| v.as_array().map_or(0, Vec::len))
        .sum();
    println!("wrote {n} candles to {path}");
}
