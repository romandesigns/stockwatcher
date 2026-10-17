//! Limit Up–Limit Down price bands, **modelled** (docs/architecture/halts-luld-fills.md §2).
//!
//! This is our reading of the LULD Plan for Tier 2 NMS stocks (every $0.10–$25 small cap is
//! Tier 2). It is a guide drawn on the chart, not the exchange's numbers: verify against the
//! official bands before any real use.
//!
//! - **Active window:** the regular session only (09:30–16:00 ET; 09:30–13:00 on an early-close
//!   day, where the closing doubling window is likewise the last 25 minutes, 12:35–13:00).
//! - **Reference price:** the arithmetic mean of price-qualifying regular-session trade prices in
//!   the preceding 5 minutes, `(t − 5 min, t]`, evaluated on every such trade. The first reference
//!   is the first regular-session trade (the first after a resume, too). A new reference is
//!   published only when the mean moved at least 1% from the current one.
//! - **Bands:** [`luld_bands`], a pure function of the reference and whether the percentage is
//!   doubled ([`is_doubled`]).
//!
//! All arithmetic is exact: prices are micro-dollars, the mean is `sum / count` in `i128` and only
//! rounded (half up, to the micro-dollar) when it becomes the published reference; the 1% test
//! compares `|sum − ref·count| · 100 ≥ ref·count`, never a float.

use market_events::{Price, SessionId};
use std::collections::VecDeque;

/// The reference-price averaging window: 5 minutes.
pub const LULD_WINDOW_NS: i64 = 300 * 1_000_000_000;
const MIN_NS: i64 = 60 * 1_000_000_000;
/// Doubling after the open: the first 15 minutes of the regular session (09:30–09:45).
const OPEN_DOUBLING_NS: i64 = 15 * MIN_NS;
/// Doubling before the close, for references below $3.00: the last 25 minutes (15:35–16:00).
const CLOSE_DOUBLING_NS: i64 = 25 * MIN_NS;

const THREE_DOLLARS: i64 = 3_000_000;
const SEVENTY_FIVE_CENTS: i64 = 750_000;
const FIFTEEN_CENTS: i64 = 150_000;
const ONE_DOLLAR: i64 = 1_000_000;
/// The finest tick ($0.0001): the floor of a lower band.
const MIN_TICK: i64 = 100;

/// Upper and lower band, both on the tick grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bands {
    pub up: Price,
    pub down: Price,
}

/// Whether the band percentage is doubled at `t` for reference `reference`, in a regular session
/// `[open, close)`: during the first 15 minutes for every price, and during the last 25 minutes
/// for references below $3.00.
pub fn is_doubled(t: i64, open: i64, close: i64, reference: Price) -> bool {
    (t >= open && t < open + OPEN_DOUBLING_NS)
        || (t >= close - CLOSE_DOUBLING_NS && t < close && reference.micros() < THREE_DOLLARS)
}

/// The band offset `P·pct` (or the $0.15 cap) times 100, exact, in micro-dollars.
fn offset_x100(p: i128, doubled: bool) -> i128 {
    let mult = if doubled { 2 } else { 1 };
    let base = if p >= THREE_DOLLARS as i128 {
        p * 10
    } else if p >= SEVENTY_FIVE_CENTS as i128 {
        p * 20
    } else {
        // The lesser of $0.15 or 75%; doubled, the lesser of $0.30 or 150% (the same crossover,
        // P = $0.20, since both terms double).
        (FIFTEEN_CENTS as i128 * 100).min(p * 75)
    };
    base * mult
}

/// The tick at a price: $0.01 at or above $1.00, $0.0001 below (Reg NMS).
fn tick_at_x100(v_x100: i128) -> i128 {
    if v_x100 >= ONE_DOLLAR as i128 * 100 {
        10_000
    } else {
        MIN_TICK as i128
    }
}

