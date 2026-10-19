//! The Halt watcher's pressure board (owner, 2026-10-19): per symbol, how one-sided and how heavy
//! the last minute of trading was, and the top [`BOARD_SIZE`] symbols by it, with hysteresis.
//!
//! It lives in the market's `Core` beside the quote board, under the same lock, and is fed the
//! same way: every trade with the chart engine's verdict on it ([`Counted`], so it excludes
//! exactly what the candles exclude), and every `chart.halt` the engine produces. The ranking and
//! the one `pressure.update` frame are built once per second by the quotes task
//! ([`crate::market::QUOTES_INTERVAL`]), never on the ingest path.
//!
//! # Pressure (trades only)
//!
//! The live feed carries trades, not quotes, so the side of a print is inferred with the **tick
//! rule**: a print above the symbol's previous print is a buy, below it a sell, at the same price
//! it inherits the previous print's side. The symbol's first print of the day has no side (it
//! counts toward the session's volume only). Only prints the engine counted for OHLC take part: they
//! are the ones with a meaningful price (an odd lot or an average-price report does not move the
//! candles either).
//!
//! Over the prints of the last [`WINDOW`] (by execution time, ending at the market clock):
//!
//! - `B`, `S`: buy and sell volume; `V = B + S`; `N` = prints with a side.
//! - **No score** unless `N >= MIN_TRADES` and `V >= MIN_VOLUME`: a thin symbol's few prints say
//!   nothing about pressure, and ranking them would fill the board with noise. (A card already on
//!   the board keeps its place down to half of each, see "Ranking".)
//! - `buy_share = B / V`; the side is **bull** when `buy_share >= 0.5`, else **bear**.
//! - **Imbalance** `I = min(1, |2·buy_share − 1| / FULL_IMBALANCE)`: linear up to a 100/0 minute.
//! - **Pace** `R = V / A`, where `A` is the symbol's own average volume per minute so far in the
//!   session (its OHLC-counted volume since the session's first such print, divided by the minutes
//!   since that print, at least one). A symbol's own average, not a market-wide one: 50k shares a
//!   minute is a frenzy for one stock and a quiet minute for another. Per session (overnight,
//!   premarket, regular, after hours), not per day: measured on the fixture day, a whole-day
//!   average diluted by the thin premarket put every symbol at 8–12× for the first half hour of
//!   the regular session, and pace stopped telling them apart.
//! - **Participation** `P = min(1, ln(1 + R) / ln(1 + PACE_CAP))`: logarithmic, so the first
//!   multiples count most, and capped at `PACE_CAP`× so a symbol waking from nothing cannot
//!   explode the scale (its first minute has `R = 1`, since the window is the whole session).
//! - **Pressure** `= round(100 · I · (0.4 + 0.6 · P))`, 0–100. Imbalance gates it: two-sided
//!   flow is not pressure however heavy; heavy one-sided flow scores up to 100, one-sided flow at
//!   the symbol's ordinary pace about 49.
//!
//! **Calibration** (owner, 2026-10-19: the gauge's ring fills with the score, so the score has to
//! tell the board's cards apart). The first constants, full imbalance at 80/20 and full pace at
//! 10×, put every card at 97–100 on live data, because reaching the top six takes exactly that:
//! over 12.5 minutes of the live board (2,970 card readings, 117 symbols, 2026-10-19 ~14:25 ET)
//! the cards' `|2·buy_share − 1|` ran 0.60 / 0.70 / 0.84 / 0.96 / 1.00 and their pace 9 / 12 / 15
//! / 29 / 70× (5th / 25th / 50th / 75th / 95th percentiles). Full imbalance at 100/0 and full
//! pace at 100× spread the same readings over 46 / 54 / 66 / 74 / 87, 0.6 % of them at 100.
//! The shape is unchanged, so the ranking and [`SWAP_MARGIN`] behave as before.
//!
//! Deterministic (a pure function of the prints applied and the clock, [`score`]) and causal (it
//! reads only prints already applied, at or before the clock that ranks them).
//!
//! Not followed: corrections and cancels (rare, and a one-minute window forgets them within the
//! minute), and quotes (not in the feed).
//!
//! # Halts
//!
//! A halted symbol has no prints, so its window would empty and it would drop off. Instead its
//! score is frozen at the halt (the last minute before it) until the resume, and a symbol that
//! was on the board stays there while halted: a halt is exactly when the watcher matters.
//!
//! # Ranking ([`PressureBoard::tick`])
//!
//! Among the symbols with a score inside the server's price band (the caller's `eligible`, the
//! same band as every other surface): members that left the band, or fell below HALF the
//! minimums, leave; free places are filled best first; then an outsider replaces the weakest
//! member only when it beats it by [`SWAP_MARGIN`] points. Display order follows the same rule: a
//! card moves above its neighbour only when it leads it by the margin. So cards neither swap in
//! and out nor reorder every second while two symbols trade places by a point or two, and a
//! symbol hovering at the minimum trade count does not blink on and off (seen on the fixture day
//! before the relaxed bar for members).

