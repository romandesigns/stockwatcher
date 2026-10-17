//! # paper-broker
//!
//! The simulated broker behind chart trading (docs/architecture/chart-trading-sim.md is the
//! contract). One paper account: orders, matching against the tape, signed positions, cash and
//! realized P&L. Nothing here is real money and nothing leaves the process.
//!
//! The crate is **pure and deterministic**: no I/O and no clock of its own. The server feeds it
//! every price-qualifying trade (`trade_effect(...).updates_ohlc`, the same filter as OHLC) with
//! that trade's time, and every command with the virtual clock's time. Given the same sequence of
//! calls it produces the same state, which is what lets a replay seek rebuild the market and
//! simply [`Broker::reset`] the account instead of trying to patch it.
//!
//! Units: prices are [`Price`] (micro-dollars, exact); money totals (cash, cost basis, P&L) are
//! `i128` micro-dollars so `qty × price` sums can never overflow; quantities are signed `i64`
//! shares (a negative position is a short); times are epoch milliseconds. Dollars as `f64` exist
//! only in [`wire`], the JSON edge.

pub mod wire;

use market_events::Price;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::fmt;

/// The account's starting cash: $100,000.
pub const STARTING_CASH_MICROS: i128 = 100_000 * 1_000_000;
/// Largest order size accepted, in shares.
pub const MAX_QTY: i64 = 1_000_000;
/// How many finished (filled, cancelled, rejected) orders stay in the state, newest kept.
pub const FINISHED_RETAINED: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    /// +1 for a buy, -1 for a sell: the sign of the position change.
    fn sign(self) -> i64 {
        match self {
            Side::Buy => 1,
            Side::Sell => -1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderType {
    Market,
    Limit,
    Stop,
    StopLimit,
}

impl OrderType {
    fn takes_limit(self) -> bool {
        matches!(self, OrderType::Limit | OrderType::StopLimit)
    }

    fn takes_stop(self) -> bool {
        matches!(self, OrderType::Stop | OrderType::StopLimit)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Open,
    /// A stop-limit whose stop was crossed: now a working limit order.
    Triggered,
    Filled,
    Cancelled,
    Rejected,
}

impl Status {
    /// Still able to fill (and to be modified or cancelled).
    pub fn is_working(self) -> bool {
        matches!(self, Status::Open | Status::Triggered)
    }
}

/// Why a well-formed order was created already `rejected`. Also used as the error code when a
/// modification carries an invalid value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    /// Not a whole number of shares in `1..=MAX_QTY`.
    BadQty,
    /// Not in the market's symbol list.
    UnknownSymbol,
    /// A limit/stop-limit without `limitPrice`, or a stop/stop-limit without `stopPrice`.
    MissingPrice,
    /// Zero or negative.
    BadPrice,
    /// Off the Reg NMS tick grid ($0.01 at or above $1.00, $0.0001 below).
    SubPenny,
}

impl RejectReason {
    pub fn code(self) -> &'static str {
        match self {
            RejectReason::BadQty => "bad_qty",
            RejectReason::UnknownSymbol => "unknown_symbol",
            RejectReason::MissingPrice => "missing_price",
            RejectReason::BadPrice => "bad_price",
            RejectReason::SubPenny => "sub_penny",
        }
    }
}

/// A modify or cancel that was refused. The order is left exactly as it was and the state does
/// not change, so the server answers only the sender (an `error` frame) and broadcasts nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandError {
    /// The order is filled, cancelled, rejected, or not known (never existed, aged out of the
    /// retained list, or wiped by a reset). All mean the same to a client: nothing to change.
    NotOpen,
    /// The request does not make sense for this order (no fields, a price the order type does not
    /// have, or a stop change after the stop already triggered).
    BadOrder(&'static str),
    /// A new value fails the same validation as `order.place`.
    Invalid(RejectReason),
}

impl CommandError {
    pub fn code(self) -> &'static str {
        match self {
            CommandError::NotOpen => "not_open",
            CommandError::BadOrder(_) => "bad_order",
            CommandError::Invalid(r) => r.code(),
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            CommandError::NotOpen => "order is not open",
            CommandError::BadOrder(m) => m,
            CommandError::Invalid(RejectReason::BadQty) => "qty must be a whole number 1..1000000",
            CommandError::Invalid(RejectReason::UnknownSymbol) => "unknown symbol",
            CommandError::Invalid(RejectReason::MissingPrice) => "missing price",
            CommandError::Invalid(RejectReason::BadPrice) => "price must be positive",
            CommandError::Invalid(RejectReason::SubPenny) => "price is off the tick grid",
        }
    }
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code(), self.message())
    }
}

