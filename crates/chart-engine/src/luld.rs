//! Limit Up–Limit Down price bands, **modelled** (docs/architecture/halts-luld-fills.md §2).
//!
//! This is our reading of the LULD Plan (the Plan to Address Extraordinary Market Volatility,
//! consolidated text through Amendment 20, cdn.luldplan.com/plans/LULD-Plan-Amendment-20.pdf;
//! the 23rd Amendment leaves Section V and the percentages unchanged) for **Tier 2** NMS stocks.
//! Tier 1 (S&P 500, Russell 1000, the listed ETPs) is not modelled: we have no point-in-time
//! membership list, and the $0.10–$25 small caps the app watches are almost all Tier 2. It is a
//! guide drawn on the chart, not the SIP's numbers: verify against the official bands before any
//! real use. Section references below are the Plan's.
//!
//! - **Active window** (§V(A)(1)): Regular Trading Hours only (09:30–16:00 ET; to 13:00 on an
//!   early-close day).
//! - **Percentage bracket** (Appendix A §II(2)–(4), (6)): chosen **once per day by the previous
//!   regular-session close** on the primary listing exchange ([`LuldParams::prev_close`]), not by
//!   the moving reference: above $3.00 → 10%; $0.75 up to **and including** $3.00 → 20%; below
//!   $0.75 → the lesser of $0.15 or 75% (of the reference). So a stock that closed at $2.00 keeps
//!   20% bands all day even after it runs to $8, and one that closed at $0.60 keeps ±$0.15. When
//!   the previous close is unknown (no closes file, an IPO) the session's first reference stands
//!   in for it; the Plan's own fallback, the primary's last sale, is not something we can see.
//! - **Doubling** (§V(A)(1)): only in the last 25 minutes (15:35–16:00, or the 25 minutes before
//!   an early close), and for Tier 2 only when the bracket price is **equal to or below** $3.00.
//!   The Plan has no doubling at the open (an earlier version of this model doubled 09:30–09:45).
//! - **Reference price** (§V(A)(1)–(2)): the arithmetic mean of price-qualifying ("Eligible
//!   Reported") trades in the preceding 5 minutes, `(t − 5 min, t]`, kept continuously as the
//!   pro-forma reference. It becomes the reference only when it moved **at least 1%** from the one
//!   in effect, **and a reference stays in effect at least 30 seconds** ([`HOLD_NS`]): inside the
//!   hold nothing is published however far the mean runs, and at the hold's expiry the mean as of
//!   that instant is tested (on the clock, without waiting for a print), then every print after.
//!   With no trade in the last 5 minutes the reference in effect stays.
//! - **First reference** (§V(B)): the **Opening Price**, i.e. the primary listing exchange's
//!   opening print, if it comes within 5 minutes of the open; the mean then restarts from that
//!   print (trades before it never count). Without one by 09:35, the 5-minute mean at 09:35, or,
//!   with no trade by then, the first eligible trade after it. No bands before either. The
//!   opening print is recognised on the tape as a print from the listing venue
//!   ([`LuldParams::listing_venue`]) with condition `O` (opening trade), `Q` (official open) or `5`
//!   (reopening trade); with the venue unknown, a `Q` print from any venue. An open on a quote
//!   (where the Plan takes the previous close) cannot be told from trades and falls to the 09:35
//!   rule.
//! - **After a halt** (§V(C)): no bands while halted; the **Reopening Price** is the next
//!   reference, with a fresh 30-second hold and the mean restarting from it. On the tape it is the
//!   first eligible print after the resume, or the listing venue's reopening/official print if that
//!   comes first. (A reopening on a quote, whose midpoint the Plan uses, again cannot be seen.)
//! - **Bands:** [`luld_bands`], a pure function of the reference, the bracket price and whether
//!   the percentage is doubled ([`is_doubled`]). Rounding is ours: the Plan text does not say, the
//!   Operating Committee's annual report says "rounding to the nearest penny"; we round each band
//!   inward on the Reg NMS tick grid so a band is never wider than the rule.
//!
//! All arithmetic is exact: prices are micro-dollars, the mean is `sum / count` in `i128` and only
//! rounded (half up, to the micro-dollar) when it becomes the published reference; the 1% test
//! compares `|sum − ref·count| · 100 ≥ ref·count`, never a float.
//!
//! **Why the hold, the opening print and the bracket matter (2026-10-31).** Five years of tape
//! against the official halt list (J:/research/stockwatcher/luld-halts): without the 30-second
//! hold the band chased every fast spike and the stock never reached it (NNDM 2022-11-03 paused at
//! 2.97 with our band at 3.27; the Plan's would have been 2.96). An inferred pause (5 minutes of
//! silence with the last print in the outer 10% of the band) matched 23% of official pauses with
//! the old model and 58% with this one, at similar precision (74% vs 72% confirmed); in the first
//! 15 minutes 0.3% vs 51% (every 4th day, 5,372 pauses; docs/architecture/halts-luld-fills.md).

