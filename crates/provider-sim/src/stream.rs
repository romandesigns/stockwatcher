//! Alpaca market-data WebSocket protocol (v2 stream), served from replay.
//!
//! Connect → `[{"T":"success","msg":"connected"}]`; authenticate within 10 s with
//! `{"action":"auth","key","secret"}` (or the `APCA-API-KEY-ID`/`APCA-API-SECRET-KEY` headers on
//! the upgrade) → `[{"T":"success","msg":"authenticated"}]`; then `subscribe`/`unsubscribe`, each
//! answered by the full current `subscription`. Data arrives as JSON arrays of messages in replay
//! order. Errors use Alpaca's codes: 400 invalid syntax, 401 not authenticated, 402 auth failed,
//! 403 already authenticated, 404 auth timeout, 407 slow client (then the server closes).
//! Not enforced: connection limits (406), symbol limits (405), entitlements (409).

use crate::alpaca::{control, error_msg, success};
use crate::rest::credentials_ok;
use crate::{Channel, Sim, StreamMsg};
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::Response;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast::error::{RecvError, TryRecvError};
use tokio::time::Instant;

const AUTH_TIMEOUT: Duration = Duration::from_secs(10);
/// Messages coalesced into one frame at most.
const MAX_BATCH: usize = 1000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Sip,
    Boats,
    News,
}

/// Subscription lists, echoed in subscription order.
#[derive(Default)]
struct Subs {
    trades: Vec<String>,
    quotes: Vec<String>,
    bars: Vec<String>,
    updated_bars: Vec<String>,
    daily_bars: Vec<String>,
    statuses: Vec<String>,
    lulds: Vec<String>,
    news: Vec<String>,
}

fn has(list: &[String], sym: &str) -> bool {
    list.iter().any(|s| s == "*" || s == sym)
}

impl Subs {
    fn list_mut(&mut self, key: &str) -> Option<&mut Vec<String>> {
        Some(match key {
            "trades" => &mut self.trades,
            "quotes" => &mut self.quotes,
            "bars" => &mut self.bars,
            "updatedBars" => &mut self.updated_bars,
            "dailyBars" => &mut self.daily_bars,
            "statuses" => &mut self.statuses,
            "lulds" => &mut self.lulds,
            "news" => &mut self.news,
            _ => return None,
        })
    }

    fn wants(&self, m: &StreamMsg) -> bool {
        let list = match m.channel {
            // Corrections and cancel/errors follow the trade subscription.
            Channel::Trades | Channel::Corrections | Channel::CancelErrors => &self.trades,
            Channel::Quotes => &self.quotes,
            Channel::Bars => &self.bars,
            Channel::UpdatedBars => &self.updated_bars,
            Channel::Statuses => &self.statuses,
        };
        has(list, &m.symbol)
    }

    fn message(&self, kind: Kind) -> String {
        if kind == Kind::News {
            return control(json!({"T": "subscription", "news": self.news}));
        }
        control(json!({
            "T": "subscription",
            "trades": self.trades,
            "quotes": self.quotes,
            "bars": self.bars,
            "updatedBars": self.updated_bars,
            "dailyBars": self.daily_bars,
            "statuses": self.statuses,
            "lulds": self.lulds,
            "corrections": self.trades,
            "cancelErrors": self.trades,
        }))
    }
}

pub async fn sip(ws: WebSocketUpgrade, State(sim): State<Sim>, headers: HeaderMap) -> Response {
    upgrade(ws, sim, headers, Kind::Sip)
}

pub async fn boats(ws: WebSocketUpgrade, State(sim): State<Sim>, headers: HeaderMap) -> Response {
    upgrade(ws, sim, headers, Kind::Boats)
}

/// Alpaca's `overnight` feed; in the simulator it carries the same overnight-session events as
/// BOATS.
pub async fn overnight(
    ws: WebSocketUpgrade,
    State(sim): State<Sim>,
    headers: HeaderMap,
) -> Response {
    upgrade(ws, sim, headers, Kind::Boats)
}

pub async fn news(ws: WebSocketUpgrade, State(sim): State<Sim>, headers: HeaderMap) -> Response {
    upgrade(ws, sim, headers, Kind::News)
}

fn upgrade(ws: WebSocketUpgrade, sim: Sim, headers: HeaderMap, kind: Kind) -> Response {
    let header_auth = credentials_ok(&sim, &headers);
    ws.max_message_size(64 * 1024)
        .on_upgrade(move |socket| session(socket, sim, kind, header_auth))
}

async fn send(socket: &mut WebSocket, text: String) -> bool {
    socket.send(Message::Text(text.into())).await.is_ok()
}

