//! The service against scripted HTTP: request building, rejections, the session rule, replace,
//! cancel and the stream relay.

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::http::mock::{MockHttp, ok};
use crate::http::{Http, Method};
use crate::service::{Publish, Service};
use crate::stream::{StreamMsg, parse_frame};

struct Harness {
    http: Arc<MockHttp>,
    frames: Arc<Mutex<Vec<String>>>,
    svc: Service,
}

fn harness() -> Harness {
    let http = Arc::new(MockHttp::default());
    let frames = Arc::new(Mutex::new(Vec::new()));
    let f = frames.clone();
    let publish: Publish = Arc::new(move |fs: Vec<String>| f.lock().unwrap().extend(fs));
    let svc = Service::with_http(
        http.clone() as Arc<dyn Http>,
        Some("ALPACA_PAPER_KEY / ALPACA_PAPER_SECRET"),
        publish,
    );
    Harness { http, frames, svc }
}

impl Harness {
    fn clock(&self, open: bool) {
        self.http.push(
            Method::Get,
            "/v2/clock",
            ok(200, &json!({"is_open": open, "next_open": "2099-01-01T14:30:00Z", "next_close": "2099-01-01T21:00:00Z"}).to_string()),
        );
    }

    fn last_state(&self) -> Value {
        let frames = self.frames.lock().unwrap();
        let s = frames
            .iter()
            .rev()
            .find(|f| f.contains("\"paper.state\""))
            .expect("a state frame");
        serde_json::from_str(s).unwrap()
    }

    fn posted_bodies(&self) -> Vec<Value> {
        self.http
            .sent()
            .into_iter()
            .filter(|r| r.method == Method::Post)
            .map(|r| serde_json::from_str(r.body.as_deref().unwrap()).unwrap())
            .collect()
    }
}

fn place(v: Value) -> paper_broker::wire::PlaceCommand {
    serde_json::from_value(v).unwrap()
}

fn alpaca_order(id: &str, status: &str) -> Value {
    json!({"id": id, "client_order_id": "c-1", "symbol": "AAPL", "side": "buy", "type": "limit",
           "qty": "1", "filled_qty": "0", "limit_price": "100", "status": status,
           "created_at": "2026-10-28T14:00:00Z", "updated_at": "2026-10-28T14:00:00Z"})
}

const ID1: &str = "61e69015-8549-4bfd-b9c3-01e75843f47d";
const ID2: &str = "71e69015-8549-4bfd-b9c3-01e75843f47d";

#[tokio::test]
async fn a_limit_order_is_posted_as_a_day_order_with_its_client_order_id() {
    let h = harness();
    h.clock(true);
    h.http.push(
        Method::Post,
        "/v2/orders",
        ok(200, &alpaca_order(ID1, "new").to_string()),
    );
    h.svc
        .place(place(json!({"clientOrderId": "c-1", "symbol": "aapl", "side": "buy", "orderType": "limit", "qty": 1, "limitPrice": 100})))
        .await;
    assert_eq!(
        h.posted_bodies(),
        vec![
            json!({"symbol": "AAPL", "qty": "1", "side": "buy", "type": "limit", "time_in_force": "day",
                    "client_order_id": "c-1", "limit_price": "100"})
        ]
    );
    let s = h.last_state();
    assert_eq!(s["broker"], "alpaca_paper");
    assert_eq!(s["simulated"], false);
    assert_eq!(s["orders"][0]["id"], ID1);
    assert_eq!(s["orders"][0]["status"], "open");
}

#[tokio::test]
async fn sub_penny_prices_are_rejected_before_anything_is_sent() {
    let h = harness();
    h.svc
        .place(place(json!({"clientOrderId": "c-2", "symbol": "AAPL", "side": "buy", "orderType": "limit", "qty": 1, "limitPrice": 100.005})))
        .await;
    assert!(h.http.sent().is_empty());
    let s = h.last_state();
    assert_eq!(s["orders"][0]["status"], "rejected");
    assert_eq!(s["orders"][0]["rejectReason"], "sub_penny");
}

#[tokio::test]
async fn extended_hours_send_limits_with_the_flag_and_refuse_market_and_stop() {
    let h = harness();
    h.clock(false);
    h.http.push(
        Method::Post,
        "/v2/orders",
        ok(200, &alpaca_order(ID1, "new").to_string()),
    );
    h.svc
        .place(place(json!({"clientOrderId": "c-3", "symbol": "AAPL", "side": "buy", "orderType": "limit", "qty": 1, "limitPrice": 100})))
        .await;
    assert_eq!(h.posted_bodies()[0]["extended_hours"], true);
    // The clock reading is cached: no second /v2/clock call is needed (none is scripted).
    for t in ["market", "stop"] {
        let mut cmd = json!({"clientOrderId": format!("c-{t}"), "symbol": "AAPL", "side": "buy", "orderType": t, "qty": 1});
        if t == "stop" {
            cmd["stopPrice"] = json!(101);
        }
        h.svc.place(place(cmd)).await;
        let s = h.last_state();
        let o = s["orders"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["clientOrderId"] == format!("c-{t}"))
            .unwrap()
            .clone();
        assert_eq!(o["status"], "rejected", "{t}");
        assert_eq!(o["rejectReason"], "extended_hours_limit_only", "{t}");
        assert!(
            s["notice"]["text"]
                .as_str()
                .unwrap()
                .contains("only limit orders")
        );
    }
    assert_eq!(h.posted_bodies().len(), 1);
}

