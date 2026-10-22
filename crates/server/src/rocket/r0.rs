//! `precursors-R0`: the rocket bake-off's round-1 pick for the experimental Rocket watch panel
//! (owner, 2026-10-21/22).
//!
//! Frozen on research branch `research/rocket-precursors` at commit `c009126`
//! (`C:\research\stockwatcher\reports\rockets\candidates\precursors-R0\frozen.json`), chosen on the
//! 11 development days only, then scored once by an independent judge on 10 untouched days
//! (2026-10-07..10-20, 329 rockets, `research/rocket-judge`). At 10 alerts a day it was one of three
//! candidates (of 20) to beat both baselines on strict precision: 7.7% of alerts were followed by
//! a rocket START 1–10 min later (Wilson 95% 3.9–14.4%; day-block bootstrap 2.7–13.4%), against
//! ~1% for volatility-matched random alerts and 3.2% for the plain volume-acceleration scanner;
//! 47% saw +10% within 30 min (random ~15%), median drawdown in that time −9.7%, recall 2.4%.
//! That rests on 8 hits: an experimental cue, not a validated signal — the panel says so.
//!
//! The rule: the stock already swung more than ~15% in the last hour (it is "in play"), it is not
//! simply grinding up (at most half of the last 3 minutes' bars closed higher), and its volume is
//! not dying; ranked by 3-min vs 30-min volume acceleration. Thresholds are the frozen ones, so the
//! alert count follows the market (a hot day flags more), capped by the policy.
//!
//! The features are the research harness's own (`features.rs`, same formulas and 15 s grid), so the
//! frozen numbers apply unchanged. A missing feature (NaN) fails the gate, as in the harness's
//! `Model::Rule`.

use super::features::{Features, ix};
use super::scorer::{RocketScorer, standard_reasons};

/// `x[RANGE60] >= RANGE60_MIN` (log of the 60-min high/low ratio).
const RANGE60_MIN: f64 = 0.154_391_848_316_780_6;
/// `x[UPC3] <= UPC3_MAX` (share of the last 3 min's bars that closed higher).
const UPC3_MAX: f64 = 0.5;
/// `x[VR3] >= VR3_MIN` (log 3-min / 30-min volume pace).
const VR3_MIN: f64 = -0.203_571_881_257_894_3;
/// The frozen threshold for 10 alerts a day (B5 11.8479…, B20 8.5290…).
pub const THRESHOLD_B10: f64 = 10.562_146_802_637_4;

pub struct PrecursorsR0;

impl RocketScorer for PrecursorsR0 {
    fn id(&self) -> &str {
        "precursors-R0"
    }
    fn version(&self) -> &str {
        "c009126"
    }
    fn score(&self, f: &Features) -> Option<f64> {
        let (range60, upc3, vr3) = (f.x[ix::RANGE60], f.x[ix::UPC3], f.x[ix::VR3]);
        // NaN compares false, so a missing feature fails the gate.
        (range60 >= RANGE60_MIN && upc3 <= UPC3_MAX && vr3 >= VR3_MIN).then_some(vr3)
    }
    fn fixed_threshold(&self) -> Option<f64> {
        Some(THRESHOLD_B10)
    }
    fn reasons(&self, f: &Features) -> Vec<String> {
        standard_reasons(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feats(range60: f64, upc3: f64, vr3: f64) -> Features {
        let mut f = Features {
            symbol: "TEST".into(),
            at_ns: 0,
            slot: 0,
            price: 1.0,
            session: None,
            prev_close: None,
            x: [f64::NAN; super::super::features::NF],
            ret30: f64::NAN,
            bar_volume: 0.0,
            bar_trades: 0.0,
            vol3: 0.0,
            vol3_prior: 0.0,
            day_high: 1.0,
            vwap: f64::NAN,
            minutes_in_session: 0.0,
        };
        f.x[ix::RANGE60] = range60;
        f.x[ix::UPC3] = upc3;
        f.x[ix::VR3] = vr3;
        f
    }

    #[test]
    fn the_frozen_gate_and_score() {
        let s = PrecursorsR0;
        assert_eq!(s.score(&feats(0.2, 0.25, 11.0)), Some(11.0));
        // Not in play, grinding, or volume dying: gated out.
        assert_eq!(s.score(&feats(0.1, 0.25, 11.0)), None);
        assert_eq!(s.score(&feats(0.2, 0.75, 11.0)), None);
        assert_eq!(s.score(&feats(0.2, 0.25, -0.5)), None);
        // Boundaries are inclusive, as frozen.
        assert_eq!(
            s.score(&feats(RANGE60_MIN, UPC3_MAX, VR3_MIN)),
            Some(VR3_MIN)
        );
        assert_eq!(s.fixed_threshold(), Some(THRESHOLD_B10));
        assert!(!s.placeholder());
    }

    #[test]
    fn a_missing_feature_fails_the_gate() {
        let s = PrecursorsR0;
        assert_eq!(s.score(&feats(f64::NAN, 0.25, 11.0)), None);
        assert_eq!(s.score(&feats(0.2, f64::NAN, 11.0)), None);
        assert_eq!(s.score(&feats(0.2, 0.25, f64::NAN)), None);
    }
}
