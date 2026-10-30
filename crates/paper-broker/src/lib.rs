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
/// How many fills `paper.state` carries (docs/architecture/halts-luld-fills.md section 3), newest
/// kept. Independent of [`FINISHED_RETAINED`]: a fill outlives its order's place in the list.
pub const FILLS_RETAINED: usize = 200;

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
    /// A bracket exit waiting for its entry to fill. It neither fills nor triggers, but it is
    /// working: it can be modified and cancelled, and it is never aged out of the state.
    Pending,
    Open,
    /// A stop-limit whose stop was crossed: now a working limit order.
    Triggered,
    Filled,
    Cancelled,
    Rejected,
}

impl Status {
    /// Not finished: able to fill (or, for `pending`, about to be able to) and to be modified or
    /// cancelled. Matches the client's `isWorking`, which shows a pending exit's line faintly.
    pub fn is_working(self) -> bool {
        matches!(self, Status::Pending | Status::Open | Status::Triggered)
    }
}

/// An order's part in a bracket. `None` on an order placed without one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Entry,
    /// A limit on the opposite side, at the bracket's `takeProfit`.
    TakeProfit,
    /// A stop on the opposite side, at the bracket's `stopLoss`.
    StopLoss,
}

impl Role {
    fn is_exit(self) -> bool {
        matches!(self, Role::TakeProfit | Role::StopLoss)
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
    /// Bracket prices out of order: for a long, not `takeProfit > entry > stopLoss` (the entry
    /// only when it has a price); for a short, the mirror.
    BadBracket,
}

impl RejectReason {
    pub fn code(self) -> &'static str {
        match self {
            RejectReason::BadQty => "bad_qty",
            RejectReason::UnknownSymbol => "unknown_symbol",
            RejectReason::MissingPrice => "missing_price",
            RejectReason::BadPrice => "bad_price",
            RejectReason::SubPenny => "sub_penny",
            RejectReason::BadBracket => "bad_bracket",
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
            CommandError::Invalid(RejectReason::BadBracket) => {
                "take-profit, entry and stop-loss are out of order"
            }
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

/// True when `tp`, `entry` and `sl` are in bracket order: for a long `tp > entry > sl`, for a
/// short `tp < entry < sl`. Only the pairs where both prices are present are compared, so the
/// same rule serves a priceless (market) entry, an entry that already filled, and an exit whose
/// sibling the user cancelled. Strict: an exit at the entry price would close at zero P&L at
/// best, which no position tool draws on purpose.
fn bracket_ok(long: bool, tp: Option<Price>, entry: Option<Price>, sl: Option<Price>) -> bool {
    let beyond = |a: Option<Price>, b: Option<Price>| match (a, b) {
        (Some(a), Some(b)) => {
            if long {
                a > b
            } else {
                a < b
            }
        }
        _ => true,
    };
    beyond(tp, sl) && beyond(tp, entry) && beyond(entry, sl)
}

/// The exits of a bracket as they arrive. A leg is `None` when the client left it out, which is
/// a `missing_price` rejection like a limit order without its limit.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Bracket {
    pub take_profit: Option<Price>,
    pub stop_loss: Option<Price>,
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
    /// Places this order as a bracket's entry with two linked exits.
    pub bracket: Option<Bracket>,
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
    /// On a bracket exit, the entry's `num`.
    pub parent_num: Option<u64>,
    pub role: Option<Role>,
    /// Numeric part of the wire `b-<n>`, shared by a bracket's entry and exits. Like ids it is
    /// never reused, not even across a reset.
    pub oco_group: Option<u64>,
}

impl Order {
    /// The wire id, e.g. `o-7`.
    pub fn id(&self) -> String {
        format!("o-{}", self.num)
    }