/// Bands for a reference price: `up = P + P·pct` rounded **down** to the tick grid and
/// `down = P − P·pct` rounded **up**, so a band is never wider than the rule; `down` is at least
/// one tick ($0.0001). The tick is chosen by the unrounded band's own price. `None` for a
/// non-positive reference.
///
/// | Reference P        | pct                     | doubled                 |
/// |--------------------|-------------------------|-------------------------|
/// | P ≥ $3.00          | 10%                     | 20%                     |
/// | $0.75 ≤ P < $3.00  | 20%                     | 40%                     |
/// | P < $0.75          | lesser of $0.15 or 75%  | lesser of $0.30 or 150% |
pub fn luld_bands(reference: Price, doubled: bool) -> Option<Bands> {
    let p = reference.micros() as i128;
    if p <= 0 {
        return None;
    }
    let off = offset_x100(p, doubled);
    let raw_up = p * 100 + off;
    let tick_up = tick_at_x100(raw_up);
    let up = raw_up.div_euclid(100 * tick_up) * tick_up;
    let raw_down = p * 100 - off;
    let down = if raw_down <= 0 {
        MIN_TICK as i128
    } else {
        let tick = tick_at_x100(raw_down);
        let unit = 100 * tick;
        (raw_down + unit - 1).div_euclid(unit) * tick
    };
    Some(Bands {
        up: Price::from_micros(up as i64),
        down: Price::from_micros(down.max(MIN_TICK as i128) as i64),
    })
}

/// One published band change. All `None` means no bands from `t`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LuldPoint {
    pub t: i64,
    pub reference: Option<Price>,
    pub up: Option<Price>,
    pub down: Option<Price>,
}

impl LuldPoint {
    fn null(t: i64) -> Self {
        LuldPoint {
            t,
            reference: None,
            up: None,
            down: None,
        }
    }

    fn is_null(&self) -> bool {
        self.up.is_none()
    }

    fn same_bands(&self, o: &LuldPoint) -> bool {
        (self.reference, self.up, self.down) == (o.reference, o.up, o.down)
    }
}

/// One symbol's LULD state for one regular session.
#[derive(Clone, Debug)]
pub(crate) struct LuldState {
    pub session: SessionId,
    open: i64,
    close: i64,
    /// Price-qualifying regular-session trades `(event_ts, micros)` in the averaging window,
    /// ascending by time.
    window: VecDeque<(i64, i64)>,
    sum: i128,
    reference: Option<Price>,
    /// Time of the latest evaluation (trade, boundary or halt). Points are stamped no earlier, so
    /// they are ascending even when a trade arrives late inside the settle window.
    eval_t: i64,
    /// Every band change of the session, ascending, strictly increasing `t`.
    pub points: Vec<LuldPoint>,
}

impl LuldState {
    pub fn new(session: SessionId, open: i64, close: i64) -> Self {
        LuldState {
            session,
            open,
            close,
            window: VecDeque::new(),
            sum: 0,
            reference: None,
            eval_t: i64::MIN,
            points: Vec::new(),
        }
    }

    fn boundaries(&self) -> [i64; 3] {
        [
            self.open + OPEN_DOUBLING_NS,
            self.close - CLOSE_DOUBLING_NS,
            self.close,
        ]
    }

    fn active(&self) -> bool {
        self.points.last().is_some_and(|p| !p.is_null())
    }

    /// The next time the bands change on their own (a doubling boundary, or the close), while
    /// there is a reference. `None` when nothing is scheduled.
    pub fn next_deadline(&self) -> Option<i64> {
        if self.reference.is_none() && !self.active() {
            return None;
        }
        self.boundaries().into_iter().find(|b| *b > self.eval_t)
    }

    /// Publishes `p` if it differs from the bands in effect. A change at the same `t` as the
    /// previous point replaces it (clients upsert by `t`).
    fn publish(&mut self, p: LuldPoint, out: &mut Vec<LuldPoint>) {
        if let Some(last) = self.points.last_mut() {
            if last.same_bands(&p) {
                return;
            }
            if last.t == p.t {
                *last = p;
                out.push(p);
                return;
            }
        }
        self.points.push(p);
        out.push(p);
    }

    fn banded(&self, t: i64, reference: Price) -> LuldPoint {
        let b = luld_bands(reference, is_doubled(t, self.open, self.close, reference));
        LuldPoint {
            t,
            reference: Some(reference),
            up: b.map(|b| b.up),
            down: b.map(|b| b.down),
        }
    }