impl std::error::Error for CommandError {}

/// True when `p` is on the Reg NMS grid: whole cents at or above $1.00, $0.0001 below. The chart
/// snaps drags to this grid, so an off-grid price is a client bug and is rejected, not rounded.
pub fn on_tick_grid(p: Price) -> bool {
    let tick = if p >= Price::from_cents(100) {
        10_000
    } else {
        100
    };
    p.micros() % tick == 0
}

fn check_qty(qty: i64) -> Result<(), RejectReason> {
    if (1..=MAX_QTY).contains(&qty) {
        Ok(())
    } else {
        Err(RejectReason::BadQty)
    }
}

fn check_price(p: Price) -> Result<(), RejectReason> {
    if !p.is_positive() {
        Err(RejectReason::BadPrice)
    } else if !on_tick_grid(p) {
        Err(RejectReason::SubPenny)
    } else {
        Ok(())
    }
}

/// An `order.place` after the JSON edge: prices already exact.
#[derive(Clone, Debug, PartialEq)]
pub struct NewOrder {
    pub client_order_id: Option<String>,
    pub symbol: String,
    pub side: Side,
    pub order_type: OrderType,
    /// Shares. The edge passes 0 for a JSON number that is not whole, so it is rejected as
    /// `bad_qty` like any other bad size.
    pub qty: i64,
    pub limit_price: Option<Price>,
    pub stop_price: Option<Price>,
}

/// An `order.modify` after the JSON edge. `None` leaves the field alone.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Modify {
    pub qty: Option<i64>,
    pub limit_price: Option<Price>,
    pub stop_price: Option<Price>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Order {
    /// Numeric part of the wire id `o-<n>`; also the creation order.
    pub num: u64,
    pub client_order_id: Option<String>,
    pub symbol: String,
    pub side: Side,
    pub order_type: OrderType,
    pub qty: i64,
    pub filled_qty: i64,
    pub avg_fill_price: Option<Price>,
    pub limit_price: Option<Price>,
    pub stop_price: Option<Price>,
    pub status: Status,
    pub reject_reason: Option<RejectReason>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Order {
    /// The wire id, e.g. `o-7`.
    pub fn id(&self) -> String {
        format!("o-{}", self.num)
    }
}

/// Parses a wire id `o-<n>`.
fn parse_id(id: &str) -> Option<u64> {
    id.strip_prefix("o-")?.parse().ok()
}

/// One execution. Every fill is a whole order (no partials, see the contract).
#[derive(Clone, Debug, PartialEq)]
pub struct Fill {
    pub order_num: u64,
    pub symbol: String,
    pub side: Side,
    pub qty: i64,
    pub price: Price,
    /// The filling trade's time, epoch ms.
    pub t: i64,
}

/// One symbol's open position. `qty` is signed (negative = short); `basis` is the signed cost of
/// the open shares, so the average price is `basis / qty` and is always positive.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Position {
    pub qty: i64,
    pub basis: i128,
    /// P&L realized on this position since it was last flat.
    pub realized: i128,
}

impl Position {
    /// Average entry price in micro-dollars (fractional: an average need not be on any grid).
    pub fn avg_price_micros(&self) -> f64 {
        if self.qty == 0 {
            0.0
        } else {
            self.basis as f64 / self.qty as f64
        }
    }

