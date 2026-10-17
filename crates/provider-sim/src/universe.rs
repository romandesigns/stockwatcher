//! The Stockwatcher price universe, as defined once in `market_events::PriceBand`
//! ($0.10 through $25.00 per share, owner decision 2026-10-17).
//!
//! Alpaca's `/v2/assets` has no price filter, so the application filters assets by a price it
//! looks up (snapshots). This helper is that filter, shared by the simulator's `/sim/universe`.

use market_events::Price;

pub const UNIVERSE_MIN: Price = market_events::PriceBand::DEFAULT.min;
pub const UNIVERSE_MAX: Price = market_events::PriceBand::DEFAULT.max;

/// Inclusive on both ends: $0.10 and $25.00 are in the universe.
pub fn in_price_band(price: Price, min: Price, max: Price) -> bool {
    price >= min && price <= max
}

/// The symbols whose price is inside `[min, max]`, in input order.
pub fn filter_band<'a>(
    prices: impl IntoIterator<Item = (&'a str, Price)>,
    min: Price,
    max: Price,
) -> Vec<&'a str> {
    prices
        .into_iter()
        .filter(|(_, p)| in_price_band(*p, min, max))
        .map(|(s, _)| s)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_is_inclusive() {
        let p = |c| Price::from_cents(c);
        assert!(in_price_band(p(10), UNIVERSE_MIN, UNIVERSE_MAX));
        assert!(in_price_band(p(2500), UNIVERSE_MIN, UNIVERSE_MAX));
        assert!(!in_price_band(
            Price::from_micros(99_999),
            UNIVERSE_MIN,
            UNIVERSE_MAX
        ));
        assert!(!in_price_band(p(2501), UNIVERSE_MIN, UNIVERSE_MAX));
        assert_eq!(
            filter_band(
                [("A", p(100)), ("B", p(4000)), ("C", p(318))],
                UNIVERSE_MIN,
                UNIVERSE_MAX
            ),
            vec!["A", "C"]
        );
    }
}
