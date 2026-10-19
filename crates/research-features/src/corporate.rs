//! Splits and renames for cross-session research values (`eligibility-research-v1`).
//!
//! - **Contemporaneous** values (band membership, intraday candles, VWAP, an indicator anchored at
//!   the day's start) use **raw** prices: within one trading date every print is already on the
//!   same share basis.
//! - **Cross-session** values (previous close, gap %, multi-day returns, indicator warmup carried
//!   from an earlier date) put the earlier prices on the later date's share basis: every split
//!   whose ex-date is in `(earlier date, later date]` multiplies prices by `old / new` and volumes
//!   by `new / old` ([`adjust_price`], [`adjust_volume`]).
//! - **A split day must not manufacture a setup.** On a symbol's ex-date the raw previous close is
//!   on the old basis (a 1-for-10 reverse split looks like +900 %). [`cross_session_reference`]
//!   never returns a raw reference across a split: it returns the adjusted previous close when
//!   corporate actions cover the interval, and **nothing** (feature unavailable) when they do not,
//!   so missing corporate-action data degrades to a missing feature, never to a fake gap. Every
//!   symbol-date with a split ex-date is also flagged `split_day` so research can stratify.
//! - **Renames** link an instrument's history across symbols: the previous close of `NEW` on its
//!   rename date is `OLD`'s ([`predecessor_on`]).
//! - **Delistings** end a symbol's data: nothing is forward-filled; a label horizon past the last
//!   print is censored. Limitation: the current archive's universe comes from today's active
//!   asset list, so symbols delisted before the import are absent (survivorship); see the doc.

use chrono::NaiveDate;
use market_events::Price;
use serde::{Deserialize, Serialize};

/// (Not serde: `chrono` is built without its `serde` feature in this workspace.)
///
/// A split as Alpaca's corporate-actions API reports it: `new_rate` shares for every `old_rate`.
/// Forward 2-for-1: old 1, new 2. Reverse 1-for-10: old 10, new 1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Split {
    pub symbol: String,
    pub ex_date: NaiveDate,
    pub old_rate: u64,
    pub new_rate: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rename {
    pub old_symbol: String,
    pub new_symbol: String,
    /// First trading date under the new symbol.
    pub effective: NaiveDate,
}

/// Corporate actions known for a date range. `covered` is the inclusive range of dates whose
/// actions were fetched; outside it nothing can be assumed (not even "no split").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CorporateActions {
    pub covered: Option<(NaiveDate, NaiveDate)>,
    pub splits: Vec<Split>,
    pub renames: Vec<Rename>,
}

/// An exact rational factor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ratio {
    pub num: u128,
    pub den: u128,
}

impl Ratio {
    pub const ONE: Ratio = Ratio { num: 1, den: 1 };
}

impl CorporateActions {
    /// Whether every date in `(from, to]` is covered.
    pub fn covers(&self, from: NaiveDate, to: NaiveDate) -> bool {
        match self.covered {
            Some((a, b)) => from.succ_opt().is_some_and(|f| a <= f) && to <= b,
            None => false,
        }
    }

    pub fn is_split_day(&self, symbol: &str, date: NaiveDate) -> bool {
        self.splits
            .iter()
            .any(|s| s.symbol == symbol && s.ex_date == date)
    }

    /// The **price** factor taking `from`'s basis to `to`'s for `symbol` (as traded on `to`;
    /// renames are followed backwards). `None` when the range is not covered.
    pub fn price_factor(&self, symbol: &str, from: NaiveDate, to: NaiveDate) -> Option<Ratio> {
        if !self.covers(from, to) {
            return None;
        }
        let mut f = Ratio::ONE;
        for s in &self.splits {
            if s.ex_date > from
                && s.ex_date <= to
                && self.same_instrument(&s.symbol, s.ex_date, symbol, to)
            {
                f.num *= s.old_rate as u128;
                f.den *= s.new_rate as u128;
            }
        }
        Some(f)
    }

    /// Whether `a` on `a_date` and `b` on `b_date` (later) are the same instrument, following
    /// renames between the dates.
    fn same_instrument(&self, a: &str, a_date: NaiveDate, b: &str, b_date: NaiveDate) -> bool {
        let mut sym = b.to_string();
        let mut date = b_date;
        loop {
            if sym == a && a_date <= b_date {
                return true;
            }
            match self
                .renames
                .iter()
                .filter(|r| r.new_symbol == sym && r.effective <= date && r.effective > a_date)
                .max_by_key(|r| r.effective)
            {
                Some(r) => {
                    sym = r.old_symbol.clone();
                    date = r.effective.pred_opt().unwrap_or(r.effective);
                }
                None => return false,
            }
        }
    }
}

/// The symbol under which the instrument trading as `symbol` on `date` traded on `prev_date`.
pub fn predecessor_on(
    ca: &CorporateActions,
    symbol: &str,
    date: NaiveDate,
    prev_date: NaiveDate,
) -> String {
    let mut sym = symbol.to_string();
    let mut d = date;
    while let Some(r) = ca
        .renames
        .iter()
        .filter(|r| r.new_symbol == sym && r.effective <= d && r.effective > prev_date)
        .max_by_key(|r| r.effective)
    {
        sym = r.old_symbol.clone();
        d = r.effective.pred_opt().unwrap_or(r.effective);
    }
    sym
}

/// `price × factor`, rounded to the nearest micro-dollar (ties away from zero).
pub fn adjust_price(p: Price, f: Ratio) -> Price {
    let n = p.micros() as i128 * f.num as i128;
    let d = f.den as i128;
    let q = (2 * n + d.signum() * d) / (2 * d);
    Price::from_micros(q as i64)
}

