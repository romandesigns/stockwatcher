//! `/ws`: JSON text frames. Mirrors packages/shared/src/protocol.ts.
//!
//! The client must send `hello` (with the token when auth is on) within
//! HELLO_TIMEOUT; the server answers `welcome`, then pings every HEARTBEAT
//! and drops a client that has been silent for 3 heartbeats.
//!
//! Chart protocol (after `welcome`; crates/chart-engine wire types, mirrored by
//! packages/chart-core/src/types.ts):
//! - client `chart.subscribe {symbol, timeframe, profileBin}` -> server
//!   `chart.snapshot`, the recent `tape.trade` rows, then increments
//!   (`chart.update` / `chart.remove` / `chart.gap`) for that symbol and
//!   timeframe and `tape.trade` for the symbol. One subscription per
//!   connection: a new subscribe (e.g. another timeframe) replaces it and
//!   starts with a fresh snapshot of the same engine state.
//! - server `replay.clock {clock}` to every client, on change and ~4 Hz while
//!   playing.
//! - client `replay.control {action, value}` (play, pause, step, stepInterval,
//!   seek, speed): honoured in replay mode, `error not_replay` in live mode.
//!   Success has no direct reply; the resulting `replay.clock` is the answer.
//! - A client that falls more than the broadcast capacity behind, and every
//!   client after a seek, gets a fresh `chart.snapshot` (resync).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade, close_code};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use tokio::time::{Instant, interval_at, timeout};

use crate::market::{ControlError, Market, OutKind, SnapshotError};
use crate::{AppState, COMMIT, VERSION, api, auth};
use chart_engine::Timeframe;
use market_events::Price;
use tokio::sync::broadcast::error::RecvError;

pub const HEARTBEAT: Duration = Duration::from_secs(15);
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_MESSAGE_BYTES: usize = 64 * 1024;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum ClientMessage {
    Hello {
        token: Option<String>,
        client: Option<String>,
    },
    Ping {
        ts: u64,
    },
    Pong {
        #[allow(dead_code)]
        ts: u64,
    },
    #[serde(rename = "chart.subscribe")]
    ChartSubscribe {
        symbol: String,
        timeframe: Timeframe,
        #[serde(rename = "profileBin", default)]
        profile_bin: Option<f64>,
    },
    #[serde(rename = "replay.control")]
    ReplayControl {
        action: String,
        #[serde(default)]
        value: Option<serde_json::Value>,
    },
}

/// The connection's chart subscription.
struct Sub {
    symbol: String,
    timeframe: Timeframe,
    bin: Price,
    /// Increments with `seq` <= this are already in the snapshot.
    seq: u64,
}

async fn send_text(socket: &mut WebSocket, text: String) -> bool {
    socket.send(Message::Text(text.into())).await.is_ok()
}

/// Sends a snapshot and the recent tape for `sub`, and records its sequence.
async fn send_snapshot(socket: &mut WebSocket, market: &Market, sub: &mut Sub) -> bool {
    match market.snapshot(&sub.symbol, sub.timeframe, sub.bin) {
        Ok(s) => {
            sub.seq = s.seq;
            if !send_text(socket, s.json).await {
                return false;
            }
            for row in s.tape {
                if !send_text(socket, row).await {
                    return false;
                }
            }
            true
        }
        Err(SnapshotError::UnknownSymbol) => {
            send(
                socket,
                &ServerMessage::Error {
                    code: "unknown_symbol",
                    message: "no data for this symbol",
                },
            )
            .await
        }
        Err(SnapshotError::OutsidePriceBand) => {
            send(
                socket,
                &ServerMessage::Error {
                    code: "outside_price_band",
                    message: "not a candidate: price outside the configured band",
                },
            )
            .await
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum ServerMessage<'a> {
    Welcome {
        version: &'a str,
        commit: &'a str,
        #[serde(rename = "heartbeatSecs")]
        heartbeat_secs: u64,
    },
    Ping {
        ts: u64,
    },
    Pong {
        ts: u64,
    },
    Error {
        code: &'a str,
        message: &'a str,
    },
}

pub async fn upgrade(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.max_message_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| session(socket, state))
}

async fn send(socket: &mut WebSocket, msg: &ServerMessage<'_>) -> bool {
    let text = serde_json::to_string(msg).expect("server messages always serialize");
    socket.send(Message::Text(text.into())).await.is_ok()
}

async fn fail(mut socket: WebSocket, code: &str, message: &str, close: u16) {
    let _ = send(&mut socket, &ServerMessage::Error { code, message }).await;
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code: close,
            reason: message.to_owned().into(),
        })))
        .await;
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Waits for the next text frame, skipping transport-level ping/pong.
async fn next_text(socket: &mut WebSocket) -> Option<String> {
    while let Some(Ok(msg)) = socket.recv().await {
        match msg {
            Message::Text(t) => return Some(t.to_string()),
            Message::Close(_) => return None,
            _ => {}
        }
    }
    None
}

