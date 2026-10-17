//! Fixed-point prices.
//!
//! Prices are an `i64` count of **micro-dollars** (1e-6 USD). Why not `f64` or `rust_decimal`:
//!
//! - Candle OHLC and volume-profile bins are *keys* (`floor(price / bin) * bin`, equality of highs and
//!   lows across timeframes, per-tick volume maps). Binary floating point cannot represent 0.01 or
//!   0.0001 exactly, so two identical trades could land in different bins depending on the
//!   arithmetic path. Integers make every comparison and bin assignment exact.
//! - US equities print in whole cents at or above $1.00 and in up to four decimals below $1.00
//!   (Reg NMS sub-penny rule; the universe here starts at $0.15). Six decimals cover both with two
//!   digits of headroom for average-price prints.
//! - `i64` is `Copy`, hashable, ordered and dependency-free; `rust_decimal` would also be exact but
//!   is 16 bytes, slower, and buys nothing this range needs. `i64::MAX` micro-dollars is ~9.2e12 USD.
//!
//! Conversion to `f64` exists only for display on the wire ([`Price::to_f64`]); it is never used
//! for aggregation.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Micro-dollars per dollar.
pub const PRICE_SCALE: i64 = 1_000_000;
const DECIMALS: usize = 6;

/// A price in micro-dollars. Serialized as the bare integer (contract v1).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Price(i64);

impl Price {
    pub const ZERO: Price = Price(0);

    pub const fn from_micros(micros: i64) -> Self {
        Price(micros)
    }

    /// Whole cents, e.g. `from_cents(318)` is $3.18.
    pub const fn from_cents(cents: i64) -> Self {
        Price(cents * 10_000)
    }

    pub const fn micros(self) -> i64 {
        self.0
    }

    /// Rounds a provider's JSON float to the nearest micro-dollar. Providers send prices as JSON
    /// numbers; this is the single place they become exact. Returns `None` for NaN/inf/overflow.
    pub fn from_f64_rounded(v: f64) -> Option<Self> {
        if !v.is_finite() {
            return None;
        }
        let scaled = (v * PRICE_SCALE as f64).round();
        if scaled.abs() >= i64::MAX as f64 {
            return None;
        }
        Some(Price(scaled as i64))
    }

    /// Display-only conversion (the wire to clients uses JSON numbers). `micros / 1e6` is correctly
    /// rounded, so `Price::from_str("3.18").to_f64() == 3.18_f64`.
    pub fn to_f64(self) -> f64 {
        self.0 as f64 / PRICE_SCALE as f64
    }

    /// The deterministic bin origin used by the volume profile: `floor(price / bin) * bin`,
    /// anchored at zero so the same price lands in the same bin regardless of session or timeframe.
    /// `bin` must be positive.
    pub fn floor_to(self, bin: Price) -> Price {
        debug_assert!(bin.0 > 0, "bin width must be positive");
        Price(self.0.div_euclid(bin.0) * bin.0)
    }

    pub fn is_positive(self) -> bool {
        self.0 > 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PriceParseError(pub String);

impl fmt::Display for PriceParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid price {:?}: expected a decimal with at most 6 fractional digits",
            self.0
        )
    }
}

impl std::error::Error for PriceParseError {}

/// Exact decimal parse: `"3.18"`, `"0.1525"`, `"-1"`. More than six fractional digits is an error
/// rather than a silent rounding.
impl FromStr for Price {
    type Err = PriceParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || PriceParseError(s.to_string());
        let (neg, body) = match s.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, s),
        };
        let (int, frac) = match body.split_once('.') {
            Some((i, f)) => (i, f),
            None => (body, ""),
        };
        if int.is_empty() && frac.is_empty() {
            return Err(err());
        }
        if frac.len() > DECIMALS
            || !int.bytes().all(|b| b.is_ascii_digit())
            || !frac.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(err());
        }
        let int_v: i64 = if int.is_empty() {
            0
        } else {
            int.parse().map_err(|_| err())?
        };
        let mut frac_v: i64 = if frac.is_empty() {
            0
        } else {
            frac.parse().map_err(|_| err())?
        };
        for _ in frac.len()..DECIMALS {
            frac_v *= 10;
        }
        let v = int_v
            .checked_mul(PRICE_SCALE)
            .and_then(|v| v.checked_add(frac_v))
            .ok_or_else(err)?;
        Ok(Price(if neg { -v } else { v }))
    }
}

/// Shortest exact decimal with at least two fractional digits: `3.18`, `0.1525`, `12.00`.
impl fmt::Display for Price {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        let abs = self.0.unsigned_abs();
        let int = abs / PRICE_SCALE as u64;
        let frac = format!("{:06}", abs % PRICE_SCALE as u64);
        let trimmed = frac.trim_end_matches('0');
        let frac = if trimmed.len() < 2 {
            &frac[..2]
        } else {
            trimmed
        };
        write!(f, "{sign}{int}.{frac}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_is_exact() {
        assert_eq!("3.18".parse::<Price>().unwrap().micros(), 3_180_000);
        assert_eq!("0.1525".parse::<Price>().unwrap().micros(), 152_500);
        assert_eq!("25".parse::<Price>().unwrap().micros(), 25_000_000);
        assert_eq!(".5".parse::<Price>().unwrap().micros(), 500_000);
        assert_eq!("-1.000001".parse::<Price>().unwrap().micros(), -1_000_001);
        assert!("1.0000001".parse::<Price>().is_err());
        assert!("1e3".parse::<Price>().is_err());
        assert!("".parse::<Price>().is_err());
        assert!(".".parse::<Price>().is_err());
    }

    #[test]
    fn display_round_trips() {
        for s in ["3.18", "0.1525", "12.00", "0.15", "-1.000001"] {
            assert_eq!(s.parse::<Price>().unwrap().to_string(), s);
        }
    }

    #[test]
    fn f64_conversions() {
        assert_eq!(
            Price::from_f64_rounded(3.18),
            Some(Price::from_micros(3_180_000))
        );
        assert_eq!(
            Price::from_f64_rounded(0.1525),
            Some(Price::from_micros(152_500))
        );
        assert_eq!(Price::from_f64_rounded(f64::NAN), None);
        assert_eq!("3.18".parse::<Price>().unwrap().to_f64(), 3.18);
    }

    #[test]
    fn floor_to_bins() {
        let bin = Price::from_cents(5);
        assert_eq!(Price::from_cents(318).floor_to(bin), Price::from_cents(315));
        assert_eq!(Price::from_cents(315).floor_to(bin), Price::from_cents(315));
        assert_eq!(
            Price::from_micros(3_149_999).floor_to(bin),
            Price::from_cents(310)
        );
    }
}