use crate::quotes::Counted;
use chart_engine::wire::HaltWire;
use chrono::NaiveDate;
use market_events::{Price, SessionKind, Trade, UnixNanos};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// The rolling window pressure is measured over.
pub const WINDOW: i64 = 60 * 1_000_000_000;
/// Minimum prints with a side in the window for a score.
pub const MIN_TRADES: usize = 10;
/// Minimum volume (shares) with a side in the window for a score.
pub const MIN_VOLUME: u64 = 2_000;
/// `|2·buy_share − 1|` at which the imbalance counts as complete (1.0 = a 100/0 minute;
/// calibration in the module docs).
pub const FULL_IMBALANCE: f64 = 1.0;
/// Pace (multiple of the symbol's own average volume per minute) at which participation is full
/// (calibration in the module docs).
pub const PACE_CAP: f64 = 100.0;
/// Cards on the board.
pub const BOARD_SIZE: usize = 6;
/// Points an outsider must lead a member by to replace it, and a card its upper neighbour by to
/// move above it.
pub const SWAP_MARGIN: u8 = 10;

const MINUTE: i64 = 60 * 1_000_000_000;

/// One symbol's pressure at a moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Score {
    /// 0–100 (module docs).
    pub pressure: u8,
    /// Buy volume / volume with a side, 0–1.
    pub buy_share: f64,
    /// Window volume / the symbol's average volume per minute so far in the session.
    pub pace: f64,
}

impl Score {
    /// Buyers lead (ties count as bullish).
    pub fn bull(&self) -> bool {
        self.buy_share >= 0.5
    }
}

/// Pressure from the window's buy and sell volume, its prints with a side, the symbol's volume so
/// far in the session and the time since the session's first print (module docs). `None` below
/// the minimums.
pub fn score(
    buy: u64,
    sell: u64,
    trades: usize,
    session_volume: u64,
    elapsed: i64,
) -> Option<Score> {
    score_with(
        buy,
        sell,
        trades,
        session_volume,
        elapsed,
        (MIN_TRADES, MIN_VOLUME),
    )
}

/// [`score`] with other minimums (a board member's relaxed bar).
fn score_with(
    buy: u64,
    sell: u64,
    trades: usize,
    session_volume: u64,
    elapsed: i64,
    (min_trades, min_volume): (usize, u64),
) -> Option<Score> {
    let v = buy + sell;
    if trades < min_trades || v < min_volume {
        return None;
    }
    let buy_share = buy as f64 / v as f64;
    let imbalance = ((2.0 * buy_share - 1.0).abs() / FULL_IMBALANCE).min(1.0);
    let minutes = (elapsed.max(MINUTE) as f64) / MINUTE as f64;
    // Right after a session boundary the window still holds the previous session's last prints,
    // so it can exceed the new session's volume: the pace is then 1, never more.
    let avg = (session_volume.max(v) as f64) / minutes;
    let pace = v as f64 / avg;
    let participation = ((1.0 + pace).ln() / (1.0 + PACE_CAP).ln()).min(1.0);
    let pressure = (100.0 * imbalance * (0.4 + 0.6 * participation)).round();
    Some(Score {
        pressure: pressure.clamp(0.0, 100.0) as u8,
        buy_share,
        pace,
    })
}

