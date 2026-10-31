//! The indicators are bit-identical to the clients' (packages/chart-core/src/indicators.ts) on the
//! fixtures that code generated (packages/chart-core/src/indicatorFixtures.ts), and both sides
//! name every spec alike (the shared key vectors). See crates/chart-engine/src/indicators.rs.

use chart_engine::indicators::{Bar, IndicatorSeries, IndicatorSpec, batch};
use chrono::NaiveDate;
use market_events::UnixNanos;
use market_events::session::{SessionId, SessionKind, classify};
use serde_json::Value;

fn load(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(&path).expect("fixture file")).expect("json")
}

fn f(v: &Value) -> f64 {
    v.as_f64().expect("number")
}

/// One fixture line back to its values (NaN before `from`).
fn decode(line: &Value, n: usize) -> Vec<u64> {
    let from = line["from"].as_u64().expect("from") as usize;
    let hex = line["hex"].as_str().expect("hex");
    assert_eq!(hex.len(), (n - from) * 16, "line length");
    let mut out = vec![f64::NAN.to_bits(); from];
    for i in 0..n - from {
        out.push(u64::from_str_radix(&hex[i * 16..i * 16 + 16], 16).expect("hex digits"));
    }
    out
}

fn same(a: u64, b: u64) -> bool {
    a == b || (f64::from_bits(a).is_nan() && f64::from_bits(b).is_nan())
}

/// The bars a case computes over. Fixture-day cases come from the engine's candles with their
/// session, filtered by the ENGINE's regular session for rth; the TS filtered by its own display
/// schedule, so the counts agreeing is part of the test (true on 2026-10-15, a full day).
fn bars_of(case: &Value, candles: &Value) -> Vec<Bar> {
    let rth = case["rth"].as_bool().expect("rth");
    match case["source"].as_str() {
        Some(src) => candles["series"][src]
            .as_array()
            .expect("series")
            .iter()
            .filter_map(|r| {
                let session: SessionId =
                    r[5].as_str().expect("session").parse().expect("session id");
                (!rth || session.kind == SessionKind::Regular).then(|| Bar {
                    t_ms: r[0].as_i64().expect("t"),
                    day: session.date,
                    h: f(&r[1]),
                    l: f(&r[2]),
                    c: f(&r[3]),
                    v: r[4].as_u64().expect("v") as f64,
                })
            })
            .collect(),
        None => {
            assert!(!rth, "hand-made cases are all sessions");
            case["candles"]
                .as_array()
                .expect("candles")
                .iter()
                .map(|r| {
                    let t_ms = r[0].as_i64().expect("t");
                    let day: NaiveDate = r[5].as_str().expect("day").parse().expect("date");
                    // The server takes the day from the bucket's session; the TS from the ET
                    // wall clock with a 20:00 rollover. They must agree wherever there can be a
                    // candle (inside a session), across DST and weekends included.
                    if let Some(s) = classify(UnixNanos::from_millis(t_ms)) {
                        assert_eq!(s.date, day, "{}: trading day of {t_ms}", case["name"]);
                    }
                    Bar {
                        t_ms,
                        day,
                        h: f(&r[1]),
                        l: f(&r[2]),
                        c: f(&r[3]),
                        v: f(&r[4]),
                    }
                })
                .collect()
        }
    }
}

#[test]
fn values_are_bit_identical_to_the_client_code() {
    let fixtures = load("indicator_fixtures.json");
    let candles = load("indicator_candles.json");
    let cases = fixtures["cases"].as_array().expect("cases");
    assert!(cases.len() >= 20, "cases: {}", cases.len());
    let mut checked = 0usize;
    let mut sessions_checked = 0usize;
    for case in cases {
        let name = case["name"].as_str().expect("name");
        let bars = bars_of(case, &candles);
        let n = case["n"].as_u64().expect("n") as usize;
        assert_eq!(bars.len(), n, "{name}: candle count after the rth filter");
        if case["candles"].is_array() {
            sessions_checked += bars.len();
        }
        for study in case["studies"].as_array().expect("studies") {
            let (_, spec) = IndicatorSpec::parse(&study["spec"]).expect("fixture spec parses");
            assert_eq!(spec.key(), study["key"].as_str().expect("key"), "{name}");
            let lines = study["lines"].as_array().expect("lines");
            assert_eq!(lines.len(), spec.lines().len(), "{name} {}", spec.key());
            let want: Vec<Vec<u64>> = lines.iter().map(|l| decode(l, n)).collect();
            for (j, l) in lines.iter().enumerate() {
                assert_eq!(l["name"].as_str(), Some(spec.lines()[j].0), "{name}");
            }
            let got = batch(&spec, &bars);
            // And the incremental series, built in one go, is the same numbers.
            let series = IndicatorSeries::build(spec, bars.clone(), usize::MAX / 2);
            for i in 0..n {
                for j in 0..lines.len() {
                    let w = want[j][i];
                    let b = got[i][j].to_bits();
                    let s = series.values(i)[j].to_bits();
                    assert!(
                        same(w, b) && same(w, s),
                        "{name} {} line {} at {i} (t {}): client {} ({w:016x}), batch {} ({b:016x}), series {} ({s:016x})",
                        spec.key(),
                        spec.lines()[j].0,
                        bars[i].t_ms,
                        f64::from_bits(w),
                        f64::from_bits(b),
                        f64::from_bits(s),
                    );
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 30_000, "values checked: {checked}");
    assert!(
        sessions_checked > 1_000,
        "hand-made candles: {sessions_checked}"
    );
}

#[test]
fn spec_keys_match_the_shared_vectors() {
    let v = load("indicator_spec_keys.json");
    let vectors = v["vectors"].as_array().expect("vectors");
    assert!(vectors.len() > 30);
    for vec in vectors {
        let got = IndicatorSpec::parse(&vec["raw"]).map(|(_, s)| s.key());
        assert_eq!(got.as_deref(), vec["key"].as_str(), "raw {}", vec["raw"]);
    }
}
