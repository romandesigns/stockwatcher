//! Observational MP5 candle diagnostic. It never gates a detector transition.
//! The candle is copied when H0 freezes, not looked up again when the episode arms.

use serde::Serialize;

/// A changed candidate rule must get a new id. These cutoffs are unvalidated.
pub const MP5_WICK_RULE: &str = "mp5-wick-v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WickStatus {
    Flagged,
    Clear,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MicroWick {
    pub rule: &'static str,
    pub status: WickStatus,
    pub bar_end: i64,
    pub known_at: i64,
    /// Frozen [open, high, low, close], in micro-dollars; missing stays missing.
    pub ohlc: Option<[i64; 4]>,
    pub upper_wick_milli: Option<i64>,
    pub close_location_milli: Option<i64>,
    pub reason: &'static str,
}

impl MicroWick {
    /// Flag >=50% upper wick AND a close in the lower half, using exact integer
    /// comparisons. Display ratios are rounded down, never used for classification.
    pub fn measure(ohlc: Option<[i64; 4]>, bar_end: i64, known_at: i64) -> Self {
        let mut d = Self {
            rule: MP5_WICK_RULE,
            status: WickStatus::Unknown,
            bar_end,
            known_at,
            ohlc,
            upper_wick_milli: None,
            close_location_milli: None,
            reason: "missing_anchor_candle",
        };
        let Some([o, h, l, c]) = ohlc else { return d };
        if known_at < bar_end {
            d.reason = "anchor_not_complete";
            return d;
        }
        if l <= 0 || h < l || o < l || o > h || c < l || c > h {
            d.reason = "invalid_anchor_candle";
            return d;
        }
        let range = h as i128 - l as i128;
        if range == 0 {
            d.reason = "zero_range";
            return d;
        }
        let wick = h as i128 - o.max(c) as i128;
        let close = c as i128 - l as i128;
        d.upper_wick_milli = Some((wick * 1000 / range) as i64);
        d.close_location_milli = Some((close * 1000 / range) as i64);
        let flagged = wick * 2 >= range && close * 2 <= range;
        d.status = if flagged {
            WickStatus::Flagged
        } else {
            WickStatus::Clear
        };
        d.reason = if flagged {
            "upper_wick_and_weak_close"
        } else {
            "candidate_rule_not_met"
        };
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_boundaries_and_strong_close_control() {
        let d = MicroWick::measure(Some([105, 120, 100, 110]), 15, 32);
        assert_eq!(d.status, WickStatus::Flagged);
        assert_eq!(
            (d.upper_wick_milli, d.close_location_milli),
            (Some(500), Some(500))
        );
        assert_eq!(
            MicroWick::measure(Some([111, 120, 100, 110]), 15, 32).status,
            WickStatus::Clear
        );
        assert_eq!(
            MicroWick::measure(Some([105, 120, 100, 111]), 15, 32).status,
            WickStatus::Clear
        );
        assert_eq!(
            MicroWick::measure(Some([100, 120, 100, 120]), 15, 32).status,
            WickStatus::Clear
        );
    }

    #[test]
    fn missing_incomplete_zero_range_and_invalid_are_unknown() {
        for (o, at) in [
            (None, 32),
            (Some([100, 120, 100, 105]), 14),
            (Some([100; 4]), 32),
            (Some([121, 120, 100, 105]), 32),
        ] {
            let d = MicroWick::measure(o, 15, at);
            assert_eq!(d.status, WickStatus::Unknown);
            assert!(d.upper_wick_milli.is_none() && d.close_location_milli.is_none());
        }
    }

    #[test]
    fn scaling_to_sub_dollar_prices_preserves_the_classification() {
        for scale in [1, 100, 10_000, 1_000_000] {
            let d = MicroWick::measure(
                Some([105 * scale, 120 * scale, 100 * scale, 108 * scale]),
                15,
                32,
            );
            assert_eq!(d.status, WickStatus::Flagged);
            assert_eq!(
                (d.upper_wick_milli, d.close_location_milli),
                (Some(600), Some(400))
            );
        }
    }
}