    fn is_exit(&self) -> bool {
        self.role.is_some_and(Role::is_exit)
    }
}

/// The price a bracket's exits are checked against while the entry is unfilled: the limit (also
/// for a stop-limit, where it is the price the order fills at), the stop for a stop, and nothing
/// for a market order.
fn entry_price(t: OrderType, limit: Option<Price>, stop: Option<Price>) -> Option<Price> {
    match t {
        OrderType::Limit | OrderType::StopLimit => limit,
        OrderType::Stop => stop,
        OrderType::Market => None,
    }
}

/// The checks [`Broker::place`] runs on a new order, minus the symbol list (which only this
/// simulator has): `bad_qty`, `missing_price`, `bad_price` / `sub_penny`, then the bracket rule,
/// first failure wins. Public for a broker route that is not this simulator (the server's Alpaca
/// paper route), so a real-routed order meets exactly the same tick grid and bracket rules.
pub fn check_new_order(req: &NewOrder) -> Result<(), RejectReason> {
    let limit = req.limit_price.filter(|_| req.order_type.takes_limit());
    let stop = req.stop_price.filter(|_| req.order_type.takes_stop());
    Broker::validate(req, limit, stop, &|_: &str| true)?;
    if let Some(b) = req.bracket {
        Broker::validate_bracket(req.side, entry_price(req.order_type, limit, stop), b)?;
    }
    Ok(())
}

/// The value checks of [`Broker::modify`] for a broker route that is not this simulator: each
/// present field must be a valid qty or an on-grid positive price.
pub fn check_modify(m: &Modify) -> Result<(), RejectReason> {
    if let Some(q) = m.qty {
        check_qty(q)?;
    }
    for p in [m.limit_price, m.stop_price].into_iter().flatten() {
        check_price(p)?;
    }
    Ok(())
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
    /// Next bracket (`ocoGroup`) number; never reset, for the same reason as `next_num`.
    next_group: u64,
    cash: i128,
    realized: i128,
    /// Working (pending, open or triggered) orders in creation order; that order decides who
    /// fills first within one trade. Pending exits live here, not in `finished`, so retention can
    /// never age one out.
    working: Vec<Order>,
    /// Finished orders, oldest first, at most [`FINISHED_RETAINED`].
    finished: VecDeque<Order>,
    positions: BTreeMap<String, Position>,
    /// The latest fills with their order's bracket role, oldest first, at most
    /// [`FILLS_RETAINED`].
    fills: VecDeque<(Fill, Option<Role>)>,
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
            next_group: 1,
            cash: STARTING_CASH_MICROS,
            realized: 0,
            working: Vec::new(),
            finished: VecDeque::new(),
            positions: BTreeMap::new(),
            fills: VecDeque::new(),
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

    /// The retained fills, oldest first, each with its order's bracket role (`None` outside a
    /// bracket).
    pub fn fills(&self) -> impl Iterator<Item = (&Fill, Option<Role>)> {
        self.fills.iter().map(|(f, r)| (f, *r))
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
    /// `known` answers whether a symbol is in the market's full symbol list. Returns the wire id
    /// (of the entry, for a bracket).
    ///
    /// A bracket is created in this one step, so no client ever sees an entry without its exits:
    /// the entry, then the take-profit, then the stop-loss, with consecutive ids, both exits
    /// `pending`. A rejected bracket is only the rejected entry; it still carries its role and
    /// group, so a client can tell which ticket it came from.
    pub fn place(&mut self, req: NewOrder, now: i64, known: impl Fn(&str) -> bool) -> String {
        let num = self.next_num;
        self.next_num += 1;
        let limit_price = req.limit_price.filter(|_| req.order_type.takes_limit());
        let stop_price = req.stop_price.filter(|_| req.order_type.takes_stop());
        let group = req.bracket.map(|_| {
            self.next_group += 1;
            self.next_group - 1
        });
        let reject = Self::validate(&req, limit_price, stop_price, &known)
            .and_then(|()| match req.bracket {
                Some(b) => Self::validate_bracket(
                    req.side,
                    entry_price(req.order_type, limit_price, stop_price),
                    b,
                ),
                None => Ok(()),
            })
            .err();
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
            parent_num: None,
            role: group.map(|_| Role::Entry),
            oco_group: group,
        };
        let id = order.id();
        match (reject, req.bracket) {
            (Some(_), _) => self.finish(order),
            (None, Some(b)) => {
                let exit = |n, role, order_type, limit_price, stop_price| Order {
                    num: n,
                    // The exits are the server's, not the ticket's: a client that keys its
                    // tickets by clientOrderId must not find three orders under one key.
                    client_order_id: None,
                    symbol: order.symbol.clone(),
                    side: match order.side {
                        Side::Buy => Side::Sell,
                        Side::Sell => Side::Buy,
                    },
                    order_type,
                    qty: order.qty,
                    filled_qty: 0,
                    avg_fill_price: None,
                    limit_price,
                    stop_price,
                    status: Status::Pending,
                    reject_reason: None,
                    created_at: now,
                    updated_at: now,
                    parent_num: Some(num),
                    role: Some(role),
                    oco_group: group,
                };
                let tp = exit(
                    num + 1,
                    Role::TakeProfit,
                    OrderType::Limit,
                    b.take_profit,
                    None,
                );
                let sl = exit(num + 2, Role::StopLoss, OrderType::Stop, None, b.stop_loss);
                self.next_num += 2;
                self.working.extend([order, tp, sl]);
            }
            (None, None) => self.working.push(order),
        }
        self.seq += 1;
        id
    }

    /// A bracket's own checks, after the entry passed its: each leg present (`missing_price`),
    /// positive and on the grid (`bad_price`, `sub_penny`, per leg in the same order as an
    /// entry's prices, take-profit first), then the three prices in bracket order
    /// (`bad_bracket`).
    fn validate_bracket(side: Side, entry: Option<Price>, b: Bracket) -> Result<(), RejectReason> {
        let (Some(tp), Some(sl)) = (b.take_profit, b.stop_loss) else {
            return Err(RejectReason::MissingPrice);
        };
        check_price(tp)?;
        check_price(sl)?;
        if bracket_ok(side == Side::Buy, Some(tp), entry, Some(sl)) {
            Ok(())
        } else {
            Err(RejectReason::BadBracket)
        }
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

    /// Changes a working order in place (same id, same place in the fill queue). A pending exit
    /// can be modified like an open one. A price change inside a bracket must keep the bracket
    /// in order against the members still working (see [`bracket_ok`]), else `bad_bracket`.
    pub fn modify(&mut self, id: &str, m: Modify, now: i64) -> Result<(), CommandError> {
        let num = parse_id(id).ok_or(CommandError::NotOpen)?;
        let idx = self
            .working
            .iter()
            .position(|o| o.num == num)
            .ok_or(CommandError::NotOpen)?;
        let order = &self.working[idx];
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
        if let Some(g) = order.oco_group
            && (m.limit_price.is_some() || m.stop_price.is_some())
            && !self.bracket_ok_after(g, num, &m)
        {
            return Err(CommandError::Invalid(RejectReason::BadBracket));
        }
        let order = &mut self.working[idx];
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

    /// Whether bracket `group` would still be in order with `m` applied to order `num`. Only
    /// working members count: a filled entry no longer bounds its exits, and an exit whose
    /// sibling was cancelled is bounded by the entry alone (if that is still unfilled).
    fn bracket_ok_after(&self, group: u64, num: u64, m: &Modify) -> bool {
        let (mut tp, mut entry, mut sl, mut long) = (None, None, None, true);
        for o in self.working.iter().filter(|o| o.oco_group == Some(group)) {
            let (limit, stop) = if o.num == num {
                (
                    m.limit_price.or(o.limit_price),
                    m.stop_price.or(o.stop_price),
                )
            } else {
                (o.limit_price, o.stop_price)
            };
            match o.role {
                Some(Role::Entry) => {
                    entry = entry_price(o.order_type, limit, stop);
                    long = o.side == Side::Buy;
                }
                Some(Role::TakeProfit) => {
                    tp = limit;
                    long = o.side == Side::Sell;
                }
                Some(Role::StopLoss) => {
                    sl = stop;
                    long = o.side == Side::Sell;
                }
                None => {}
            }
        }
        bracket_ok(long, tp, entry, sl)
    }

    /// Cancels a working order. Cancelling a bracket's entry cancels its exits too (they are
    /// still pending, since the entry has not filled): exits without a position to close would
    /// open one. Cancelling one exit leaves the other working, and no longer linked.
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
        let entry = order.role == Some(Role::Entry);
        self.finish(order);
        if entry {
            for o in self
                .working
                .iter_mut()
                .filter(|o| o.parent_num == Some(num))
            {
                o.status = Status::Cancelled;
                o.updated_at = now;
            }
            self.retire_finished();
        }
        self.seq += 1;
        Ok(())
    }

    /// Moves every order that is no longer working to the finished list, in creation order.
    fn retire_finished(&mut self) {
        let (done, working): (Vec<Order>, Vec<Order>) = std::mem::take(&mut self.working)
            .into_iter()
            .partition(|o| !o.status.is_working());
        self.working = working;
        for o in done {
            self.finish(o);
        }
    }

    /// Flat account, no orders. Used by `paper.reset` and by every replay rebuild (seek): orders
    /// cannot survive the tape being rewritten under them.
    pub fn reset(&mut self) {
        self.cash = STARTING_CASH_MICROS;
        self.realized = 0;
        self.working.clear();
        self.finished.clear();
        self.positions.clear();
        self.fills.clear();
        self.seq += 1;
    }

    /// Feeds one price-qualifying trade (the caller applies the OHLC filter). Matching follows the
    /// contract's table; every order fills in full on one trade, and within one trade working
    /// orders are visited in creation order.
    ///
    /// Brackets: an entry's fill opens its pending exits, but only after this trade's matching
    /// is done, so they are checked from the next trade (as a triggered stop-limit is). An exit's
    /// fill cancels its sibling (OCO) on the same trade, before the sibling can be visited.
    pub fn on_trade(&mut self, symbol: &str, price: Price, t: i64) -> TradeOutcome {
        let mut out = TradeOutcome::default();
        if !self.working.iter().any(|o| o.symbol == symbol) {
            return out;
        }
        // Brackets whose exit filled on this trade, and entries that filled on it.
        let mut closed_groups = Vec::new();
        let mut filled_entries = Vec::new();
        let mut roles = Vec::new();
        for o in self.working.iter_mut() {
            if o.symbol != symbol {
                continue;
            }
            if o.is_exit() && o.oco_group.is_some_and(|g| closed_groups.contains(&g)) {
                // The sibling already closed the position on this print. Prices keep the two
                // exits apart (a take-profit is beyond its stop-loss), so this is a guard, not
                // a path a valid bracket takes.
                o.status = Status::Cancelled;
                o.updated_at = t;
                continue;
            }
            // The status is read before this trade touches the order, so a stop-limit triggered
            // by this very trade is not also limit-checked against it ("from the next trade").
            // A pending exit matches nothing.
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
                match (o.role, o.oco_group) {
                    (Some(Role::Entry), _) => filled_entries.push(o.num),
                    (Some(_), Some(g)) => closed_groups.push(g),
                    _ => {}
                }
                roles.push(o.role);
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
        for o in self.working.iter_mut() {
            let parent_filled = o.parent_num.is_some_and(|n| filled_entries.contains(&n));
            if o.status == Status::Pending && parent_filled {
                o.status = Status::Open;
                o.updated_at = t;
            } else if o.status.is_working()
                && o.is_exit()
                && o.oco_group.is_some_and(|g| closed_groups.contains(&g))
            {
                // OCO: the sibling filled. Visited earlier in this trade (a take-profit comes
                // before its stop-loss), so it is still working here. rejectReason stays null.
                o.status = Status::Cancelled;
                o.updated_at = t;
            }
        }
        for (f, role) in out.fills.iter().zip(roles) {
            self.apply_fill(f);
            if self.fills.len() == FILLS_RETAINED {
                self.fills.pop_front();
            }
            self.fills.push_back((f.clone(), role));
        }
        self.retire_finished();
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
