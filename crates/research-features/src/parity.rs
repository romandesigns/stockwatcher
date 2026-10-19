//! The Rust side of the TypeScript parity check: the candle file both sides read, the TS
//! script's output format, and the value-by-value comparison.

use crate::indicators::{chart_build_slots, chart_presets, chart_session_slots};
use chart_engine::Timeframe;
use chart_engine::wire::{CandleWire, GapWire};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The candle file exchanged with the TypeScript side.
#[derive(Serialize, Deserialize)]
pub struct CandleFile {
    pub symbol: String,
    pub day: String,
    pub timeframe: Timeframe,
    /// Provenance: how the candles were made.
    pub source: String,
    pub candles: Vec<CandleWire>,
    pub gaps: Vec<GapWire>,
}

/// The TS script's output: per variant (`ext`, `rth`) per line, values with `null` for NaN.
#[derive(Deserialize)]
pub struct TsOut {
    pub variants: BTreeMap<String, BTreeMap<String, Vec<Option<f64>>>>,
}

#[derive(Serialize, Default)]
pub struct LineDiff {
    pub points: usize,
    pub defined: usize,
    pub nan_mismatches: usize,
    pub max_abs_diff: f64,
    pub max_rel_diff: f64,
}

/// Compares Rust and TS line by line; returns per `variant/line` stats.
pub fn compare(candles: &CandleFile, ts: &TsOut) -> Result<BTreeMap<String, LineDiff>, String> {
    let all = chart_build_slots(&candles.candles, &candles.gaps);
    let mut out = BTreeMap::new();
    for (variant, lines) in &ts.variants {
        let slots = match variant.as_str() {
            "ext" => chart_session_slots(&all, true),
            "rth" => chart_session_slots(&all, false),
            v => return Err(format!("unknown variant {v}")),
        };
        let rust = chart_presets(&slots);
        if rust.len() != lines.len() {
            return Err(format!(
                "{variant}: {} Rust lines vs {} TS lines",
                rust.len(),
                lines.len()
            ));
        }
        for (name, tsv) in lines {
            let rv = rust
                .get(name)
                .ok_or_else(|| format!("{variant}: TS line {name} has no Rust counterpart"))?;
            if rv.len() != tsv.len() {
                return Err(format!(
                    "{variant}/{name}: length {} vs {}",
                    rv.len(),
                    tsv.len()
                ));
            }
            let mut d = LineDiff {
                points: rv.len(),
                ..Default::default()
            };
            for (r, t) in rv.iter().zip(tsv) {
                match (r.is_nan(), t) {
                    (true, None) => {}
                    (false, Some(t)) => {
                        d.defined += 1;
                        let abs = (r - t).abs();
                        d.max_abs_diff = d.max_abs_diff.max(abs);
                        if *t != 0.0 {
                            d.max_rel_diff = d.max_rel_diff.max(abs / t.abs());
                        }
                    }
                    _ => d.nan_mismatches += 1,
                }
            }
            out.insert(format!("{variant}/{name}"), d);
        }
    }
    Ok(out)
}
