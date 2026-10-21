//! Three-valued pillar results and the pure measurements behind them.
//!
//! Every comparison is exact integer arithmetic on the stored units (micro-dollars, shares,
//! basis points, thousandths); the displayed values (`value_*`) are rounded copies and never take
//! part in a classification, so a display rounding cannot move a boundary.

use serde::Serialize;

/// PASS: admissible evidence satisfies the rule. FAIL: admissible evidence contradicts it.
/// UNKNOWN: the evidence the rule needs is missing, stale, ambiguous or outside covered sources.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Tri {
    Pass,
    Fail,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PillarId {
    Price,
    Gain,
    Rvol,
    Float,
    News,
}

impl PillarId {
    /// The wire name, the same string the serialized form uses.
    pub fn as_str(self) -> &'static str {
        match self {
            PillarId::Price => "price",
            PillarId::Gain => "gain",
            PillarId::Rvol => "rvol",
            PillarId::Float => "float",
            PillarId::News => "news",
        }
    }

    pub const ALL: [PillarId; 5] = [
        PillarId::Price,
        PillarId::Gain,
        PillarId::Rvol,
        PillarId::Float,
        PillarId::News,
    ];
}

/// One pillar's result at one decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PillarResult {
    pub pillar: PillarId,
    pub state: Tri,
    /// The measured value in `unit`, when measurable (an UNKNOWN can still carry a value, e.g. a
    /// price that is stale).
    pub value: Option<i64>,
    pub unit: &'static str,
    /// `">= 1000"`, `"in [2000000, 20000000]"`, …, in the same unit.
    pub rule: String,
    /// A stable code: `in_band`, `below_min`, `adjustment_coverage_unknown`, …
    pub reason: &'static str,
    /// The evidence record(s) the result rests on (id@version), empty for trade-derived pillars.
    pub evidence: Vec<String>,
    /// As-of time of the input (ns): the price's print, the float's retrieval, …
    pub as_of: Option<i64>,
}

impl PillarResult {
    pub fn new(
        pillar: PillarId,
        state: Tri,
        value: Option<i64>,
        unit: &'static str,
        rule: String,
        reason: &'static str,
    ) -> Self {
        PillarResult {
            pillar,
            state,
            value,
            unit,
            rule,
            reason,
            evidence: Vec::new(),
            as_of: None,
        }
    }

    pub fn with_evidence(mut self, ids: Vec<String>) -> Self {
        self.evidence = ids;
        self
    }

