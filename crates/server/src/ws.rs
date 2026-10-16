//! `/ws`: JSON text frames. Mirrors packages/shared/src/protocol.ts.
//!
//! The client must send `hello` (with the token when auth is on) within
//! HELLO_TIMEOUT; the server answers `welcome`, then pings every HEARTBEAT
//! and drops a client that has been silent for 3 heartbeats.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade, close_code};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use tokio::time::{Instant, interval_at, timeout};

use crate::{AppState, COMMIT, VERSION, auth};

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
