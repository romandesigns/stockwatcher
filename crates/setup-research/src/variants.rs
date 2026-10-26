//! The pre-registered variants (protocol section 8). Each one is the embedded default policy with
//! only the named fields changed, parsed by the production parsers, so a variant is exactly what
//! the engine would run if that file shipped.

use serde_json::{Value, json};
use stock_context::{DETECTION_V1, DetectionPolicy, SETUPS_V0, SetupPolicy};

fn patch(base: &str, section: &str, changes: &[(&str, i64)]) -> String {
    let mut v: Value = serde_json::from_str(base).expect("embedded policy is JSON");
    for (k, x) in changes {
        let slot = v
            .get_mut(section)
            .and_then(|s| s.get_mut(*k))
            .unwrap_or_else(|| panic!("{section}.{k} exists"));
        *slot = json!(x);
    }
    serde_json::to_string_pretty(&v).expect("serializes")
}

/// `(id, changes)` for `bull_flag_v0` (setups-v0 `bull_flag`).
pub const BULL: &[(&str, &[(&str, i64)])] = &[
    ("BF0", &[]),
    ("BF1", &[("impulse_min_bp", 500)]),
    ("BF2", &[("max_retrace_milli", 382)]),
    (
        "BF3",
        &[
            ("impulse_vol_mult_milli", 3000),
            ("confirm_vol_mult_milli", 1500),
        ],
    ),
    ("BF4", &[("flag_max_bars", 8)]),
    (
        "BF5",
        &[
            ("impulse_min_bp", 500),
            ("max_retrace_milli", 382),
            ("impulse_vol_mult_milli", 3000),
            ("confirm_vol_mult_milli", 1500),
        ],
    ),
];

/// `(id, changes)` for `micro_pullback_v0` (detection-v1 `micro`).
pub const MICRO: &[(&str, &[(&str, i64)])] = &[
    ("MP0", &[]),
    ("MP1", &[("impulse_min_bp", 500)]),
    ("MP2", &[("max_depth_milli", 382)]),
    ("MP3", &[("impulse_min_coverage_milli", 750)]),
    ("MP4", &[("armed_max_slots", 4)]),
    (
        "MP5",
        &[
            ("impulse_min_bp", 500),
            ("max_depth_milli", 382),
            ("impulse_min_coverage_milli", 750),
        ],
    ),
];

pub fn bull(ids: &[String]) -> Vec<(String, SetupPolicy)> {
    BULL.iter()
        .filter(|(id, _)| ids.is_empty() || ids.iter().any(|x| x == id))
        .map(|(id, ch)| {
            let p = if ch.is_empty() {
                SetupPolicy::v0()
            } else {
                SetupPolicy::parse(&patch(SETUPS_V0, "bull_flag", ch)).expect("variant parses")
            };
            (id.to_string(), p)
        })
        .collect()
}

pub fn micro(ids: &[String]) -> Vec<(String, DetectionPolicy)> {
    // "V2": the production policy, detection-v2 (micro_pullback_mp5), unpatched.
    if ids.iter().any(|x| x == "V2") {
        return vec![("V2".to_string(), DetectionPolicy::v2())];
    }
    MICRO
        .iter()
        .filter(|(id, _)| ids.is_empty() || ids.iter().any(|x| x == id))
        .map(|(id, ch)| {
            let p = if ch.is_empty() {
                DetectionPolicy::v1()
            } else {
                DetectionPolicy::parse(&patch(DETECTION_V1, "micro", ch)).expect("variant parses")
            };
            (id.to_string(), p)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_parses_and_changes_only_its_fields() {
        let b = bull(&[]);
        assert_eq!(b.len(), 6);
        assert_eq!(b[0].1, SetupPolicy::v0());
        assert_eq!(b[1].1.body.bull_flag.impulse_min_bp, 500);
        assert_eq!(
            b[1].1.body.bull_flag.near_bp,
            SetupPolicy::v0().body.bull_flag.near_bp
        );
        let m = micro(&[]);
        assert_eq!(m.len(), 6);
        assert_eq!(m[0].1, DetectionPolicy::v1());
        assert_eq!(m[5].1.body.micro.max_depth_milli, 382);
        assert_eq!(
            m[5].1.body.assessment,
            DetectionPolicy::v1().body.assessment
        );
        assert_eq!(m[4].1.body.ignition, DetectionPolicy::v1().body.ignition);
    }
}
