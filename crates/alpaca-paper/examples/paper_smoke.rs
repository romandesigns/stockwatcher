//! One live smoke test against Alpaca PAPER (never run in CI):
//!
//!   cargo run -p alpaca-paper --example paper_smoke -- path/to/.env [SYMBOL]
//!
//! Reads only the key names from that file (ALPACA_PAPER_KEY/SECRET, else ALPACA_API_KEY/SECRET),
//! never prints a value, an account number or a balance. Steps: GET /v2/account (prints the HTTP
//! status and whether the account is ACTIVE); then, only if the keys work on paper, one 1-share
//! buy limit at $5.00 (far under any $10+ stock, so it cannot fill) is placed, seen on the trade
//! stream, cancelled, and the cancel seen on the stream.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use alpaca_paper::alpaca::AlpacaOrder;
use alpaca_paper::http::{Api, Method, ReqwestHttp};
use alpaca_paper::stream::{StreamSink, TradeUpdate};
use alpaca_paper::{CredSource, Credentials};

#[derive(Clone, Default)]
struct Sink {
    listening: Arc<Mutex<bool>>,
    events: Arc<Mutex<Vec<(String, String)>>>,
}

impl StreamSink for Sink {
    async fn listening(&self) {
        *self.listening.lock().unwrap() = true;
    }
    fn update(&self, u: TradeUpdate) {
        self.events
            .lock()
            .unwrap()
            .push((u.order.id.clone(), u.event.clone()));
    }
    fn down(&self, reason: &str, unauthorized: bool) {
        println!("stream down (unauthorized={unauthorized}): {reason}");
    }
    fn connecting(&self) {}
}

fn read_env(path: &str) -> std::collections::HashMap<String, String> {
    let wanted = [
        "ALPACA_PAPER_KEY",
        "ALPACA_PAPER_SECRET",
        "ALPACA_API_KEY",
        "ALPACA_API_SECRET",
    ];
    std::fs::read_to_string(path)
        .expect("env file")
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().trim_matches('"').to_owned()))
        .filter(|(k, _)| wanted.contains(&k.as_str()))
        .collect()
}

async fn wait_for(sink: &Sink, id: &str, events: &[&str]) -> Option<String> {
    for _ in 0..100 {
        if let Some((_, e)) = sink
            .events
            .lock()
            .unwrap()
            .iter()
            .find(|(i, e)| i == id && events.contains(&e.as_str()))
        {
            return Some(e.clone());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    None
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("path to .env");
    let symbol = args.next().unwrap_or_else(|| "AAPL".into());
    let env = read_env(&path);
    let get = |k: &str| env.get(k).cloned().filter(|v| !v.is_empty());
    // Straight from the names (not from_lookup), so even live-looking shared keys get the one
    // harmless GET against the paper host that tells whether they are paper keys.
    let creds = match (get("ALPACA_PAPER_KEY"), get("ALPACA_PAPER_SECRET")) {
        (Some(k), Some(s)) => Credentials::new(k, s, CredSource::Paper),
        _ => match (get("ALPACA_API_KEY"), get("ALPACA_API_SECRET")) {
            (Some(k), Some(s)) => Credentials::new(k, s, CredSource::Shared),
            _ => {
                println!("no Alpaca keys in {path}");
                return;
            }
        },
    };
    println!("keys from: {}", creds.source.env_names());
    let api = Api::new(Arc::new(ReqwestHttp::new(creds.clone()).expect("http")));
    let account = api.call(Method::Get, "/v2/account", None).await;
    match &account {
        Ok(r) => {
            let active = serde_json::from_str::<serde_json::Value>(&r.body)
                .map(|v| v["status"] == "ACTIVE")
                .unwrap_or(false);
            println!(
                "GET /v2/account on paper: HTTP {} ACTIVE={active}",
                r.status
            );
        }
        Err(e) => {
            println!(
                "GET /v2/account on paper: HTTP {} -> not paper keys; stopping",
                e.status().map_or("none".into(), |s| s.to_string())
            );
            return;
        }
    }

    let sink = Sink::default();
    let s = sink.clone();
    let c = creds.clone();
    tokio::spawn(async move { alpaca_paper::stream::run(c, s).await });
    for _ in 0..100 {
        if *sink.listening.lock().unwrap() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    println!("stream listening: {}", *sink.listening.lock().unwrap());

    let clock = api.call(Method::Get, "/v2/clock", None).await.ok();
    let open = clock
        .and_then(|r| serde_json::from_str::<serde_json::Value>(&r.body).ok())
        .is_some_and(|v| v["is_open"] == true);
    let coid = format!("sw-smoke-{}", chrono::Utc::now().timestamp_millis());
    let mut body = serde_json::json!({"symbol": symbol, "qty": "1", "side": "buy", "type": "limit",
        "time_in_force": "day", "limit_price": "5", "client_order_id": coid});
    if !open {
        body["extended_hours"] = serde_json::json!(true);
    }
    println!(
        "market open: {open}; placing 1 {symbol} buy limit $5 (extended_hours={})",
        !open
    );
    let placed = match api
        .call(Method::Post, "/v2/orders", Some(body.to_string()))
        .await
    {
        Ok(r) => serde_json::from_str::<AlpacaOrder>(&r.body).expect("order"),
        Err(e) => {
            println!("place refused: HTTP {:?} {}", e.status(), e.message());
            return;
        }
    };
    println!("placed: status {}", placed.status);
    let seen = wait_for(&sink, &placed.id, &["new", "accepted", "pending_new"]).await;
    println!("stream saw the order: {seen:?}");
    let del = api
        .call(Method::Delete, &format!("/v2/orders/{}", placed.id), None)
        .await;
    println!(
        "cancel: {}",
        match &del {
            Ok(r) => format!("HTTP {}", r.status),
            Err(e) => format!("refused {e}"),
        }
    );
    let seen = wait_for(&sink, &placed.id, &["canceled"]).await;
    println!("stream saw the cancel: {seen:?}");
}
