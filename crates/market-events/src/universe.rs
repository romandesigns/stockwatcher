//! The Stockwatcher price universe: which stocks are candidates at all.
//!
//! Owner decision (2026-10-17): only stocks priced **$0.10 through $25.00 per share** (both ends
//! inclusive) are let through to anything the user sees. The spec's earlier $0.15 floor is
//! superseded. One definition lives here so the server, the provider simulator and any future
//! consumer cannot disagree about the band.
//!
//! The band is applied to a stock's latest price-qualifying trade at the current clock (replay
//! or live): a stock moves in and out of the candidate set as it trades through the edges.

use crate::Price;

/// An inclusive price band.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PriceBand {
    pub min: Price,
    pub max: Price,
}

impl PriceBand {
    /// $0.10 through $25.00.
    pub const DEFAULT: PriceBand = PriceBand {
        min: Price::from_cents(10),
        max: Price::from_cents(2500),
    };

    pub fn new(min: Price, max: Price) -> Result<Self, String> {
        if min.micros() <= 0 || max < min {
            return Err(format!("invalid price band {min}..={max}"));
        }
        Ok(PriceBand { min, max })
    }

    /// Inclusive on both ends: $0.10 and $25.00 are candidates.
    pub fn contains(&self, price: Price) -> bool {
        price >= self.min && price <= self.max
    }
}

impl Default for PriceBand {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_band_is_ten_cents_to_twenty_five_dollars_inclusive() {
        let b = PriceBand::DEFAULT;
        assert!(b.contains(Price::from_cents(10)));
        assert!(b.contains(Price::from_cents(2500)));
        assert!(b.contains(Price::from_micros(100_000)));
        assert!(!b.contains(Price::from_micros(99_999)));
        assert!(!b.contains(Price::from_micros(25_000_001)));
    }

    #[test]
    fn rejects_inverted_or_non_positive_bands() {
        assert!(PriceBand::new(Price::from_cents(500), Price::from_cents(100)).is_err());
        assert!(PriceBand::new(Price::from_cents(0), Price::from_cents(100)).is_err());
        assert!(PriceBand::new(Price::from_cents(10), Price::from_cents(10)).is_ok());
    }
}
