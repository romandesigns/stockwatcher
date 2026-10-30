//! Alpaca Trading API v2 shapes (only the fields used) and request bodies.
//!
//! Alpaca sends every number as a string ("3.38", "100"). The account number and other
//! identifying fields are deliberately not deserialized, so they cannot end up in a log or a
//! frame by accident.

use market_events::{PRICE_SCALE, Price};
use paper_broker::{Modify, NewOrder, OrderType, Side};
use serde::Deserialize;
use serde_json::{Map, Value, json};

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct AlpacaOrder {
    pub id: String,
    #[serde(default)]
    pub client_order_id: Option<String>,
    pub symbol: String,
    pub side: String,
    #[serde(rename = "type")]
    pub order_type: String,
    #[serde(default)]
    pub qty: Option<String>,
    #[serde(default)]
    pub filled_qty: Option<String>,
    #[serde(default)]
    pub filled_avg_price: Option<String>,
    #[serde(default)]
    pub limit_price: Option<String>,
    #[serde(default)]
    pub stop_price: Option<String>,
    pub status: String,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub order_class: Option<String>,
    #[serde(default)]
    pub legs: Option<Vec<AlpacaOrder>>,
    #[serde(default)]
    pub replaces: Option<String>,
    #[serde(default)]
    pub replaced_by: Option<String>,
    #[serde(default)]
    pub extended_hours: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct AlpacaAccount {
    pub status: String,
    #[serde(default)]
    pub cash: Option<String>,
    #[serde(default)]
    pub buying_power: Option<String>,
    #[serde(default)]
    pub equity: Option<String>,
    #[serde(default)]
    pub last_equity: Option<String>,
    #[serde(default)]
    pub shorting_enabled: Option<bool>,
    #[serde(default)]
    pub trading_blocked: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct AlpacaPosition {
    pub symbol: String,
    pub qty: String,
    pub avg_entry_price: String,
    #[serde(default)]
    pub side: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct AlpacaClock {
    pub is_open: bool,
    #[serde(default)]
    pub next_open: Option<String>,
    #[serde(default)]
    pub next_close: Option<String>,
}

/// One `FILL` account activity (resync of the fills list).
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct FillActivity {
    pub order_id: String,
    pub symbol: String,
    pub side: String,
    pub qty: String,
    pub price: String,
    pub transaction_time: String,
}

pub fn dec(s: Option<&str>) -> Option<f64> {
    s.and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite())
}

/// RFC 3339 to epoch ms; 0 when missing or unreadable.
pub fn ms(s: Option<&str>) -> i64 {
    s.and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
        .map_or(0, |d| d.timestamp_millis())
}

/// An exact price as Alpaca wants it: a decimal string, no float noise, no trailing zeros.
pub fn price_str(p: Price) -> String {
    let m = p.micros();
    let whole = m / PRICE_SCALE;
    let frac = (m % PRICE_SCALE).abs();
    if frac == 0 {
        return whole.to_string();
    }
    let f = format!("{frac:06}");
    format!("{whole}.{}", f.trim_end_matches('0'))
}

pub fn side_str(s: Side) -> &'static str {
    match s {
        Side::Buy => "buy",
        Side::Sell => "sell",
    }
}

pub fn type_str(t: OrderType) -> &'static str {
    match t {
        OrderType::Market => "market",
        OrderType::Limit => "limit",
        OrderType::Stop => "stop",
        OrderType::StopLimit => "stop_limit",
    }
}

/// Alpaca accepts up to 128 characters; ours are short ids from the ticket. Anything else is
/// replaced by a server-made id (the order still goes through, idempotency comes from that id).
pub fn valid_client_order_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}