/// A halt in progress, as the engine reported it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct HaltInfo {
    /// Epoch ms.
    pub start: i64,
    /// Provider reason (Alpaca `rc`), e.g. `"LUDP"` for a LULD pause.
    pub reason: String,
}

#[derive(Clone, Debug)]
struct Entry {
    symbol: Arc<str>,
    day: Option<NaiveDate>,
    /// Prints with a side, in arrival order: (execution time ns, size, buy).
    window: VecDeque<(i64, u64, bool)>,
    buy: u64,
    sell: u64,
    /// Newest execution time in the window (prunes on apply, so a busy symbol's window stays
    /// one minute long between ticks).
    newest: i64,
    /// The previous OHLC-counted print's price and side (tick rule), in arrival order.
    last_price: Option<Price>,
    last_side: Option<bool>,
    /// The session the baseline below belongs to, its OHLC-counted volume and its first print.
    session: Option<SessionKind>,
    session_volume: u64,
    session_start: i64,
    halt: Option<HaltInfo>,
    /// The score at the halt, shown until the resume.
    frozen: Option<Score>,
    /// The score at the last tick.
    last: Option<Score>,
}

impl Entry {
    fn new(symbol: Arc<str>) -> Self {
        Entry {
            symbol,
            day: None,
            window: VecDeque::new(),
            buy: 0,
            sell: 0,
            newest: i64::MIN,
            last_price: None,
            last_side: None,
            session: None,
            session_volume: 0,
            session_start: 0,
            halt: None,
            frozen: None,
            last: None,
        }
    }

    /// Drops prints at or before `now − WINDOW`.
    fn prune(&mut self, now: i64) {
        let cut = now.saturating_sub(WINDOW);
        while let Some(&(ts, size, buy)) = self.window.front() {
            if ts > cut {
                break;
            }
            self.window.pop_front();
            if buy {
                self.buy -= size;
            } else {
                self.sell -= size;
            }
        }
    }

    /// The score at `now` (the frozen one while halted).
    fn evaluate(&mut self, now: i64) -> Option<Score> {
        self.evaluate_with(now, (MIN_TRADES, MIN_VOLUME))
    }

    fn evaluate_with(&mut self, now: i64, minimums: (usize, u64)) -> Option<Score> {
        if self.halt.is_some() {
            return self.frozen;
        }
        self.prune(now);
        score_with(
            self.buy,
            self.sell,
            self.window.len(),
            self.session_volume,
            now - self.session_start,
            minimums,
        )
    }
}

/// One card of the board.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub symbol: Arc<str>,
    pub score: Score,
    pub halt: Option<HaltInfo>,
}

#[derive(Debug, Default)]
pub struct PressureBoard {
    index: HashMap<Arc<str>, usize>,
    entries: Vec<Entry>,
    /// The cards shown, in display order (entry indices).
    board: Vec<usize>,
}

impl PressureBoard {
    pub fn new() -> Self {
        Self::default()
    }

    fn entry_index(&mut self, symbol: &str) -> usize {
        if let Some(&i) = self.index.get(symbol) {
            return i;
        }
        let s: Arc<str> = Arc::from(symbol);
        let i = self.entries.len();
        self.entries.push(Entry::new(s.clone()));
        self.index.insert(s, i);
        i
    }

