//! The paper account's trade updates stream (`wss://paper-api.alpaca.markets/stream`, stream
//! `trade_updates`): parsing, and the connection loop with reconnect and backoff.
//!
//! Protocol (docs.alpaca.markets, "WebSocket streaming"): send
//! `{"action":"auth","key":..,"secret":..}`, expect `{"stream":"authorization","data":{"status":
//! "authorized"}}`, send `{"action":"listen","data":{"streams":["trade_updates"]}}`, expect
//! `{"stream":"listening",..}`, then `{"stream":"trade_updates","data":{"event":..,"order":{..},
//! "price":..,"qty":..,"timestamp":..}}`. The paper endpoint sends **binary** frames holding the
//! same JSON, so both frame kinds are parsed.
//!
//! Fills come only from this stream (no polling loop). After every (re)connect the service takes
//! a REST snapshot, so anything missed while disconnected is resynced.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::alpaca::AlpacaOrder;
use crate::creds::Credentials;
use crate::host::{PAPER_STREAM, ensure_paper};

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct TradeUpdate {
    pub event: String,
    pub order: AlpacaOrder,
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub price: Option<String>,
    #[serde(default)]
    pub qty: Option<String>,
    #[serde(default)]
    pub position_qty: Option<String>,
}

impl TradeUpdate {
    pub fn is_fill(&self) -> bool {
        matches!(self.event.as_str(), "fill" | "partial_fill")
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum StreamMsg {
    Authorized,
    Unauthorized,
    Listening(Vec<String>),
    TradeUpdate(Box<TradeUpdate>),
    /// `{"action":"error","data":{"error_message":..}}`, sent before Alpaca closes.
    Error(String),
    Other,
}

/// One frame's JSON (text or binary) to a message.
pub fn parse_frame(bytes: &[u8]) -> Result<StreamMsg, String> {
    let v: Value = serde_json::from_slice(bytes).map_err(|e| format!("not JSON: {e}"))?;
    if v["action"] == "error" {
        let m = v["data"]["error_message"]
            .as_str()
            .unwrap_or("stream error")
            .to_owned();
        return Ok(StreamMsg::Error(m));
    }
    match v["stream"].as_str() {
        Some("authorization") => Ok(if v["data"]["status"] == "authorized" {
            StreamMsg::Authorized
        } else {
            StreamMsg::Unauthorized
        }),
        Some("listening") => Ok(StreamMsg::Listening(
            v["data"]["streams"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
        )),
        Some("trade_updates") => serde_json::from_value::<TradeUpdate>(v["data"].clone())
            .map(|u| StreamMsg::TradeUpdate(Box::new(u)))
            .map_err(|e| format!("bad trade update: {e}")),
        _ => Ok(StreamMsg::Other),
    }
}

/// What the loop reports to the service.
pub trait StreamSink: Send + Sync {
    /// Connected, authorized and listening: time to resync.
    fn listening(&self) -> impl std::future::Future<Output = ()> + Send;
    fn update(&self, u: TradeUpdate);
    /// The stream is down; `unauthorized` when Alpaca refused the keys.
    fn down(&self, reason: &str, unauthorized: bool);
    fn connecting(&self);
}

const HANDSHAKE: Duration = Duration::from_secs(10);
const PING_EVERY: Duration = Duration::from_secs(30);
/// No frame at all (not even a pong) for this long: the connection is presumed dead.
const SILENCE: Duration = Duration::from_secs(75);
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// After Alpaca refused the keys, retry rarely (they only change with a restart, but a transient
/// auth outage should still heal).
const UNAUTHORIZED_WAIT: Duration = Duration::from_secs(300);
/// A session that streamed this long resets the backoff.
const STABLE: Duration = Duration::from_secs(60);

enum End {
    Lost(String),
    Unauthorized(String),
}

/// Runs forever: connect, authorize, listen, relay; on any failure wait (exponential, jittered)
/// and reconnect.
pub async fn run(creds: Credentials, sink: impl StreamSink) {
    let mut wait = BACKOFF_MIN;
    let mut seed = (std::process::id() as u64) ^ 0x9e37_79b9_7f4a_7c15;
    loop {
        sink.connecting();
        let started = Instant::now();
        let (reason, unauthorized) = match session(&creds, &sink).await {
            End::Lost(r) => (r, false),
            End::Unauthorized(r) => (r, true),
        };
        sink.down(&reason, unauthorized);
        if started.elapsed() >= STABLE {
            wait = BACKOFF_MIN;
        }
        let base = if unauthorized {
            UNAUTHORIZED_WAIT
        } else {
            wait
        };
        // Equal jitter: half fixed, half random.
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let half = base.as_millis() as u64 / 2;
        let sleep = Duration::from_millis(half + seed % (half + 1));
        tracing::info!(target: "alpaca_paper", reason = %reason, retry_in_ms = sleep.as_millis() as u64, "trade stream down");
        tokio::time::sleep(sleep).await;
        wait = (wait * 2).min(BACKOFF_MAX);
    }
}

fn tls() -> Result<tokio_tungstenite::Connector, String> {
    use rustls_platform_verifier::BuilderVerifierExt;
    let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
    let cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("tls: {e}"))?
        .with_platform_verifier()
        .map_err(|e| format!("tls verifier: {e}"))?
        .with_no_client_auth();
    Ok(tokio_tungstenite::Connector::Rustls(std::sync::Arc::new(
        cfg,
    )))
}

async fn session(creds: &Credentials, sink: &impl StreamSink) -> End {
    if let Err(e) = ensure_paper(PAPER_STREAM) {
        return End::Lost(e.to_string());
    }
    let connector = match tls() {
        Ok(c) => c,
        Err(e) => return End::Lost(e),
    };
    let connecting = tokio_tungstenite::connect_async_tls_with_config(
        PAPER_STREAM,
        None,
        false,
        Some(connector),
    );
    let ws = match tokio::time::timeout(HANDSHAKE, connecting).await {
        Err(_) => return End::Lost("connect timed out".into()),
        Ok(Err(e)) => return End::Lost(creds.scrub(&format!("connect failed: {e}"))),
        Ok(Ok((ws, _))) => ws,
    };
    let (mut tx, mut rx) = ws.split();
    let auth = json!({"action": "auth", "key": creds.key(), "secret": creds.secret()});
    if let Err(e) = tx.send(Message::Text(auth.to_string().into())).await {
        return End::Lost(format!("send failed: {e}"));
    }
    drop(auth);
    let mut listening = false;
    let deadline = Instant::now() + HANDSHAKE;
    let mut last_frame = Instant::now();
    let mut ping = tokio::time::interval_at(Instant::now() + PING_EVERY, PING_EVERY);
    loop {
        let msg = tokio::select! {
            _ = tokio::time::sleep_until(deadline), if !listening => {
                return End::Lost("no listen acknowledgement in time".into());
            }
            _ = tokio::time::sleep_until(last_frame + SILENCE) => {
                return End::Lost("no frame for 75 s; connection presumed dead".into());
            }
            _ = ping.tick() => {
                if let Err(e) = tx.send(Message::Ping(Default::default())).await {
                    return End::Lost(format!("ping failed: {e}"));
                }
                continue;
            }
            m = rx.next() => m,
        };
        last_frame = Instant::now();
        let bytes: Vec<u8> = match msg {
            None => return End::Lost("connection closed".into()),
            Some(Err(e)) => return End::Lost(format!("read error: {e}")),
            Some(Ok(Message::Text(t))) => t.as_bytes().to_vec(),
            Some(Ok(Message::Binary(b))) => b.to_vec(),
            Some(Ok(Message::Close(f))) => {
                return End::Lost(format!(
                    "closed by Alpaca{}",
                    f.map(|f| format!(": {}", f.reason)).unwrap_or_default()
                ));
            }
            Some(Ok(_)) => continue,
        };
        match parse_frame(&bytes) {
            Ok(StreamMsg::Authorized) => {
                let listen = json!({"action": "listen", "data": {"streams": ["trade_updates"]}});
                if let Err(e) = tx.send(Message::Text(listen.to_string().into())).await {
                    return End::Lost(format!("send failed: {e}"));
                }
            }
            Ok(StreamMsg::Unauthorized) => {
                return End::Unauthorized("Alpaca paper refused the server's keys".into());
            }
            Ok(StreamMsg::Listening(s)) if s.iter().any(|x| x == "trade_updates") => {
                listening = true;
                // Inline on purpose: updates arriving meanwhile wait in the socket and are applied
                // after the snapshot, in order, so the newest state wins.
                sink.listening().await;
            }
            Ok(StreamMsg::Listening(_)) => {
                return End::Lost("trade_updates was not granted".into());
            }
            Ok(StreamMsg::TradeUpdate(u)) => sink.update(*u),
            Ok(StreamMsg::Error(m)) => {
                let m = creds.scrub(&m);
                if m.to_ascii_lowercase().contains("auth") {
                    return End::Unauthorized(m);
                }
                return End::Lost(m);
            }
            Ok(StreamMsg::Other) => {}
            Err(e) => tracing::warn!(target: "alpaca_paper", error = %e, "unreadable stream frame"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_frames_parse() {
        assert_eq!(
            parse_frame(br#"{"stream":"authorization","data":{"status":"authorized","action":"authenticate"}}"#),
            Ok(StreamMsg::Authorized)
        );
        assert_eq!(
            parse_frame(br#"{"stream":"authorization","data":{"status":"unauthorized","action":"authenticate"}}"#),
            Ok(StreamMsg::Unauthorized)
        );
        assert_eq!(
            parse_frame(br#"{"stream":"listening","data":{"streams":["trade_updates"]}}"#),
            Ok(StreamMsg::Listening(vec!["trade_updates".into()]))
        );
        assert_eq!(
            parse_frame(br#"{"action":"error","data":{"error_message":"auth timeout"}}"#),
            Ok(StreamMsg::Error("auth timeout".into()))
        );
        assert!(parse_frame(b"not json").is_err());
    }

    #[test]
    fn a_fill_update_parses_with_its_execution_fields() {
        let frame = br#"{"stream":"trade_updates","data":{"event":"fill","execution_id":"x",
            "timestamp":"2026-10-28T14:00:02.5Z","price":"179.08","qty":"1","position_qty":"1",
            "order":{"id":"61e69015-8549-4bfd-b9c3-01e75843f47d","client_order_id":"c-1",
            "symbol":"AAPL","side":"buy","type":"limit","order_type":"limit","qty":"1",
            "filled_qty":"1","filled_avg_price":"179.08","limit_price":"180","status":"filled",
            "created_at":"2026-10-28T14:00:00Z","updated_at":"2026-10-28T14:00:02.5Z",
            "order_class":"","extended_hours":false,"legs":null}}}"#;
        let StreamMsg::TradeUpdate(u) = parse_frame(frame).unwrap() else {
            panic!("not a trade update")
        };
        assert!(u.is_fill());
        assert_eq!(u.price.as_deref(), Some("179.08"));
        assert_eq!(u.order.status, "filled");
        assert_eq!(u.order.filled_avg_price.as_deref(), Some("179.08"));
    }

    #[test]
    fn a_cancel_update_parses_without_execution_fields() {
        let frame = br#"{"stream":"trade_updates","data":{"event":"canceled",
            "timestamp":"2026-10-28T14:00:05Z","order":{"id":"61e69015-8549-4bfd-b9c3-01e75843f47d",
            "symbol":"AAPL","side":"buy","type":"limit","qty":"1","filled_qty":"0",
            "limit_price":"90","status":"canceled"}}}"#;
        let StreamMsg::TradeUpdate(u) = parse_frame(frame).unwrap() else {
            panic!("not a trade update")
        };
        assert!(!u.is_fill());
        assert_eq!(u.event, "canceled");
        assert!(u.price.is_none());
    }

    #[test]
    fn unknown_streams_are_ignored() {
        assert_eq!(
            parse_frame(br#"{"stream":"other","data":{}}"#),
            Ok(StreamMsg::Other)
        );
    }
}