#[tokio::test]
async fn alpacas_rejection_text_reaches_the_state() {
    let h = harness();
    h.clock(true);
    h.http.push(
        Method::Post,
        "/v2/orders",
        ok(
            403,
            r#"{"code":40310000,"message":"asset AAPL cannot be sold short"}"#,
        ),
    );
    h.svc
        .place(place(json!({"clientOrderId": "c-4", "symbol": "AAPL", "side": "sell", "orderType": "market", "qty": 1})))
        .await;
    let s = h.last_state();
    assert_eq!(s["orders"][0]["rejectReason"], "broker_rejected");
    assert_eq!(
        s["orders"][0]["rejectMessage"],
        "Alpaca: asset AAPL cannot be sold short"
    );
}

#[tokio::test]
async fn without_keys_every_order_is_rejected_as_not_configured() {
    let frames = Arc::new(Mutex::new(Vec::<String>::new()));
    let f = frames.clone();
    let svc = Service::unconfigured(
        "set ALPACA_PAPER_KEY / ALPACA_PAPER_SECRET".into(),
        Arc::new(move |fs: Vec<String>| f.lock().unwrap().extend(fs)),
    );
    assert!(!svc.configured());
    svc.place(place(json!({"clientOrderId": "c-5", "symbol": "AAPL", "side": "buy", "orderType": "market", "qty": 1})))
        .await;
    let s: Value = serde_json::from_str(frames.lock().unwrap().last().unwrap()).unwrap();
    assert_eq!(s["orders"][0]["rejectReason"], "not_configured");
    assert!(
        s["orders"][0]["rejectMessage"]
            .as_str()
            .unwrap()
            .starts_with("Alpaca paper is not configured on the server")
    );
    assert_eq!(s["link"]["state"], "not_configured");
    assert_eq!(
        svc.modify(serde_json::from_value(json!({"id": ID1, "limitPrice": 1})).unwrap())
            .await
            .unwrap_err()
            .code,
        "not_configured"
    );
}

#[tokio::test]
async fn modify_is_a_replace_and_the_new_id_takes_the_old_ones_place() {
    let h = harness();
    h.clock(true);
    h.http.push(
        Method::Post,
        "/v2/orders",
        ok(200, &alpaca_order(ID1, "new").to_string()),
    );
    h.svc
        .place(place(json!({"clientOrderId": "c-1", "symbol": "AAPL", "side": "buy", "orderType": "limit", "qty": 1, "limitPrice": 100})))
        .await;
    let mut replaced = alpaca_order(ID2, "new");
    replaced["limit_price"] = json!("99.5");
    replaced["replaces"] = json!(ID1);
    h.http.push(
        Method::Patch,
        &format!("/v2/orders/{ID1}"),
        ok(200, &replaced.to_string()),
    );
    h.svc
        .modify(serde_json::from_value(json!({"id": ID1, "limitPrice": 99.5})).unwrap())
        .await
        .unwrap();
    let patch = h
        .http
        .sent()
        .into_iter()
        .find(|r| r.method == Method::Patch)
        .unwrap();
    assert!(patch.url.ends_with(&format!("/v2/orders/{ID1}")));
    assert_eq!(
        serde_json::from_str::<Value>(patch.body.as_deref().unwrap()).unwrap(),
        json!({"limit_price": "99.5"})
    );
    let s = h.last_state();
    assert_eq!(s["orders"].as_array().unwrap().len(), 1);
    assert_eq!(s["orders"][0]["id"], ID2);
    assert_eq!(s["orders"][0]["limitPrice"], 99.5);
}

#[tokio::test]
async fn modify_and_cancel_refuse_unknown_or_unsafe_ids_without_a_request() {
    let h = harness();
    for id in ["o-7", "../account", ID1] {
        assert_eq!(h.svc.cancel(id).await.unwrap_err().code, "not_open", "{id}");
    }
    assert!(h.http.sent().is_empty());
}

#[tokio::test]
async fn cancel_deletes_then_reads_the_order_back() {
    let h = harness();
    h.clock(true);
    h.http.push(
        Method::Post,
        "/v2/orders",
        ok(200, &alpaca_order(ID1, "new").to_string()),
    );
    h.svc
        .place(place(json!({"clientOrderId": "c-1", "symbol": "AAPL", "side": "buy", "orderType": "limit", "qty": 1, "limitPrice": 100})))
        .await;
    h.http
        .push(Method::Delete, &format!("/v2/orders/{ID1}"), ok(204, ""));
    h.http.push(
        Method::Get,
        &format!("/v2/orders/{ID1}"),
        ok(200, &alpaca_order(ID1, "canceled").to_string()),
    );
    h.svc.cancel(ID1).await.unwrap();
    assert_eq!(h.last_state()["orders"][0]["status"], "cancelled");
}