    /// Applies one print with the engine's verdict on it. Cheap: a hash lookup, a push and a few
    /// integer updates (it runs on the ingest path, under the core lock).
    pub fn apply_trade(&mut self, t: &Trade, counted: Counted) {
        if !counted.ohlc {
            return;
        }
        let Some(session) = t.session else { return };
        let i = self.entry_index(&t.symbol);
        let e = &mut self.entries[i];
        match e.day {
            Some(d) if session.date < d => return,
            Some(d) if session.date == d => {}
            // A new trading date starts the symbol over (the tick rule, the window and the day's
            // average), like the quote board's row. A halt is kept: it ends with its resume.
            _ => {
                let (halt, frozen) = (e.halt.take(), e.frozen.take());
                *e = Entry::new(e.symbol.clone());
                e.day = Some(session.date);
                e.halt = halt;
                e.frozen = frozen;
            }
        }
        // A new session restarts the pace baseline only: the tick rule and the window carry on
        // (the price is continuous across the bell).
        if e.session != Some(session.kind) {
            e.session = Some(session.kind);
            e.session_volume = 0;
            e.session_start = t.event_ts.0;
        }
        e.session_volume += t.size;
        let side = match e.last_price {
            Some(p) if t.price > p => Some(true),
            Some(p) if t.price < p => Some(false),
            Some(_) => e.last_side,
            None => None,
        };
        e.last_price = Some(t.price);
        let Some(buy) = side else { return };
        e.last_side = Some(buy);
        let ts = t.event_ts.0;
        e.window.push_back((ts, t.size, buy));
        if buy {
            e.buy += t.size;
        } else {
            e.sell += t.size;
        }
        if ts > e.newest {
            e.newest = ts;
            e.prune(ts);
        }
    }

    /// A `chart.halt` from the engine: a halt starting freezes the symbol's score as of the halt,
    /// the resume (the same `start`, `end` set) releases it.
    pub fn apply_halt(&mut self, symbol: &str, h: &HaltWire) {
        let i = self.entry_index(symbol);
        let e = &mut self.entries[i];
        match h.end {
            None => {
                let at = UnixNanos::from_millis(h.start).0;
                let s = e.evaluate(at).or(e.last);
                e.frozen = s;
                e.halt = Some(HaltInfo {
                    start: h.start,
                    reason: h.reason.clone(),
                });
            }
            Some(_) => {
                if e.halt.as_ref().is_some_and(|x| x.start == h.start) {
                    e.halt = None;
                    e.frozen = None;
                }
            }
        }
    }

    /// Scores every symbol at `now` (epoch ns) and updates the board (module docs, "Ranking").
    /// `eligible` says whether a symbol is inside the price band. Returns the cards in display
    /// order.
    pub fn tick(&mut self, now: i64, eligible: impl Fn(&str) -> bool) -> Vec<Card> {
        // Members are scored against the relaxed bar, everyone else against the full one (module
        // docs, "Ranking"); the score itself does not depend on the bar.
        let relaxed = (MIN_TRADES / 2, MIN_VOLUME / 2);
        let mut member = vec![false; self.entries.len()];
        for &i in &self.board {
            member[i] = true;
        }
        let scores: Vec<Option<Score>> = self
            .entries
            .iter_mut()
            .enumerate()
            .map(|(i, e)| {
                let s = if member[i] {
                    e.evaluate_with(now, relaxed)
                } else {
                    e.evaluate(now)
                };
                e.last = s;
                s
            })
            .collect();
        let entries = &self.entries;
        let ok = |i: usize| eligible(&entries[i].symbol);
        let pressure = |i: usize| scores[i].map_or(0, |s| s.pressure);
        let halted = |i: usize| entries[i].halt.is_some();

        // Members stay while scored and in the band; a halted member stays regardless.
        self.board
            .retain(|&i| ok(i) && (halted(i) || scores[i].is_some()));
        let mut outside: Vec<usize> = (0..entries.len())
            .filter(|&i| scores[i].is_some() && ok(i) && !member[i])
            .collect();
        // Best first; ties by symbol, so the board is the same on every run.
        outside.sort_by(|&a, &b| {
            pressure(b)
                .cmp(&pressure(a))
                .then_with(|| entries[a].symbol.cmp(&entries[b].symbol))
        });
        let mut outside = outside.into_iter().peekable();
        while self.board.len() < BOARD_SIZE {
            match outside.next() {
                Some(i) => self.board.push(i),
                None => break,
            }
        }
        // An outsider takes the weakest unhalted member's place only by the margin.
        while let Some(&best) = outside.peek() {
            let weakest = self
                .board
                .iter()
                .enumerate()
                .filter(|&(_, &i)| !halted(i))
                .min_by_key(|&(pos, &i)| (pressure(i), std::cmp::Reverse(pos)))
                .map(|(pos, &i)| (pos, i));
            match weakest {
                Some((pos, w)) if pressure(best) >= pressure(w) + SWAP_MARGIN => {
                    self.board[pos] = best;
                    outside.next();
                }
                _ => break,
            }
        }
        // Order: a card moves above its neighbour only when it leads it by the margin (a stable
        // bubble pass until nothing moves; at most BOARD_SIZE² steps).
        let card_pressure = |i: usize| -> u8 {
            match &entries[i].halt {
                Some(_) => entries[i]
                    .frozen
                    .or(entries[i].last)
                    .map_or(0, |s| s.pressure),
                None => pressure(i),
            }
        };
        loop {
            let mut moved = false;
            for k in 1..self.board.len() {
                if card_pressure(self.board[k])
                    >= card_pressure(self.board[k - 1]).saturating_add(SWAP_MARGIN)
                {
                    self.board.swap(k, k - 1);
                    moved = true;
                }
            }
            if !moved {
                break;
            }
        }
        self.board
            .iter()
            .map(|&i| {
                let e = &entries[i];
                Card {
                    symbol: e.symbol.clone(),
                    score: if e.halt.is_some() {
                        e.frozen.or(e.last)
                    } else {
                        scores[i]
                    }
                    .unwrap_or(Score {
                        pressure: 0,
                        buy_share: 0.5,
                        pace: 0.0,
                    }),
                    halt: e.halt.clone(),
                }
            })
            .collect()
    }