    /// Applies a signed fill of `delta` shares at `price` and returns the P&L it realized.
    ///
    /// Reducing a position realizes `(exit − avg) × closed` (sign-flipped for a short) and removes
    /// the closed shares' proportional share of the basis. Crossing through zero closes the whole
    /// old side first, then opens the remainder fresh at the fill price.
    fn apply(&mut self, delta: i64, price: Price) -> i128 {
        let p = price.micros() as i128;
        if self.qty == 0 || self.qty.signum() == delta.signum() {
            self.qty += delta;
            self.basis += delta as i128 * p;
            return 0;
        }
        let held = self.qty.unsigned_abs() as i128;
        let closing = held.min(delta.unsigned_abs() as i128);
        // When everything closes this is exactly `basis`, so no rounding residue is left behind.
        let removed = self.basis * closing / held;
        let pnl = self.qty.signum() as i128 * closing * p - removed;
        self.basis -= removed;
        self.qty += delta.signum() * closing as i64;
        let remaining = delta.unsigned_abs() as i128 - closing;
        if remaining > 0 {
            self.qty = delta.signum() * remaining as i64;
            self.basis = delta.signum() as i128 * remaining * p;
        }
        self.realized += pnl;
        pnl
    }
}

/// What one trade did. `changed` is true when anything in the state moved (a fill or a stop-limit
/// trigger), i.e. when a fresh `paper.state` must go out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TradeOutcome {
    pub fills: Vec<Fill>,
    pub changed: bool,
}

/// The paper account. See the crate docs.
#[derive(Clone, Debug)]
pub struct Broker {
    /// Bumped on every state change and never reset, so a client can drop a stale `paper.state`.
    seq: u64,
    /// Next order number. Never reset either, so an id seen before a reset is never reused for a
    /// different order after it.
    next_num: u64,
    cash: i128,
    realized: i128,
    /// Working (open or triggered) orders in creation order; that order decides who fills first
    /// within one trade.
    working: Vec<Order>,
    /// Finished orders, oldest first, at most [`FINISHED_RETAINED`].
    finished: VecDeque<Order>,
    positions: BTreeMap<String, Position>,
}

impl Default for Broker {
    fn default() -> Self {
        Broker::new()
    }
}

impl Broker {
    pub fn new() -> Self {
        Broker {
            seq: 0,
            next_num: 1,
            cash: STARTING_CASH_MICROS,
            realized: 0,
            working: Vec::new(),
            finished: VecDeque::new(),
            positions: BTreeMap::new(),
        }
    }

    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Cash in micro-dollars. Buying power is not enforced, so this can go negative.
    pub fn cash(&self) -> i128 {
        self.cash
    }

    /// Account-wide realized P&L in micro-dollars, including positions that went flat.
    pub fn realized(&self) -> i128 {
        self.realized
    }

    pub fn position(&self, symbol: &str) -> Option<&Position> {
        self.positions.get(symbol)
    }

    pub fn positions(&self) -> impl Iterator<Item = (&str, &Position)> {
        self.positions.iter().map(|(s, p)| (s.as_str(), p))
    }

    /// Every working order plus the retained finished ones, in creation order.
    pub fn orders(&self) -> Vec<&Order> {
        let mut v: Vec<&Order> = self.working.iter().chain(self.finished.iter()).collect();
        v.sort_by_key(|o| o.num);
        v
    }

    pub fn order(&self, id: &str) -> Option<&Order> {
        let num = parse_id(id)?;
        self.working
            .iter()
            .chain(self.finished.iter())
            .find(|o| o.num == num)
    }

    fn finish(&mut self, order: Order) {
        if self.finished.len() == FINISHED_RETAINED {
            self.finished.pop_front();
        }
        self.finished.push_back(order);
    }

