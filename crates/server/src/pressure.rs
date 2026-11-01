//! The Halt watcher's board: the symbols whose one-sided trade-flow pressure is **building up fast
//! while the price runs at its LULD band**, at most [`BOARD_SIZE`], usually fewer, often none.
//!
//! Owner, 2026-10-20: "the idea of the halt watcher is to observe for a rapid increasing amounting
//! pressure that could potentially lead to a halt", and "if such event isn't happening then such
//! stock gauge should not be displayed in the panel". So the board is no longer the top six by
//! pressure (PR #38): a card exists only while a build-up is under way, and the gauge shows the
//! halt-risk score below, not the pressure level. Zero cards is the normal, honest state.
//!
//! It lives in the market's `Core` beside the quote board, under the same lock, and is fed the
//! same way: every trade with the chart engine's verdict on it ([`Counted`], so it excludes
//! exactly what the candles exclude), every `chart.halt` and every `chart.luld` (the modelled
//! band, `chart_engine::luld`) the engine produces, from events and from clock advances. The
//! ranking and the one `pressure.update` frame are built once per second by the quotes task
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
//! Over the prints of a [`WINDOW`] (one minute, by execution time) ending at time `t`:
//!
//! - `B`, `S`: buy and sell volume; `V = B + S`; `N` = prints with a side.
//! - **No score** unless `N >= MIN_TRADES` and `V >= MIN_VOLUME`: a thin symbol's few prints say
//!   nothing about pressure. (A card already on the board is scored down to half of each.)
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
//! The pressure constants were calibrated on 12.5 minutes of the live board (PR #38, 2026-10-19
//! ~14:25 ET, 2,970 card readings, 117 symbols): full imbalance at 100/0 and full pace at 100×
//! spread the then top-six cards over 46 / 54 / 66 / 74 / 87 (5th–95th percentiles), where 80/20
//! and 10× had put every card at 97–100. The level still feeds price alerts on pressure
//! (`crate::alerts`, [`PressureBoard::score_of`]) and the card's tooltip; the board itself ranks by
//! the halt risk below.
//!
//! Not followed: corrections and cancels (rare, and a one-minute window forgets them within the
//! minute), and quotes (not in the feed).
//!
//! # Halt risk (the build-up)
//!
//! A LULD pause is what the watcher anticipates: the price reaches its band (the 5-minute mean,
//! held at least 30 s, ± 10 % above a $3 previous close, 20 % from $0.75 to $3, the lesser of $0.15
//! or 75 % below, doubled in the last 25 minutes at or below $3; `chart_engine::luld`) and stays
//! there for 15 seconds. The level of the pressure does not anticipate it: a stock can sell 90/10 all
//! afternoon and drift. What comes before a pause is the pressure **rising** while the price runs
//! **at** the band on the same side. Per symbol, at the clock `t`, with `H` = [`HORIZON`]:
//!
//! - `P(t)`: the pressure over `(t − 1 min, t]` and its side (bull = toward the upper band,
//!   bear = the lower); `P(t − H)` over `(t − H − 1 min, t − H]`, counted as 0 when it was on the
//!   other side or below the minimums (a quiet minute before is no pressure on this side).
//! - **Rise** `Δ = P(t) − P(t − H)`, −100…100: the build-up. A steady 90 has `Δ ≈ 0`; 30 → 80 in
//!   the horizon has `Δ = 50`.
//! - **Band** on that side, `L` (the modelled upper band for bull, lower for bear), its
//!   **half-width** `h = |L − ref| / ref` (the band's own percentage: 10 % above a $3 previous
//!   close, 20 % from $0.75, up to 75 % below, doubled before the close), and the **distance** `d = max(0, ±(L − p) / p)`
//!   from the last price `p`. Distances are compared with `h`, not in absolute percent, so one set
//!   of constants serves a $0.40 stock with a 37 % band and a $12 one with 10 %.
//! - **Move** `m = ±(p − p(t − H)) / p(t − H)` toward the band, `p(t − H)` the last print at or
//!   before `t − H`; the **time to band** at that pace `T = d · H / m` (`m > 0`; `T = 0` at the
//!   band).
//! - Components, each 0–1: build-up `Bu = clamp(Δ / RISE_FULL)`, urgency
//!   `U = clamp(ln(T_FAR / T) / ln(T_FAR / T_NEAR))` (logarithmic: 15 s and 30 s differ as much
//!   as 5 and 10 minutes; 0 when not moving toward the band), closeness `C = clamp(1 − d / h)`
//!   (the share of the reference→band distance already covered).
//! - **Risk** `= round(100 · (W_RISE · Bu + W_URGENCY · U + W_CLOSE · C))`, 0–100: what the gauge
//!   shows. Weights in [`W_RISE`], [`W_URGENCY`], [`W_CLOSE`].
//!
//! [`HORIZON`] = 30 s: a LULD pause needs 15 s at the band, and the runs that end in one take tens
//! of seconds to a few minutes, so the change has to be read over well under a minute to lead the
//! pause at all; under ~20 s the two one-minute windows share two thirds or more of their prints,
//! so `Δ` barely moves and a single block print swings it. 30 s is half the window: `Δ` weighs
//! the newest half-minute against the half-minute a minute earlier.
//!
//! Deterministic (a pure function of the prints, bands and halts applied and the clock,
//! [`halt_risk`]) and causal (it reads only what was applied at or before the clock that ranks).
//!
//! # Entry and exit ([`PressureBoard::tick`])
//!
//! Eligible: inside the server's price band (the caller's `eligible`, the same band as every other
//! surface) **and** with a modelled LULD band, i.e. the regular session and not halted. Outside it
//! (premarket, after hours, the minutes after a resume before the reopening print, a symbol whose
//! history the live market has not loaded yet) there is no band to run at, and the pauses that
//! happen there are news halts the tape cannot anticipate: no card. A symbol enters only when ALL
//! hold:
//!
//! - `Δ >= ENTER_RISE` (a real build-up, not drift);
//! - it moved toward the band by at least `MIN_MOVE` of the half-width over the horizon, so a
//!   bid/ask bounce is not a run; scaled by the band, that is 0.5 % of the price on a 10 % band
//!   and 1.9 % on a $0.40 stock's 37 % one;
//! - it has covered at least `ENTER_CLOSE` of the way from the reference to the band (a fast stock
//!   in the middle of its band is not about to halt: see "Calibration");
//! - at that pace it reaches the band within `T_ENTER`;
//! - risk `>= ENTER_RISK`.
//!
//! A card stays while it is **building**: `Δ > 0` and the price not moving away (`m >= 0`), or it
//! is **pinned** within `PIN` of the half-width from the band, the 15-second limit state before a
//! pause, when the price stops and the flow often flattens. When it stops building it leaves after
//! [`HOLD`] seconds of not building (a single flat second does not blink it off). It leaves at once
//! when it leaves the price band, and after the hold when it loses its LULD band (the close).
//!
//! **Halts.** A halted symbol has no prints: its pressure and risk are frozen at the halt (the band
//! too: the engine clears it right after the halt) and it stays through the halt if it was on the
//! board, or was building with `Δ >= ENTER_RISE / 2` at the halt (a fast run can halt before the
//! once-a-second ranking saw it). After the resume it follows the normal exit rule. A halt takes
//! the weakest unhalted card's place when the board is full.
//!
//! **Ranking.** At most [`BOARD_SIZE`] cards, strongest risk first. An outsider replaces the
//! weakest unhalted member only when it beats it by [`SWAP_MARGIN`], and a card moves above its
//! neighbour only when it leads it by the margin, so cards neither swap nor reorder every second.
//!
//! # Calibration (2026-10-20)
//!
//! The bench `crates/server/tests/halt_bench.rs` replays archive days through the engine and this
//! board as the ingest feeds them, ranks every second of the regular session, and reports the
//! cards shown and, for every LULD pause, the lead time and the entries no pause followed. It
//! takes the pauses from halt statuses when the day is a live recording, and infers them on a
//! trades-only import (a regular-session silence of 4.5+ minutes right after a print at the band).
//!
//! **What it could run on here.** Locally there is one real day of one symbol (MDXH, 2026-10-16,
//! SIP, 15,287 OHLC-counted prints, $0.19–$0.55) and the synthetic fixture day (four symbols;
//! FIXTURE DATA, not market evidence). The 22 SIP days (146,429 symbol-days) live only in the
//! VPS volume. **Neither local day has a LULD pause**: MDXH traded without a two-minute silence
//! all session and never covered more than 10 % of its band (a sub-$0.75 stock's band is
//! $0.15, 47–144 % of its price); the fixture's 10:05 halts are a scheduled silence, not a run
//! at a band (the board rightly ignores them). So the lead time and the false alarms on real
//! pauses are **not measured yet**: run the bench on the VPS archive before trusting the entry
//! constants beyond what follows.
//!
//! **What was measured.**
//!
//! - The rise is noisy on its own. On MDXH, sampled every 10 s through the regular session, the
//!   rise reached 25 in 9.6 % of samples, 30 in 6.5 %, 40 in 2.0 % and 50 in 0.13 %: a one-minute
//!   pressure swings by tens of points all day. Hence [`ENTER_RISE`] = 30 (the top ~6 %) and
//!   [`RISE_FULL`] = 50 (where it becomes rare), and the band gates do the rest.
//! - A first cut without the closeness gate (time to band within 300 s, risk 40) showed MDXH
//!   twice at 09:31–09:33 (28 s and 23 s), its then doubled opening band 75 % away (the LULD
//!   model doubled the first 15 minutes until 2026-10-31; the Plan does not): a 7.5 % move in
//!   30 s projected that band within 300 s. A fast stock is not a halt candidate on its own,
//!   hence [`ENTER_CLOSE`] (past half-way to the band), [`T_ENTER`] = 180 s and
//!   [`ENTER_RISK`] = 50. The fixture showed SMPL twice the same way (12:17, 12:19), 8 % of the
//!   price from a band it had covered less than half of; both are gone too.
//! - With the constants below: **zero cards** in 23,401 ranked seconds on each day, the honest
//!   state for days without a run at a band. Closest approach: MDXH 10 % of its half-width, the
//!   fixture's in-band symbols 44 %.
//!
//! **Set from the LULD rule, not from data** (to re-check with the bench): [`HORIZON`] (above),
//! [`T_NEAR`] = 15 s (the limit state), [`T_FAR`] = 10 min, [`PIN`] = 10 % of the half-width
//! (1 % of the price on a 10 % band: inside the bid/ask of most small caps at the band),
//! [`MIN_MOVE`] = 5 % of the half-width (above a one-cent bounce on a $10 stock, the size of one
//! on a $0.40 one), [`HOLD`] = 5 s, and the weights: the build-up leads (it is what the owner
//! asked to watch), urgency next (closer and faster = higher), closeness last (being near the
//! band without a run is the steady case the board exists to ignore). The other entry gates at
//! their minimums (rise 30, half-way, 180 s) give 48, so [`ENTER_RISK`] = 50 asks a little more
//! of at least one; the worked example in the tests, 30 → 80 with the price 3.8 % from a 10 %
//! band and 59 s from it at its pace, scores 78; at the band with a full rise it is 100.