async fn session(mut socket: WebSocket, state: AppState) {
    let mut shutdown = state.shutdown_signal();
    if *shutdown.borrow() {
        return fail(
            socket,
            "shutting_down",
            "server restarting",
            close_code::AWAY,
        )
        .await;
    }

    let hello = match timeout(HELLO_TIMEOUT, next_text(&mut socket)).await {
        Ok(Some(text)) => serde_json::from_str::<ClientMessage>(&text).ok(),
        Ok(None) => return,
        Err(_) => {
            return fail(
                socket,
                "hello_timeout",
                "expected hello",
                close_code::POLICY,
            )
            .await;
        }
    };
    let Some(ClientMessage::Hello { token, client }) = hello else {
        return fail(
            socket,
            "bad_request",
            "first message must be hello",
            close_code::POLICY,
        )
        .await;
    };
    if let Some(expected) = state.api_token()
        && !token
            .as_deref()
            .is_some_and(|t| auth::token_matches(expected, t))
    {
        tracing::info!(client = client.as_deref(), "ws hello rejected: bad token");
        // 4401 mirrors HTTP 401 in the application close-code range.
        return fail(socket, "unauthorized", "invalid access token", 4401).await;
    }
    tracing::debug!(client = client.as_deref(), "ws client connected");

    let welcome = ServerMessage::Welcome {
        version: VERSION,
        commit: COMMIT,
        heartbeat_secs: HEARTBEAT.as_secs(),
    };
    if !send(&mut socket, &welcome).await {
        return;
    }

    let market = state.market().clone();
    let mut feed = market.subscribe();
    if let Some(frame) = market.clock_frame()
        && !send_text(&mut socket, frame).await
    {
        return;
    }
    let mut sub: Option<Sub> = None;

    let mut heartbeat = interval_at(Instant::now() + HEARTBEAT, HEARTBEAT);
    let mut last_seen = Instant::now();
    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                if last_seen.elapsed() > HEARTBEAT * 3 {
                    tracing::debug!("ws client silent for 3 heartbeats, closing");
                    break;
                }
                if !send(&mut socket, &ServerMessage::Ping { ts: now_ms() }).await {
                    break;
                }
            }
            out = feed.recv() => {
                let ok = match out {
                    Ok(o) => match &o.kind {
                        OutKind::Clock(frame) => send_text(&mut socket, frame.clone()).await,
                        OutKind::Resync => match sub.as_mut() {
                            Some(s) => send_snapshot(&mut socket, &market, s).await,
                            None => true,
                        },
                        OutKind::Chart { symbol, timeframe, update, json } => match sub.as_ref() {
                            // The stock traded out of the price band: it is no longer a
                            // candidate, so its stream stops here and the client is told why.
                            Some(s) if *symbol == s.symbol && !market.is_candidate(symbol) => {
                                sub = None;
                                send(
                                    &mut socket,
                                    &ServerMessage::Error {
                                        code: "outside_price_band",
                                        message: "no longer a candidate: price left the configured band",
                                    },
                                )
                                .await
                            }
                            Some(s)
                                if *symbol == s.symbol
                                    && o.seq > s.seq
                                    && timeframe.is_none_or(|tf| tf == s.timeframe) =>
                            {
                                send_text(&mut socket, market.rebin(update, json, s.bin)).await
                            }
                            _ => true,
                        },
                    },
                    Err(RecvError::Lagged(n)) => {
                        tracing::debug!(skipped = n, "ws client lagged; resyncing with a snapshot");
                        match sub.as_mut() {
                            Some(s) => send_snapshot(&mut socket, &market, s).await,
                            None => true,
                        }
                    }
                    Err(RecvError::Closed) => true,
                };
                if !ok {
                    break;
                }
            }
            _ = shutdown.changed() => {
                fail(socket, "shutting_down", "server restarting", close_code::AWAY).await;
                return;
            }
            msg = socket.recv() => {
                let Some(Ok(msg)) = msg else { break };
                last_seen = Instant::now();
                let Message::Text(text) = msg else {
                    if matches!(msg, Message::Close(_)) { break }
                    continue;
                };
                let reply = match serde_json::from_str::<ClientMessage>(&text) {
                    Ok(ClientMessage::Ping { ts }) => Some(ServerMessage::Pong { ts }),
                    Ok(ClientMessage::Pong { .. }) => None,
                    Ok(ClientMessage::ChartSubscribe { symbol, timeframe, profile_bin }) => {
                        match api::parse_bin(profile_bin, market.default_bin()) {
                            Ok(bin) => {
                                let mut s = Sub {
                                    symbol: symbol.trim().to_ascii_uppercase(),
                                    timeframe,
                                    bin,
                                    seq: 0,
                                };
                                if !send_snapshot(&mut socket, &market, &mut s).await {
                                    break;
                                }
                                sub = Some(s);
                                None
                            }
                            Err(_) => Some(ServerMessage::Error {
                                code: "bad_request",
                                message: "invalid profileBin",
                            }),
                        }
                    }
                    Ok(ClientMessage::ReplayControl { action, value }) => {
                        match market.control(&action, value.as_ref()).await {
                            Ok(_) => None,
                            Err(ControlError::NotReplay) => Some(ServerMessage::Error {
                                code: "not_replay",
                                message: "replay controls are only available in replay mode",
                            }),
                            Err(ControlError::BadRequest(m)) => {
                                tracing::debug!(error = %m, "bad replay.control");
                                Some(ServerMessage::Error {
                                    code: "bad_request",
                                    message: "invalid replay.control",
                                })
                            }
                            Err(ControlError::Failed(_)) => Some(ServerMessage::Error {
                                code: "replay_unavailable",
                                message: "replay is not running",
                            }),
                        }
                    }
                    Ok(ClientMessage::Hello { .. }) | Err(_) => Some(ServerMessage::Error {
                        code: "bad_request",
                        message: "unexpected message",
                    }),
                };
                if let Some(reply) = reply
                    && !send(&mut socket, &reply).await
                {
                    break;
                }
            }
        }
    }
}