#[tokio::test]
async fn a_stream_fill_updates_the_order_and_sends_a_fill_frame_after_the_state() {
    let h = harness();
    let frame = json!({"stream": "trade_updates", "data": {"event": "fill", "timestamp": "2026-10-28T14:00:02Z",
        "price": "99.9", "qty": "1", "position_qty": "1",
        "order": {"id": ID1, "symbol": "AAPL", "side": "buy", "type": "limit", "qty": "1", "filled_qty": "1",
                  "filled_avg_price": "99.9", "limit_price": "100", "status": "filled",
                  "created_at": "2026-10-28T14:00:00Z", "updated_at": "2026-10-28T14:00:02Z"}}});
    let StreamMsg::TradeUpdate(u) = parse_frame(frame.to_string().as_bytes()).unwrap() else {
        panic!()
    };
    h.svc.apply_update(*u);
    let frames = h.frames.lock().unwrap().clone();
    let n = frames.len();
    let state: Value = serde_json::from_str(&frames[n - 2]).unwrap();
    let fill: Value = serde_json::from_str(&frames[n - 1]).unwrap();
    assert_eq!(state["orders"][0]["status"], "filled");
    assert_eq!(state["fills"][0]["price"], 99.9);
    assert_eq!(fill["type"], "paper.fill");
    assert_eq!(fill["broker"], "alpaca_paper");
    assert_eq!(fill["simulated"], false);
    assert_eq!(fill["t"], 1_793_196_002_000_i64);
}

#[tokio::test]
async fn a_401_on_resync_marks_the_route_unauthorized_and_orders_are_refused() {
    let h = harness();
    h.http.push(
        Method::Get,
        "/v2/account",
        ok(401, r#"{"message":"unauthorized."}"#),
    );
    assert!(h.svc.resync().await.is_err());
    let st = h.svc.status().borrow().clone();
    assert!(!st.configured);
    assert_eq!(st.state, "unauthorized");
    assert!(st.message.contains("ALPACA_PAPER_KEY"));
    h.svc
        .place(place(json!({"clientOrderId": "c-6", "symbol": "AAPL", "side": "buy", "orderType": "market", "qty": 1})))
        .await;
    assert_eq!(
        h.last_state()["orders"][0]["rejectReason"],
        "not_configured"
    );
    assert_eq!(h.http.sent().len(), 1);
}

#[tokio::test]
async fn resync_takes_the_whole_snapshot_and_marks_the_route_configured() {
    let h = harness();
    h.http.push(Method::Get, "/v2/account", ok(200, r#"{"status":"ACTIVE","cash":"100000","buying_power":"400000","equity":"100000","last_equity":"100000"}"#));
    h.http.push(
        Method::Get,
        "/v2/positions",
        ok(
            200,
            r#"[{"symbol":"AAPL","qty":"2","avg_entry_price":"100","side":"long"}]"#,
        ),
    );
    h.http.push(
        Method::Get,
        "/v2/orders?status=open",
        ok(200, &json!([alpaca_order(ID1, "new")]).to_string()),
    );
    h.http.push(
        Method::Get,
        "/v2/orders?status=closed",
        ok(200, &json!([alpaca_order(ID2, "filled")]).to_string()),
    );
    h.http.push(Method::Get, "/v2/account/activities/FILL", ok(200, &json!([{"order_id": ID2, "symbol": "AAPL", "side": "buy", "qty": "1", "price": "100", "transaction_time": "2026-10-28T14:00:01Z"}]).to_string()));
    h.svc.resync().await.unwrap();
    let s = h.last_state();
    assert_eq!(s["account"]["buyingPower"], 400000.0);
    assert_eq!(s["positions"][0]["qty"], 2);
    assert_eq!(s["orders"].as_array().unwrap().len(), 2);
    assert_eq!(s["fills"].as_array().unwrap().len(), 1);
    assert!(h.svc.status().borrow().configured);
}

#[test]
fn the_real_client_refuses_the_live_host_before_sending() {
    use crate::http::{Request, ReqwestHttp};
    let creds = crate::Credentials::new("PKTEST", "secret", crate::CredSource::Paper);
    let http = ReqwestHttp::new(creds).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for url in [
        "https://api.alpaca.markets/v2/orders",
        "https://api.alpaca.markets/v2/account",
    ] {
        let req = Request {
            method: Method::Get,
            url: url.into(),
            body: None,
        };
        let e = rt.block_on(http.send(&req)).unwrap_err();
        assert!(e.contains("refused"), "{e}");
    }
}