    /// Ends the bands at `t`: clears the reference and the window and publishes a null point if
    /// bands were in effect.
    fn stop(&mut self, t: i64, out: &mut Vec<LuldPoint>) {
        self.reference = None;
        self.window.clear();
        self.sum = 0;
        if self.active() {
            self.publish(LuldPoint::null(t), out);
        }
    }

    /// Processes the doubling boundaries and the close up to `now` (`inclusive`: also one at
    /// exactly `now`). A boundary is stamped at its own time, whoever crosses it (the clock or a
    /// trade), so linear play, stepping and a seek rebuild publish the same points.
    pub fn advance(&mut self, now: i64, inclusive: bool, out: &mut Vec<LuldPoint>) {
        for b in self.boundaries() {
            if b <= self.eval_t || b > now || (b == now && !inclusive) {
                continue;
            }
            self.eval_t = b;
            if b == self.close {
                self.stop(b, out);
            } else if let Some(r) = self.reference {
                let p = self.banded(b, r);
                self.publish(p, out);
            }
        }
    }

    /// A price-qualifying regular-session trade of this session while the symbol is not halted.
    pub fn on_trade(&mut self, ts: i64, price: Price, out: &mut Vec<LuldPoint>) {
        // A boundary at exactly `ts` is covered by this evaluation, so it is not published twice.
        self.advance(ts, false, out);
        if ts >= self.close || ts < self.open {
            return;
        }
        let t = ts.max(self.eval_t);
        self.eval_t = t;
        let pos = self.window.partition_point(|(w, _)| *w <= ts);
        self.window.insert(pos, (ts, price.micros()));
        self.sum += price.micros() as i128;
        while let Some(&(w, m)) = self.window.front()
            && w <= t - LULD_WINDOW_NS
        {
            self.window.pop_front();
            self.sum -= m as i128;
        }
        let count = self.window.len() as i128;
        if count == 0 {
            return;
        }
        let moved = match self.reference {
            None => true,
            Some(r) => {
                let rc = r.micros() as i128 * count;
                (self.sum - rc).abs() * 100 >= rc
            }
        };
        if moved {
            // Half up to the micro-dollar; prices are positive.
            let mean = (2 * self.sum + count).div_euclid(2 * count);
            self.reference = Some(Price::from_micros(mean as i64));
        }
        if let Some(r) = self.reference {
            let p = self.banded(t, r);
            self.publish(p, out);
        }
    }