    /// Creates an order. Always changes the state: a well-formed order that fails validation is
    /// still created, as `rejected` with a reason, so the rejection shows up for every client.
    /// `known` answers whether a symbol is in the market's full symbol list. Returns the wire id.
    pub fn place(&mut self, req: NewOrder, now: i64, known: impl Fn(&str) -> bool) -> String {
        let num = self.next_num;
        self.next_num += 1;
        let limit_price = req.limit_price.filter(|_| req.order_type.takes_limit());
        let stop_price = req.stop_price.filter(|_| req.order_type.takes_stop());
        let reject = Self::validate(&req, limit_price, stop_price, &known).err();
        let order = Order {
            num,
            client_order_id: req.client_order_id,
            symbol: req.symbol,
            side: req.side,
            order_type: req.order_type,
            qty: req.qty,
            filled_qty: 0,
            avg_fill_price: None,
            limit_price,
            stop_price,
            status: if reject.is_some() {
                Status::Rejected
            } else {
                Status::Open
            },
            reject_reason: reject,
            created_at: now,
            updated_at: now,
        };
        let id = order.id();
        if reject.is_some() {
            self.finish(order);
        } else {
            self.working.push(order);
        }
        self.seq += 1;
        id
    }

    /// First failing check wins: size, symbol, presence of prices, then each price's value.
    fn validate(
        req: &NewOrder,
        limit: Option<Price>,
        stop: Option<Price>,
        known: &impl Fn(&str) -> bool,
    ) -> Result<(), RejectReason> {
        check_qty(req.qty)?;
        if !known(&req.symbol) {
            return Err(RejectReason::UnknownSymbol);
        }
        if (req.order_type.takes_limit() && limit.is_none())
            || (req.order_type.takes_stop() && stop.is_none())
        {
            return Err(RejectReason::MissingPrice);
        }
        for p in [limit, stop].into_iter().flatten() {
            check_price(p)?;
        }
        Ok(())
    }

    /// Changes a working order in place (same id, same place in the fill queue).
    pub fn modify(&mut self, id: &str, m: Modify, now: i64) -> Result<(), CommandError> {
        let num = parse_id(id).ok_or(CommandError::NotOpen)?;
        let order = self
            .working
            .iter_mut()
            .find(|o| o.num == num)
            .ok_or(CommandError::NotOpen)?;
        if m.qty.is_none() && m.limit_price.is_none() && m.stop_price.is_none() {
            return Err(CommandError::BadOrder("nothing to modify"));
        }
        if m.limit_price.is_some() && !order.order_type.takes_limit() {
            return Err(CommandError::BadOrder("this order type has no limit price"));
        }
        if m.stop_price.is_some() && !order.order_type.takes_stop() {
            return Err(CommandError::BadOrder("this order type has no stop price"));
        }
        // A triggered stop-limit is already a plain limit; moving its stop would change nothing
        // and silently accepting it would mislead the client about what is working.
        if m.stop_price.is_some() && order.status == Status::Triggered {
            return Err(CommandError::BadOrder("stop already triggered"));
        }
        if let Some(q) = m.qty {
            check_qty(q).map_err(CommandError::Invalid)?;
        }
        for p in [m.limit_price, m.stop_price].into_iter().flatten() {
            check_price(p).map_err(CommandError::Invalid)?;
        }
        if let Some(q) = m.qty {
            order.qty = q;
        }
        if let Some(p) = m.limit_price {
            order.limit_price = Some(p);
        }
        if let Some(p) = m.stop_price {
            order.stop_price = Some(p);
        }
        order.updated_at = now;
        self.seq += 1;
        Ok(())
    }

    pub fn cancel(&mut self, id: &str, now: i64) -> Result<(), CommandError> {
        let num = parse_id(id).ok_or(CommandError::NotOpen)?;
        let idx = self
            .working
            .iter()
            .position(|o| o.num == num)
            .ok_or(CommandError::NotOpen)?;
        let mut order = self.working.remove(idx);
        order.status = Status::Cancelled;
        order.updated_at = now;
        self.finish(order);
        self.seq += 1;
        Ok(())
    }