use chrono::NaiveDate;
use market_events::{Price, SessionId};
use std::collections::VecDeque;

/// The reference-price averaging window: 5 minutes.
pub const LULD_WINDOW_NS: i64 = 300 * 1_000_000_000;
/// The minimum time a reference stays in effect (§V(A)(2)): 30 seconds.
pub const HOLD_NS: i64 = 30 * 1_000_000_000;
const MIN_NS: i64 = 60 * 1_000_000_000;
/// How long after the open the primary's opening print may set the first reference (§V(B)).
const OPENING_GRACE_NS: i64 = 5 * MIN_NS;
/// Doubling before the close, for bracket prices at or below $3.00: the last 25 minutes.
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

/// What the Plan needs beyond the tape, for one symbol and trading date.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LuldParams {
    /// The previous regular-session close on the primary listing exchange, on the day's share
    /// basis. Picks the percentage bracket for the whole day and whether the close doubles
    /// (Appendix A §II(6)). `None`: the session's first reference stands in.
    pub prev_close: Option<Price>,
    /// The primary listing exchange as an Alpaca venue code (`Q` Nasdaq, `N` NYSE, `A` NYSE
    /// American, `P` NYSE Arca, `Z` Cboe BZX; [`venue_code`]). Only its opening/reopening print
    /// is the Plan's Opening Price.
    pub listing_venue: Option<String>,
}

/// Where the engine gets [`LuldParams`] (the server: `daily-closes.json` and `assets.json`).
/// Asked once per symbol and regular session, when the session's LULD state is created.
pub trait LuldInputs: Send + Sync + std::fmt::Debug {
    fn params(&self, symbol: &str, date: NaiveDate) -> LuldParams;
}

/// The Alpaca venue code of a listing exchange as `assets.json` names it; `None` for an exchange
/// without LULD bands (OTC) or one we do not know.
pub fn venue_code(listing: &str) -> Option<&'static str> {
    Some(match listing.trim() {
        "NASDAQ" => "Q",
        "NYSE" => "N",
        "AMEX" => "A",
        "ARCA" => "P",
        "BATS" => "Z",
        _ => return None,
    })
}

/// Whether the band percentage is doubled at `t` in a regular session ending at `close`
/// (exclusive): during the last 25 minutes, when the bracket price (the previous close) is at or
/// below $3.00 (§V(A)(1), Tier 2).
pub fn is_doubled(t: i64, close: i64, bracket_price: Price) -> bool {
    t >= close - CLOSE_DOUBLING_NS && t < close && bracket_price.micros() <= THREE_DOLLARS
}

/// The band offset times 100, exact, in micro-dollars: `P·pct` for the bracket of
/// `bracket_price` (Appendix A §II(2)–(4)), applied to the reference `p`.
fn offset_x100(p: i128, bracket_price: i64, doubled: bool) -> i128 {
    let mult = if doubled { 2 } else { 1 };
    let base = if bracket_price > THREE_DOLLARS {
        p * 10
    } else if bracket_price >= SEVENTY_FIVE_CENTS {
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
/// one tick ($0.0001). The tick is chosen by the unrounded band's own price. The bracket comes
/// from `bracket_price` (the previous close), the offset from the reference. `None` for a
/// non-positive reference.
///
/// | Bracket price (previous close) | pct                     | doubled                 |
/// |--------------------------------|-------------------------|-------------------------|
/// | > $3.00                        | 10%                     | 20%                     |
/// | $0.75 ≤ · ≤ $3.00              | 20%                     | 40%                     |
/// | < $0.75                        | lesser of $0.15 or 75%  | lesser of $0.30 or 150% |
pub fn luld_bands(reference: Price, bracket_price: Price, doubled: bool) -> Option<Bands> {
    let p = reference.micros() as i128;
    if p <= 0 {
        return None;
    }
    let off = offset_x100(p, bracket_price.micros(), doubled);
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

/// Where the session is in the reference-price rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Waiting for the first reference of the day (§V(B)).
    Opening,
    /// Halted, or waiting for the reopening print after a halt (§V(C)).
    Reopening,
    /// A reference is in effect; the 5-minute mean drives it (§V(A)(2)).
    Running,
}

/// A scheduled change, in the order [`LuldState::advance`] handles ties.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Due {
    /// The closing doubling window opens.
    Doubling,
    /// 5 minutes after the open without an opening print.
    OpeningGrace,
    /// The current reference's 30-second hold ends.
    HoldExpiry,
    /// The end of the regular session.
    Close,
}

/// One print as LULD sees it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Print<'a> {
    pub ts: i64,
    pub price: Price,
    /// Price-qualifying (updates OHLC): an Eligible Reported Transaction.
    pub eligible: bool,
    pub exchange: &'a str,
    pub conditions: &'a [String],
}