    /// The symbol halted at `h`: no bands until the first trade after the resume, which becomes
    /// the new reference (as the reopening print does under the Plan).
    pub fn on_halt(&mut self, h: i64, out: &mut Vec<LuldPoint>) {
        self.advance(h, true, out);
        let t = h.max(self.eval_t);
        self.eval_t = t;
        self.stop(t, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use market_events::SessionKind;
    use market_events::session::et;

    fn px(s: &str) -> Price {
        s.parse().unwrap()
    }

    fn bands(p: &str, doubled: bool) -> (String, String) {
        let b = luld_bands(px(p), doubled).unwrap();
        (b.up.to_string(), b.down.to_string())
    }

    fn s(a: &str, b: &str) -> (String, String) {
        (a.to_string(), b.to_string())
    }

    #[test]
    fn contract_example() {
        // $3.38 · 10%: 3.718 → 3.71 (down), 3.042 → 3.05 (up).
        assert_eq!(bands("3.38", false), s("3.71", "3.05"));
    }

    #[test]
    fn tier_boundaries() {
        // At $3.00 exactly: 10%.
        assert_eq!(bands("3.00", false), s("3.30", "2.70"));
        // Just below: 20%. 2.99 · 1.2 = 3.588 → 3.58; 2.99 · 0.8 = 2.392 → 2.40.
        assert_eq!(bands("2.99", false), s("3.58", "2.40"));
        // At $0.75 exactly: 20%. 0.90 and 0.60 on the sub-dollar grid.
        assert_eq!(bands("0.75", false), s("0.90", "0.60"));
        // Just below $0.75: the $0.15 rule.
        assert_eq!(bands("0.7499", false), s("0.8999", "0.5999"));
    }

    #[test]
    fn fifteen_cent_rule_takes_the_lesser() {
        // P ≥ $0.20: $0.15 is less than 75%.
        assert_eq!(bands("0.50", false), s("0.65", "0.35"));
        assert_eq!(bands("0.20", false), s("0.35", "0.05"));
        // P < $0.20: 75% is less. 0.12 · 0.75 = 0.09.
        assert_eq!(bands("0.12", false), s("0.21", "0.03"));
        // 0.1001 · 0.75 = 0.075075: up 0.175175 → 0.1751, down 0.025025 → 0.0251.
        assert_eq!(bands("0.1001", false), s("0.1751", "0.0251"));
    }

    #[test]
    fn doubled_percentages() {
        assert_eq!(bands("3.38", true), s("4.05", "2.71")); // 20%: 4.056, 2.704
        assert_eq!(bands("2.00", true), s("2.80", "1.20")); // 40%
        // $0.30 or 150%: 0.50 → ±0.30; the lower band floors at one tick.
        assert_eq!(bands("0.50", true), s("0.80", "0.20"));
        assert_eq!(bands("0.12", true), s("0.30", "0.0001")); // 150%: 0.12 − 0.18 < 0
        assert_eq!(bands("0.20", true), s("0.50", "0.0001")); // 0.20 − 0.30 < 0
    }

    #[test]
    fn rounding_direction_and_grid_choice() {
        // Upper band rounds down, lower band rounds up, never wider than the rule.
        // 1.01 · 1.2 = 1.212 → 1.21; 1.01 · 0.8 = 0.808 (sub-dollar grid) → 0.808.
        assert_eq!(bands("1.01", false), s("1.21", "0.808"));
        // 1.2501 is off-grid as a mean: 1.50012 → 1.50; 1.00008 → 1.01 (cent grid, up).
        assert_eq!(bands("1.2501", false), s("1.50", "1.01"));
        // 0.8333 · 1.2 = 0.99996 (sub-dollar) → 0.9999; 0.8333 · 0.8 = 0.66664 → 0.6667.
        assert_eq!(bands("0.8333", false), s("0.9999", "0.6667"));
        for p in ["3.38", "2.99", "0.7499", "1.2501", "0.1001", "24.97"] {
            for d in [false, true] {
                let b = luld_bands(px(p), d).unwrap();
                let off = offset_x100(px(p).micros() as i128, d);
                assert!(b.up.micros() as i128 * 100 <= px(p).micros() as i128 * 100 + off);
                assert!(
                    b.down.micros() as i128 * 100 >= px(p).micros() as i128 * 100 - off,
                    "{p} {d}"
                );
            }
        }
        assert_eq!(luld_bands(Price::ZERO, false), None);
    }

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 15).unwrap()
    }

    fn at(h: u32, m: u32, s: u32) -> i64 {
        et(day(), h, m, s, 0).unwrap().0
    }

    fn state() -> LuldState {
        LuldState::new(
            SessionId::new(day(), SessionKind::Regular),
            at(9, 30, 0),
            at(16, 0, 0),
        )
    }

    fn up(p: &LuldPoint) -> String {
        p.up.map(|x| x.to_string()).unwrap_or_default()
    }

    #[test]
    fn doubling_windows() {
        let (o, c) = (at(9, 30, 0), at(16, 0, 0));
        assert!(is_doubled(at(9, 30, 0), o, c, px("10")));
        assert!(is_doubled(at(9, 44, 59), o, c, px("10")));
        assert!(!is_doubled(at(9, 45, 0), o, c, px("10")));
        // 15:35–16:00 only below $3.00.
        assert!(!is_doubled(at(15, 34, 59), o, c, px("2.99")));
        assert!(is_doubled(at(15, 35, 0), o, c, px("2.99")));
        assert!(!is_doubled(at(15, 35, 0), o, c, px("3.00")));
        assert!(!is_doubled(at(16, 0, 0), o, c, px("2.99")));
    }

