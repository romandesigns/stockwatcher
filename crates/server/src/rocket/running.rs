//! The "running" entry into Runners (owner, 2026-10-22): every stock in `$0.10–$25` that IS
//! running, by what it is doing, not by a prediction. R2 flags and bursts are forecasts with a
//! daily budget; on 2026-10-22 they missed TANH until $24.61 (it ran from $8 to $28), MLTX
//! ($10.40 → $13.33 in half an hour), XLAB's afternoon breakout and CJMB's +95 %.
//!
//! # The rule
//!
//! At each completed 15 s barrier, a symbol whose barrier bar traded enters when all hold:
//!
//! 1. **Price-relative move.** `close / base_low − 1 ≥ req`, where `base_low` is the lowest low
//!    of the last 60 minutes and `req = clamp(K × vol, FLOOR, CAP)`. `vol` is the symbol's own
//!    normal volatility: the median daily range `(high − low) / low` over its last 20 raw daily
//!    bars ([`facts`]). So a +8 % hour on a $10 stock whose days span 5 % counts like +25 % on a
//!    $0.30 stock whose days span 25 %. A symbol with fewer than 10 daily bars (a new listing)
//!    needs `CAP`.
//! 2. **A fresh high** on 15 s bars: the barrier bar's high is above every bar of the hour
//!    before it, so the stock is breaking out of its recent range. Not the session high: TANH
//!    on 2026-10-22 ran from an $8 base under its morning high, and a session-high rule held it
//!    back to $12.15 at 14:04 (the hour high lets it in at $11.49 at 13:56); on the calibration
//!    days the hour high also caught more legs (61.9 % against 59.6 %) for two more entries a
//!    day.
//! 3. **Real money.** At least `DOLLARS5` traded in the last 5 minutes, and the last 30
//!    minutes' volume at least `RVOL30` × the symbol's 30-minute share of its ADV30 (skipped when
//!    the ADV is unknown).
//! 4. **In band** `$0.10–$25` at the close, and the symbol has no row today (a running entry,
//!    an R2 flag or a burst): one row per symbol a day; its momentum states follow the later
//!    legs.
//!
//! Causal: everything is read from bars completed by the barrier and facts from before the day.
//! Cheap: one pass over the ring (≤ 256 slots) for the symbols whose barrier bar traded, the
//! same pass Runners already makes; no state beyond the session high on the tape's `Sym`.
//!
//! # Calibration
//!
//! `running::calib` (ignored, run by hand) replays archive days through the production chart
//! engine and the tape, and reports entries per day and the share of the day's in-band +20 %
//! legs (`reports/rockets/events-g20`) entered, with the entry point as a fraction of the leg.
//! Set on 9 archive days (2026-09-22 to 10-02, every symbol whose daily bar touched the band,
//! ~6,600 a day, replayed through the production chart engine): **42 entries a day median, p90
//! 48** (min 22, max 48). Of the days' 733 in-band +20 % legs, **61.9 % had a running row by
//! their peak** (32.5 % entered inside the leg itself, the rest earlier the same day), and an
//! entry inside a leg came a median **61 % of the way** from the leg's start to its peak: this
//! confirms a move under way, it does not call the bottom. The loosest settings that keep about
//! 40 a day were picked; a dollar floor of $25k with a 3× volume ratio would enter TANH at
//! $11.10 at 13:43 but at ~120 entries a day. On 2026-10-22: SHFS 04:38 $0.244, XLAB 07:19 $3.53
//! … 10:18 $3.87, CJMB 08:15 $0.54, MLTX 09:58 $12.77 (after its 09:30–09:45 stair-step: its
//! first 15 minutes traded under 2× its usual 30-minute volume), TANH 13:56 $11.49.

use std::collections::HashMap;

use chrono::NaiveDate;
use market_archive::Archive;

use super::features::RunInputs;

pub const STRATEGY: &str = "running";
pub const VERSION: &str = "v1";

/// Band, dollars.
pub const BAND: (f64, f64) = (0.10, 25.0);
/// `req = clamp(K × vol, FLOOR, CAP)` (calibration: module docs).
pub const K: f64 = 2.5;
pub const FLOOR: f64 = 0.10;
pub const CAP: f64 = 0.25;
/// Dollars in the last 5 minutes.
pub const DOLLARS5: f64 = 100_000.0;
/// Last 30 minutes' volume against ADV30 × 30 / 390.
pub const RVOL30: f64 = 5.0;
/// A runaway guard: at most this many running entries a day.
pub const MAX_PER_DAY: usize = 150;

/// The per-symbol facts the rule needs, from the archive's raw daily bars before the day.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Fact {
    /// Median daily range `(h − l) / l` over the last 20 raw daily bars (≥ 10 of them).
    pub vol: Option<f64>,
    /// Mean daily volume over the last 30 (≥ 10), the burst detector's ADV30 rule.
    pub adv: Option<f64>,
}

/// Why a symbol entered: the row's reasons.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub price: f64,
    pub base_low: f64,
    pub gain: f64,
    pub req: f64,
    pub dollars5: f64,
    /// `None` without an ADV.
    pub rvol30: Option<f64>,
}

impl Entry {
    pub fn reasons(&self) -> Vec<String> {
        let mut v = vec![
            format!(
                "+{:.0}% off the 60-min low (needs {:.0}%)",
                self.gain * 100.0,
                self.req * 100.0
            ),
            "new 1-hour high".to_string(),
            format!("${} in 5 min", short_dollars(self.dollars5)),
        ];
        if let Some(r) = self.rvol30 {
            v.push(format!("{r:.1}× usual volume"));
        }
        v
    }
}

fn short_dollars(x: f64) -> String {
    if x >= 1e6 {
        format!("{:.1}M", x / 1e6)
    } else {
        format!("{:.0}k", x / 1e3)
    }
}