/// One symbol's LULD state for one regular session.
#[derive(Clone, Debug)]
pub(crate) struct LuldState {
    pub session: SessionId,
    open: i64,
    close: i64,
    params: LuldParams,
    /// The price that picks the bracket: the previous close, else the first reference.
    bracket: Option<Price>,
    phase: Phase,
    /// The 09:35 rule has run (or is moot).
    grace_done: bool,
    /// Eligible trades `(event_ts, micros)` in the averaging window, ascending by time.
    window: VecDeque<(i64, i64)>,
    sum: i128,
    reference: Option<Price>,
    /// When the reference in effect was set; its hold runs to `ref_since + HOLD_NS`.
    ref_since: i64,
    /// The hold's expiry has not been evaluated yet.
    hold_pending: bool,
    /// The time of the (re)opening print that set the reference, so the same venue's matching
    /// trade report at that instant is not counted twice in the mean.
    opened_at: Option<i64>,
    /// Time of the latest evaluation (trade, boundary or halt). Points are stamped no earlier, so
    /// they are ascending even when a trade arrives late inside the settle window.
    eval_t: i64,
    /// Every band change of the session, ascending, strictly increasing `t`.
    pub points: Vec<LuldPoint>,
}

impl LuldState {
    pub fn new(session: SessionId, open: i64, close: i64, params: LuldParams) -> Self {
        LuldState {
            session,
            open,
            close,
            bracket: params.prev_close.filter(|p| p.micros() > 0),
            params,
            phase: Phase::Opening,
            grace_done: false,
            window: VecDeque::new(),
            sum: 0,
            reference: None,
            ref_since: i64::MIN,
            hold_pending: false,
            opened_at: None,
            eval_t: i64::MIN,
            points: Vec::new(),
        }
    }

    fn active(&self) -> bool {
        self.points.last().is_some_and(|p| !p.is_null())
    }

    /// The listing venue's opening or reopening print (see the module doc).
    fn is_opening_print(&self, exchange: &str, conditions: &[String]) -> bool {
        let has = |set: &[&str]| conditions.iter().any(|c| set.contains(&c.trim()));
        match self.params.listing_venue.as_deref() {
            Some(v) => exchange == v && has(&["O", "Q", "5"]),
            None => has(&["Q"]),
        }
    }

