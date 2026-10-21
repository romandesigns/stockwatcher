//! Exact, platform-independent arithmetic for the detectors.
//!
//! Medians are kept doubled (`median2 = 2 × median`) so an even count needs no rounding, and the
//! logarithm uses only IEEE-754 `+ − × ÷` in a fixed order (no `libm`), so live and replay on any
//! machine compute the same parts-per-million value bit for bit.

const LN2: f64 = std::f64::consts::LN_2;

/// `round(10⁶ · ln(num / den))` for positive `num`, `den`.
pub fn ln_ratio_ppm(num: i64, den: i64) -> i64 {
    debug_assert!(num > 0 && den > 0);
    let mut x = num as f64 / den as f64;
    let mut k = 0i32;
    // Range reduction by exact powers of two into [0.75, 1.5).
    while x >= 1.5 {
        x /= 2.0;
        k += 1;
    }
    while x < 0.75 {
        x *= 2.0;
        k -= 1;
    }
    // ln x = 2 atanh(y), y = (x − 1)/(x + 1), |y| ≤ 0.2: 30 terms are far below 1e-15.
    let y = (x - 1.0) / (x + 1.0);
    let y2 = y * y;
    let mut term = y;
    let mut sum = y;
    for n in 1..30 {
        term *= y2;
        sum += term / (2 * n + 1) as f64;
    }
    let ln = 2.0 * sum + k as f64 * LN2;
    (ln * 1_000_000.0).round() as i64
}

/// Twice the median (exact for an even count). `None` for an empty sample.
pub fn median2(v: &[i64]) -> Option<i64> {
    if v.is_empty() {
        return None;
    }
    let mut s = v.to_vec();
    s.sort_unstable();
    let n = s.len();
    Some(if n % 2 == 1 {
        2 * s[n / 2]
    } else {
        s[n / 2 - 1] + s[n / 2]
    })
}

/// `(2·median, 4·MAD)` of a sample: MAD = median |x − median|.
pub fn median_mad4(v: &[i64]) -> Option<(i64, i64)> {
    let m2 = median2(v)?;
    let dev: Vec<i64> = v.iter().map(|&x| (2 * x - m2).abs()).collect();
    Some((m2, median2(&dev)?))
}

/// `num / den` in basis points, truncated toward zero.
pub fn bp(num: i64, den: i64) -> i64 {
    ((num as i128 * 10_000) / den as i128) as i64
}

/// `num / den` in thousandths, truncated.
pub fn milli(num: i64, den: i64) -> i64 {
    ((num as i128 * 1000) / den as i128) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ln_matches_known_values() {
        assert_eq!(ln_ratio_ppm(101, 100), 9_950);
        assert_eq!(ln_ratio_ppm(100, 101), -9_950);
        assert_eq!(ln_ratio_ppm(1, 1), 0);
        assert_eq!(ln_ratio_ppm(2, 1), 693_147);
        assert_eq!(ln_ratio_ppm(10, 1), 2_302_585);
        assert_eq!(ln_ratio_ppm(1, 1000), -6_907_755);
        // Within one ppm of std's ln across a sweep (std is only the oracle here).
        for (a, b) in [
            (3_400_000, 3_000_000),
            (5_080_000, 5_000_000),
            (123_457, 99_999),
        ] {
            let want = ((a as f64 / b as f64).ln() * 1e6).round() as i64;
            assert!((ln_ratio_ppm(a, b) - want).abs() <= 1);
        }
    }

    #[test]
    fn medians_are_exact_for_even_counts() {
        assert_eq!(median2(&[1, 2, 3, 4]), Some(5));
        assert_eq!(median2(&[3, 1, 2]), Some(4));
        assert_eq!(median2(&[]), None);
        // [1,2,3,4]: median 2.5, deviations 1.5,0.5,0.5,1.5 → MAD 1.0 → 4·MAD = 4.
        assert_eq!(median_mad4(&[1, 2, 3, 4]), Some((5, 4)));
        assert_eq!(median_mad4(&[7, 7, 7]), Some((14, 0)));
    }
}
