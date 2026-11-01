//! The server's copy of the Alpaca paper account, and its `paper.state` / `paper.fill` frames.
//!
//! The frames keep the simulator's field names (docs/architecture/chart-trading-sim.md), so the
//! order ticket and the chart's order lines work unchanged, with `"simulated": false` and
//! `"broker": "alpaca_paper"`. What differs, all documented in docs/architecture/alpaca-paper.md:
//!
//! - Order ids are Alpaca's (UUIDs). A modify is an Alpaca *replace*, which gives the order a new
//!   id: the old one leaves the list and the new one takes its place.
//! - `rejectReason` is a string code (ours: `sub_penny`, `extended_hours_limit_only`, ... or
//!   `broker_rejected`), with Alpaca's own text in `rejectMessage`.
//! - `account` adds `buyingPower` and `equity`; `startingCash` is Alpaca's `last_equity` (the
//!   previous close) and `realizedPnl` is 0 (Alpaca does not report it).
//! - `link` says whether the trade stream is up; `notice` is the last refusal, for a toast.
//!
//! Bounded like the simulator: every working order, plus the last [`FINISHED_RETAINED`] finished
//! ones, plus the last [`FILLS_RETAINED`] fills.

use std::collections::{HashMap, VecDeque};

use paper_broker::{FILLS_RETAINED, FINISHED_RETAINED, OrderType, Role, Side, Status};
use serde::Serialize;

use crate::alpaca::{AlpacaAccount, AlpacaOrder, AlpacaPosition, FillActivity, dec, ms};

pub const BROKER: &str = "alpaca_paper";

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderOut {
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
    pub reject_reason: Option<String>,
    pub reject_message: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub parent_id: Option<String>,
    pub role: Option<Role>,
    pub oco_group: Option<String>,
    pub extended_hours: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountOut {
    pub starting_cash: f64,
    pub cash: f64,
    pub realized_pnl: f64,
    pub buying_power: Option<f64>,
    pub equity: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PositionOut {
    pub symbol: String,
    pub qty: i64,
    pub avg_price: f64,
    pub realized_pnl: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FillOut {
    pub order_id: String,
    pub symbol: String,
    pub side: Side,
    pub qty: i64,
    pub price: f64,
    pub t: i64,
    pub role: Option<Role>,
}

/// The trade stream's state, as clients see it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LinkOut {
    /// `connecting`, `live`, `down`, `unauthorized` or `not_configured`.
    pub state: &'static str,
    pub message: String,
}

/// The last refusal (place, modify or cancel), so a client can show it once.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Notice {
    pub at: i64,
    pub text: String,
    pub order_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StateOut<'a> {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub simulated: bool,
    pub broker: &'static str,
    pub seq: u64,
    pub account: &'a AccountOut,
    pub orders: Vec<&'a OrderOut>,
    pub positions: &'a [PositionOut],
    pub fills: Vec<&'a FillOut>,
    pub link: &'a LinkOut,
    pub notice: Option<NoticeOut<'a>>,
}

/// A notice as sent: with `fresh`, decided when the frame is rendered on the server's clock, so
/// a client never judges the toast's age against its own clock (stockwatcher `clock.v1`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NoticeOut<'a> {
    #[serde(flatten)]
    pub notice: &'a Notice,
    /// At most [`NOTICE_FRESH_MS`] old when the frame was rendered: a client toasts it (once per
    /// `at`). A connection's first frame can carry an old refusal: `false`, no toast.
    pub fresh: bool,
}

/// How long a refusal is worth a toast (the clients' old `freshNotice` default).
pub const NOTICE_FRESH_MS: i64 = 15_000;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FillFrame<'a> {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub simulated: bool,
    pub broker: &'static str,
    pub order_id: &'a str,
    pub symbol: &'a str,
    pub side: Side,
    pub qty: i64,
    pub price: f64,
    pub t: i64,
}

fn side(s: &str) -> Option<Side> {
    match s {
        "buy" => Some(Side::Buy),
        // Activities spell an opening short `sell_short`.
        "sell" | "sell_short" => Some(Side::Sell),
        _ => None,
    }
}