/// `volume / factor` (shares scale inversely to price), rounded to the nearest share.
pub fn adjust_volume(v: u64, f: Ratio) -> u64 {
    let n = v as u128 * f.den;
    ((2 * n + f.num) / (2 * f.num)) as u64
}

/// The previous-session reference a cross-session feature may use on `date`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossSession {
    /// The previous close on `date`'s share basis; `None` when it cannot be trusted
    /// (corporate actions not covered, or no raw close).
    pub prev_close: Option<Price>,
    pub split_day: bool,
    pub covered: bool,
    /// The symbol whose close was used (differs on a rename).
    pub prev_symbol: String,
}

/// Previous-close reference for `symbol` on `date`, given the raw close on `prev_date` of the
/// instrument's previous symbol (look it up with [`predecessor_on`]).
pub fn cross_session_reference(
    ca: &CorporateActions,
    symbol: &str,
    date: NaiveDate,
    prev_date: NaiveDate,
    raw_prev_close: Option<Price>,
) -> CrossSession {
    let factor = ca.price_factor(symbol, prev_date, date);
    CrossSession {
        prev_close: match (raw_prev_close, factor) {
            (Some(p), Some(f)) => Some(adjust_price(p, f)),
            _ => None,
        },
        split_day: ca.is_split_day(symbol, date),
        covered: factor.is_some(),
        prev_symbol: predecessor_on(ca, symbol, date, prev_date),
    }
}

/// Whether a label horizon ending at `horizon_end` is censored by the symbol's data ending
/// (delisting, or the end of the archive) at `last_data`.
pub fn censored<T: PartialOrd>(horizon_end: T, last_data: T) -> bool {
    horizon_end > last_data
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, m, day).unwrap()
    }

    fn ca() -> CorporateActions {
        CorporateActions {
            covered: Some((d(1, 1), d(12, 31))),
            splits: vec![
                // 1-for-10 reverse split of RVS on Oct 15.
                Split {
                    symbol: "RVS".into(),
                    ex_date: d(10, 15),
                    old_rate: 10,
                    new_rate: 1,
                },
                // 3-for-2 forward split of FWD on Oct 15.
                Split {
                    symbol: "FWD".into(),
                    ex_date: d(10, 15),
                    old_rate: 2,
                    new_rate: 3,
                },
                // OLDN splits before its rename to NEWN.
                Split {
                    symbol: "OLDN".into(),
                    ex_date: d(10, 13),
                    old_rate: 1,
                    new_rate: 2,
                },
            ],
            renames: vec![Rename {
                old_symbol: "OLDN".into(),
                new_symbol: "NEWN".into(),
                effective: d(10, 15),
            }],
        }
    }

    #[test]
    fn reverse_split_day_does_not_manufacture_a_gap() {
        let c = ca();
        let r =
            cross_session_reference(&c, "RVS", d(10, 15), d(10, 14), Some(Price::from_cents(8)));
        // $0.08 on the old basis = $0.80 on the new: a print at $0.80 is a 0 % gap, not +900 %.
        assert_eq!(r.prev_close, Some(Price::from_cents(80)));
        assert!(r.split_day && r.covered);
        // The day after: no split in (Oct 15, Oct 16], factor 1.
        let r =
            cross_session_reference(&c, "RVS", d(10, 16), d(10, 15), Some(Price::from_cents(80)));
        assert_eq!(r.prev_close, Some(Price::from_cents(80)));
        assert!(!r.split_day);
    }

    #[test]
    fn forward_split_and_rounding() {
        let c = ca();
        let f = c.price_factor("FWD", d(10, 14), d(10, 15)).unwrap();
        assert_eq!(f, Ratio { num: 2, den: 3 });
        // $10.00 * 2/3 = $6.666667 (rounded to the micro-dollar).
        assert_eq!(
            adjust_price(Price::from_cents(1000), f),
            Price::from_micros(6_666_667)
        );
        assert_eq!(adjust_volume(1000, f), 1500);
        // Multi-day span covering the split.
        assert_eq!(c.price_factor("FWD", d(10, 1), d(10, 20)), Some(f));
        // Span not including it.
        assert_eq!(
            c.price_factor("FWD", d(10, 15), d(10, 20)),
            Some(Ratio::ONE)
        );
    }

    #[test]
    fn missing_coverage_degrades_to_no_feature() {
        let mut c = ca();
        c.covered = Some((d(10, 16), d(12, 31)));
        let r =
            cross_session_reference(&c, "RVS", d(10, 15), d(10, 14), Some(Price::from_cents(8)));
        assert_eq!(r.prev_close, None);
        assert!(!r.covered);
        let none = CorporateActions::default();
        assert_eq!(none.price_factor("X", d(10, 14), d(10, 15)), None);
    }

    #[test]
    fn renames_link_history_and_carry_splits() {
        let c = ca();
        assert_eq!(predecessor_on(&c, "NEWN", d(10, 15), d(10, 14)), "OLDN");
        assert_eq!(predecessor_on(&c, "NEWN", d(10, 16), d(10, 15)), "NEWN");
        // NEWN on Oct 15 vs OLDN on Oct 12: OLDN's Oct 13 split applies.
        assert_eq!(
            c.price_factor("NEWN", d(10, 12), d(10, 15)),
            Some(Ratio { num: 1, den: 2 })
        );
        let r = cross_session_reference(
            &c,
            "NEWN",
            d(10, 15),
            d(10, 14),
            Some(Price::from_cents(500)),
        );
        assert_eq!(r.prev_symbol, "OLDN");
        assert_eq!(r.prev_close, Some(Price::from_cents(500)));
    }

    #[test]
    fn censoring() {
        assert!(censored(10, 5));
        assert!(!censored(5, 5));
    }
}