use crate::quotes::Counted;
use chart_engine::wire::{HaltWire, LuldWire};
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
/// `|2·buy_share − 1|` at which the imbalance counts as complete (1.0 = a 100/0 minute).
pub const FULL_IMBALANCE: f64 = 1.0;
/// Pace (multiple of the symbol's own average volume per minute) at which participation is full.
pub const PACE_CAP: f64 = 100.0;
/// Cards on the board, at most.
pub const BOARD_SIZE: usize = 6;
/// Risk points an outsider must lead a member by to replace it, and a card its upper neighbour by
/// to move above it.
pub const SWAP_MARGIN: u8 = 10;

/// The build-up horizon: the rise and the move are measured over it (module docs).
pub const HORIZON: i64 = 30 * 1_000_000_000;
/// Rise (pressure points over the horizon) that fills the build-up component.
pub const RISE_FULL: f64 = 50.0;
/// Time to band (seconds) at or under which urgency is full: the 15 s a LULD limit state lasts.
pub const T_NEAR: f64 = 15.0;
/// Time to band (seconds) at or beyond which urgency is zero: past 10 minutes the 5-minute
/// reference has moved on and the projection says nothing.
pub const T_FAR: f64 = 600.0;
/// Weights of the build-up, urgency and closeness components (they sum to 1).
pub const W_RISE: f64 = 0.4;
pub const W_URGENCY: f64 = 0.35;
pub const W_CLOSE: f64 = 0.25;
/// Entry: minimum rise over the horizon.
pub const ENTER_RISE: i16 = 30;
/// Entry: minimum move toward the band over the horizon, as a share of the band's half-width.
pub const MIN_MOVE: f64 = 0.05;
/// Entry: the band must be reachable at the current pace within this many seconds.
pub const T_ENTER: f64 = 180.0;
/// Entry: minimum share of the reference→band distance already covered (closeness).
pub const ENTER_CLOSE: f64 = 0.5;
/// Entry: minimum risk.
pub const ENTER_RISK: u8 = 50;
/// Within this share of the half-width from the band a card is pinned (the limit state).
pub const PIN: f64 = 0.1;
/// Seconds of not building before a card leaves.
pub const HOLD: i64 = 5 * 1_000_000_000;

const MINUTE: i64 = 60 * 1_000_000_000;
const SECOND: f64 = 1_000_000_000.0;

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

/// The modelled LULD band in effect (`chart.luld`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    pub up: Price,
    pub down: Price,
    /// The reference price (5-minute mean) the band was drawn around.
    pub reference: Price,
}

impl Band {
    /// From a `chart.luld` point; `None` for a null point (no bands from then).
    pub fn from_wire(l: &LuldWire) -> Option<Band> {
        let px = |v: Option<f64>| v.and_then(Price::from_f64_rounded);
        Some(Band {
            up: px(l.up)?,
            down: px(l.down)?,
            reference: px(l.reference)?,
        })
    }
}

/// One symbol's halt risk at a moment (module docs, "Halt risk").
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Risk {
    /// 0–100, what the gauge shows.
    pub risk: u8,
    /// `Δ`: the side's pressure now minus over the minute a horizon earlier, −100…100.
    pub rise: i16,
    /// The side the flow pushes: buying toward the upper band (true), selling the lower.
    pub up: bool,
    /// The band on that side.
    pub band: Price,
    /// Distance from the last price to that band, a fraction of the price (0.012 = 1.2 %).
    pub to_band: f64,
    /// Move toward the band over the horizon, a fraction of the earlier price (negative: away).
    pub moved: f64,
    /// Seconds to the band at the current pace; `None` when not moving toward it.
    pub secs_to_band: Option<f64>,
    /// The band's half-width, a fraction of its reference (0.10 for a 10 % band).
    pub half_width: f64,
}

impl Risk {
    /// The entry conditions (module docs, "Entry and exit"), all at once.
    pub fn enters(&self) -> bool {
        self.rise >= ENTER_RISE
            && self.moved >= MIN_MOVE * self.half_width
            && self.secs_to_band.is_some_and(|s| s <= T_ENTER)
            && self.to_band <= (1.0 - ENTER_CLOSE) * self.half_width
            && self.risk >= ENTER_RISK
    }

    /// Within [`PIN`] of the half-width from the band: the limit state before a pause.
    pub fn pinned(&self) -> bool {
        self.to_band <= PIN * self.half_width
    }

    /// Still building up: the rise positive and the price not moving away, or pinned.
    pub fn building(&self) -> bool {
        (self.rise > 0 && self.moved >= 0.0) || self.pinned()
    }

    /// Building with at least half the entry rise: what keeps a non-member through a halt.
    fn building_at_halt(&self) -> bool {
        self.building() && self.rise >= ENTER_RISE / 2
    }
}

/// The halt risk from the pressure now and a horizon earlier, the last price, the last price at
/// or before a horizon earlier and the band in effect (module docs, "Halt risk"). Pure.
pub fn halt_risk(
    now: Score,
    before: Option<Score>,
    price: Price,
    anchor: Option<Price>,
    band: &Band,
) -> Risk {
    let bull = now.bull();
    let dir = if bull { 1.0 } else { -1.0 };
    let earlier = before
        .filter(|b| b.bull() == bull)
        .map_or(0, |b| b.pressure);
    let rise = now.pressure as i16 - earlier as i16;
    let target = if bull { band.up } else { band.down };
    let (p, l, r) = (
        price.micros() as f64,
        target.micros() as f64,
        band.reference.micros() as f64,
    );
    // A band never sits on its reference; a degenerate one counts as 1 % wide.
    let half_width = ((l - r).abs() / r.max(1.0)).max(0.01);
    let to_band = if p > 0.0 {
        (dir * (l - p) / p).max(0.0)
    } else {
        0.0
    };
    let moved = match anchor {
        Some(a) if a.micros() > 0 => dir * (p - a.micros() as f64) / a.micros() as f64,
        _ => 0.0,
    };
    let horizon = HORIZON as f64 / SECOND;
    let secs_to_band = if to_band == 0.0 {
        Some(0.0)
    } else if moved > 0.0 {
        Some(to_band * horizon / moved)
    } else {
        None
    };
    let unit = |x: f64| x.clamp(0.0, 1.0);
    let build = unit(rise as f64 / RISE_FULL);
    let urgency = secs_to_band.map_or(0.0, |t| {
        unit((T_FAR / t.max(1e-9)).ln() / (T_FAR / T_NEAR).ln())
    });
    let close = unit(1.0 - to_band / half_width);
    let risk = (100.0 * (W_RISE * build + W_URGENCY * urgency + W_CLOSE * close)).round();
    Risk {
        risk: risk.clamp(0.0, 100.0) as u8,
        rise,
        up: bull,
        band: target,
        to_band,
        moved,
        secs_to_band,
        half_width,
    }
}