    /// Flat account, no orders. Used by `paper.reset` and by every replay rebuild (seek): orders
    /// cannot survive the tape being rewritten under them.
    pub fn reset(&mut self) {
        self.cash = STARTING_CASH_MICROS;
        self.realized = 0;
        self.working.clear();
        self.finished.clear();
        self.positions.clear();
        self.seq += 1;
    }

    /// Feeds one price-qualifying trade (the caller applies the OHLC filter). Matching follows the
    /// contract's table; every order fills in full on one trade, and within one trade working
    /// orders are visited in creation order.
    pub fn on_trade(&mut self, symbol: &str, price: Price, t: i64) -> TradeOutcome {
        let mut out = TradeOutcome::default();
        if !self.working.iter().any(|o| o.symbol == symbol) {
            return out;
        }
        let mut filled = Vec::new();
        for (idx, o) in self.working.iter_mut().enumerate() {
            if o.symbol != symbol {
                continue;
            }
            // The status is read before this trade touches the order, so a stop-limit triggered
            // by this very trade is not also limit-checked against it ("from the next trade").
            let fill_at = match (o.order_type, o.status) {
                (OrderType::Market, Status::Open) => Some(price),
                (OrderType::Limit, Status::Open) | (OrderType::StopLimit, Status::Triggered) => {
                    let l = o.limit_price.expect("validated limit");
                    // Traded through, not touched: a print at the limit does not prove our
                    // order (behind an unknown queue) would have been reached.
                    let through = match o.side {
                        Side::Buy => price < l,
                        Side::Sell => price > l,
                    };
                    through.then_some(l)
                }
                (OrderType::Stop, Status::Open) => {
                    let s = o.stop_price.expect("validated stop");
                    // A stop becomes a market order when hit and fills on the same print.
                    stop_hit(o.side, price, s).then_some(price)
                }
                (OrderType::StopLimit, Status::Open) => {
                    if stop_hit(o.side, price, o.stop_price.expect("validated stop")) {
                        o.status = Status::Triggered;
                        o.updated_at = t;
                        out.changed = true;
                    }
                    None
                }
                _ => None,
            };
            if let Some(p) = fill_at {
                o.status = Status::Filled;
                o.filled_qty = o.qty;
                o.avg_fill_price = Some(p);
                o.updated_at = t;
                filled.push(idx);
                out.fills.push(Fill {
                    order_num: o.num,
                    symbol: o.symbol.clone(),
                    side: o.side,
                    qty: o.qty,
                    price: p,
                    t,
                });
            }
        }
        for f in &out.fills {
            self.apply_fill(f);
        }
        // Remove back to front so earlier indices stay valid.
        let mut done = Vec::with_capacity(filled.len());
        for idx in filled.into_iter().rev() {
            done.push(self.working.remove(idx));
        }
        for o in done.into_iter().rev() {
            self.finish(o);
        }
        if !out.fills.is_empty() {
            out.changed = true;
        }
        if out.changed {
            self.seq += 1;
        }
        out
    }

    fn apply_fill(&mut self, f: &Fill) {
        let delta = f.side.sign() * f.qty;
        // Buys debit, sells credit; buying power is deliberately not enforced.
        self.cash -= delta as i128 * f.price.micros() as i128;
        let pos = self.positions.entry(f.symbol.clone()).or_default();
        self.realized += pos.apply(delta, f.price);
        if pos.qty == 0 {
            // Flat: dropped from the list; its realized P&L already lives in the account total.
            self.positions.remove(&f.symbol);
        }
    }
}

fn stop_hit(side: Side, price: Price, stop: Price) -> bool {
    match side {
        Side::Buy => price >= stop,
        Side::Sell => price <= stop,
    }
}

#[cfg(test)]
mod tests;