async fn session(mut socket: WebSocket, sim: Sim, kind: Kind, header_auth: bool) {
    let mut rx = sim.0.tx.subscribe();
    if !send(&mut socket, success("connected")).await {
        return;
    }
    let mut authed = header_auth;
    if authed && !send(&mut socket, success("authenticated")).await {
        return;
    }
    let deadline = Instant::now() + AUTH_TIMEOUT;
    let mut subs = Subs::default();
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline), if !authed => {
                let _ = send(&mut socket, error_msg(404, "auth timeout")).await;
                let _ = socket.send(Message::Close(None)).await;
                return;
            }
            msg = socket.recv() => {
                let Some(Ok(msg)) = msg else { return };
                let text = match msg {
                    Message::Text(t) => t.to_string(),
                    Message::Close(_) => return,
                    _ => continue,
                };
                let reply = handle(&sim, kind, &text, &mut authed, &mut subs);
                for r in reply.messages {
                    if !send(&mut socket, r).await {
                        return;
                    }
                }
                if reply.close {
                    let _ = socket.send(Message::Close(None)).await;
                    return;
                }
            }
            got = rx.recv() => {
                let first = match got {
                    Ok(b) => b,
                    Err(RecvError::Lagged(_)) => {
                        let _ = send(&mut socket, error_msg(407, "slow client")).await;
                        let _ = socket.send(Message::Close(None)).await;
                        return;
                    }
                    Err(RecvError::Closed) => return,
                };
                if !authed || kind == Kind::News {
                    continue;
                }
                let mut batches: Vec<Arc<Vec<StreamMsg>>> = vec![first];
                let mut n = batches[0].len();
                while n < MAX_BATCH {
                    match rx.try_recv() {
                        Ok(b) => {
                            n += b.len();
                            batches.push(b);
                        }
                        Err(TryRecvError::Lagged(_)) => {
                            let _ = send(&mut socket, error_msg(407, "slow client")).await;
                            let _ = socket.send(Message::Close(None)).await;
                            return;
                        }
                        Err(_) => break,
                    }
                }
                let parts: Vec<&str> = batches
                    .iter()
                    .flat_map(|b| b.iter())
                    .filter(|m| m.overnight == (kind == Kind::Boats) && subs.wants(m))
                    .map(|m| m.json.as_str())
                    .collect();
                if !parts.is_empty() && !send(&mut socket, format!("[{}]", parts.join(","))).await {
                    return;
                }
            }
        }
    }
}

struct Reply {
    messages: Vec<String>,
    close: bool,
}

fn handle(sim: &Sim, kind: Kind, text: &str, authed: &mut bool, subs: &mut Subs) -> Reply {
    let one = |m: String| Reply {
        messages: vec![m],
        close: false,
    };
    let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(text) else {
        return one(error_msg(400, "invalid syntax"));
    };
    match obj.get("action").and_then(Value::as_str) {
        Some("auth") => {
            if *authed {
                return one(error_msg(403, "already authenticated"));
            }
            let cfg = sim.config();
            let key = obj.get("key").and_then(Value::as_str).unwrap_or("");
            let secret = obj.get("secret").and_then(Value::as_str).unwrap_or("");
            let ok = bool::from(subtle::ConstantTimeEq::ct_eq(
                key.as_bytes(),
                cfg.alpaca_key.as_bytes(),
            )) & bool::from(subtle::ConstantTimeEq::ct_eq(
                secret.as_bytes(),
                cfg.alpaca_secret.as_bytes(),
            ));
            if ok {
                *authed = true;
                one(success("authenticated"))
            } else {
                Reply {
                    messages: vec![error_msg(402, "auth failed")],
                    close: true,
                }
            }
        }
        Some(action @ ("subscribe" | "unsubscribe")) => {
            if !*authed {
                return one(error_msg(401, "not authenticated"));
            }
            for (key, val) in &obj {
                if key == "action" {
                    continue;
                }
                let allowed = match kind {
                    Kind::News => key == "news",
                    _ => key != "news",
                };
                let (true, Some(list), Some(arr)) = (allowed, subs.list_mut(key), val.as_array())
                else {
                    return one(error_msg(400, "invalid syntax"));
                };
                for sym in arr.iter().filter_map(Value::as_str) {
                    let sym = if sym == "*" {
                        sym.to_string()
                    } else {
                        sym.to_ascii_uppercase()
                    };
                    if action == "subscribe" {
                        if !list.contains(&sym) {
                            list.push(sym);
                        }
                    } else {
                        list.retain(|s| *s != sym);
                    }
                }
            }
            one(subs.message(kind))
        }
        _ => one(error_msg(400, "invalid syntax")),
    }
}