    #[test]
    fn first_trade_sets_the_reference_and_boundaries_republish() {
        let mut l = state();
        let mut out = Vec::new();
        l.on_trade(at(9, 31, 0), px("2.00"), &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].t, up(&out[0])), (at(9, 31, 0), "2.80".into())); // doubled 40%
        out.clear();
        // The clock crosses 09:45: the same reference, normal percentage, stamped at 09:45.
        l.advance(at(9, 50, 0), true, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].t, up(&out[0])), (at(9, 45, 0), "2.40".into()));
        out.clear();
        // 15:35 doubles again (below $3), and 16:00 ends the bands.
        l.advance(at(16, 0, 0), true, &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!((out[0].t, up(&out[0])), (at(15, 35, 0), "2.80".into()));
        assert_eq!(out[1], LuldPoint::null(at(16, 0, 0)));
        assert_eq!(l.next_deadline(), None);
    }

    #[test]
    fn no_closing_doubling_at_or_above_three_dollars() {
        let mut l = state();
        let mut out = Vec::new();
        l.on_trade(at(10, 0, 0), px("3.00"), &mut out);
        out.clear();
        l.advance(at(16, 0, 0), true, &mut out);
        assert_eq!(out, vec![LuldPoint::null(at(16, 0, 0))]);
    }

    #[test]
    fn publishes_only_on_a_one_percent_move() {
        let mut l = state();
        let mut out = Vec::new();
        l.on_trade(at(10, 0, 0), px("3.00"), &mut out);
        // Mean (3.00 + 3.05) / 2 = 3.025: +0.83%, nothing.
        l.on_trade(at(10, 0, 1), px("3.05"), &mut out);
        assert_eq!(out.len(), 1);
        // Mean (3.00 + 3.05 + 3.01) / 3 = 3.02: still < 1%.
        l.on_trade(at(10, 0, 2), px("3.01"), &mut out);
        assert_eq!(out.len(), 1);
        // Mean (3.00+3.05+3.01+3.06)/4 = 3.03: exactly +1% → published, reference 3.03.
        l.on_trade(at(10, 0, 3), px("3.06"), &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].reference, Some(px("3.03")));
        assert_eq!(out[1].t, at(10, 0, 3));
    }

    #[test]
    fn the_window_is_the_preceding_five_minutes() {
        let mut l = state();
        let mut out = Vec::new();
        l.on_trade(at(10, 0, 0), px("3.00"), &mut out);
        // At 10:05:00 the 10:00:00 trade is exactly 5 minutes old and leaves the window: the
        // mean is the new trade alone, +10%.
        l.on_trade(at(10, 5, 0), px("3.30"), &mut out);
        assert_eq!(out.last().unwrap().reference, Some(px("3.30")));
    }

    #[test]
    fn halt_nulls_the_bands_and_the_next_trade_restarts_them() {
        let mut l = state();
        let mut out = Vec::new();
        l.on_trade(at(10, 0, 0), px("3.00"), &mut out);
        out.clear();
        l.on_halt(at(10, 1, 0), &mut out);
        assert_eq!(out, vec![LuldPoint::null(at(10, 1, 0))]);
        assert_eq!(l.next_deadline(), None);
        out.clear();
        // Boundaries publish nothing while there is no reference.
        l.advance(at(15, 40, 0), true, &mut out);
        assert!(out.is_empty());
        // After the resume, the first trade is the reference, however far it moved.
        l.on_trade(at(15, 41, 0), px("3.001"), &mut out);
        assert_eq!(out[0].reference, Some(px("3.001")));
        // A second halt while already null publishes nothing new.
        out.clear();
        l.on_halt(at(15, 42, 0), &mut out);
        l.on_halt(at(15, 43, 0), &mut out);
        assert_eq!(out, vec![LuldPoint::null(at(15, 42, 0))]);
    }

    #[test]
    fn a_trade_on_a_boundary_publishes_one_point() {
        let mut l = state();
        let mut out = Vec::new();
        l.on_trade(at(9, 40, 0), px("2.00"), &mut out);
        l.on_trade(at(9, 45, 0), px("2.00"), &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!((out[1].t, up(&out[1])), (at(9, 45, 0), "2.40".into()));
        assert!(l.points.windows(2).all(|w| w[0].t < w[1].t));
    }

    #[test]
    fn a_late_trade_never_stamps_a_point_in_the_past() {
        let mut l = state();
        let mut out = Vec::new();
        l.on_trade(at(10, 0, 10), px("3.00"), &mut out);
        l.on_trade(at(10, 0, 9), px("3.60"), &mut out); // arrived late, +10% mean
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].t, at(10, 0, 10));
        assert_eq!(out[1].reference, Some(px("3.30")));
    }
}
