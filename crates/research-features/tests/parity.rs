//! Rust `chart_*` vs the chart's own TypeScript, on checked-in fixtures.
//!
//! `tests/fixtures/<SYM>-<day>-<tf>.candles.json`: chart-engine candles + gaps (`rf candles`,
//! trimmed to a window). `<...>.ts.json`: `parity/ts-indicators.ts` run on that file, i.e. the
//! values chart-core computes for every preset, with extended hours on (`ext`) and off (`rth`).
//!
//! Tolerance: 1e-9 absolute. The port is bit-exact when both sides read identical doubles
//! (verified with serde_json's `float_roundtrip`, max |diff| 0.0 on every series); this crate uses
//! serde_json's default parser, which may round a decimal 1 ULP differently from JavaScript, so
//! the observed residual is ≤ 1 ULP (≈ 1.4e-14 for values near 100).

use research_features::indicators::{
    Slot, chart_build_slots, chart_closes, chart_ema, chart_vwap_hlc3, completed_and_forming,
};
use research_features::parity::{CandleFile, TsOut, compare};
use std::path::PathBuf;

const TOLERANCE: f64 = 1e-9;

const FIXTURES: &[&str] = &[
    "MDXH-2026-10-16-15s",
    "MDXH-2026-10-16-30s",
    "MDXH-2026-10-16-1m",
    "MDXH-2026-10-16-5m",
    "SMPL-2026-10-15-1m",
    "SMPL-2026-10-15-5m",
];

fn fixture(name: &str) -> (CandleFile, TsOut) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let read =
        |f: String| std::fs::read_to_string(dir.join(&f)).unwrap_or_else(|e| panic!("{f}: {e}"));
    let c: CandleFile = serde_json::from_str(&read(format!("{name}.candles.json"))).unwrap();
    let t: TsOut = serde_json::from_str(&read(format!("{name}.ts.json"))).unwrap();
    (c, t)
}

#[test]
fn every_preset_line_matches_the_typescript() {
    for name in FIXTURES {
        let (c, t) = fixture(name);
        let diffs = compare(&c, &t).unwrap();
        // 11 lines (vwap, ema-9, ema-20, sma-50, bb x3, rsi, macd x3) x 2 variants.
        assert_eq!(diffs.len(), 22, "{name}");
        let mut defined = 0;
        for (line, d) in &diffs {
            assert_eq!(d.nan_mismatches, 0, "{name} {line}: NaN pattern differs");
            assert!(
                d.max_abs_diff <= TOLERANCE,
                "{name} {line}: max |diff| {:e}",
                d.max_abs_diff
            );
            defined += d.defined;
        }
        assert!(defined > 0, "{name}: nothing compared");
    }
}

/// The chart computes indicators on the forming candle's current values; research reads the
/// completed and forming values separately. On a real series with the newest candle marked
/// forming, the forming reading equals what the chart (TS) shows at that slot, and the completed
/// reading equals the chart's value at the last final candle.
#[test]
fn forming_and_completed_readings_on_real_candles() {
    let (mut c, t) = fixture("MDXH-2026-10-16-1m");
    c.candles.last_mut().unwrap().is_final = false;
    let slots = chart_build_slots(&c.candles, &c.gaps);
    let ts = &t.variants["ext"];
    let last = slots.len() - 1;
    let prev_candle = slots[..last]
        .iter()
        .rposition(|s| matches!(s, Slot::Candle(_)))
        .unwrap();
    for (line, f) in [
        (
            "ema-9/EMA",
            Box::new(|s: &[Slot]| chart_ema(&chart_closes(s), 9))
                as Box<dyn Fn(&[Slot]) -> Vec<f64>>,
        ),
        ("vwap/VWAP", Box::new(chart_vwap_hlc3)),
    ] {
        let r = completed_and_forming(&slots, &f);
        let ts_forming = ts[line][last].unwrap();
        let ts_completed = ts[line][prev_candle].unwrap();
        assert!(
            (r.forming.unwrap() - ts_forming).abs() <= TOLERANCE,
            "{line}"
        );
        assert!(
            (r.completed.unwrap() - ts_completed).abs() <= TOLERANCE,
            "{line}"
        );
        assert_eq!(r.forming_t, Some(slots[last].t()));
        assert_eq!(r.completed_t, Some(slots[prev_candle].t()));
    }
}