    /// Scheduled changes still ahead of `eval_t` (a doubling boundary or the close only while
    /// there is something to change).
    fn dues(&self) -> impl Iterator<Item = (i64, Due)> + '_ {
        let banded = self.reference.is_some() || self.active();
        let doubling = (banded && self.close - CLOSE_DOUBLING_NS > self.eval_t)
            .then_some((self.close - CLOSE_DOUBLING_NS, Due::Doubling));
        let close = (banded && self.close > self.eval_t).then_some((self.close, Due::Close));
        let grace = (self.phase == Phase::Opening && !self.grace_done && !self.window.is_empty())
            .then_some((self.open + OPENING_GRACE_NS, Due::OpeningGrace));
        let hold = self
            .hold_pending
            .then_some((self.ref_since.saturating_add(HOLD_NS), Due::HoldExpiry));
        [doubling, close, grace, hold].into_iter().flatten()
    }

    /// The next time the bands may change on their own (the closing doubling window, the close,
    /// 09:35 without an opening print, a hold's expiry). `None` when nothing is scheduled.
    pub fn next_deadline(&self) -> Option<i64> {
        self.dues().map(|(t, _)| t).min()
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
        let bracket = self.bracket.unwrap_or(reference);
        let b = luld_bands(reference, bracket, is_doubled(t, self.close, bracket));
        LuldPoint {
            t,
            reference: Some(reference),
            up: b.map(|b| b.up),
            down: b.map(|b| b.down),
        }
    }

    /// Makes `r` the reference from `t`, starting its 30-second hold.
    fn set_reference(&mut self, t: i64, r: Price, out: &mut Vec<LuldPoint>) {
        self.reference = Some(r);
        self.bracket.get_or_insert(r);
        self.ref_since = t;
        self.hold_pending = true;
        self.phase = Phase::Running;
        let p = self.banded(t, r);
        self.publish(p, out);
    }

    fn clear_window(&mut self) {
        self.window.clear();
        self.sum = 0;
    }

    fn insert(&mut self, ts: i64, micros: i64) {
        let pos = self.window.partition_point(|(w, _)| *w <= ts);
        self.window.insert(pos, (ts, micros));
        self.sum += micros as i128;
    }

    fn prune(&mut self, t: i64) {
        while let Some(&(w, m)) = self.window.front()
            && w <= t - LULD_WINDOW_NS
        {
            self.window.pop_front();
            self.sum -= m as i128;
        }
    }

    /// The window's mean, rounded half up to the micro-dollar (prices are positive).
    fn mean(&self) -> Option<Price> {
        let count = self.window.len() as i128;
        (count > 0).then(|| Price::from_micros((2 * self.sum + count).div_euclid(2 * count) as i64))
    }

    /// The 1% test of the pro-forma reference as of `t`, outside any hold.
    fn test(&mut self, t: i64, out: &mut Vec<LuldPoint>) {
        self.prune(t);
        let (Some(r), count) = (self.reference, self.window.len() as i128) else {
            return;
        };
        if count == 0 {
            // No trade in the last 5 minutes: the reference in effect stays (§V(A)(1)).
            return;
        }
        let rc = r.micros() as i128 * count;
        if (self.sum - rc).abs() * 100 >= rc
            && let Some(m) = self.mean()
        {
            self.set_reference(t, m, out);
        }
    }

    /// A (re)opening price at `t` from the print at `ts`: the reference, and the mean restarts
    /// from it (§V(B)(1), §V(C)(1)).
    fn open_at(&mut self, t: i64, ts: i64, price: Price, out: &mut Vec<LuldPoint>) {
        self.clear_window();
        self.insert(ts, price.micros());
        self.opened_at = Some(ts);
        self.grace_done = true;
        self.set_reference(t, price, out);
    }

    /// Ends the bands at `t`: clears the reference and the window and publishes a null point if
    /// bands were in effect.
    fn stop(&mut self, t: i64, out: &mut Vec<LuldPoint>) {
        self.reference = None;
        self.hold_pending = false;
        self.opened_at = None;
        self.clear_window();
        if self.active() {
            self.publish(LuldPoint::null(t), out);
        }
    }

    /// Processes the scheduled changes up to `now` (`inclusive`: also one at exactly `now`), in
    /// time order. A change is stamped at its own time, whoever reaches it (the clock or a
    /// trade), so linear play, stepping and a seek rebuild publish the same points.
    pub fn advance(&mut self, now: i64, inclusive: bool, out: &mut Vec<LuldPoint>) {
        while let Some((t, due)) = self.dues().min() {
            if t > now || (t == now && !inclusive) {
                return;
            }
            self.eval_t = self.eval_t.max(t);
            match due {
                Due::Doubling => {
                    if let Some(r) = self.reference {
                        let p = self.banded(t, r);
                        self.publish(p, out);
                    }
                }
                Due::OpeningGrace => {
                    // §V(B)(2): no opening print within 5 minutes; the mean of the preceding
                    // 5 minutes (non-empty, or this would not be due).
                    self.grace_done = true;
                    self.prune(t);
                    if let Some(m) = self.mean() {
                        self.set_reference(t, m, out);
                    }
                }
                Due::HoldExpiry => {
                    self.hold_pending = false;
                    self.test(t, out);
                }
                Due::Close => {
                    self.stop(t, out);
                    self.phase = Phase::Running;
                    self.grace_done = true;
                }
            }
        }
    }

    /// A price-qualifying regular-session trade while not halted (tests and simple callers).
    #[cfg(test)]
    pub fn on_trade(&mut self, ts: i64, price: Price, out: &mut Vec<LuldPoint>) {
        self.on_print(
            Print {
                ts,
                price,
                eligible: true,
                exchange: "",
                conditions: &[],
            },
            out,
        );
    }

    /// A regular-session print of this session while the symbol is not halted: an eligible
    /// trade, or the listing venue's (re)opening print (which may itself not be eligible: `Q`).
    /// Anything else is ignored.
    pub fn on_print(&mut self, pr: Print<'_>, out: &mut Vec<LuldPoint>) {
        // A change at exactly `ts` is covered by this evaluation, so it is not published twice.
        self.advance(pr.ts, false, out);
        if pr.ts >= self.close || pr.ts < self.open {
            return;
        }
        let opening = self.is_opening_print(pr.exchange, pr.conditions);
        if !pr.eligible && !opening {
            return;
        }
        let t = pr.ts.max(self.eval_t);
        self.eval_t = t;
        match self.phase {
            Phase::Opening => {
                if opening && pr.ts < self.open + OPENING_GRACE_NS && !self.grace_done {
                    self.open_at(t, pr.ts, pr.price, out);
                    return;
                }
                if !pr.eligible {
                    return;
                }
                self.insert(pr.ts, pr.price.micros());
                if t >= self.open + OPENING_GRACE_NS {
                    // Past 09:35 with no reference yet: the 5-minute mean as of this print
                    // ("the first eligible trade after 9:35" when nothing traded before it).
                    self.grace_done = true;
                    self.prune(t);
                    if let Some(m) = self.mean() {
                        self.set_reference(t, m, out);
                    }
                }
            }
            Phase::Reopening => self.open_at(t, pr.ts, pr.price, out),
            Phase::Running => {
                if !pr.eligible || (opening && self.opened_at == Some(pr.ts)) {
                    return;
                }
                self.insert(pr.ts, pr.price.micros());
                if t >= self.ref_since.saturating_add(HOLD_NS) {
                    self.hold_pending = false;
                    self.test(t, out);
                } else {
                    // Inside the hold: the mean moves, the reference does not.
                    self.prune(t);
                }
                // A doubling boundary at exactly `t` was left to this evaluation.
                if let Some(r) = self.reference {
                    let p = self.banded(t, r);
                    self.publish(p, out);
                }
            }
        }
    }

    /// The symbol halted at `h`: no bands until the reopening print, which becomes the new
    /// reference (§V(C)).
    pub fn on_halt(&mut self, h: i64, out: &mut Vec<LuldPoint>) {
        self.advance(h, true, out);
        let t = h.max(self.eval_t);
        self.eval_t = t;
        self.stop(t, out);
        if t < self.close {
            self.phase = Phase::Reopening;
            self.grace_done = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use market_events::SessionKind;
    use market_events::session::et;

    fn px(s: &str) -> Price {
        s.parse().unwrap()
    }

    /// Bands with the bracket picked by the reference itself (previous close = reference).
    fn bands(p: &str, doubled: bool) -> (String, String) {
        bands_on(p, p, doubled)
    }

    fn bands_on(p: &str, prev_close: &str, doubled: bool) -> (String, String) {
        let b = luld_bands(px(p), px(prev_close), doubled).unwrap();
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
    fn bracket_boundaries_follow_appendix_a() {
        // "more than $3.00": 10%. 3.0001 · 1.1 = 3.30011 → 3.30; 2.70009 → 2.71.
        assert_eq!(bands("3.0001", false), s("3.30", "2.71"));
        // "$0.75 and up to and including $3.00": $3.00 itself is 20%.
        assert_eq!(bands("3.00", false), s("3.60", "2.40"));
        // 2.99 · 1.2 = 3.588 → 3.58; 2.99 · 0.8 = 2.392 → 2.40.
        assert_eq!(bands("2.99", false), s("3.58", "2.40"));
        // At $0.75 exactly: 20%. 0.90 and 0.60 on the sub-dollar grid.
        assert_eq!(bands("0.75", false), s("0.90", "0.60"));
        // "less than $0.75": the $0.15 rule.
        assert_eq!(bands("0.7499", false), s("0.8999", "0.5999"));
    }

    #[test]
    fn the_previous_close_picks_the_bracket_not_the_reference() {
        // Closed at $2.00, now referenced at $8.00: still 20%.
        assert_eq!(bands_on("8.00", "2.00", false), s("9.60", "6.40"));
        // Closed at $3.00 (20% bracket), referenced above $3: 20%.
        assert_eq!(bands_on("3.50", "3.00", false), s("4.20", "2.80"));
        // Closed at $0.60, referenced at $2.00: the lesser of $0.15 or 75%, i.e. ±$0.15.
        assert_eq!(bands_on("2.00", "0.60", false), s("2.15", "1.85"));
        // Closed at $5.00, faded to $2.00: 10%.
        assert_eq!(bands_on("2.00", "5.00", false), s("2.20", "1.80"));
        // And the closing doubling follows the same price: closed at $5, no doubling at $2.
        let close = 1_000 * MIN_NS;
        assert!(!is_doubled(close - MIN_NS, close, px("5.00")));
        assert!(is_doubled(close - MIN_NS, close, px("2.00")));
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
        assert_eq!(bands("3.00", true), s("4.20", "1.80")); // 40%: $3.00 is in the 20% bracket
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
        for p in [
            "3.38", "3.00", "2.99", "0.7499", "1.2501", "0.1001", "24.97",
        ] {
            for d in [false, true] {
                let b = luld_bands(px(p), px(p), d).unwrap();
                let off = offset_x100(px(p).micros() as i128, px(p).micros(), d);
                assert!(b.up.micros() as i128 * 100 <= px(p).micros() as i128 * 100 + off);
                assert!(
                    b.down.micros() as i128 * 100 >= px(p).micros() as i128 * 100 - off,
                    "{p} {d}"
                );
            }
        }
        assert_eq!(luld_bands(Price::ZERO, px("1"), false), None);
    }

    const SEC: i64 = 1_000_000_000;

    fn day() -> chrono::NaiveDate {
        chrono::NaiveDate::from_ymd_opt(2026, 10, 15).unwrap()
    }

    fn at(h: u32, m: u32, s: u32) -> i64 {
        et(day(), h, m, s, 0).unwrap().0
    }

    fn state_with(prev_close: Option<&str>, venue: Option<&str>) -> LuldState {
        LuldState::new(
            SessionId::new(day(), SessionKind::Regular),
            at(9, 30, 0),
            at(16, 0, 0),
            LuldParams {
                prev_close: prev_close.map(px),
                listing_venue: venue.map(str::to_string),
            },
        )
    }

    fn conds(c: &[&str]) -> Vec<String> {
        c.iter().map(|s| s.to_string()).collect()
    }

    /// Feeds one print: `conditions` as delivered, `eligible` = price-qualifying.
    fn feed(
        l: &mut LuldState,
        ts: i64,
        p: &str,
        exchange: &str,
        conditions: &[&str],
        eligible: bool,
        out: &mut Vec<LuldPoint>,
    ) {
        let c = conds(conditions);
        let pr = Print {
            ts,
            price: px(p),
            eligible,
            exchange,
            conditions: &c,
        };
        l.on_print(pr, out);
    }

    /// A state (listing venue Nasdaq) whose first reference is `p`, set at `t` (after 09:35) by
    /// the first trade of the day.
    fn running(prev_close: Option<&str>, t: i64, p: &str) -> (LuldState, Vec<LuldPoint>) {
        let mut l = state_with(prev_close, Some("Q"));
        let mut out = Vec::new();
        feed(&mut l, t, p, "V", &[], true, &mut out);
        assert_eq!(out.len(), 1);
        (l, out)
    }

    fn up(p: &LuldPoint) -> String {
        p.up.map(|x| x.to_string()).unwrap_or_default()
    }

    #[test]
    fn doubling_windows() {
        let c = at(16, 0, 0);
        // No doubling at the open (the Plan has none).
        assert!(!is_doubled(at(9, 30, 0), c, px("2.00")));
        assert!(!is_doubled(at(9, 44, 59), c, px("2.00")));
        // 15:35–16:00 for bracket prices at or below $3.00.
        assert!(!is_doubled(at(15, 34, 59), c, px("2.99")));
        assert!(is_doubled(at(15, 35, 0), c, px("2.99")));
        assert!(is_doubled(at(15, 35, 0), c, px("3.00")));
        assert!(!is_doubled(at(15, 35, 0), c, px("3.0001")));
        assert!(!is_doubled(at(16, 0, 0), c, px("2.99")));
        // An early close: the 25 minutes before 13:00.
        let early = at(13, 0, 0);
        assert!(!is_doubled(at(12, 34, 59), early, px("2.00")));
        assert!(is_doubled(at(12, 35, 0), early, px("2.00")));
    }

    #[test]
    fn the_listing_venues_opening_print_is_the_first_reference() {
        let mut l = state_with(Some("2.00"), Some("Q"));
        let mut out = Vec::new();
        // Other venues trade first: no bands yet, and these prints never enter the mean.
        feed(&mut l, at(9, 30, 1), "2.50", "V", &[], true, &mut out);
        feed(&mut l, at(9, 30, 2), "2.60", "D", &[], true, &mut out);
        // Another venue's own official open is not the Opening Price.
        feed(&mut l, at(9, 30, 2), "2.55", "P", &["Q"], false, &mut out);
        assert!(out.is_empty());
        // Nasdaq's official open (`Q`, not itself price-qualifying) sets the reference.
        feed(&mut l, at(9, 30, 3), "2.00", "Q", &["Q"], false, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].t, at(9, 30, 3));
        assert_eq!(out[0].reference, Some(px("2.00")));
        assert_eq!(up(&out[0]), "2.40"); // 20%, not doubled at the open
        // Nasdaq's matching opening trade at the same instant does not count twice.
        feed(
            &mut l,
            at(9, 30, 3),
            "2.00",
            "Q",
            &["@", "O"],
            true,
            &mut out,
        );
        // After the hold the mean is the opening print plus what followed it only:
        // (2.00 + 2.03) / 2 = 2.015 (+0.75%): nothing. With 2.50/2.60 it would have moved.
        feed(&mut l, at(9, 30, 40), "2.03", "V", &[], true, &mut out);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn without_an_opening_print_the_0935_mean_is_the_first_reference() {
        let mut l = state_with(Some("2.00"), Some("Q"));
        let mut out = Vec::new();
        feed(&mut l, at(9, 31, 0), "2.00", "V", &[], true, &mut out);
        feed(&mut l, at(9, 33, 0), "2.10", "D", &[], true, &mut out);
        assert!(out.is_empty());
        assert_eq!(l.next_deadline(), Some(at(9, 35, 0)));
        // The clock reaches 09:35: the mean of the preceding 5 minutes, stamped at 09:35.
        l.advance(at(9, 36, 0), true, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].t, at(9, 35, 0));
        assert_eq!(out[0].reference, Some(px("2.05")));
        // A late opening print no longer replaces it.
        feed(&mut l, at(9, 36, 0), "3.00", "Q", &["Q"], false, &mut out);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn with_nothing_traded_by_0935_the_next_trade_is_the_reference() {
        let mut l = state_with(None, Some("Q"));
        let mut out = Vec::new();
        assert_eq!(l.next_deadline(), None);
        l.advance(at(9, 40, 0), true, &mut out);
        assert!(out.is_empty());
        feed(&mut l, at(9, 41, 0), "1.50", "V", &[], true, &mut out);
        assert_eq!(out[0].reference, Some(px("1.50")));
    }

    #[test]
    fn with_the_venue_unknown_any_official_open_counts() {
        let mut l = state_with(None, None);
        let mut out = Vec::new();
        feed(&mut l, at(9, 30, 0), "4.00", "V", &["O"], true, &mut out);
        assert!(
            out.is_empty(),
            "an opening trade from an unknown venue is not enough"
        );
        feed(&mut l, at(9, 30, 1), "4.10", "P", &["Q"], false, &mut out);
        assert_eq!(out[0].reference, Some(px("4.10")));
    }

    #[test]
    fn a_reference_holds_30_seconds() {
        // The NNDM 2022-11-03 shape: a fast run toward the band inside the hold.
        let t0 = at(11, 0, 0);
        let (mut l, mut out) = running(Some("2.70"), t0, "2.70");
        out.clear();
        for k in 1..=20 {
            l.on_trade(t0 + k * SEC, px("3.20"), &mut out);
        }
        assert!(out.is_empty(), "inside the hold nothing is published");
        assert_eq!(l.next_deadline(), Some(t0 + HOLD_NS));
        // At expiry the clock alone publishes the mean as of then: (2.70 + 20 · 3.20) / 21.
        l.advance(t0 + HOLD_NS, true, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].t, t0 + HOLD_NS);
        assert_eq!(out[0].reference, Some(px("3.17619")));
        assert_eq!(l.next_deadline(), Some(t0 + 2 * HOLD_NS));
    }

    #[test]
    fn a_hold_that_expires_unmoved_tests_every_later_print() {
        let t0 = at(10, 0, 0);
        let (mut l, mut out) = running(Some("3.00"), t0, "3.00");
        out.clear();
        l.advance(t0 + HOLD_NS, true, &mut out);
        assert!(out.is_empty());
        assert_eq!(l.next_deadline(), Some(at(15, 35, 0)));
        // Mean (3.00 + 3.05) / 2 = 3.025: +0.83%, nothing.
        l.on_trade(t0 + 31 * SEC, px("3.05"), &mut out);
        // Mean (3.00 + 3.05 + 3.01) / 3 = 3.02: still < 1%.
        l.on_trade(t0 + 32 * SEC, px("3.01"), &mut out);
        assert!(out.is_empty());
        // Mean (3.00+3.05+3.01+3.06)/4 = 3.03: exactly +1% → published at this print.
        l.on_trade(t0 + 33 * SEC, px("3.06"), &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].reference, Some(px("3.03")));
        assert_eq!(out[0].t, t0 + 33 * SEC);
        // And a fresh hold starts.
        l.on_trade(t0 + 34 * SEC, px("3.90"), &mut out);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn the_window_is_the_preceding_five_minutes() {
        let (mut l, mut out) = running(None, at(10, 0, 0), "3.00");
        out.clear();
        // At 10:05:00 the 10:00:00 print is exactly 5 minutes old and leaves the window: the
        // mean is the new trade alone, +10%.
        l.on_trade(at(10, 5, 0), px("3.30"), &mut out);
        assert_eq!(out.last().unwrap().reference, Some(px("3.30")));
    }

    #[test]
    fn with_no_trade_in_five_minutes_the_reference_stays() {
        let (mut l, mut out) = running(None, at(10, 0, 0), "3.00");
        out.clear();
        l.on_trade(at(10, 0, 10), px("3.01"), &mut out);
        // The hold expires at 10:00:30 with a mean of 3.005 (< 1%): nothing; and 10 minutes of
        // silence change nothing either.
        l.advance(at(10, 10, 0), true, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn the_first_reference_picks_the_bracket_when_the_previous_close_is_unknown() {
        let (mut l, mut out) = running(None, at(10, 0, 0), "2.50");
        assert_eq!(up(&out[0]), "3.00"); // 20%
        out.clear();
        // The stock runs to $5: still the 20% bracket the first reference picked.
        l.on_trade(at(10, 6, 0), px("5.00"), &mut out);
        assert_eq!(out[0].reference, Some(px("5.00")));
        assert_eq!(up(&out[0]), "6.00");
    }

    #[test]
    fn closing_doubling_follows_the_previous_close() {
        // Closed at $2.00, trading at $4: 20% doubles to 40% at 15:35.
        let (mut l, mut out) = running(Some("2.00"), at(10, 0, 0), "4.00");
        assert_eq!(up(&out[0]), "4.80");
        out.clear();
        l.advance(at(16, 0, 0), true, &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!((out[0].t, up(&out[0])), (at(15, 35, 0), "5.60".into()));
        assert_eq!(out[1], LuldPoint::null(at(16, 0, 0)));
        assert_eq!(l.next_deadline(), None);
        // Closed at $3.01, trading at $2.50: 10%, and no closing doubling.
        let (mut l, mut out) = running(Some("3.01"), at(10, 0, 0), "2.50");
        assert_eq!(up(&out[0]), "2.75");
        out.clear();
        l.advance(at(16, 0, 0), true, &mut out);
        assert_eq!(out, vec![LuldPoint::null(at(16, 0, 0))]);
    }

    #[test]
    fn halt_nulls_the_bands_and_the_reopening_print_restarts_them() {
        let (mut l, mut out) = running(None, at(10, 0, 0), "3.00");
        out.clear();
        l.on_trade(at(10, 0, 40), px("3.10"), &mut out); // after the hold: mean 3.05, +1.7%
        assert_eq!(out.len(), 1);
        out.clear();
        l.on_halt(at(10, 1, 0), &mut out);
        assert_eq!(out, vec![LuldPoint::null(at(10, 1, 0))]);
        assert_eq!(l.next_deadline(), None);
        out.clear();
        // Boundaries publish nothing while there is no reference.
        l.advance(at(15, 40, 0), true, &mut out);
        assert!(out.is_empty());
        // The listing venue's reopening print (`5`) is the reference, however far it moved,
        // with the mean restarting from it (the pre-halt 3.00/3.10 no longer count). The bracket
        // is still the first reference's (3.00, 20%), doubled in the closing window: 3.60 · 1.4.
        feed(&mut l, at(15, 41, 0), "3.60", "Q", &["5"], true, &mut out);
        assert_eq!(out[0].reference, Some(px("3.60")));
        assert_eq!(out[0].t, at(15, 41, 0));
        assert_eq!(up(&out[0]), "5.04");
        out.clear();
        // A fresh 30-second hold.
        l.on_trade(at(15, 41, 20), px("3.90"), &mut out);
        assert!(out.is_empty(), "the reopening reference holds 30 s");
        // Then at 15:41:30 the mean (3.60 + 3.90) / 2 = 3.75 is +4.2%: published at expiry.
        l.advance(at(15, 41, 30), true, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].reference, Some(px("3.75")));
        out.clear();
        // A second halt while already null publishes nothing new.
        l.on_halt(at(15, 42, 0), &mut out);
        l.on_halt(at(15, 43, 0), &mut out);
        assert_eq!(out, vec![LuldPoint::null(at(15, 42, 0))]);
    }

    #[test]
    fn after_a_halt_the_first_eligible_print_reopens_too() {
        let (mut l, mut out) = running(Some("5.00"), at(10, 0, 0), "5.00");
        l.on_halt(at(10, 1, 0), &mut out);
        out.clear();
        l.on_trade(at(10, 6, 0), px("5.50"), &mut out);
        assert_eq!(out[0].reference, Some(px("5.50")));
        assert_eq!(up(&out[0]), "6.05");
    }

    #[test]
    fn a_trade_on_a_boundary_publishes_one_point() {
        let (mut l, mut out) = running(Some("2.00"), at(15, 30, 0), "2.00");
        l.on_trade(at(15, 35, 0), px("2.00"), &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!((out[1].t, up(&out[1])), (at(15, 35, 0), "2.80".into()));
        assert!(l.points.windows(2).all(|w| w[0].t < w[1].t));
    }

    #[test]
    fn a_late_trade_never_stamps_a_point_in_the_past() {
        let (mut l, mut out) = running(None, at(10, 0, 0), "3.00");
        l.on_trade(at(10, 0, 40), px("3.00"), &mut out);
        l.on_trade(at(10, 0, 39), px("3.60"), &mut out); // arrived late: mean 3.20, +6.7%
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].t, at(10, 0, 40));
        assert_eq!(out[1].reference, Some(px("3.20")));
    }

    #[test]
    fn venue_codes() {
        assert_eq!(venue_code("NASDAQ"), Some("Q"));
        assert_eq!(venue_code("AMEX"), Some("A"));
        assert_eq!(venue_code("OTC"), None);
    }
}