/// A halt in progress, as the engine reported it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct HaltInfo {
    /// Epoch ms.
    pub start: i64,
    /// Provider reason (Alpaca `rc`), e.g. `"LUDP"` for a LULD pause.
    pub reason: String,
}

/// One print with a side: (execution time ns, size, buy, price).
type Print = (i64, u64, bool, Price);

/// The prints of the last `WINDOW + HORIZON`, in three segments by execution time, so both the
/// window ending now and the one ending a horizon earlier are running sums (no rescan per tick):
/// `[0]` = `(t − H − W, t − W]`, `[1]` = `(t − W, t − H]`, `[2]` = `(t − H, t]`. The window now
/// is `[1] + [2]`, a horizon earlier `[0] + [1]`. The roll clock never goes backward. Each
/// segment is sorted by execution time, with arrival order preserved for equal timestamps.
/// Ordered prints append in O(1); retained late prints are inserted in their current segment.
/// Prints at or before the oldest cut are dropped, never reintroduced into a closed window.
/// Given fixed sides and distinct execution timestamps, sums and the anchor do not depend on
/// arrival order. The upstream tick rule deliberately still depends on arrival order.
#[derive(Clone, Debug, Default)]
struct Segments {
    seg: [VecDeque<Print>; 3],
    /// (buy, sell) volume per segment.
    sums: [(u64, u64); 3],
    clock: Option<i64>,
    /// The price of the newest print that left segment 2: the last price at or before `t − H`.
    anchor: Option<Price>,
    anchor_ts: Option<i64>,
}

impl Segments {
    /// Returns false for an expired print (the caller counts the rejection).
    fn push(&mut self, p: Print) -> bool {
        self.roll(p.0);
        let now = self.clock.expect("roll sets the clock");
        if p.0 <= now.saturating_sub(HORIZON + WINDOW) {
            return false;
        }
        let k = if p.0 <= now.saturating_sub(WINDOW) {
            0
        } else if p.0 <= now.saturating_sub(HORIZON) {
            1
        } else {
            2
        };
        if k < 2 {
            self.observe_anchor(&p);
        }
        Self::add(&mut self.sums[k], &p, true);
        let seg = &mut self.seg[k];
        if seg.back().is_none_or(|last| last.0 <= p.0) {
            seg.push_back(p);
        } else {
            // Upper bound: equal execution times retain their arrival order.
            let at = seg.partition_point(|existing| existing.0 <= p.0);
            seg.insert(at, p);
        }
        true
    }

    fn observe_anchor(&mut self, p: &Print) {
        if self.anchor_ts.is_none_or(|ts| p.0 >= ts) {
            self.anchor_ts = Some(p.0);
            self.anchor = Some(p.3);
        }
    }

    fn add(sum: &mut (u64, u64), p: &Print, plus: bool) {
        let v = if p.2 { &mut sum.0 } else { &mut sum.1 };
        if plus {
            *v += p.1;
        } else {
            *v -= p.1;
        }
    }

    /// Moves prints forward to the segments of clock `now`.
    fn roll(&mut self, now: i64) {
        let now = self.clock.map_or(now, |clock| clock.max(now));
        self.clock = Some(now);
        let cuts = [
            now.saturating_sub(HORIZON + WINDOW),
            now.saturating_sub(WINDOW),
            now.saturating_sub(HORIZON),
        ];
        // Newest segment first, so a print that moves down is checked against the next cut in
        // the same pass (a quiet symbol rolled after minutes moves straight through).
        for k in (0..3).rev() {
            while let Some(&p) = self.seg[k].front() {
                if p.0 > cuts[k] {
                    break;
                }
                self.seg[k].pop_front();
                Self::add(&mut self.sums[k], &p, false);
                if k == 2 {
                    self.observe_anchor(&p);
                }
                if k > 0 {
                    Self::add(&mut self.sums[k - 1], &p, true);
                    self.seg[k - 1].push_back(p);
                }
            }
        }
    }

    /// (buy, sell, prints) of the window now (`lag = false`) or a horizon earlier.
    fn window(&self, lag: bool) -> (u64, u64, usize) {
        let (a, b) = if lag { (0, 1) } else { (1, 2) };
        (
            self.sums[a].0 + self.sums[b].0,
            self.sums[a].1 + self.sums[b].1,
            self.seg[a].len() + self.seg[b].len(),
        )
    }

    /// Volume of the prints after `t − H` (segment 2).
    fn recent_volume(&self) -> u64 {
        self.sums[2].0 + self.sums[2].1
    }
}

/// What a halted symbol shows until the resume.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Frozen {
    score: Option<Score>,
    risk: Option<Risk>,
}

#[derive(Clone, Debug)]
struct Entry {
    symbol: Arc<str>,
    day: Option<NaiveDate>,
    prints: Segments,
    /// The previous OHLC-counted print's price and side (tick rule), in arrival order.
    last_price: Option<Price>,
    last_side: Option<bool>,
    /// Latest execution price, separate from the arrival-order tick rule. Equal execution
    /// times use the latest arrival; an older print never moves this timestamp backward.
    last_exec: Option<(i64, Price)>,
    /// The session the baseline below belongs to, its OHLC-counted volume and its first print.
    session: Option<SessionKind>,
    session_volume: u64,
    session_start: i64,
    /// The modelled LULD band in effect; `None` outside the regular session and while halted.
    band: Option<Band>,
    halt: Option<HaltInfo>,
    /// Latest matching official resume, epoch ns. Kept across day rolls and history adoption.
    resumed_at: Option<i64>,
    /// The readings at the halt, shown until the resume.
    frozen: Option<Frozen>,
    /// The readings at the last tick.
    last: Option<Score>,
    last_risk: Option<Risk>,
    /// The risk the card shows: the last one it had on the board (a member that loses its
    /// reading, no band or too few prints, keeps showing it until it leaves).
    shown: Option<Risk>,
    /// Board state: since when (clock ns) a member has not been building; kept through a halt
    /// when it was building at the halt.
    idle_since: Option<i64>,
    halt_watch: bool,
}

impl Entry {
    fn new(symbol: Arc<str>) -> Self {
        Entry {
            symbol,
            day: None,
            prints: Segments::default(),
            last_price: None,
            last_side: None,
            last_exec: None,
            session: None,
            session_volume: 0,
            session_start: 0,
            band: None,
            halt: None,
            resumed_at: None,
            frozen: None,
            last: None,
            last_risk: None,
            shown: None,
            idle_since: None,
            halt_watch: false,
        }
    }

    /// The pressure now and a horizon earlier, at `now` (not frozen; rolls the segments).
    fn scores(&mut self, now: i64, minimums: (usize, u64)) -> (Option<Score>, Option<Score>) {
        self.prints.roll(now);
        let now = self.prints.clock.expect("roll sets the clock");
        let (b, s, n) = self.prints.window(false);
        let cur = score_with(
            b,
            s,
            n,
            self.session_volume,
            now - self.session_start,
            minimums,
        );
        let (b, s, n) = self.prints.window(true);
        let before = score_with(
            b,
            s,
            n,
            self.session_volume
                .saturating_sub(self.prints.recent_volume()),
            now - HORIZON - self.session_start,
            (MIN_TRADES, MIN_VOLUME),
        );
        (cur, before)
    }

    /// The pressure and the halt risk at `now` (the frozen ones while halted).
    fn evaluate(&mut self, now: i64, minimums: (usize, u64)) -> (Option<Score>, Option<Risk>) {
        if let Some(f) = self.frozen.filter(|_| self.halt.is_some()) {
            return (f.score, f.risk);
        }
        let (cur, before) = self.scores(now, minimums);
        let risk = match (cur, self.band, self.last_exec) {
            (Some(c), Some(b), Some((_, p))) => {
                Some(halt_risk(c, before, p, self.prints.anchor, &b))
            }
            _ => None,
        };
        (cur, risk)
    }
}

/// One card of the board.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub symbol: Arc<str>,
    pub score: Score,
    pub risk: Risk,
    pub halt: Option<HaltInfo>,
}

#[derive(Debug, Default)]
pub struct PressureBoard {
    index: HashMap<Arc<str>, usize>,
    entries: Vec<Entry>,
    /// The cards shown, in display order (entry indices).
    board: Vec<usize>,
    /// Expired sided prints rejected on this board's ingest path, since construction.
    expired_prints_dropped: u64,
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