/// The required move for a symbol's facts.
pub fn required(f: Option<&Fact>) -> f64 {
    f.and_then(|f| f.vol)
        .map_or(CAP, |v| (K * v).clamp(FLOOR, CAP))
}

/// The rule (module docs) on one barrier's inputs. `None`: no entry.
pub(crate) fn decide(x: &RunInputs, f: Option<&Fact>) -> Option<Entry> {
    if !(BAND.0..=BAND.1).contains(&x.price) || !x.fresh_high60 || x.dollars5 < DOLLARS5 {
        return None;
    }
    if x.base_low.is_nan() || x.base_low <= 0.0 {
        return None;
    }
    let gain = x.price / x.base_low - 1.0;
    let req = required(f);
    if gain < req {
        return None;
    }
    let rvol30 = f
        .and_then(|f| f.adv)
        .filter(|a| *a > 0.0)
        .map(|a| x.vol30 / (a * 30.0 / 390.0));
    if rvol30.is_some_and(|r| r < RVOL30) {
        return None;
    }
    Some(Entry {
        price: x.price,
        base_low: x.base_low,
        gain,
        req,
        dollars5: x.dollars5,
        rvol30,
    })
}

/// [`Fact`]s for every symbol with enough raw daily bars before `day`. Reads up to 30 files of
/// `daily-bars/raw/` once per trading day, off the ingest path.
pub fn facts(archive: &Archive, day: NaiveDate) -> HashMap<String, Fact> {
    let dates: Vec<NaiveDate> = archive
        .raw_bar_dates()
        .unwrap_or_default()
        .into_iter()
        .filter(|d| *d < day)
        .collect();
    let tail = &dates[dates.len().saturating_sub(30)..];
    let n = tail.len();
    let mut vols: HashMap<String, Vec<f64>> = HashMap::new();
    let mut ranges: HashMap<String, Vec<f64>> = HashMap::new();
    for (i, d) in tail.iter().enumerate() {
        let Some(b) = archive.read_raw_bars(*d).ok().flatten() else {
            continue;
        };
        for (s, bar) in &b.bars {
            if let Some(v) = bar.v {
                vols.entry(s.clone()).or_default().push(v as f64);
            }
            if i + 20 >= n
                && let (Some(h), Some(l)) = (bar.h, bar.l)
                && l > 0.0
                && h >= l
            {
                ranges.entry(s.clone()).or_default().push((h - l) / l);
            }
        }
    }
    let mut out: HashMap<String, Fact> = HashMap::new();
    for (s, v) in vols {
        if v.len() >= 10 {
            out.entry(s).or_default().adv = Some(v.iter().sum::<f64>() / v.len() as f64);
        }
    }
    for (s, mut r) in ranges {
        if r.len() >= 10 {
            r.sort_by(f64::total_cmp);
            let m = r.len() / 2;
            let med = if r.len() % 2 == 1 {
                r[m]
            } else {
                (r[m - 1] + r[m]) / 2.0
            };
            out.entry(s).or_default().vol = Some(med);
        }
    }
    out
}

#[cfg(test)]
mod calib;

#[cfg(test)]
mod tests {
    use super::*;

    fn x(price: f64, base_low: f64) -> RunInputs {
        RunInputs {
            price,
            fresh_high: true,
            fresh_high60: true,
            base_low,
            dollars5: 200_000.0,
            vol30: 1e6,
        }
    }

    /// Price-relative: +13 % enters a quiet $10 stock and not a wild $0.30 one.
    #[test]
    fn the_required_move_scales_with_the_symbols_own_range() {
        let quiet = Fact {
            vol: Some(0.05),
            adv: Some(1e6),
        };
        let wild = Fact {
            vol: Some(0.25),
            adv: Some(1e6),
        };
        assert!(decide(&x(11.3, 10.0), Some(&quiet)).is_some());
        assert!(decide(&x(11.2, 10.0), Some(&quiet)).is_none());
        assert!(decide(&x(0.339, 0.30), Some(&wild)).is_none());
        assert!(decide(&x(0.38, 0.30), Some(&wild)).is_some());
        // The floor: a stock that barely moves still needs +10 %.
        let flat = Fact {
            vol: Some(0.01),
            adv: Some(1e6),
        };
        assert!(decide(&x(10.9, 10.0), Some(&flat)).is_none());
        assert!(decide(&x(11.1, 10.0), Some(&flat)).is_some());
        // No facts: the cap.
        assert!(decide(&x(12.4, 10.0), None).is_none());
        assert!(decide(&x(12.6, 10.0), None).is_some());
    }

    #[test]
    fn it_needs_a_fresh_high_money_and_the_band() {
        let f = Fact {
            vol: Some(0.05),
            adv: Some(1e6),
        };
        let mut i = x(11.5, 10.0);
        assert!(decide(&i, Some(&f)).is_some());
        // Under an earlier session high is fine; under the last hour's high is not.
        i.fresh_high = false;
        assert!(decide(&i, Some(&f)).is_some());
        i.fresh_high60 = false;
        assert!(decide(&i, Some(&f)).is_none());
        let mut i = x(11.5, 10.0);
        i.dollars5 = 50_000.0;
        assert!(decide(&i, Some(&f)).is_none());
        // 30 minutes of 1M shares against an ADV of 10M: 1.3× usual, not enough.
        let busy = Fact {
            vol: Some(0.05),
            adv: Some(1e7),
        };
        assert!(decide(&x(11.5, 10.0), Some(&busy)).is_none());
        assert!(decide(&x(26.0, 20.0), None).is_none(), "above the band");
        assert!(decide(&x(0.13, 0.09), None).is_some(), "$0.10 is in");
    }
}