    /// Replaces `symbol`'s state with `other`'s (the live market loaded its history: `other` was
    /// built from that history and the events held meanwhile, the quote board's
    /// [`crate::quotes::QuoteBoard::replace_from`]). Board membership is untouched.
    pub fn replace_from(&mut self, symbol: &str, other: &PressureBoard) {
        let Some(src) = other.index.get(symbol).map(|&j| other.entries[j].clone()) else {
            return;
        };
        let i = self.entry_index(symbol);
        let last = self.entries[i].last;
        self.entries[i] = src;
        self.entries[i].last = last;
    }

    /// One symbol's score at `now`, whatever the band (tests and diagnostics).
    pub fn score_of(&mut self, symbol: &str, now: i64) -> Option<Score> {
        let &i = self.index.get(symbol)?;
        self.entries[i].evaluate(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use market_events::session::et;

    const BOTH: Counted = Counted {
        ohlc: true,
        volume: true,
    };

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 16).unwrap()
    }

    fn at(h: u32, m: u32, s: u32) -> i64 {
        et(day(), h, m, s, 0).unwrap().0
    }

    fn trade(sym: &str, ts: i64, cents: i64, size: u64) -> Trade {
        Trade::new(sym, UnixNanos(ts), Price::from_cents(cents), size, 1)
    }

    /// `n` prints of `size`, one a second from `from`, each one cent above the last (all buys
    /// after the first) or below it (all sells).
    fn run(b: &mut PressureBoard, sym: &str, from: i64, n: u32, size: u64, start: i64, up: bool) {
        for k in 0..n {
            let c = if up {
                start + k as i64
            } else {
                start - k as i64
            };
            b.apply_trade(&trade(sym, from + k as i64 * 1_000_000_000, c, size), BOTH);
        }
    }