/// An Alpaca order id as a path segment: a UUID's characters only, so a client id can never
/// steer the request to another path.
pub fn valid_order_id(id: &str) -> bool {
    (8..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}

/// `POST /v2/orders` body. `time_in_force` is always `day`. `extended_hours` comes from the
/// session rule ([`crate::session`]), which has already refused what Alpaca would refuse.
pub fn order_body(req: &NewOrder, client_order_id: &str, extended_hours: bool) -> Value {
    let mut b = Map::new();
    b.insert("symbol".into(), json!(req.symbol));
    b.insert("qty".into(), json!(req.qty.to_string()));
    b.insert("side".into(), json!(side_str(req.side)));
    b.insert("type".into(), json!(type_str(req.order_type)));
    b.insert("time_in_force".into(), json!("day"));
    b.insert("client_order_id".into(), json!(client_order_id));
    if matches!(req.order_type, OrderType::Limit | OrderType::StopLimit)
        && let Some(p) = req.limit_price
    {
        b.insert("limit_price".into(), json!(price_str(p)));
    }
    if matches!(req.order_type, OrderType::Stop | OrderType::StopLimit)
        && let Some(p) = req.stop_price
    {
        b.insert("stop_price".into(), json!(price_str(p)));
    }
    if extended_hours {
        b.insert("extended_hours".into(), json!(true));
    }
    if let Some(br) = req.bracket
        && let (Some(tp), Some(sl)) = (br.take_profit, br.stop_loss)
    {
        b.insert("order_class".into(), json!("bracket"));
        b.insert(
            "take_profit".into(),
            json!({ "limit_price": price_str(tp) }),
        );
        b.insert("stop_loss".into(), json!({ "stop_price": price_str(sl) }));
    }
    Value::Object(b)
}

/// `PATCH /v2/orders/{id}` body (a replace: Alpaca answers with a NEW order id).
pub fn replace_body(m: &Modify) -> Value {
    let mut b = Map::new();
    if let Some(q) = m.qty {
        b.insert("qty".into(), json!(q.to_string()));
    }
    if let Some(p) = m.limit_price {
        b.insert("limit_price".into(), json!(price_str(p)));
    }
    if let Some(p) = m.stop_price {
        b.insert("stop_price".into(), json!(price_str(p)));
    }
    Value::Object(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use paper_broker::Bracket;

    fn p(d: f64) -> Price {
        Price::from_f64_rounded(d).unwrap()
    }

    fn order(t: OrderType) -> NewOrder {
        NewOrder {
            client_order_id: Some("c-1".into()),
            symbol: "AAPL".into(),
            side: Side::Buy,
            order_type: t,
            qty: 100,
            limit_price: Some(p(3.38)),
            stop_price: Some(p(3.5)),
            bracket: None,
        }
    }

    #[test]
    fn prices_are_exact_decimal_strings() {
        assert_eq!(price_str(p(3.38)), "3.38");
        assert_eq!(price_str(p(0.1 + 0.2)), "0.3");
        assert_eq!(price_str(p(0.0123)), "0.0123");
        assert_eq!(price_str(p(250.0)), "250");
    }

    #[test]
    fn each_order_type_sends_only_its_own_prices() {
        let lim = order_body(&order(OrderType::Limit), "c-1", false);
        assert_eq!(
            lim,
            json!({"symbol":"AAPL","qty":"100","side":"buy","type":"limit","time_in_force":"day",
                   "client_order_id":"c-1","limit_price":"3.38"})
        );
        let mkt = order_body(&order(OrderType::Market), "c-1", false);
        assert!(mkt.get("limit_price").is_none() && mkt.get("stop_price").is_none());
        let stp = order_body(&order(OrderType::Stop), "c-1", false);
        assert_eq!(stp["stop_price"], "3.5");
        assert!(stp.get("limit_price").is_none());
        let sl = order_body(&order(OrderType::StopLimit), "c-1", false);
        assert_eq!(
            (sl["limit_price"].as_str(), sl["stop_price"].as_str()),
            (Some("3.38"), Some("3.5"))
        );
    }

    #[test]
    fn extended_hours_and_brackets_are_flagged() {
        let ext = order_body(&order(OrderType::Limit), "c-1", true);
        assert_eq!(ext["extended_hours"], true);
        assert_eq!(ext["time_in_force"], "day");
        let mut o = order(OrderType::Limit);
        o.bracket = Some(Bracket {
            take_profit: Some(p(3.5)),
            stop_loss: Some(p(3.31)),
        });
        let b = order_body(&o, "c-1", false);
        assert_eq!(b["order_class"], "bracket");
        assert_eq!(b["take_profit"]["limit_price"], "3.5");
        assert_eq!(b["stop_loss"]["stop_price"], "3.31");
    }

    #[test]
    fn replace_sends_only_the_changed_fields() {
        let m = Modify {
            qty: None,
            limit_price: Some(p(3.36)),
            stop_price: None,
        };
        assert_eq!(replace_body(&m), json!({"limit_price":"3.36"}));
    }

    #[test]
    fn ids_are_checked_before_they_reach_a_url() {
        assert!(valid_order_id("61e69015-8549-4bfd-b9c3-01e75843f47d"));
        assert!(!valid_order_id("../account"));
        assert!(!valid_order_id("o-7"));
        assert!(valid_client_order_id("c-1697"));
        assert!(!valid_client_order_id("has space"));
    }
}
