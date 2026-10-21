//! The detection policy (`policy/detection-v0.json`), embedded and identified by the SHA-256 of
//! its exact bytes like every other stock-context policy. A changed number is a new file and id.

use serde::Deserialize;
use sha2::{Digest, Sha256};

pub const DETECTION_V0: &str = include_str!("../../policy/detection-v0.json");

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Universe {
    /// The active profile (production band, inclusive): gates detection.
    pub active_band_min_micros: i64,
    pub active_band_max_micros: i64,
    /// The research band, recorded on every feature snapshot, never a gate.
    pub research_band_min_micros: i64,
    pub research_band_max_micros: i64,
    pub segments: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Timing {
    /// Must equal the pillar grid: detection reads the engine's own bars.
    pub bar_secs: i64,
    pub settle_ms: i64,
    /// A price older than this is stale for current assessments and micro arming.
    pub price_fresh_secs: i64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Baseline {
    pub source: String,
    pub window_slots: i64,
    pub min_valid_slots: i64,
    pub returns_max: usize,
    pub returns_min: usize,
    /// 1.4826 × 10⁴.
    pub mad_scale_e4: i64,
    pub s_floor_increments: i64,
    pub range_floor_increments: i64,
    pub increment_below_1_micros: i64,
    pub increment_at_or_above_1_micros: i64,
    pub resolution_proxy: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ContextRules {
    pub source: String,
    pub a_min_milli: i64,
    pub n_base_min: u64,
    pub reference_percentile: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct IgnitionRules {
    pub id: String,
    pub source: String,
    pub z_min_milli: i64,
    pub return_min_num: i64,
    pub return_min_den: i64,
    pub range_e_min_milli: i64,
    pub cooldown_false_bars: i64,
    pub cooldown_secs: i64,
    pub display_slots: i64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ExpansionRules {
    pub range_diag_milli: i64,
    pub body_diag_milli: i64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct MomentumRules {
    pub building_id: String,
    pub acceleration_id: String,
    /// Bars per comparison step (2 × 15 s = the handoff's 30 s).
    pub step_slots: i64,
    pub r_min_ppm: i64,
    pub a_min_ppm: i64,
    pub displacement_min_increments: i64,
    pub expiry_slots: i64,
    pub cooldown_slots: i64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct BreakoutRules {
    pub id: String,
    pub retention_slots: i64,
    pub extended_bp: i64,
    pub cooldown_slots: i64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct MicroRules {
    pub id: String,
    pub lookback_slots: i64,
    pub impulse_min_bp: i64,
    pub impulse_min_increments: i64,
    pub impulse_min_coverage_milli: i64,
    pub max_depth_milli: i64,
    pub min_pullback_bars: u32,
    pub formation_max_slots: i64,
    pub armed_max_slots: i64,
    pub extended_bp: i64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Lifecycle {
    pub transition_log_max: usize,
    pub recent_max: usize,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct DetectionPolicyBody {
    pub schema: u32,
    pub id: String,
    pub experimental: bool,
    pub alerts_enabled: bool,
    pub note: String,
    pub universe: Universe,
    pub timing: Timing,
    pub baseline: Baseline,
    pub context: ContextRules,
    pub ignition: IgnitionRules,
    pub expansion: ExpansionRules,
    pub momentum: MomentumRules,
    pub breakout: BreakoutRules,
    pub micro: MicroRules,
    pub lifecycle: Lifecycle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetectionPolicy {
    pub body: DetectionPolicyBody,
    pub hash: String,
}

impl DetectionPolicy {
    pub fn parse(text: &str) -> Result<DetectionPolicy, String> {
        let body: DetectionPolicyBody =
            serde_json::from_str(text).map_err(|e| format!("detection policy: {e}"))?;
        let (b, m, mo, i) = (&body.baseline, &body.micro, &body.momentum, &body.ignition);
        let ok = body.schema == 1
            && body.timing.bar_secs > 0
            && body.timing.settle_ms >= 0
            && body.timing.price_fresh_secs > 0
            && b.window_slots > 0
            && (1..=b.window_slots).contains(&b.min_valid_slots)
            && b.returns_min > 0
            && b.returns_max >= b.returns_min
            && b.mad_scale_e4 > 0
            && b.s_floor_increments > 0
            && b.range_floor_increments > 0
            && b.increment_below_1_micros > 0
            && b.increment_at_or_above_1_micros > 0
            && i.return_min_den > 0
            && i.return_min_num >= i.return_min_den
            && i.z_min_milli > 0
            && i.range_e_min_milli > 0
            && i.display_slots > 0
            && mo.step_slots > 0
            && mo.expiry_slots > 0
            && m.lookback_slots > 1
            && m.impulse_min_bp > 0
            && (0..=1000).contains(&m.max_depth_milli)
            && (0..=1000).contains(&m.impulse_min_coverage_milli)
            && m.min_pullback_bars > 0
            && m.formation_max_slots > 0
            && m.armed_max_slots > 0
            && body.breakout.retention_slots > 0
            && body.lifecycle.transition_log_max > 0
            && body.lifecycle.recent_max > 0
            && body.universe.active_band_min_micros > 0
            && body.universe.active_band_max_micros >= body.universe.active_band_min_micros;
        if !ok {
            return Err("detection policy: unsupported schema or invalid rules".into());
        }
        // Audio and push stay off for every experimental family.
        if body.alerts_enabled {
            return Err(
                "detection policy: alerts are not supported for experimental families".into(),
            );
        }
        let hash = Sha256::digest(text.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        Ok(DetectionPolicy { body, hash })
    }

    pub fn v0() -> DetectionPolicy {
        DetectionPolicy::parse(DETECTION_V0).expect("embedded detection-v0.json parses")
    }

    /// `detection-v0@<first 12 hex>`.
    pub fn tag(&self) -> String {
        format!("{}@{}", self.body.id, &self.hash[..12])
    }

    /// The price resolution proxy at `price` (micro-dollars), per the proposal's increment
    /// schedule. A versioned proxy, not a claim about the exchange's trade increment.
    pub fn increment(&self, price: i64) -> i64 {
        if price >= 1_000_000 {
            self.body.baseline.increment_at_or_above_1_micros
        } else {
            self.body.baseline.increment_below_1_micros
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v0_parses_is_experimental_and_hash_follows_bytes() {
        let p = DetectionPolicy::v0();
        assert!(p.body.experimental);
        assert!(!p.body.alerts_enabled);
        assert!(p.tag().starts_with("detection-v0@"));
        assert_eq!(p.body.ignition.z_min_milli, 4000);
        assert_eq!(p.body.baseline.returns_max, 80);
        assert_eq!(p.body.baseline.returns_min, 30);
        let edited = DETECTION_V0.replacen("\"z_min_milli\": 4000", "\"z_min_milli\": 3000", 1);
        assert_ne!(DetectionPolicy::parse(&edited).unwrap().hash, p.hash);
        let loud =
            DETECTION_V0.replacen("\"alerts_enabled\": false", "\"alerts_enabled\": true", 1);
        assert!(DetectionPolicy::parse(&loud).is_err());
        assert_eq!(p.increment(999_999), 100);
        assert_eq!(p.increment(1_000_000), 10_000);
    }
}