    #[test]
    fn the_formula() {
        // Below the minimums: no score.
        assert_eq!(score(5_000, 0, 9, 5_000, MINUTE), None);
        assert_eq!(score(1_000, 999, 20, 1_999, MINUTE), None);
        // Balanced flow is no pressure, however heavy.
        let s = score(50_000, 50_000, 100, 100_000, 30 * MINUTE).unwrap();
        assert_eq!(s.pressure, 0);
        assert!(s.bull(), "a tie counts as bullish");
        // One-sided at the symbol's ordinary pace (the window is one minute of an even day):
        // I = 1, R = 1, P = ln 2 / ln 101.
        let s = score(10_000, 0, 50, 300_000, 30 * MINUTE).unwrap();
        assert!((s.pace - 1.0).abs() < 1e-9);
        let expect = (100.0 * (0.4 + 0.6 * (2f64.ln() / 101f64.ln()))).round() as u8;
        assert_eq!(s.pressure, expect);
        assert_eq!(expect, 49);
        // 80/20 at 10× pace, the weakest card a live board held (it used to score 100):
        // I = 0.6, P = ln 11 / ln 101.
        let s = score(80_000, 20_000, 200, 300_000, 30 * MINUTE).unwrap();
        assert!((s.pace - 10.0).abs() < 1e-9);
        assert_eq!(s.pressure, 43);
        // Only a 100/0 minute at 100× pace or beyond fills the gauge, never above 100.
        let s = score(100_000, 0, 200, 100_000, 200 * MINUTE).unwrap();
        assert!((s.pace - 200.0).abs() < 1e-9);
        assert_eq!(s.pressure, 100, "capped, never above 100");
        // Bearish mirrors bullish.
        let bear = score(20_000, 80_000, 200, 300_000, 30 * MINUTE).unwrap();
        assert!(!bear.bull());
        assert_eq!(bear.pressure, 43);
        // 65/35 (I = 0.3) at 3× pace.
        let s = score(19_500, 10_500, 60, 300_000, 30 * MINUTE).unwrap();
        let p = (4f64.ln() / 101f64.ln()).min(1.0);
        assert_eq!(s.pressure, (100.0 * 0.3 * (0.4 + 0.6 * p)).round() as u8);
    }

    #[test]
    fn a_sparse_symbols_first_minute_does_not_explode() {
        // The window is the whole session: R = 1, not V / tiny.
        let s = score(30_000, 0, 30, 30_000, 5 * 1_000_000_000).unwrap();
        assert!((s.pace - 1.0).abs() < 1e-9);
        assert_eq!(s.pressure, 49);
    }

    #[test]
    fn the_tick_rule_with_zero_ticks_inheriting_the_side() {
        let mut b = PressureBoard::new();
        let t0 = at(10, 0, 0);
        // 500 (no side), 501 buy, 501 buy (zero tick), 500 sell, 500 sell, 502 buy …
        let prices = [500, 501, 501, 500, 500, 502, 502, 502, 503, 503, 503, 503];
        for (k, c) in prices.iter().enumerate() {
            b.apply_trade(
                &trade("AAA", t0 + k as i64 * 1_000_000_000, *c, 1_000),
                BOTH,
            );
        }
        let s = b.score_of("AAA", t0 + 12 * 1_000_000_000).unwrap();
        // 11 prints with a side: buys 501, 501, 502, 502, 502, 503 ×4 = 9; sells 2.
        assert!((s.buy_share - 9.0 / 11.0).abs() < 1e-9, "{s:?}");
    }

    #[test]
    fn excluded_and_volume_only_prints_take_no_part() {
        let mut b = PressureBoard::new();
        let t0 = at(10, 0, 0);
        run(&mut b, "AAA", t0, 12, 500, 500, true);
        let before = b.score_of("AAA", t0 + 12_000_000_000);
        let odd_lot = Counted {
            ohlc: false,
            volume: true,
        };
        // An odd lot far below would be a sell and reset the tick rule: it must not.
        b.apply_trade(&trade("AAA", t0 + 12_000_000_000, 100, 99), odd_lot);
        let none = Counted {
            ohlc: false,
            volume: false,
        };
        b.apply_trade(&trade("AAA", t0 + 12_000_000_000, 100, 9_999), none);
        assert_eq!(b.score_of("AAA", t0 + 12_000_000_000), before);
    }

    #[test]
    fn the_window_is_the_last_minute_at_the_clock() {
        let mut b = PressureBoard::new();
        let t0 = at(10, 0, 0);
        run(&mut b, "AAA", t0, 30, 500, 500, true);
        assert!(b.score_of("AAA", t0 + 30_000_000_000).is_some());
        // A minute after the last print nothing is left in the window.
        assert_eq!(b.score_of("AAA", t0 + 90_000_000_000), None);
    }

