//! Session volume profile.
//!
//! Volume is stored **per exact price** (a micro-dollar tick map), not per bin, so any bin width
//! can be produced exactly at read time: re-binning never estimates. Bins are anchored at zero:
//! a price `p` falls in bin `floor(p / bin) * bin`, independent of timeframe and of the first
//! trade's price.

use market_events::Price;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TickProfile {
    ticks: BTreeMap<i64, u64>,
    total: u64,
}

impl TickProfile {
    pub fn add(&mut self, price: Price, size: u64) {
        if size == 0 {
            return;
        }
        *self.ticks.entry(price.micros()).or_insert(0) += size;
        self.total += size;
    }

    /// Removes volume previously added at `price` (cancel/correction). Saturates rather than
    /// underflowing; an emptied tick is dropped so it never shows as a zero row.
    pub fn sub(&mut self, price: Price, size: u64) {
        if let Some(v) = self.ticks.get_mut(&price.micros()) {
            let take = size.min(*v);
            *v -= take;
            self.total -= take;
            if *v == 0 {
                self.ticks.remove(&price.micros());
            }
        }
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    pub fn is_empty(&self) -> bool {
        self.ticks.is_empty()
    }

    /// Exact volume of the bin of width `bin` containing `price`.
    pub fn bin_total(&self, bin: Price, price: Price) -> u64 {
        let lo = price.floor_to(bin).micros();
        self.ticks
            .range(lo..lo + bin.micros())
            .map(|(_, v)| v)
            .sum()
    }

    /// Non-empty bins of width `bin`, ascending.
    pub fn rows(&self, bin: Price) -> Vec<(Price, u64)> {
        let mut out: Vec<(Price, u64)> = Vec::new();
        for (&p, &v) in &self.ticks {
            let b = Price::from_micros(p).floor_to(bin);
            match out.last_mut() {
                Some((last, acc)) if *last == b => *acc += v,
                _ => out.push((b, v)),
            }
        }
        out
    }
}

/// Point of control: the bin with the most volume; the lowest price wins a tie.
pub fn poc(rows: &[(Price, u64)]) -> Option<Price> {
    poc_index(rows).map(|i| rows[i].0)
}

fn poc_index(rows: &[(Price, u64)]) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (i, (_, v)) in rows.iter().enumerate() {
        if best.is_none_or(|b| *v > rows[b].1) {
            best = Some(i);
        }
    }
    best
}

/// Value area containing at least `percent`% of the volume: start at the POC and repeatedly add
/// the adjacent non-empty bin (above or below) with more volume; on a tie, the bin above. Returns
/// `(low bin, high bin, volume inside)`. Works on the non-empty rows, so empty bins inside the
/// range are skipped over, not counted.
pub fn value_area(rows: &[(Price, u64)], percent: u32) -> Option<(Price, Price, u64)> {
    let poc = poc_index(rows)?;
    let total: u128 = rows.iter().map(|(_, v)| *v as u128).sum();
    let target = (total * percent as u128).div_ceil(100);
    let (mut lo, mut hi) = (poc, poc);
    let mut acc = rows[poc].1 as u128;
    while acc < target {
        let up = rows.get(hi + 1).map(|r| r.1);
        let down = lo.checked_sub(1).map(|i| rows[i].1);
        match (up, down) {
            (Some(u), Some(d)) if u >= d => {
                hi += 1;
                acc += u as u128;
            }
            (Some(_), Some(d)) | (None, Some(d)) => {
                lo -= 1;
                acc += d as u128;
            }
            (Some(u), None) => {
                hi += 1;
                acc += u as u128;
            }
            (None, None) => break,
        }
    }
    Some((rows[lo].0, rows[hi].0, acc as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(cents: i64) -> Price {
        Price::from_cents(cents)
    }

    #[test]
    fn rebinning_is_exact() {
        let mut p = TickProfile::default();
        p.add(Price::from_micros(3_181_200), 100); // 3.1812
        p.add(c(318), 50);
        p.add(c(319), 25);
        p.add(c(321), 10);
        assert_eq!(
            p.rows(c(1)),
            vec![(c(318), 150), (c(319), 25), (c(321), 10)]
        );
        assert_eq!(p.rows(c(5)), vec![(c(315), 175), (c(320), 10)]);
        assert_eq!(
            p.rows(Price::from_micros(100)),
            vec![
                (c(318), 50),
                (Price::from_micros(3_181_200), 100),
                (c(319), 25),
                (c(321), 10)
            ]
        );
        assert_eq!(p.bin_total(c(5), c(316)), 175);
        assert_eq!(p.total(), 185);
        p.sub(c(318), 50);
        assert_eq!(
            p.rows(c(1)),
            vec![(c(318), 100), (c(319), 25), (c(321), 10)]
        );
        p.sub(Price::from_micros(3_181_200), 100);
        assert_eq!(p.rows(c(1)), vec![(c(319), 25), (c(321), 10)]);
    }

    #[test]
    fn poc_and_value_area() {
        let rows = vec![
            (c(100), 10),
            (c(101), 30),
            (c(102), 40),
            (c(103), 15),
            (c(104), 5),
        ];
        assert_eq!(poc(&rows), Some(c(102)));
        // total 100, target 70: 40 → +30 (below, 30 > 15) = 70.
        assert_eq!(value_area(&rows, 70), Some((c(101), c(102), 70)));
        // Tie at the POC picks the lower price.
        assert_eq!(poc(&[(c(1), 5), (c(2), 5)]), Some(c(1)));
        assert_eq!(value_area(&[], 70), None);
        assert_eq!(value_area(&[(c(5), 9)], 70), Some((c(5), c(5), 9)));
    }
}