fn order_type(t: &str) -> Option<OrderType> {
    match t {
        "market" => Some(OrderType::Market),
        "limit" => Some(OrderType::Limit),
        // A trailing stop (placed elsewhere, e.g. Alpaca's dashboard) draws as a stop.
        "stop" | "trailing_stop" => Some(OrderType::Stop),
        "stop_limit" => Some(OrderType::StopLimit),
        _ => None,
    }
}

/// Alpaca's order status in our vocabulary. `held` is a bracket exit waiting for its entry, which
/// is exactly our `pending`.
pub fn status(s: &str) -> Status {
    match s {
        "held" => Status::Pending,
        "filled" => Status::Filled,
        "canceled" | "expired" | "replaced" | "done_for_day" => Status::Cancelled,
        "rejected" => Status::Rejected,
        // new, accepted, pending_new, partially_filled, pending_cancel, pending_replace,
        // accepted_for_bidding, calculated, stopped, suspended: still working.
        _ => Status::Open,
    }
}

fn whole(s: Option<&str>) -> i64 {
    dec(s).map_or(0, |v| v.trunc() as i64)
}

/// A bracket's group name, from its entry's id.
fn group_of(entry_id: &str) -> String {
    format!("b-{entry_id}")
}

#[derive(Debug)]
pub struct Book {
    seq: u64,
    account: AccountOut,
    /// Working orders, any order (sorted by creation when the state is built).
    working: Vec<OrderOut>,
    finished: VecDeque<OrderOut>,
    positions: Vec<PositionOut>,
    fills: VecDeque<FillOut>,
    /// Bracket exit id -> (entry id, role), learned from the entry's nested `legs`; a leg's own
    /// update does not name its parent.
    links: HashMap<String, (String, Role)>,
    link: LinkOut,
    notice: Option<Notice>,
    next_local: u64,
}

impl Book {
    pub fn new(link: LinkOut) -> Self {
        Book {
            seq: 0,
            account: AccountOut::default(),
            working: Vec::new(),
            finished: VecDeque::new(),
            positions: Vec::new(),
            fills: VecDeque::new(),
            links: HashMap::new(),
            link,
            notice: None,
            next_local: 1,
        }
    }

    pub fn order(&self, id: &str) -> Option<&OrderOut> {
        self.working
            .iter()
            .chain(self.finished.iter())
            .find(|o| o.id == id)
    }

    pub fn working(&self, id: &str) -> Option<&OrderOut> {
        self.working.iter().find(|o| o.id == id)
    }

    pub fn link(&self) -> &LinkOut {
        &self.link
    }

    pub fn set_link(&mut self, link: LinkOut) -> bool {
        if self.link == link {
            return false;
        }
        self.link = link;
        true
    }

    pub fn set_notice(&mut self, at: i64, text: String, order_id: Option<String>) {
        self.notice = Some(Notice { at, text, order_id });
    }

    pub fn set_account(&mut self, a: &AlpacaAccount) {
        let cash = dec(a.cash.as_deref()).unwrap_or(0.0);
        let equity = dec(a.equity.as_deref());
        self.account = AccountOut {
            starting_cash: dec(a.last_equity.as_deref()).or(equity).unwrap_or(cash),
            cash,
            realized_pnl: 0.0,
            buying_power: dec(a.buying_power.as_deref()),
            equity,
        };
    }

    pub fn set_positions(&mut self, ps: &[AlpacaPosition]) {
        self.positions = ps
            .iter()
            .filter_map(|p| {
                let q = dec(Some(&p.qty))?;
                // Alpaca reports shorts with side "short" (and, in current versions, a negative
                // qty): the sign comes from the side so both spellings agree.
                let short = p.side.as_deref() == Some("short") || q < 0.0;
                let qty = (q.abs().trunc() as i64) * if short { -1 } else { 1 };
                (qty != 0).then(|| PositionOut {
                    symbol: p.symbol.clone(),
                    qty,
                    avg_price: dec(Some(&p.avg_entry_price)).unwrap_or(0.0),
                    realized_pnl: 0.0,
                })
            })
            .collect();
        self.positions.sort_by(|a, b| a.symbol.cmp(&b.symbol));
    }

    fn remove(&mut self, id: &str) -> Option<OrderOut> {
        if let Some(i) = self.working.iter().position(|o| o.id == id) {
            return Some(self.working.remove(i));
        }
        let i = self.finished.iter().position(|o| o.id == id)?;
        self.finished.remove(i)
    }