    pub fn as_of(mut self, t: Option<i64>) -> Self {
        self.as_of = t;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Qualification {
    Qualified,
    NotQualified,
    Undetermined,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub pass: u8,
    pub fail: u8,
    pub unknown: u8,
}

/// QUALIFIED if every pillar passes, NOT_QUALIFIED if any fails, else UNDETERMINED. The counts
/// are counts only, never a confidence.
pub fn aggregate(states: impl IntoIterator<Item = Tri>) -> (Qualification, Counts) {
    let mut c = Counts::default();
    for s in states {
        match s {
            Tri::Pass => c.pass += 1,
            Tri::Fail => c.fail += 1,
            Tri::Unknown => c.unknown += 1,
        }
    }
    let q = if c.fail > 0 {
        Qualification::NotQualified
    } else if c.unknown > 0 {
        Qualification::Undetermined
    } else {
        Qualification::Qualified
    };
    (q, c)
}

/// Price in band, inclusive. `stale` makes it UNKNOWN (the value is still reported).
pub fn price_pillar(price_micros: Option<i64>, stale: bool, min: i64, max: i64) -> PillarResult {
    let rule = format!("in [{min}, {max}]");
    match price_micros {
        None => PillarResult::new(
            PillarId::Price,
            Tri::Unknown,
            None,
            "micros",
            rule,
            "no_qualifying_trade",
        ),
        Some(p) if stale => PillarResult::new(
            PillarId::Price,
            Tri::Unknown,
            Some(p),
            "micros",
            rule,
            "price_stale",
        ),
        Some(p) if p < min => PillarResult::new(
            PillarId::Price,
            Tri::Fail,
            Some(p),
            "micros",
            rule,
            "below_min",
        ),
        Some(p) if p > max => PillarResult::new(
            PillarId::Price,
            Tri::Fail,
            Some(p),
            "micros",
            rule,
            "above_max",
        ),
        Some(p) => PillarResult::new(
            PillarId::Price,
            Tri::Pass,
            Some(p),
            "micros",
            rule,
            "in_band",
        ),
    }
}

/// Gain in basis points, truncated toward zero, for display; the comparison is exact:
/// `P / C - 1 >= g / 10000`  ⇔  `P * 10000 >= C * (10000 + g)`.
pub fn gain_bp(price: i64, close: i64) -> i64 {
    (((price as i128 - close as i128) * 10_000) / close as i128) as i64
}

pub fn gain_passes(price: i64, close: i64, min_bp: i64) -> bool {
    price as i128 * 10_000 >= close as i128 * (10_000 + min_bp as i128)
}

/// `numerator / (sum / n)` as an exact comparison against `min_milli / 1000`, and its value in
/// thousandths (truncated) for display. `None` when the denominator is zero: unavailable, never
/// infinite.
pub fn ratio_vs_mean(numerator: u64, sum: u64, n: usize) -> Option<u64> {
    if sum == 0 || n == 0 {
        return None;
    }
    Some(((numerator as u128 * n as u128 * 1000) / sum as u128) as u64)
}

pub fn ratio_passes(numerator: u64, sum: u64, n: usize, min_milli: u64) -> bool {
    sum > 0 && numerator as u128 * n as u128 * 1000 >= min_milli as u128 * sum as u128
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregation_is_three_valued() {
        use Tri::*;
        assert_eq!(aggregate([Pass; 5]).0, Qualification::Qualified);
        // A failure wins over unknowns, and the unknown stays visible in the counts.
        let (q, c) = aggregate([Pass, Pass, Pass, Fail, Unknown]);
        assert_eq!(q, Qualification::NotQualified);
        assert_eq!(
            c,
            Counts {
                pass: 3,
                fail: 1,
                unknown: 1
            }
        );
        assert_eq!(
            aggregate([Pass, Pass, Unknown, Pass, Pass]).0,
            Qualification::Undetermined
        );
    }

    #[test]
    fn price_band_is_inclusive_and_exact() {
        let (lo, hi) = (2_000_000, 20_000_000);
        assert_eq!(
            price_pillar(Some(2_000_000), false, lo, hi).state,
            Tri::Pass
        );
        assert_eq!(
            price_pillar(Some(1_999_999), false, lo, hi).state,
            Tri::Fail
        );
        assert_eq!(
            price_pillar(Some(20_000_000), false, lo, hi).state,
            Tri::Pass
        );
        // $20.000001 would display as "20.00" but is above the band.
        assert_eq!(
            price_pillar(Some(20_000_001), false, lo, hi).reason,
            "above_max"
        );
        assert_eq!(
            price_pillar(Some(5_000_000), true, lo, hi).state,
            Tri::Unknown
        );
        assert_eq!(
            price_pillar(None, false, lo, hi).reason,
            "no_qualifying_trade"
        );
    }

    #[test]
    fn gain_boundary_is_exact() {
        // Exactly +10 % passes; a micro-dollar less does not, though both display as 10.00 %.
        assert!(gain_passes(1_100_000, 1_000_000, 1000));
        assert!(!gain_passes(1_099_999, 1_000_000, 1000));
        assert_eq!(gain_bp(1_099_999, 1_000_000), 999);
        assert_eq!(gain_bp(900_000, 1_000_000), -1000);
    }

    #[test]
    fn ratio_boundary_zero_and_display() {
        // 5.0x exactly: today 500, 30 sessions summing 3000 (mean 100).
        assert!(ratio_passes(500, 3000, 30, 5000));
        assert!(!ratio_passes(499, 3000, 30, 5000));
        assert_eq!(ratio_vs_mean(499, 3000, 30), Some(4990));
        assert_eq!(ratio_vs_mean(10, 0, 30), None);
        assert!(!ratio_passes(10, 0, 30, 5000));
    }
}