    /// Applies one print with the engine's verdict on it. Ordered traffic appends in O(1)
    /// amortized time; a late retained print uses an ordered insert (under the core lock).
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
            // A new trading date starts the symbol over (the tick rule, the windows and the day's
            // average), like the quote board's row. A halt is kept: it ends with its resume. So
            // is the band: the engine's own points set and clear it (the close publishes a null
            // point), and a band point applied before the day's first print belongs to that day.
            _ => {
                let mut fresh = Entry::new(e.symbol.clone());
                fresh.day = Some(session.date);
                fresh.halt = e.halt.take();
                fresh.resumed_at = e.resumed_at;
                fresh.frozen = e.frozen.take();
                fresh.halt_watch = e.halt_watch;
                fresh.band = e.band;
                *e = fresh;
            }
        }
        // A new session restarts the pace baseline only: the tick rule and the windows carry on
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
        let ts = t.event_ts.0;
        if e.last_exec.is_none_or(|(last, _)| ts >= last) {
            e.last_exec = Some((ts, t.price));
        }
        let Some(buy) = side else {
            e.prints.roll(ts);
            return;
        };
        e.last_side = Some(buy);
        if !e.prints.push((ts, t.size, buy, t.price)) {
            self.expired_prints_dropped = self.expired_prints_dropped.saturating_add(1);
        }
    }

    /// A `chart.luld` point from the engine: the band in effect from then (`None` for a null
    /// point: the close, a halt).
    pub fn apply_luld(&mut self, symbol: &str, l: &LuldWire) {
        let i = self.entry_index(symbol);
        self.entries[i].band = Band::from_wire(l);
    }

    /// A `chart.halt` from the engine: a halt starting freezes the symbol's readings as of the
    /// halt (before the engine's null band point that follows it), the resume (the same `start`,
    /// `end` set) releases them.
    pub fn apply_halt(&mut self, symbol: &str, h: &HaltWire) {
        let i = self.entry_index(symbol);
        let member = self.board.contains(&i);
        let e = &mut self.entries[i];
        match h.end {
            None => {
                let at = UnixNanos::from_millis(h.start).0;
                if e.halt
                    .as_ref()
                    .is_some_and(|active| h.start <= active.start)
                    || e.resumed_at.is_some_and(|resume| at <= resume)
                {
                    return;
                }
                // A newer official halt supersedes an older still-open halt, so its matching
                // official resume can terminate carried state. Never fabricate a timer resume.
                e.halt = None;
                e.frozen = None;
                let (score, risk) = e.evaluate(at, (MIN_TRADES / 2, MIN_VOLUME / 2));
                let frozen = Frozen {
                    score: score.or(e.last),
                    risk: risk.or(e.last_risk),
                };
                e.halt_watch = member || frozen.risk.is_some_and(|r| r.building_at_halt());
                e.frozen = Some(frozen);
                e.halt = Some(HaltInfo {
                    start: h.start,
                    reason: h.reason.clone(),
                });
            }
            Some(end) => {
                let resume = UnixNanos::from_millis(end).0;
                if end >= h.start
                    && e.halt.as_ref().is_some_and(|x| x.start == h.start)
                    && e.resumed_at.is_none_or(|previous| resume >= previous)
                {
                    e.resumed_at = Some(resume);
                    e.halt = None;
                    e.frozen = None;
                    e.halt_watch = false;
                    // The exit rule starts afresh at the resume (module docs, "Halts").
                    e.idle_since = None;
                }
            }
        }
    }

    /// Scores every symbol at `now` (epoch ns) and updates the board (module docs, "Entry and
    /// exit"). `eligible` says whether a symbol is inside the price band. Returns the cards in
    /// display order.
    pub fn tick(&mut self, now: i64, eligible: impl Fn(&str) -> bool) -> Vec<Card> {
        // Members are scored against the relaxed bar, everyone else against the full one; the
        // score itself does not depend on the bar.
        let relaxed = (MIN_TRADES / 2, MIN_VOLUME / 2);
        let mut member = vec![false; self.entries.len()];
        for &i in &self.board {
            member[i] = true;
        }
        let readings: Vec<(Option<Score>, Option<Risk>)> = self
            .entries
            .iter_mut()
            .enumerate()
            .map(|(i, e)| {
                let r = e.evaluate(
                    now,
                    if member[i] {
                        relaxed
                    } else {
                        (MIN_TRADES, MIN_VOLUME)
                    },
                );
                e.last = r.0;
                e.last_risk = r.1;
                r
            })
            .collect();
        let elig: Vec<bool> = self.entries.iter().map(|e| eligible(&e.symbol)).collect();
        let ok = |i: usize| elig[i];
        let halted = |e: &Entry| e.halt.is_some();

        // Members: out of the price band leaves at once; halted stays; otherwise the exit rule.
        let mut kept = Vec::with_capacity(BOARD_SIZE);
        for &i in &self.board {
            if !ok(i) {
                continue;
            }
            let e = &mut self.entries[i];
            if halted(e) {
                kept.push(i);
                continue;
            }
            if readings[i].1.is_some_and(|r| r.building()) {
                e.idle_since = None;
                kept.push(i);
                continue;
            }
            let since = *e.idle_since.get_or_insert(now);
            if now - since < HOLD {
                kept.push(i);
            } else {
                e.idle_since = None;
            }
        }
        self.board = kept;
        let rk = |i: usize| readings[i].1.map_or(0, |r| r.risk);
        let entries = &self.entries;
        // Halted symbols that were building at the halt: they stay through it.
        let mut watch: Vec<usize> = (0..entries.len())
            .filter(|&i| !member[i] && halted(&entries[i]) && entries[i].halt_watch && ok(i))
            .collect();
        watch.sort_by(|&a, &b| {
            rk(b)
                .cmp(&rk(a))
                .then_with(|| entries[a].symbol.cmp(&entries[b].symbol))
        });
        for i in watch {
            if self.board.len() < BOARD_SIZE {
                self.board.push(i);
            } else if let Some(pos) = self
                .board
                .iter()
                .enumerate()
                .filter(|&(_, &j)| !halted(&entries[j]))
                .min_by_key(|&(pos, &j)| (rk(j), std::cmp::Reverse(pos)))
                .map(|(pos, _)| pos)
            {
                self.board[pos] = i;
            }
        }
        // Outsiders that pass the entry, best first; ties by symbol, so the board is the same on
        // every run.
        let mut outside: Vec<usize> = (0..entries.len())
            .filter(|&i| {
                !self.board.contains(&i)
                    && !halted(&entries[i])
                    && ok(i)
                    && readings[i].1.is_some_and(|r| r.enters())
            })
            .collect();
        outside.sort_by(|&a, &b| {
            rk(b)
                .cmp(&rk(a))
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
                .filter(|&(_, &i)| !halted(&entries[i]))
                .min_by_key(|&(pos, &i)| (rk(i), std::cmp::Reverse(pos)))
                .map(|(pos, &i)| (pos, i));
            match weakest {
                Some((pos, w)) if rk(best) >= rk(w).saturating_add(SWAP_MARGIN) => {
                    self.board[pos] = best;
                    outside.next();
                }
                _ => break,
            }
        }
        // Order: a card moves above its neighbour only when it leads it by the margin (a stable
        // bubble pass until nothing moves; at most BOARD_SIZE² steps).
        loop {
            let mut moved = false;
            for k in 1..self.board.len() {
                if rk(self.board[k]) >= rk(self.board[k - 1]).saturating_add(SWAP_MARGIN) {
                    self.board.swap(k, k - 1);
                    moved = true;
                }
            }
            if !moved {
                break;
            }
        }
        for &i in &self.board {
            let e = &mut self.entries[i];
            // A newcomer starts with a clean exit clock.
            if !member[i] {
                e.idle_since = None;
            }
            if let Some(r) = readings[i].1 {
                e.shown = Some(r);
            }
        }
        // Every card has a risk (entering needs one; a halt freezes one), so this drops nothing;
        // it only keeps the board and the cards the same list if that ever failed.
        let entries = &self.entries;
        self.board.retain(|&i| entries[i].shown.is_some());
        self.board
            .iter()
            .map(|&i| {
                let e = &entries[i];
                Card {
                    symbol: e.symbol.clone(),
                    score: readings[i].0.unwrap_or(Score {
                        pressure: 0,
                        buy_share: 0.5,
                        pace: 0.0,
                    }),
                    risk: e.shown.expect("retained above"),
                    halt: e.halt.clone(),
                }
            })
            .collect()
    }

    /// Replaces `symbol`'s state with `other`'s (the live market loaded its history: `other` was
    /// built from that history and the events held meanwhile, the quote board's
    /// [`crate::quotes::QuoteBoard::replace_from`]). Board membership and its exit clock are
    /// untouched.
    pub fn replace_from(&mut self, symbol: &str, other: &PressureBoard) {
        let Some(src) = other.index.get(symbol).map(|&j| other.entries[j].clone()) else {
            return;
        };
        let i = self.entry_index(symbol);
        let old = &self.entries[i];
        let (last, last_risk, shown, idle_since) =
            (old.last, old.last_risk, old.shown, old.idle_since);
        self.entries[i] = src;
        let e = &mut self.entries[i];
        e.last = last;
        e.last_risk = last_risk;
        e.shown = shown;
        e.idle_since = idle_since;
    }

    /// One symbol's pressure at `now`, whatever the band (price alerts, tests and diagnostics).
    pub fn score_of(&mut self, symbol: &str, now: i64) -> Option<Score> {
        let &i = self.index.get(symbol)?;
        self.entries[i].evaluate(now, (MIN_TRADES, MIN_VOLUME)).0
    }

    /// One symbol's halt risk at `now`, on the board or not (tests, the calibration bench).
    pub fn risk_of(&mut self, symbol: &str, now: i64) -> Option<Risk> {
        let &i = self.index.get(symbol)?;
        self.entries[i].evaluate(now, (MIN_TRADES, MIN_VOLUME)).1
    }

    /// Active official halt and latest matching resume (epoch ns), for history adoption.
    /// An open halt lasts until a matching official resume or a newer official halt replaces it.
    pub fn halt_state(&self, symbol: &str) -> (bool, Option<i64>) {
        self.index.get(symbol).map_or((false, None), |&i| {
            let e = &self.entries[i];
            (e.halt.is_some(), e.resumed_at)
        })
    }

    /// Read-only ingest telemetry. Counts expired sided prints rejected by this board instance;
    /// restoring another board's symbol state does not reset or import its diagnostic counter.
    pub fn expired_prints_dropped(&self) -> u64 {
        self.expired_prints_dropped
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
    const S: i64 = 1_000_000_000;

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
            b.apply_trade(&trade(sym, from + k as i64 * S, c, size), BOTH);
        }
    }

    fn luld(t: i64, reference: f64, up: f64, down: f64) -> LuldWire {
        LuldWire {
            t: t / 1_000_000,
            t_ns: t,
            reference: Some(reference),
            up: Some(up),
            down: Some(down),
        }
    }

    fn null_luld(t: i64) -> LuldWire {
        LuldWire {
            t: t / 1_000_000,
            t_ns: t,
            reference: None,
            up: None,
            down: None,
        }
    }

    fn halt(start: i64, end: Option<i64>) -> HaltWire {
        HaltWire {
            start: start / 1_000_000,
            start_ns: start,
            end: end.map(|e| e / 1_000_000),
            end_ns: end,
            reason: "LUDP".into(),
            code: "H".into(),
        }
    }

    // --- Pressure (unchanged since PR #38) ---------------------------------------------------

    #[test]
    fn the_formula() {
        // Below the minimums: no score.
        assert_eq!(score(5_000, 0, 9, 5_000, MINUTE), None);
        assert_eq!(score(1_000, 999, 20, 1_999, MINUTE), None);
        // Balanced flow is no pressure, however heavy.
        let s = score(50_000, 50_000, 100, 100_000, 30 * MINUTE).unwrap();
        assert_eq!(s.pressure, 0);
        assert!(s.bull(), "a tie counts as bullish");
        // One-sided at the symbol's ordinary pace: I = 1, R = 1, P = ln 2 / ln 101.
        let s = score(10_000, 0, 50, 300_000, 30 * MINUTE).unwrap();
        assert!((s.pace - 1.0).abs() < 1e-9);
        assert_eq!(s.pressure, 49);
        // 80/20 at 10× pace: I = 0.6, P = ln 11 / ln 101.
        let s = score(80_000, 20_000, 200, 300_000, 30 * MINUTE).unwrap();
        assert!((s.pace - 10.0).abs() < 1e-9);
        assert_eq!(s.pressure, 43);
        // Only a 100/0 minute at 100× pace or beyond fills the gauge, never above 100.
        let s = score(100_000, 0, 200, 100_000, 200 * MINUTE).unwrap();
        assert_eq!(s.pressure, 100, "capped, never above 100");
        // Bearish mirrors bullish.
        let bear = score(20_000, 80_000, 200, 300_000, 30 * MINUTE).unwrap();
        assert!(!bear.bull());
        assert_eq!(bear.pressure, 43);
    }

    #[test]
    fn a_sparse_symbols_first_minute_does_not_explode() {
        // The window is the whole session: R = 1, not V / tiny.
        let s = score(30_000, 0, 30, 30_000, 5 * S).unwrap();
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
            b.apply_trade(&trade("AAA", t0 + k as i64 * S, *c, 1_000), BOTH);
        }
        let s = b.score_of("AAA", t0 + 12 * S).unwrap();
        // 11 prints with a side: buys 501, 501, 502, 502, 502, 503 ×4 = 9; sells 2.
        assert!((s.buy_share - 9.0 / 11.0).abs() < 1e-9, "{s:?}");
    }

    #[test]
    fn excluded_and_volume_only_prints_take_no_part() {
        let mut b = PressureBoard::new();
        let t0 = at(10, 0, 0);
        run(&mut b, "AAA", t0, 12, 500, 500, true);
        let before = b.score_of("AAA", t0 + 12 * S);
        let odd_lot = Counted {
            ohlc: false,
            volume: true,
        };
        // An odd lot far below would be a sell and reset the tick rule: it must not.
        b.apply_trade(&trade("AAA", t0 + 12 * S, 100, 99), odd_lot);
        let none = Counted {
            ohlc: false,
            volume: false,
        };
        b.apply_trade(&trade("AAA", t0 + 12 * S, 100, 9_999), none);
        assert_eq!(b.score_of("AAA", t0 + 12 * S), before);
    }

    #[test]
    fn the_window_is_the_last_minute_at_the_clock() {
        let mut b = PressureBoard::new();
        let t0 = at(10, 0, 0);
        run(&mut b, "AAA", t0, 30, 500, 500, true);
        assert!(b.score_of("AAA", t0 + 30 * S).is_some());
        // A minute after the last print nothing is left in the window.
        assert_eq!(b.score_of("AAA", t0 + 90 * S), None);
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
            let ts = open + k as i64 * 3 * S;
            let c = 600 + (k % 2) as i64;
            b.apply_trade(&trade("AAA", ts, c, 500), BOTH);
        }
        let s = b.score_of("AAA", open + 20 * MINUTE).unwrap();
        // Against the regular session's own average the minute is ordinary (0.95: the last print
        // is 3 s before the clock, so 19 of a minute's 20 are in the window).
        assert!((s.pace - 0.95).abs() < 1e-9, "{s:?}");
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

    #[test]
    fn replace_from_takes_the_loaded_history() {
        let t0 = at(10, 0, 0);
        let mut live = PressureBoard::new();
        run(&mut live, "AAA", t0 + 50 * S, 3, 100, 500, true);
        assert_eq!(live.score_of("AAA", t0 + 53 * S), None);
        let mut hist = PressureBoard::new();
        run(&mut hist, "AAA", t0, 53, 1_000, 500, true);
        hist.apply_luld("AAA", &luld(t0, 5.0, 5.5, 4.5));
        live.replace_from("AAA", &hist);
        assert_eq!(
            live.score_of("AAA", t0 + 53 * S),
            hist.score_of("AAA", t0 + 53 * S)
        );
        assert_eq!(
            live.risk_of("AAA", t0 + 53 * S),
            hist.risk_of("AAA", t0 + 53 * S),
            "the band comes with the history"
        );
    }

    // --- Halt risk ---------------------------------------------------------------------------

    fn sc(pressure: u8, bull: bool) -> Score {
        Score {
            pressure,
            buy_share: if bull { 0.9 } else { 0.1 },
            pace: 3.0,
        }
    }

    fn px(c: i64) -> Price {
        Price::from_cents(c)
    }

    /// A $5.00 reference with a 10 % band: $4.50–$5.50.
    const BAND: Band = Band {
        up: Price::from_cents(550),
        down: Price::from_cents(450),
        reference: Price::from_cents(500),
    };

    #[test]
    fn the_risk_formula() {
        // 30 → 80 in the horizon, $5.20 → $5.30 (1.9 %), 3.8 % (0.2 / 5.3) from the $5.50 band.
        let r = halt_risk(
            sc(80, true),
            Some(sc(30, true)),
            px(530),
            Some(px(520)),
            &BAND,
        );
        assert_eq!(r.rise, 50);
        assert_eq!(r.band, px(550));
        assert!((r.half_width - 0.10).abs() < 1e-12);
        assert!((r.to_band - 0.2 / 5.3).abs() < 1e-12);
        assert!((r.moved - 0.1 / 5.2).abs() < 1e-12);
        let secs = (0.2 / 5.3) * 30.0 / (0.1 / 5.2);
        assert!((r.secs_to_band.unwrap() - secs).abs() < 1e-9);
        let urgency = ((T_FAR / secs).ln() / (T_FAR / T_NEAR).ln()).clamp(0.0, 1.0);
        let close = 1.0 - (0.2 / 5.3) / 0.10;
        let expect = (100.0 * (W_RISE + W_URGENCY * urgency + W_CLOSE * close)).round() as u8;
        assert_eq!(r.risk, expect);
        assert_eq!(r.risk, 78, "the worked example in the module docs");
        assert!(r.enters(), "{r:?}");
        // A steady 90 is not a build-up: no rise, and without movement no urgency.
        let r = halt_risk(
            sc(90, true),
            Some(sc(90, true)),
            px(530),
            Some(px(530)),
            &BAND,
        );
        assert_eq!(r.rise, 0);
        assert_eq!(r.secs_to_band, None);
        assert!(!r.enters() && !r.building(), "{r:?}");
        // The earlier minute on the other side counts as no pressure on this one.
        let r = halt_risk(
            sc(60, true),
            Some(sc(70, false)),
            px(530),
            Some(px(520)),
            &BAND,
        );
        assert_eq!(r.rise, 60);
        // Selling runs at the lower band, mirrored.
        let r = halt_risk(
            sc(80, false),
            Some(sc(30, false)),
            px(470),
            Some(px(480)),
            &BAND,
        );
        assert_eq!(r.band, px(450));
        assert!((r.to_band - 0.2 / 4.7).abs() < 1e-12);
        assert!(r.moved > 0.0 && r.enters(), "{r:?}");
        // At (or past) the band: no distance, full urgency and closeness, pinned.
        let r = halt_risk(
            sc(80, true),
            Some(sc(30, true)),
            px(551),
            Some(px(540)),
            &BAND,
        );
        assert_eq!((r.to_band, r.secs_to_band), (0.0, Some(0.0)));
        assert_eq!(r.risk, 100);
        assert!(r.pinned());
    }

    #[test]
    fn rising_pressure_moving_away_from_the_band_or_far_from_it_does_not_enter() {
        // Buying rises, but the price fell: away from the upper band.
        let r = halt_risk(
            sc(80, true),
            Some(sc(30, true)),
            px(520),
            Some(px(530)),
            &BAND,
        );
        assert!(r.moved < 0.0 && r.secs_to_band.is_none());
        assert!(!r.enters() && !r.building(), "{r:?}");
        // Toward the band, fast, but still most of the band away (closeness < 0.5).
        let r = halt_risk(
            sc(80, true),
            Some(sc(30, true)),
            px(502),
            Some(px(490)),
            &BAND,
        );
        assert!(r.secs_to_band.unwrap() < T_ENTER);
        assert!(!r.enters(), "{r:?}");
        // Close, but the move is bid/ask noise (< 5 % of the half-width).
        let r = halt_risk(
            sc(80, true),
            Some(sc(30, true)),
            px(530),
            Some(px(529)),
            &BAND,
        );
        assert!(!r.enters(), "{r:?}");
        // Close and moving, but the pressure only rose a little: building, not entering.
        let r = halt_risk(
            sc(80, true),
            Some(sc(60, true)),
            px(530),
            Some(px(520)),
            &BAND,
        );
        assert!(!r.enters() && r.building(), "{r:?}");
    }

    // --- The board -----------------------------------------------------------------------------

    /// AAA, $5.00 reference and a 10 % band ($4.50–$5.50) from 10:00. A balanced minute around
    /// $5.00 (alternating ticks, 1,000 a print), then 30 seconds of heavy buying (5,000 a print,
    /// a cent up each, $5.00 → $5.29). Returns the time of the last print.
    fn build_up(b: &mut PressureBoard, t0: i64) -> i64 {
        b.apply_luld("AAA", &luld(t0, 5.0, 5.5, 4.5));
        for k in 0..60 {
            let c = 500 + (k % 2);
            b.apply_trade(&trade("AAA", t0 + k * S, c, 1_000), BOTH);
        }
        run(b, "AAA", t0 + 60 * S, 30, 5_000, 500, true);
        t0 + 89 * S
    }

    #[test]
    fn rising_pressure_toward_a_near_band_makes_a_card() {
        let mut b = PressureBoard::new();
        let t0 = at(10, 0, 0);
        let last = build_up(&mut b, t0);
        let cards = b.tick(last + S, |_| true);
        assert_eq!(cards.len(), 1, "{cards:?}");
        let c = &cards[0];
        assert_eq!(&*c.symbol, "AAA");
        assert!(c.score.bull());
        assert!(c.risk.rise >= ENTER_RISE, "{c:?}");
        assert_eq!(c.risk.band, px(550));
        assert!(c.risk.risk >= ENTER_RISK, "{c:?}");
        // Outside the price band: no card.
        let mut b = PressureBoard::new();
        build_up(&mut b, t0);
        assert!(b.tick(last + S, |_| false).is_empty());
    }

    #[test]
    fn steady_high_pressure_near_the_band_makes_no_card() {
        let mut b = PressureBoard::new();
        let t0 = at(10, 0, 0);
        b.apply_luld("AAA", &luld(t0, 5.0, 5.5, 4.5));
        // One uptick to $5.40, then three minutes of prints at $5.40: every one a buy (zero
        // ticks inherit the side), pressure high and flat, the price still.
        b.apply_trade(&trade("AAA", t0, 539, 1_000), BOTH);
        for k in 1..180 {
            b.apply_trade(&trade("AAA", t0 + k * S, 540, 2_000), BOTH);
        }
        let now = t0 + 180 * S;
        let p = b.score_of("AAA", now).unwrap();
        assert!(p.pressure >= 45 && p.bull(), "{p:?}");
        let r = b.risk_of("AAA", now).unwrap();
        assert!(r.rise.abs() <= 2, "{r:?}");
        assert!(b.tick(now, |_| true).is_empty());
    }

    #[test]
    fn no_modelled_band_no_card() {
        // The same build-up without a LULD band (premarket, after a halt): nothing to run at.
        let mut b = PressureBoard::new();
        let t0 = at(10, 0, 0);
        let last = build_up(&mut b, t0);
        b.apply_luld("AAA", &null_luld(last));
        assert_eq!(b.risk_of("AAA", last + S), None);
        assert!(b.tick(last + S, |_| true).is_empty());
    }

    #[test]
    fn a_card_leaves_after_the_hold_once_the_build_up_stops() {
        let mut b = PressureBoard::new();
        let t0 = at(10, 0, 0);
        let last = build_up(&mut b, t0);
        assert_eq!(b.tick(last + S, |_| true).len(), 1);
        // No more prints. The rise stays positive while the run's prints weigh more in the
        // window now than in the one a horizon earlier; at t0 + 140 s both read 45 (the run's
        // last ten prints now, the whole run a horizon ago at a heavier pace), the price has not
        // moved for the horizon and is 4 % from the band (not pinned), so it is not building, and
        // HOLD later it leaves (not before: a flat second does not blink it off).
        assert!(b.risk_of("AAA", t0 + 139 * S).unwrap().building());
        // (A member is read against half the minimums: nine prints are left at 140 s.)
        let i = b.index["AAA"];
        let relaxed = (MIN_TRADES / 2, MIN_VOLUME / 2);
        let r = b.entries[i].evaluate(t0 + 140 * S, relaxed).1.unwrap();
        assert!(!r.building(), "{r:?}");
        let mut b = PressureBoard::new();
        build_up(&mut b, t0);
        assert_eq!(b.tick(last + S, |_| true).len(), 1);
        let mut gone = None;
        for t in (last + 2 * S..last + 120 * S).step_by(S as usize) {
            if b.tick(t, |_| true).is_empty() {
                gone = Some(t);
                break;
            }
        }
        assert_eq!(gone, Some(t0 + 140 * S + HOLD));
    }

    #[test]
    fn a_halt_keeps_the_card_frozen_until_the_resume_then_the_exit_rule() {
        let mut b = PressureBoard::new();
        let t0 = at(10, 0, 0);
        let last = build_up(&mut b, t0);
        assert_eq!(b.tick(last + S, |_| true).len(), 1);
        let h = last + 2 * S;
        b.apply_halt("AAA", &halt(h, None));
        // The engine clears the band right after the halt.
        b.apply_luld("AAA", &null_luld(h));
        let frozen = b.tick(h + S, |_| true);
        assert_eq!(frozen.len(), 1);
        for t in [h + 60 * S, h + 299 * S] {
            let cards = b.tick(t, |_| true);
            assert_eq!(cards.len(), 1);
            assert_eq!(cards[0].risk, frozen[0].risk, "frozen through the halt");
            assert_eq!(cards[0].score, frozen[0].score);
            assert_eq!(cards[0].halt.as_ref().unwrap().reason, "LUDP");
            assert_eq!(cards[0].risk.band, px(550), "the band as of the halt");
        }
        // The resume: no prints, no band yet, so it is not building and leaves after the hold.
        let r = h + 300 * S;
        b.apply_halt("AAA", &halt(h, Some(r)));
        assert_eq!(b.tick(r + S, |_| true).len(), 1);
        assert!(b.tick(r + S + HOLD, |_| true).is_empty());
    }

    #[test]
    fn a_run_that_halts_before_any_ranking_still_gets_its_card() {
        let mut b = PressureBoard::new();
        let t0 = at(10, 0, 0);
        let last = build_up(&mut b, t0);
        // Halted before the once-a-second ranking saw it on the board.
        b.apply_halt("AAA", &halt(last + S, None));
        b.apply_luld("AAA", &null_luld(last + S));
        let cards = b.tick(last + 2 * S, |_| true);
        assert_eq!(cards.len(), 1);
        assert!(cards[0].halt.is_some());
        // A halted symbol that was not building is not shown.
        let mut b = PressureBoard::new();
        b.apply_luld("BBB", &luld(t0, 5.0, 5.5, 4.5));
        for k in 0..60 {
            b.apply_trade(&trade("BBB", t0 + k * S, 500 + (k % 2), 1_000), BOTH);
        }
        b.apply_halt("BBB", &halt(t0 + 61 * S, None));
        assert!(b.tick(t0 + 62 * S, |_| true).is_empty());
    }

    #[test]
    fn deterministic_under_replay() {
        let t0 = at(10, 0, 0);
        // The same prints and the same ranking clock give the same board, every second.
        let mk = || {
            let mut b = PressureBoard::new();
            let last = build_up(&mut b, t0);
            (last..last + 90 * S)
                .step_by(S as usize)
                .map(|t| b.tick(t, |_| true))
                .collect::<Vec<_>>()
        };
        assert_eq!(mk(), mk());
        // The risk is a pure function of the prints and the clock: ranking every second on the
        // way, or not at all, reads the same at the end.
        let mut ticked = PressureBoard::new();
        let mut once = PressureBoard::new();
        let last = build_up(&mut ticked, t0);
        build_up(&mut once, t0);
        for t in (t0..last).step_by(S as usize) {
            ticked.tick(t, |_| true);
        }
        let now = last + 3 * S;
        assert_eq!(ticked.risk_of("AAA", now), once.risk_of("AAA", now));
        assert_eq!(ticked.score_of("AAA", now), once.score_of("AAA", now));
    }

    #[test]
    fn at_most_six_strongest_first_with_the_margin() {
        let mut b = PressureBoard::new();
        let t0 = at(10, 0, 0);
        // Eight symbols with the same build-up, their upper bands further away one by one: the
        // closer, the riskier.
        let syms = ["A", "B", "C", "D", "E", "F", "G", "H"];
        for (k, s) in syms.iter().enumerate() {
            let up = 5.32 + 0.02 * k as f64;
            b.apply_luld(s, &luld(t0, 5.0, up, 4.5));
            for j in 0..60 {
                b.apply_trade(&trade(s, t0 + j * S, 500 + (j % 2), 1_000), BOTH);
            }
            run(&mut b, s, t0 + 60 * S, 30, 5_000, 500, true);
        }
        let cards = b.tick(t0 + 90 * S, |_| true);
        assert_eq!(cards.len(), BOARD_SIZE);
        let on: Vec<&str> = cards.iter().map(|c| &*c.symbol).collect();
        assert!(!on.contains(&"G") && !on.contains(&"H"), "{on:?}");
        assert!(
            cards
                .windows(2)
                .all(|w| w[0].risk.risk + SWAP_MARGIN > w[1].risk.risk),
            "no card leads its upper neighbour by the margin"
        );
    }

    #[test]
    fn the_lagged_window_is_the_minute_a_horizon_ago() {
        let mut s = Segments::default();
        let t0 = at(10, 0, 0);
        for k in 0..120 {
            s.push((t0 + k * S, 100, k < 60, px(500 + k)));
        }
        s.roll(t0 + 119 * S);
        // Now: (59, 119] = 60 prints, all sells; a horizon ago: (29, 89] = 30 buys, 30 sells.
        assert_eq!(s.window(false), (0, 6_000, 60));
        assert_eq!(s.window(true), (3_000, 3_000, 60));
        // The last price at or before now − H (89 s).
        assert_eq!(s.anchor, Some(px(589)));
        // Minutes later every print has moved through.
        s.roll(t0 + 119 * S + 10 * MINUTE);
        assert_eq!(s.window(false), (0, 0, 0));
        assert_eq!(s.window(true), (0, 0, 0));
        assert_eq!(s.anchor, Some(px(619)));
    }

    #[test]
    fn late_print_is_inserted_into_its_current_execution_segment() {
        let mut s = Segments::default();
        let t0 = at(10, 0, 0);
        s.roll(t0 + 100 * S);
        s.push((t0 + 15 * S, 100, true, px(501)));
        s.push((t0 + 55 * S, 200, false, px(502)));
        s.push((t0 + 85 * S, 300, true, px(503)));
        assert_eq!(s.window(false), (300, 200, 2));
        assert_eq!(s.window(true), (100, 200, 2));
        assert_eq!(s.anchor, Some(px(502)));
    }

    #[test]
    fn expired_late_print_cannot_reenter_a_closed_window() {
        let mut s = Segments::default();
        let t0 = at(10, 0, 0);
        s.roll(t0 + 100 * S);
        s.push((t0 + 10 * S, 100_000, true, px(500)));
        assert_eq!(s.window(false), (0, 0, 0));
        assert_eq!(s.window(true), (0, 0, 0));
    }

    #[test]
    fn late_anchor_uses_the_newest_execution_before_the_horizon() {
        let mut s = Segments::default();
        let t0 = at(10, 0, 0);
        s.push((t0 + 60 * S, 100, true, px(500)));
        s.roll(t0 + 100 * S);
        s.push((t0 + 65 * S, 100, true, px(510)));
        s.push((t0 + 62 * S, 100, true, px(505)));
        assert_eq!(s.anchor, Some(px(510)));
    }

    #[test]
    fn late_print_does_not_replace_the_latest_execution_price_in_risk() {
        let t0 = at(10, 0, 0);
        let mut b = PressureBoard::new();
        b.apply_luld("AAA", &luld(t0, 1.0, 1.1, 0.9));
        for k in 0..=10 {
            b.apply_trade(&trade("AAA", t0 + k * S, 100 + k, 1_000), BOTH);
        }
        assert_eq!(b.risk_of("AAA", t0 + 11 * S).unwrap().to_band, 0.0);
        let late = Trade::new(
            "AAA",
            UnixNanos(t0 + 9 * S + S / 2),
            Price::from_f64_rounded(1.095).unwrap(),
            1_000,
            1,
        );
        b.apply_trade(&late, BOTH);
        assert_eq!(b.risk_of("AAA", t0 + 11 * S).unwrap().to_band, 0.0);
    }

    #[test]
    fn expired_large_late_print_does_not_outlive_newer_front_prints() {
        let t0 = at(10, 0, 0);
        let mut b = PressureBoard::new();
        b.apply_trade(&trade("AAA", t0, 100, 1_000), BOTH);
        for k in 0..10 {
            let t = Trade::new(
                "AAA",
                UnixNanos(t0 + 61 * S + k * S / 20),
                Price::from_micros(1_100_000 + k * 1_000),
                1_000,
                1,
            );
            b.apply_trade(&t, BOTH);
        }
        let late = Trade::new(
            "AAA",
            UnixNanos(t0 + 60 * S + 9 * S / 10),
            Price::from_micros(1_105_000),
            100_000,
            1,
        );
        b.apply_trade(&late, BOTH);
        assert_eq!(
            b.score_of("AAA", t0 + 121 * S),
            None,
            "only nine newer prints remain, below the unchanged ten-print minimum"
        );
    }

    #[test]
    fn segment_boundaries_are_open_on_the_left_and_closed_on_the_right() {
        let t0 = at(10, 0, 0);
        let mut s = Segments::default();
        s.roll(t0 + 100 * S);
        assert!(!s.push((t0 + 10 * S, 100_000, true, px(500))));
        for (ts, size) in [
            (10 * S + 1, 1),
            (40 * S, 2),
            (40 * S + 1, 4),
            (70 * S, 8),
            (70 * S + 1, 16),
            (100 * S, 32),
        ] {
            assert!(s.push((t0 + ts, size, true, px(500))));
        }
        assert_eq!(s.sums, [(3, 0), (12, 0), (48, 0)]);
        assert_eq!(s.window(false), (60, 0, 4));
        assert_eq!(s.window(true), (15, 0, 4));
    }

    #[test]
    fn the_roll_clock_and_execution_price_never_move_backward() {
        let t0 = at(10, 0, 0);
        let mut b = PressureBoard::new();
        run(&mut b, "AAA", t0, 120, 1_000, 500, true);
        let expected = b.score_of("AAA", t0 + 120 * S);
        assert_eq!(b.score_of("AAA", t0 + 90 * S), expected);
        b.apply_trade(&trade("AAA", t0 + 80 * S, 100, 1_000), BOTH);
        let e = &b.entries[b.index["AAA"]];
        assert_eq!(e.prints.clock, Some(t0 + 120 * S));
        assert_eq!(e.last_exec, Some((t0 + 119 * S, px(619))));
        assert_eq!(
            e.last_price,
            Some(px(100)),
            "tick rule remains in arrival order"
        );
    }

    #[test]
    fn equal_execution_timestamps_preserve_arrival_order() {
        let t0 = at(10, 0, 0);
        let mut s = Segments::default();
        s.roll(t0 + 100 * S);
        for (seconds, cents) in [(95, 501), (94, 502), (94, 503), (65, 504), (65, 505)] {
            s.push((t0 + seconds * S, 100, true, px(cents)));
        }
        assert_eq!(
            s.seg[2].iter().map(|p| p.3).collect::<Vec<_>>(),
            vec![px(502), px(503), px(501)]
        );
        assert_eq!(s.anchor, Some(px(505)));
        let mut b = PressureBoard::new();
        for cents in [501, 502] {
            b.apply_trade(&trade("AAA", t0 + 100 * S, cents, 100), BOTH);
        }
        assert_eq!(
            b.entries[b.index["AAA"]].last_exec,
            Some((t0 + 100 * S, px(502)))
        );
    }

    #[test]
    fn fixed_side_distinct_time_permutations_have_identical_windows_and_anchor() {
        let t0 = at(10, 0, 0);
        let prints = [
            (t0 + 11 * S, 10, true, px(501)),
            (t0 + 40 * S, 20, false, px(502)),
            (t0 + 41 * S, 30, true, px(503)),
            (t0 + 70 * S, 40, false, px(504)),
            (t0 + 71 * S, 50, true, px(505)),
            (t0 + 99 * S, 60, false, px(506)),
        ];
        fn permute(prefix: usize, order: &mut [usize; 6], check: &mut impl FnMut(&[usize; 6])) {
            if prefix == order.len() {
                check(order);
                return;
            }
            for i in prefix..order.len() {
                order.swap(prefix, i);
                permute(prefix + 1, order, check);
                order.swap(prefix, i);
            }
        }
        let mut checked = 0;
        permute(0, &mut [0, 1, 2, 3, 4, 5], &mut |order| {
            let mut s = Segments::default();
            s.roll(t0 + 100 * S);
            let mut b = PressureBoard::new();
            for &i in order {
                s.push(prints[i]);
                let p = prints[i];
                b.apply_trade(&Trade::new("AAA", UnixNanos(p.0), p.3, p.1, 1), BOTH);
            }
            assert_eq!(s.window(false), (80, 100, 4), "{order:?}");
            assert_eq!(s.window(true), (40, 60, 4), "{order:?}");
            assert_eq!(s.anchor, Some(px(504)), "{order:?}");
            assert_eq!(s.anchor_ts, Some(t0 + 70 * S));
            assert_eq!(
                b.entries[b.index["AAA"]].last_exec,
                Some((t0 + 99 * S, px(506))),
                "{order:?}"
            );
            s.roll(t0 + 110 * S);
            assert_eq!(s.window(false), (50, 100, 3), "{order:?}");
            assert_eq!(s.window(true), (80, 60, 4), "{order:?}");
            checked += 1;
        });
        assert_eq!(checked, 720);
    }

    #[test]
    fn first_no_side_print_still_sets_the_latest_execution_price() {
        let t0 = at(10, 0, 0);
        let mut b = PressureBoard::new();
        b.apply_trade(&trade("AAA", t0, 500, 100), BOTH);
        let e = &b.entries[b.index["AAA"]];
        assert_eq!(e.last_exec, Some((t0, px(500))));
        assert_eq!(e.last_side, None);
        assert_eq!(e.prints.window(false), (0, 0, 0));
    }

    #[test]
    fn expired_drop_telemetry_is_read_only_and_survives_restore_and_day_roll() {
        let t0 = at(10, 0, 0);
        let mut b = PressureBoard::new();
        b.apply_trade(&trade("AAA", t0, 500, 100), BOTH);
        b.score_of("AAA", t0 + 100 * S);
        b.apply_trade(&trade("AAA", t0 + 10 * S, 501, 100_000), BOTH);
        assert_eq!(b.expired_prints_dropped(), 1);
        assert_eq!(b.expired_prints_dropped(), 1);
        let mut hist = PressureBoard::new();
        hist.apply_trade(&trade("AAA", t0 + 100 * S, 502, 100), BOTH);
        b.replace_from("AAA", &hist);
        let next = et(NaiveDate::from_ymd_opt(2026, 10, 19).unwrap(), 4, 0, 0, 0)
            .unwrap()
            .0;
        b.apply_trade(&trade("AAA", next, 503, 100), BOTH);
        assert_eq!(b.expired_prints_dropped(), 1);
    }

    #[test]
    fn only_a_valid_matching_resume_clears_the_active_halt() {
        let t0 = at(10, 0, 0);
        let mut b = PressureBoard::new();
        assert_eq!(b.halt_state("AAA"), (false, None));
        b.apply_halt("AAA", &halt(t0, None));
        b.apply_halt("AAA", &halt(t0 - S, Some(t0 + 20 * S)));
        b.apply_halt("AAA", &halt(t0, Some(t0 - S)));
        assert_eq!(b.halt_state("AAA"), (true, None));
        b.apply_halt("AAA", &halt(t0, Some(t0 + 60 * S)));
        assert_eq!(b.halt_state("AAA"), (false, Some(t0 + 60 * S)));
        // A delayed original start cannot reopen an officially completed halt.
        b.apply_halt("AAA", &halt(t0, None));
        assert_eq!(b.halt_state("AAA"), (false, Some(t0 + 60 * S)));
        b.apply_halt("AAA", &halt(t0 + 120 * S, None));
        b.apply_halt("AAA", &halt(t0, Some(t0 + 60 * S)));
        assert_eq!(b.halt_state("AAA"), (true, Some(t0 + 60 * S)));
        b.apply_halt("AAA", &halt(t0 + 120 * S, Some(t0 + 180 * S)));
        assert_eq!(b.halt_state("AAA"), (false, Some(t0 + 180 * S)));
    }

    #[test]
    fn official_halt_state_survives_restore_day_roll_and_newer_halt_supersession() {
        let t0 = at(10, 0, 0);
        let mut hist = PressureBoard::new();
        hist.apply_trade(&trade("AAA", t0, 500, 100), BOTH);
        hist.apply_halt("AAA", &halt(t0, None));
        hist.apply_halt("AAA", &halt(t0, Some(t0 + 60 * S)));
        hist.apply_halt("AAA", &halt(t0 + 120 * S, None));
        let mut live = PressureBoard::new();
        live.replace_from("AAA", &hist);
        assert_eq!(live.halt_state("AAA"), (true, Some(t0 + 60 * S)));
        let next = et(NaiveDate::from_ymd_opt(2026, 10, 19).unwrap(), 4, 0, 0, 0)
            .unwrap()
            .0;
        live.apply_trade(&trade("AAA", next, 500, 100), BOTH);
        assert_eq!(live.halt_state("AAA"), (true, Some(t0 + 60 * S)));
        // A newer official start replaces stale carried state and gives it a terminal resume.
        live.apply_halt("AAA", &halt(next + S, None));
        live.apply_halt("AAA", &halt(t0 + 120 * S, Some(next + 2 * S)));
        assert_eq!(live.halt_state("AAA"), (true, Some(t0 + 60 * S)));
        live.apply_halt("AAA", &halt(next + S, Some(next + 60 * S)));
        assert_eq!(live.halt_state("AAA"), (false, Some(next + 60 * S)));
        let mut adopted = PressureBoard::new();
        adopted.replace_from("AAA", &live);
        assert_eq!(adopted.halt_state("AAA"), (false, Some(next + 60 * S)));
    }
}
