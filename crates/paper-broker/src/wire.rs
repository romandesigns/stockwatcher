//! The JSON edge: client commands in, `paper.state` / `paper.fill` out. Field names follow
//! docs/architecture/chart-trading-sim.md exactly (mirrored by packages/chart-core/src/trading.ts).
//!
//! Dollars travel as JSON numbers and become exact [`Price`]s here, rounded to the micro-dollar.
//! Rounding first and checking the tick grid after is deliberate: `3.38` arriving as
//! `3.3799999999999999` is still $3.38, while `3.385` stays off the cent grid and is rejected as
//! `sub_penny` by the broker.

use crate::{
    Broker, Fill, Modify, NewOrder, Order, OrderType, Position, RejectReason, Side, Status,
};
use market_events::{PRICE_SCALE, Price};
use serde::{Deserialize, Serialize};

/// `order.place` as sent by a client (the `type` tag is handled by the caller's enum).
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaceCommand {
    #[serde(default)]
    pub client_order_id: Option<String>,
    pub symbol: String,
    pub side: Side,
    pub order_type: OrderType,
    pub qty: f64,
    #[serde(default)]
    pub limit_price: Option<f64>,
    #[serde(default)]
    pub stop_price: Option<f64>,
}

/// `order.modify`.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModifyCommand {
    pub id: String,
    #[serde(default)]
    pub qty: Option<f64>,
    #[serde(default)]
    pub limit_price: Option<f64>,
    #[serde(default)]
    pub stop_price: Option<f64>,
}

/// `order.cancel`.
#[derive(Clone, Debug, Deserialize)]
pub struct CancelCommand {
    pub id: String,
}

/// A JSON share count as an `i64`. Anything that is not a whole number maps to 0, which the
/// broker rejects as `bad_qty`: a fractional size is a validation failure (shown in state), not a
/// malformed message.
pub fn qty_from_json(q: f64) -> i64 {
    if q.is_finite() && q.fract() == 0.0 && q.abs() < 1e15 {
        q as i64
    } else {
        0
    }
}

/// Dollars to an exact price. A value too large for micro-dollars becomes zero so the broker
/// rejects it as `bad_price` rather than the edge inventing a different error path.
pub fn price_from_json(p: f64) -> Price {
    Price::from_f64_rounded(p).unwrap_or(Price::ZERO)
}

impl PlaceCommand {
    /// `symbol` is normalized like `chart.subscribe` (trimmed, upper case).
    pub fn into_new_order(self) -> NewOrder {
        NewOrder {
            client_order_id: self.client_order_id,
            symbol: self.symbol.trim().to_ascii_uppercase(),
            side: self.side,
            order_type: self.order_type,
            qty: qty_from_json(self.qty),
            limit_price: self.limit_price.map(price_from_json),
            stop_price: self.stop_price.map(price_from_json),
        }
    }
}

impl ModifyCommand {
    pub fn into_modify(self) -> (String, Modify) {
        (
            self.id,
            Modify {
                qty: self.qty.map(qty_from_json),
                limit_price: self.limit_price.map(price_from_json),
                stop_price: self.stop_price.map(price_from_json),
            },
        )
    }
}

fn dollars(micros: i128) -> f64 {
    micros as f64 / PRICE_SCALE as f64
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountWire {
    pub starting_cash: f64,
    pub cash: f64,
    pub realized_pnl: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderWire {
    pub id: String,
    pub client_order_id: Option<String>,
    pub symbol: String,
    pub side: Side,
    pub order_type: OrderType,
    pub qty: i64,
    pub filled_qty: i64,
    pub avg_fill_price: Option<f64>,
    pub limit_price: Option<f64>,
    pub stop_price: Option<f64>,
    pub status: Status,
    pub reject_reason: Option<RejectReason>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<&Order> for OrderWire {
    fn from(o: &Order) -> Self {
        OrderWire {
            id: o.id(),
            client_order_id: o.client_order_id.clone(),
            symbol: o.symbol.clone(),
            side: o.side,
            order_type: o.order_type,
            qty: o.qty,
            filled_qty: o.filled_qty,
            avg_fill_price: o.avg_fill_price.map(Price::to_f64),
            limit_price: o.limit_price.map(Price::to_f64),
            stop_price: o.stop_price.map(Price::to_f64),
            status: o.status,
            reject_reason: o.reject_reason,
            created_at: o.created_at,
            updated_at: o.updated_at,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PositionWire {
    pub symbol: String,
    pub qty: i64,
    pub avg_price: f64,
    pub realized_pnl: f64,
}

impl PositionWire {
    fn new(symbol: &str, p: &Position) -> Self {
        PositionWire {
            symbol: symbol.to_string(),
            qty: p.qty,
            avg_price: p.avg_price_micros() / PRICE_SCALE as f64,
            realized_pnl: dollars(p.realized),
        }
    }
}

/// The whole truth; clients replace their copy.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PaperState {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub simulated: bool,
    pub seq: u64,
    pub account: AccountWire,
    pub orders: Vec<OrderWire>,
    pub positions: Vec<PositionWire>,
}

/// One execution, sent alongside the `paper.state` that contains it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaperFill {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub simulated: bool,
    pub order_id: String,
    pub symbol: String,
    pub side: Side,
    pub qty: i64,
    pub price: f64,
    pub t: i64,
}

impl From<&Fill> for PaperFill {
    fn from(f: &Fill) -> Self {
        PaperFill {
            kind: "paper.fill",
            simulated: true,
            order_id: format!("o-{}", f.order_num),
            symbol: f.symbol.clone(),
            side: f.side,
            qty: f.qty,
            price: f.price.to_f64(),
            t: f.t,
        }
    }
}

impl Broker {
    pub fn state(&self) -> PaperState {
        PaperState {
            kind: "paper.state",
            simulated: true,
            seq: self.seq(),
            account: AccountWire {
                starting_cash: dollars(crate::STARTING_CASH_MICROS),
                cash: dollars(self.cash()),
                realized_pnl: dollars(self.realized()),
            },
            orders: self.orders().into_iter().map(OrderWire::from).collect(),
            positions: self
                .positions()
                .map(|(s, p)| PositionWire::new(s, p))
                .collect(),
        }
    }
}