    fn finish(&mut self, o: OrderOut) {
        if self.finished.len() >= FINISHED_RETAINED {
            self.finished.pop_front();
        }
        self.finished.push_back(o);
    }

    fn insert(&mut self, o: OrderOut) {
        if o.status.is_working() {
            self.working.push(o);
        } else {
            self.links.remove(&o.id);
            self.finish(o);
        }
    }

    /// Applies one Alpaca order (a REST answer or a stream update), with its nested legs.
    /// Unknown sides or types (options, crypto placed elsewhere) are ignored.
    pub fn upsert(&mut self, o: &AlpacaOrder) {
        // A replace: the old order leaves, the new one inherits its place in a bracket.
        if let Some(old) = o.replaces.as_deref() {
            if let Some(l) = self.links.remove(old) {
                self.links.insert(o.id.clone(), l);
            }
            self.remove(old);
        }
        if o.status == "replaced" {
            if let (Some(new), Some(l)) = (o.replaced_by.as_deref(), self.links.get(&o.id).cloned())
            {
                self.links.insert(new.to_owned(), l);
            }
            self.links.remove(&o.id);
            self.remove(&o.id);
            return;
        }
        let bracket_entry = o.order_class.as_deref() == Some("bracket")
            && o.legs.as_ref().is_some_and(|l| !l.is_empty());
        if bracket_entry {
            for leg in o.legs.as_deref().unwrap_or_default() {
                let role = if leg.order_type == "limit" {
                    Role::TakeProfit
                } else {
                    Role::StopLoss
                };
                self.links.insert(leg.id.clone(), (o.id.clone(), role));
            }
        }
        let previous = self.remove(&o.id);
        let (Some(side), Some(order_type)) = (side(&o.side), order_type(&o.order_type)) else {
            return;
        };
        let (parent_id, role, oco_group) = if bracket_entry {
            (None, Some(Role::Entry), Some(group_of(&o.id)))
        } else if let Some((parent, role)) = self.links.get(&o.id) {
            (Some(parent.clone()), Some(*role), Some(group_of(parent)))
        } else if let Some(p) = &previous {
            // A leg update after its link aged out keeps what it had.
            (p.parent_id.clone(), p.role, p.oco_group.clone())
        } else {
            (None, None, None)
        };
        let st = status(&o.status);
        let created_at = ms(o.created_at.as_deref());
        let out = OrderOut {
            id: o.id.clone(),
            client_order_id: o.client_order_id.clone().filter(|c| !c.is_empty()),
            symbol: o.symbol.clone(),
            side,
            order_type,
            qty: whole(o.qty.as_deref()),
            filled_qty: whole(o.filled_qty.as_deref()),
            avg_fill_price: dec(o.filled_avg_price.as_deref()),
            limit_price: dec(o.limit_price.as_deref()),
            stop_price: dec(o.stop_price.as_deref()),
            status: st,
            reject_reason: (st == Status::Rejected).then(|| "broker_rejected".to_owned()),
            reject_message: (st == Status::Rejected)
                .then(|| "Alpaca rejected the order".to_owned()),
            created_at,
            updated_at: ms(o.updated_at.as_deref()).max(created_at),
            parent_id,
            role,
            oco_group,
            extended_hours: o.extended_hours,
        };
        self.insert(out);
        if bracket_entry {
            for leg in o.legs.as_deref().unwrap_or_default() {
                self.upsert(leg);
            }
        }
    }

    /// A rejection made here (validation, the session rule, an Alpaca refusal or "not
    /// configured"): a finished order with a local id `r-<n>`, so the rejection shows in the state
    /// like the simulator's. Returns its id.
    #[allow(clippy::too_many_arguments)]
    pub fn reject_local(
        &mut self,
        req: &paper_broker::NewOrder,
        client_order_id: Option<String>,
        code: &str,
        message: &str,
        now: i64,
    ) -> String {
        let id = format!("r-{}", self.next_local);
        self.next_local += 1;
        let p = |x: Option<market_events::Price>| x.map(market_events::Price::to_f64);
        self.finish(OrderOut {
            id: id.clone(),
            client_order_id,
            symbol: req.symbol.clone(),
            side: req.side,
            order_type: req.order_type,
            qty: req.qty,
            filled_qty: 0,
            avg_fill_price: None,
            limit_price: p(req.limit_price),
            stop_price: p(req.stop_price),
            status: Status::Rejected,
            reject_reason: Some(code.to_owned()),
            reject_message: Some(message.to_owned()),
            created_at: now,
            updated_at: now,
            parent_id: None,
            role: req.bracket.map(|_| Role::Entry),
            oco_group: None,
            extended_hours: false,
        });
        self.set_notice(now, message.to_owned(), Some(id.clone()));
        id
    }