    #[test]
    fn deterministic_the_same_prints_and_clock_give_the_same_board() {
        let mk = || {
            let mut b = PressureBoard::new();
            let t0 = at(10, 0, 0);
            with_pressures(&mut b, t0, &[("A", 3), ("B", 9), ("C", 5), ("D", 5)]);
            b.tick(t0 + 31_000_000_000, |_| true)
        };
        let (x, y) = (mk(), mk());
        assert_eq!(x, y);
        // C and D tie: the symbol breaks it.
        let syms: Vec<&str> = x.iter().map(|c| &*c.symbol).collect();
        assert_eq!(syms, ["A", "C", "D", "B"]);
    }

    #[test]
    fn the_pace_baseline_restarts_with_each_session() {
        let mut b = PressureBoard::new();
        // A thin premarket: 300 shares a minute for half an hour.
        for m in 0..30u32 {
            for k in 0..3u32 {
                let ts = at(9, m, k * 10);
                let c = 500 + ((m + k) % 2) as i64;
                b.apply_trade(&trade("AAA", ts, c, 100), BOTH);
            }
        }
        // The regular session trades 10,000 a minute, evenly, for 20 minutes.
        let open = at(9, 30, 0);
        for k in 0..(20 * 20) {
            let ts = open + k as i64 * 3_000_000_000;
            let c = 600 + (k % 2) as i64;
            b.apply_trade(&trade("AAA", ts, c, 500), BOTH);
        }
        let now = open + 20 * MINUTE;
        let s = b.score_of("AAA", now).unwrap();
        // Against the regular session's own average the minute is ordinary; against the day's
        // (diluted by the premarket) it would read ~1.4× and climb toward the cap the earlier in
        // the session it was measured.
        // (0.95: the last print is 3 s before `now`, so 19 of a minute's 20 are in the window.)
        assert!((s.pace - 0.95).abs() < 1e-9, "{s:?}");
    }

    #[test]
    fn a_member_keeps_its_place_down_to_half_the_minimums() {
        let mut b = PressureBoard::new();
        let t0 = at(10, 0, 0);
        run(&mut b, "AAA", t0, 12, 500, 500, true);
        let now = t0 + 12_000_000_000;
        assert_eq!(b.tick(now, |_| true).len(), 1);
        // Half a minute later only 6 of its prints are inside the window: below the full bar
        // (a newcomer with 6 would not get on), above the relaxed one.
        let later = t0 + 66_000_000_000;
        assert_eq!(b.score_of("AAA", later), None);
        assert_eq!(b.tick(later, |_| true).len(), 1, "still on the board");
        // Below half the bar it leaves.
        assert!(b.tick(t0 + 68_000_000_000, |_| true).is_empty());
    }

    #[test]
    fn a_new_trading_date_starts_over() {
        let mut b = PressureBoard::new();
        run(&mut b, "AAA", at(15, 59, 0), 20, 500, 500, true);
        let next = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
        let t = et(next, 4, 0, 0, 0).unwrap().0;
        b.apply_trade(&trade("AAA", t, 900, 100), BOTH);
        assert_eq!(b.score_of("AAA", t), None);
    }

    fn halt(start: i64, end: Option<i64>) -> HaltWire {
        HaltWire {
            start: start / 1_000_000,
            end: end.map(|e| e / 1_000_000),
            reason: "LUDP".into(),
            code: "H".into(),
        }
    }

    #[test]
    fn a_halt_freezes_the_score_and_keeps_the_card_until_the_resume() {
        let mut b = PressureBoard::new();
        let t0 = at(10, 0, 0);
        run(&mut b, "AAA", t0, 40, 1_000, 500, true);
        let all = |_: &str| true;
        let now = t0 + 40_000_000_000;
        let cards = b.tick(now, all);
        assert_eq!(cards.len(), 1);
        let before = cards[0].score;
        b.apply_halt("AAA", &halt(now, None));
        // Minutes later, no prints: still on the board, same score, marked halted.
        let later = now + 5 * MINUTE;
        let cards = b.tick(later, all);
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].score, before);
        assert_eq!(cards[0].halt.as_ref().unwrap().reason, "LUDP");
        // The resume releases it; with an empty window it leaves.
        b.apply_halt("AAA", &halt(now, Some(later)));
        assert!(b.tick(later + 1, all).is_empty());
    }

    #[test]
    fn ranking_fills_best_first_and_respects_the_band() {
        let mut b = PressureBoard::new();
        let t0 = at(10, 0, 0);
        // Eight symbols, rising pressure with the index (more one-sided prints).
        for (k, s) in ["A", "B", "C", "D", "E", "F", "G", "H"].iter().enumerate() {
            let sells = 8 - k as u32;
            run(&mut b, s, t0, 20, 1_000, 500, true);
            run(&mut b, s, t0 + 20_000_000_000, sells, 1_000, 450, false);
        }
        let now = t0 + 40_000_000_000;
        let cards = b.tick(now, |s| s != "H");
        let syms: Vec<&str> = cards.iter().map(|c| &*c.symbol).collect();
        assert_eq!(syms.len(), BOARD_SIZE);
        assert!(!syms.contains(&"H"), "outside the band");
        assert!(
            !syms.contains(&"A"),
            "the weakest of the seven left out: {syms:?}"
        );
        assert!(
            cards
                .windows(2)
                .all(|w| w[0].score.pressure + SWAP_MARGIN > w[1].score.pressure),
            "no card leads its upper neighbour by the margin"
        );
    }

    /// Symbols with 30 prints each, `sells` of them downticks (fewer sells = more pressure).
    fn with_pressures(b: &mut PressureBoard, t0: i64, syms: &[(&str, u32)]) {
        for (s, sells) in syms {
            run(b, s, t0, 30 - sells, 1_000, 500, true);
            run(
                b,
                s,
                t0 + (30 - sells) as i64 * 1_000_000_000,
                *sells,
                1_000,
                400,
                false,
            );
        }
    }

    #[test]
    fn hysteresis_an_outsider_needs_the_margin_to_take_a_place() {
        let mut b = PressureBoard::new();
        let t0 = at(10, 0, 0);
        with_pressures(
            &mut b,
            t0,
            &[("A", 8), ("B", 8), ("C", 8), ("D", 8), ("E", 8), ("F", 8)],
        );
        let now = t0 + 31_000_000_000;
        let first: Vec<Arc<str>> = b
            .tick(now, |_| true)
            .into_iter()
            .map(|c| c.symbol)
            .collect();
        assert_eq!(first.len(), 6);
        let weakest = b.score_of("A", now).unwrap().pressure;
        // G a little better than the members: not enough.
        with_pressures(&mut b, t0, &[("G", 7)]);
        let g = b.score_of("G", now).unwrap().pressure;
        assert!(g > weakest && g < weakest + SWAP_MARGIN, "{g} vs {weakest}");
        let second: Vec<Arc<str>> = b
            .tick(now, |_| true)
            .into_iter()
            .map(|c| c.symbol)
            .collect();
        assert_eq!(second, first, "no reshuffle for a few points");
        // Z far better: it takes a place.
        with_pressures(&mut b, t0, &[("Z", 0)]);
        let third: Vec<Arc<str>> = b
            .tick(now, |_| true)
            .into_iter()
            .map(|c| c.symbol)
            .collect();
        assert!(third.iter().any(|s| &**s == "Z"));
        assert_eq!(&*third[0], "Z", "and leads, by more than the margin");
        assert!(!third.iter().any(|s| &**s == "G"));
    }

    #[test]
    fn replace_from_takes_the_loaded_history() {
        let t0 = at(10, 0, 0);
        let mut live = PressureBoard::new();
        run(&mut live, "AAA", t0 + 50_000_000_000, 3, 100, 500, true);
        assert_eq!(live.score_of("AAA", t0 + 53_000_000_000), None);
        let mut hist = PressureBoard::new();
        run(&mut hist, "AAA", t0, 53, 1_000, 500, true);
        live.replace_from("AAA", &hist);
        assert_eq!(
            live.score_of("AAA", t0 + 53_000_000_000),
            hist.score_of("AAA", t0 + 53_000_000_000)
        );
    }
}