    /// Records a fill from the stream. False when it is already in the list (a resync that
    /// already brought it in).
    pub fn add_fill(&mut self, f: FillOut) -> bool {
        if self
            .fills
            .iter()
            .any(|x| x.order_id == f.order_id && x.t == f.t && x.qty == f.qty && x.price == f.price)
        {
            return false;
        }
        if self.fills.len() >= FILLS_RETAINED {
            self.fills.pop_front();
        }
        self.fills.push_back(f);
        true
    }

    /// A fill as the stream reports it (`fill` / `partial_fill`).
    pub fn fill_from_update(
        &self,
        o: &AlpacaOrder,
        price: Option<&str>,
        qty: Option<&str>,
        t: Option<&str>,
    ) -> Option<FillOut> {
        Some(FillOut {
            order_id: o.id.clone(),
            symbol: o.symbol.clone(),
            side: side(&o.side)?,
            qty: whole(qty).max(0),
            price: dec(price)?,
            t: ms(t),
            role: self.order(&o.id).and_then(|x| x.role),
        })
    }

    /// The resync: replaces orders, positions, account and fills with a REST snapshot. Local
    /// rejections stay (Alpaca never knew about them).
    pub fn replace_all(
        &mut self,
        account: &AlpacaAccount,
        positions: &[AlpacaPosition],
        orders: &[AlpacaOrder],
        fills: &[FillActivity],
    ) {
        let local: Vec<OrderOut> = self
            .finished
            .iter()
            .filter(|o| o.id.starts_with("r-"))
            .cloned()
            .collect();
        self.working.clear();
        self.finished.clear();
        // Oldest first, so the newest finished orders are the ones retained.
        let mut sorted: Vec<&AlpacaOrder> = orders.iter().collect();
        sorted.sort_by_key(|o| ms(o.updated_at.as_deref()).max(ms(o.created_at.as_deref())));
        for o in sorted {
            self.upsert(o);
        }
        let mut finished: Vec<OrderOut> = self.finished.drain(..).chain(local).collect();
        finished.sort_by_key(|o| o.updated_at);
        for o in finished {
            self.finish(o);
        }
        self.set_account(account);
        self.set_positions(positions);
        let mut fs: Vec<FillOut> = fills
            .iter()
            .filter_map(|f| {
                Some(FillOut {
                    order_id: f.order_id.clone(),
                    symbol: f.symbol.clone(),
                    side: side(&f.side)?,
                    qty: whole(Some(&f.qty)),
                    price: dec(Some(&f.price))?,
                    t: ms(Some(&f.transaction_time)),
                    role: self.order(&f.order_id).and_then(|o| o.role),
                })
            })
            .collect();
        fs.sort_by_key(|f| f.t);
        self.fills.clear();
        for f in fs {
            self.add_fill(f);
        }
    }

    /// The `paper.state` frame; bumps `seq`.
    pub fn state_frame(&mut self, now_ms: i64) -> String {
        self.seq += 1;
        self.render(now_ms)
    }

    /// The current state without a new `seq` (a connection's first frame), its notice judged
    /// fresh or not at `now_ms`.
    pub fn render(&self, now_ms: i64) -> String {
        let mut orders: Vec<&OrderOut> = self.working.iter().chain(self.finished.iter()).collect();
        orders.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        serde_json::to_string(&StateOut {
            kind: "paper.state",
            simulated: false,
            broker: BROKER,
            seq: self.seq,
            account: &self.account,
            orders,
            positions: &self.positions,
            fills: self.fills.iter().collect(),
            link: &self.link,
            notice: self.notice.as_ref().map(|n| NoticeOut {
                notice: n,
                fresh: (0..=NOTICE_FRESH_MS).contains(&(now_ms - n.at)),
            }),
        })
        .expect("state serializes")
    }

    pub fn fill_frame(f: &FillOut) -> String {
        serde_json::to_string(&FillFrame {
            kind: "paper.fill",
            simulated: false,
            broker: BROKER,
            order_id: &f.order_id,
            symbol: &f.symbol,
            side: f.side,
            qty: f.qty,
            price: f.price,
            t: f.t,
        })
        .expect("fill serializes")
    }

    #[cfg(test)]
    pub fn counts(&self) -> (usize, usize, usize) {
        (self.working.len(), self.finished.len(), self.fills.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn link() -> LinkOut {
        LinkOut {
            state: "live",
            message: String::new(),
        }
    }

    fn order(v: Value) -> AlpacaOrder {
        serde_json::from_value(v).unwrap()
    }

    fn base(id: &str, status: &str) -> Value {
        json!({"id": id, "client_order_id": "c-1", "symbol": "AAPL", "side": "buy", "type": "limit",
               "qty": "1", "filled_qty": "0", "limit_price": "100", "status": status,
               "created_at": "2026-10-28T14:00:00Z", "updated_at": "2026-10-28T14:00:01Z",
               "order_class": "simple", "extended_hours": false})
    }

    fn state(b: &mut Book) -> Value {
        serde_json::from_str(&b.state_frame(0)).unwrap()
    }

    #[test]
    fn the_state_is_the_simulators_shape_marked_as_alpaca_paper() {
        let mut b = Book::new(link());
        b.upsert(&order(base("aaaaaaaa-1", "new")));
        let s = state(&mut b);
        assert_eq!(s["type"], "paper.state");
        assert_eq!(s["simulated"], false);
        assert_eq!(s["broker"], "alpaca_paper");
        let o = &s["orders"][0];
        assert_eq!(o["status"], "open");
        assert_eq!(o["orderType"], "limit");
        assert_eq!(o["limitPrice"], 100.0);
        assert_eq!(o["clientOrderId"], "c-1");
        assert_eq!(o["createdAt"], 1_793_196_000_000_i64);
    }

    #[test]
    fn a_replace_swaps_the_old_id_for_the_new_one() {
        let mut b = Book::new(link());
        b.upsert(&order(base("aaaaaaaa-1", "new")));
        let mut n = base("bbbbbbbb-2", "new");
        n["replaces"] = json!("aaaaaaaa-1");
        n["limit_price"] = json!("99");
        b.upsert(&order(n));
        assert!(b.order("aaaaaaaa-1").is_none());
        assert_eq!(b.working("bbbbbbbb-2").unwrap().limit_price, Some(99.0));
        // The stream's later "replaced" event for the old order changes nothing.
        let mut old = base("aaaaaaaa-1", "replaced");
        old["replaced_by"] = json!("bbbbbbbb-2");
        b.upsert(&order(old));
        assert_eq!(b.counts(), (1, 0, 0));
    }

    #[test]
    fn a_bracket_maps_to_entry_and_held_exits_linked_by_group() {
        let mut b = Book::new(link());
        let mut e = base("eeeeeeee-1", "new");
        e["order_class"] = json!("bracket");
        let mut tp = base("ffffffff-2", "held");
        tp["side"] = json!("sell");
        tp["limit_price"] = json!("110");
        let mut sl = base("abababab-3", "held");
        sl["side"] = json!("sell");
        sl["type"] = json!("stop");
        sl["stop_price"] = json!("95");
        e["legs"] = json!([tp, sl]);
        b.upsert(&order(e));
        let s = state(&mut b);
        let by = |id: &str| {
            s["orders"]
                .as_array()
                .unwrap()
                .iter()
                .find(|o| o["id"] == id)
                .unwrap()
                .clone()
        };
        assert_eq!(by("eeeeeeee-1")["role"], "entry");
        assert_eq!(by("ffffffff-2")["role"], "take_profit");
        assert_eq!(by("ffffffff-2")["status"], "pending");
        assert_eq!(by("abababab-3")["role"], "stop_loss");
        assert_eq!(by("abababab-3")["parentId"], "eeeeeeee-1");
        assert_eq!(by("abababab-3")["ocoGroup"], "b-eeeeeeee-1");
        // A later update of one leg alone keeps its place in the bracket.
        let mut sl2 = base("abababab-3", "new");
        sl2["side"] = json!("sell");
        sl2["type"] = json!("stop");
        b.upsert(&order(sl2));
        assert_eq!(b.working("abababab-3").unwrap().role, Some(Role::StopLoss));
    }

    #[test]
    fn finished_orders_and_fills_are_bounded() {
        let mut b = Book::new(link());
        for i in 0..(FINISHED_RETAINED + 10) {
            b.upsert(&order(base(&format!("cccccccc-{i}"), "canceled")));
        }
        for i in 0..(FILLS_RETAINED + 10) {
            b.add_fill(FillOut {
                order_id: format!("x{i}"),
                symbol: "AAPL".into(),
                side: Side::Buy,
                qty: 1,
                price: 1.0,
                t: i as i64,
                role: None,
            });
        }
        assert_eq!(b.counts(), (0, FINISHED_RETAINED, FILLS_RETAINED));
        // The newest are kept.
        assert!(
            b.order(&format!("cccccccc-{}", FINISHED_RETAINED + 9))
                .is_some()
        );
        assert!(b.order("cccccccc-0").is_none());
    }

    #[test]
    fn a_duplicate_fill_is_dropped() {
        let mut b = Book::new(link());
        let f = FillOut {
            order_id: "a".into(),
            symbol: "AAPL".into(),
            side: Side::Buy,
            qty: 1,
            price: 1.0,
            t: 5,
            role: None,
        };
        assert!(b.add_fill(f.clone()));
        assert!(!b.add_fill(f));
    }

    #[test]
    fn positions_are_signed_and_account_carries_buying_power() {
        let mut b = Book::new(link());
        b.set_positions(&[
            serde_json::from_value(
                json!({"symbol":"TSLA","qty":"-5","avg_entry_price":"200.5","side":"short"}),
            )
            .unwrap(),
            serde_json::from_value(
                json!({"symbol":"AAPL","qty":"10","avg_entry_price":"100","side":"long"}),
            )
            .unwrap(),
        ]);
        b.set_account(
            &serde_json::from_value(
                json!({"status":"ACTIVE","cash":"1000","buying_power":"4000",
            "equity":"1500","last_equity":"1400"}),
            )
            .unwrap(),
        );
        let s = state(&mut b);
        assert_eq!(s["positions"][0]["symbol"], "AAPL");
        assert_eq!(s["positions"][1]["qty"], -5);
        assert_eq!(s["account"]["buyingPower"], 4000.0);
        assert_eq!(s["account"]["equity"], 1500.0);
        assert_eq!(s["account"]["startingCash"], 1400.0);
    }

    #[test]
    fn a_local_rejection_shows_in_state_with_its_reason_and_survives_a_resync() {
        let mut b = Book::new(link());
        let req = paper_broker::NewOrder {
            client_order_id: Some("c-9".into()),
            symbol: "AAPL".into(),
            side: Side::Buy,
            order_type: OrderType::Market,
            qty: 1,
            limit_price: None,
            stop_price: None,
            bracket: None,
        };
        let id = b.reject_local(
            &req,
            Some("c-9".into()),
            "extended_hours_limit_only",
            "only limits now",
            7,
        );
        let s = state(&mut b);
        assert_eq!(s["orders"][0]["id"], id);
        assert_eq!(s["orders"][0]["status"], "rejected");
        assert_eq!(s["orders"][0]["rejectReason"], "extended_hours_limit_only");
        assert_eq!(s["notice"]["text"], "only limits now");
        // Rendered at its own time (state() renders at 0): the server judged it fresh then.
        let at7: Value = serde_json::from_str(&b.render(7)).unwrap();
        assert_eq!(at7["notice"]["fresh"], true);
        let later: Value = serde_json::from_str(&b.render(7 + NOTICE_FRESH_MS + 1)).unwrap();
        assert_eq!(
            later["notice"]["fresh"], false,
            "an old refusal never toasts"
        );
        let acct: AlpacaAccount = serde_json::from_value(json!({"status":"ACTIVE"})).unwrap();
        b.replace_all(&acct, &[], &[order(base("aaaaaaaa-1", "new"))], &[]);
        assert!(b.order(&id).is_some());
        assert!(b.working("aaaaaaaa-1").is_some());
    }
}
