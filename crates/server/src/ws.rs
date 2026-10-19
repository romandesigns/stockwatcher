//! `/ws`: JSON text frames. Mirrors packages/shared/src/protocol.ts.
//!
//! The client must send `hello` (with the token when auth is on) within
//! HELLO_TIMEOUT; `hello.source` (`live` | `replay`, absent = `MARKET_DATA_MODE`) picks which
//! of the two markets the connection reads for its whole life (crate::market::Markets); the server answers `welcome`, then pings every HEARTBEAT
//! and drops a client that has been silent for 3 heartbeats.
//!
//! Chart protocol (after `welcome`; crates/chart-engine wire types, mirrored by
//! packages/chart-core/src/types.ts):
//! - client `chart.subscribe {symbol, timeframe, profileBin, id?}` -> server
//!   `chart.snapshot`, the recent `tape.trade` rows, then increments
//!   (`chart.update` / `chart.remove` / `chart.gap`) for that symbol and
//!   timeframe, and `tape.trade`, `chart.halt` and `chart.luld` for the symbol
//!   whatever its timeframe (docs/architecture/halts-luld-fills.md).
//! - **Up to MAX_CHARTS charts per connection** (2026-10-18, the dashboard's multi-chart
//!   panel). `id` names a chart slot (≤ MAX_CHART_ID chars; absent = `""`, the single slot every
//!   client used before): a subscribe with an id already held replaces that slot's chart (e.g.
//!   another timeframe) and starts with a fresh snapshot of the same engine state; a new id when
//!   MAX_CHARTS are held is refused with `error too_many_charts`. Client
//!   `chart.unsubscribe {id?}` frees a slot. One socket for every chart, rather than one per
//!   chart, so the market stream is filtered once and paper, settings and clock frames are not
//!   repeated per chart.
//! - **Each frame goes out once per connection**, whatever the number of slots: a candle frame
//!   once per distinct (symbol, timeframe, bin) held, a tape, halt or LULD row once per symbol.
//!   Two slots showing the same chart share one stream; the client routes frames by the symbol,
//!   timeframe and `profileBin` they already carry, not by slot.
//! - Chart errors (`unknown_symbol`, `outside_price_band`) carry the `symbol` and, for a named
//!   slot, its `id`, so a client with several charts knows which one was refused.
//! - server `replay.clock {clock}` to every client, on change and ~4 Hz while
//!   playing.
//! - client `replay.control {action, value}` (play, pause, step, stepInterval,
//!   seek, speed, day): honoured in replay mode, `error not_replay` in live mode.
//!   Success has no direct reply; the resulting `replay.clock` is the answer.
//! - `day` (value `"YYYY-MM-DD"`, one of `GET /api/replay/state`'s `days`)
//!   switches the replayed day: a rebuild exactly like a seek (paper account
//!   reset, tape and quote board rebuilt), on the new day at the configured
//!   `REPLAY_START` time of day, playing or paused as before. A day the
//!   archive does not offer gets `error unknown_day`, one whose files cannot
//!   be opened `error day_unavailable`; either way nothing changes.
//! - A client that falls more than the broadcast capacity behind, and every
//!   client after a seek or a day switch, gets a fresh `chart.snapshot`
//!   (resync). After a day switch the subscribed symbol may not exist on the
//!   new day: the resync then answers `error unknown_symbol` (or
//!   `outside_price_band`), and the client picks a symbol from `/api/symbols`,
//!   which already lists the new day's candidates.
//!
//! Simulated trading (docs/architecture/chart-trading-sim.md, after `welcome`):
//! - server `paper.state` once right after `welcome`, then to every connection
//!   on any change, with a `paper.fill` per fill. A lagged client gets a fresh
//!   `paper.state` too, since it may have missed one.
//! - client `order.place`, `order.modify`, `order.cancel`, `paper.reset`.
//!   Success has no direct reply; the broadcast `paper.state` is the answer.
//!   A malformed or unknown `order.*`/`paper.*` command gets `error bad_order`;
//!   a refused modify/cancel gets `error` with the broker's code (`not_open`, ...).
//!
//! Quote board (the ticker search's live rows; crates/server/src/quotes.rs, after `welcome`):
//! - client `quotes.subscribe` (no payload) -> server `quotes.snapshot {t, quotes: [row]}` at
//!   once with every price-band candidate that has traded, then `quotes.update {t, quotes: [row],
//!   gone?: [symbol]}` with only the rows that changed, at most once per second of wall time
//!   (crate::market::QUOTES_INTERVAL; nothing is sent when nothing changed). `gone` lists symbols
//!   that left the price band. `t` is the replay/live clock (epoch ms) at the take.
//! - Row: `{s, p, pc?, o, v, n, pre?, name?, x?}`: symbol, last price, previous close (the
//!   change-% reference whenever present; from the archive's daily-closes.json), the fallback
//!   reference (the regular open, or the day's first price while `pre` is true), day volume,
//!   prints counted; `name`/`x` (exchange) only in snapshots and for rows entering the band, and
//!   only when the archive's assets.json has them. Prices are JSON numbers in dollars. Change % is
//!   `(p - pc) / pc`, else `(p - o) / o`, computed by the client.
//! - client `quotes.unsubscribe` stops the updates. A subscribe while subscribed re-sends the
//!   snapshot. Subscribing again after a reconnect is the client's job, as for charts.
//! - After a seek (rebuild) and for a lagged connection, a subscribed client gets a fresh
//!   `quotes.snapshot` (it replaces the client's board); no updates go out during a rebuild.
//!
//! Synced settings (crates/server/src/settings.rs, docs/architecture/settings-sync.md, after
//! `welcome`): the user's chart preferences and drawings, shared by every client.
//! - server `settings.snapshot {docs: [{ns, rev, updatedAt, value}]}` once right after `welcome`
//!   (after `paper.state` and the clock), with EVERY namespace (drawings are small, so all symbols'
//!   drawings come at once rather than per chart subscription). It is the whole truth: a client
//!   replaces its copy of each namespace listed. Sent again to a lagged connection; a reconnect
//!   gets one like any new connection.
//! - client `settings.set {ns, value, opId, ifAbsent?}` replaces a namespace's value; with
//!   `ifAbsent: true` only if it has never been written (a client's one-time migration of its
//!   local settings: the first client to connect seeds the shared value).
//! - client `settings.patch {ns, set?: {key: value}, remove?: [key], opId}` sets/removes entries
//!   of an object-valued namespace (a symbol's drawings by id, `drawings:<SYMBOL>`), so edits of
//!   different drawings from two devices never overwrite each other.
//! - server `settings.changed {ns, rev, updatedAt, value, opId}` to EVERY connection, the writer
//!   included, after each applied write (whole value, also after a patch). Last writer wins by
//!   arrival; `rev` climbs by one per namespace; the writer recognises its `opId`. An `ifAbsent`
//!   set that found a value answers only the writer, with the current document and its opId
//!   (rev unchanged).
//! - server `settings.rejected {ns, opId, code, message}` to the writer alone: `bad_request`,
//!   `too_large`, `store_full`, `not_a_map` (drop the edit) or `settings_unavailable` (the disk
//!   write failed: keep it and resend after the next reconnect). Nothing changed.
//! - Values are opaque JSON: the clients own the schemas and validate what they read.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade, close_code};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use tokio::time::{Instant, interval_at, timeout};

use crate::market::{ControlError, Market, OutKind, QuotesGuard, SnapshotError};
use crate::settings::{SettingsError, SettingsOp, rejected_frame as settings_rejected};
use crate::{AppState, COMMIT, Settings, VERSION, api, auth};
use chart_engine::Timeframe;
use market_events::Price;
use paper_broker::wire::{CancelCommand, ModifyCommand, PlaceCommand};
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
        /// `live` or `replay`: which market this connection reads (crate::market::Markets).
        /// Absent = the server's default (`MARKET_DATA_MODE`).
        #[serde(default)]
        source: Option<crate::market::Source>,
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
        /// The chart slot; absent = the single default slot.
        #[serde(default)]
        id: Option<String>,
    },
    #[serde(rename = "chart.unsubscribe")]
    ChartUnsubscribe {
        #[serde(default)]
        id: Option<String>,
    },
    #[serde(rename = "replay.control")]
    ReplayControl {
        action: String,
        #[serde(default)]
        value: Option<serde_json::Value>,
    },
    #[serde(rename = "order.place")]
    OrderPlace(PlaceCommand),
    #[serde(rename = "order.modify")]
    OrderModify(ModifyCommand),
    #[serde(rename = "order.cancel")]
    OrderCancel(CancelCommand),
    #[serde(rename = "paper.reset")]
    PaperReset,
    #[serde(rename = "quotes.subscribe")]
    QuotesSubscribe,
    #[serde(rename = "quotes.unsubscribe")]
    QuotesUnsubscribe,
    #[serde(rename = "settings.set")]
    SettingsSet {
        ns: String,
        value: serde_json::Value,
        #[serde(rename = "opId")]
        op_id: String,
        #[serde(rename = "ifAbsent", default)]
        if_absent: bool,
    },
    #[serde(rename = "settings.patch")]
    SettingsPatch {
        ns: String,
        #[serde(default)]
        set: serde_json::Map<String, serde_json::Value>,
        #[serde(default)]
        remove: Vec<String>,
        #[serde(rename = "opId")]
        op_id: String,
    },
}

/// For a frame in the settings namespace that failed to parse: its `ns` and `opId` if readable,
/// so the rejection names the edit. None for any other frame.
fn settings_frame_ids(text: &str) -> Option<(String, String)> {
    let v = serde_json::from_str::<serde_json::Value>(text).ok()?;
    if !v.get("type")?.as_str()?.starts_with("settings.") {
        return None;
    }
    let field = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_owned();
    Some((field("ns"), field("opId")))
}

/// Applies one settings write for this connection. The success answer (`settings.changed`)
/// reaches this connection through its own settings feed like everyone else's; only an
/// `ifAbsent` reply or a rejection is sent directly. False when the socket is gone.
async fn apply_setting(
    socket: &mut WebSocket,
    settings: &Settings,
    ns: &str,
    op_id: &str,
    op: SettingsOp,
) -> bool {
    match settings.apply(ns, op_id, op).await {
        Ok(None) => true,
        Ok(Some(reply)) => send_text(socket, reply).await,
        Err(e) => {
            if e != SettingsError::Unavailable {
                tracing::debug!(ns, code = e.code(), "settings write refused");
            }
            send_text(socket, settings_rejected(ns, op_id, e.code(), e.message())).await
        }
    }
}

/// The connection's quotes opt-in. Dropping it (unsubscribe, or the session ending) releases the
/// market's subscriber count.
struct QuotesSub {
    _guard: QuotesGuard,
    /// `quotes.update`s with `seq` <= this are already in the snapshot.
    seq: u64,
}

/// Sends a fresh `quotes.snapshot` and records its sequence.
async fn send_quotes_snapshot(socket: &mut WebSocket, market: &Market, q: &mut QuotesSub) -> bool {
    let (seq, json) = market.quotes_snapshot();
    q.seq = seq;
    send_text(socket, json).await
}

/// True for a frame whose `type` is in the paper-trading namespace. Such a frame that failed to
/// parse is a bad order (`bad_order`), not a generic `bad_request`, so the client's order ticket
/// can tell its own bug apart from a protocol mismatch.
fn is_paper_command(text: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|v| v.get("type")?.as_str().map(str::to_owned))
        .is_some_and(|t| t.starts_with("order.") || t.starts_with("paper."))
}

/// How many charts one connection may stream at once (the dashboard's multi-chart panel holds 4).
pub const MAX_CHARTS: usize = 4;
/// Longest chart slot id a client may use.
const MAX_CHART_ID: usize = 32;

/// One of the connection's chart slots.
struct Sub {
    /// The slot's name (`""` for the single default slot).
    id: String,
    symbol: String,
    timeframe: Timeframe,
    bin: Price,
    /// Increments with `seq` <= this are already in the snapshot.
    seq: u64,
}

impl Sub {
    /// Slots with the same key show the same chart, so they share one stream.
    fn same_chart(&self, other: &Sub) -> bool {
        self.symbol == other.symbol && self.timeframe == other.timeframe && self.bin == other.bin
    }
}

async fn send_text(socket: &mut WebSocket, text: String) -> bool {
    socket.send(Message::Text(text.into())).await.is_ok()
}

/// A chart error naming its symbol and (for a named slot) the slot, so a client with several
/// charts knows which one it concerns. Same `{type:"error", code, message}` shape as every
/// other error, with two extra fields a single-chart client ignores.
fn chart_error(code: &str, message: &str, symbol: &str, id: &str) -> String {
    let mut v =
        serde_json::json!({ "type": "error", "code": code, "message": message, "symbol": symbol });
    if !id.is_empty() {
        v["id"] = id.into();
    }
    v.to_string()
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
            let frame = chart_error(
                "unknown_symbol",
                "no data for this symbol",
                &sub.symbol,
                &sub.id,
            );
            send_text(socket, frame).await
        }
        Err(SnapshotError::OutsidePriceBand) => {
            let frame = chart_error(
                "outside_price_band",
                "not a candidate: price outside the configured band",
                &sub.symbol,
                &sub.id,
            );
            send_text(socket, frame).await
        }
    }
}

/// A fresh snapshot for every chart held (after a rebuild or a lag), once per distinct chart:
/// slots sharing a chart take the first one's sequence.
async fn send_snapshots(socket: &mut WebSocket, market: &Market, subs: &mut [Sub]) -> bool {
    for i in 0..subs.len() {
        if let Some(j) = (0..i).find(|&j| subs[j].same_chart(&subs[i])) {
            subs[i].seq = subs[j].seq;
            continue;
        }
        if !send_snapshot(socket, market, &mut subs[i]).await {
            return false;
        }
    }
    true
}

/// [`send_snapshots`] for the slots showing `symbol` only.
async fn send_snapshots_of(
    socket: &mut WebSocket,
    market: &Market,
    subs: &mut [Sub],
    symbol: &str,
) -> bool {
    for i in 0..subs.len() {
        if subs[i].symbol != symbol {
            continue;
        }
        if let Some(j) = (0..i).find(|&j| subs[j].same_chart(&subs[i])) {
            subs[i].seq = subs[j].seq;
            continue;
        }
        if !send_snapshot(socket, market, &mut subs[i]).await {
            return false;
        }
    }
    true
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
    let Some(ClientMessage::Hello {
        token,
        client,
        source,
    }) = hello
    else {
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
    // Subscribed before the paper snapshot below, so no change can fall between the two (at
    // worst the client gets the same state twice, which is harmless: it replaces, never merges).
    let market = state.market_for(source).clone();
    let mut feed = market.subscribe();
    if !send(&mut socket, &welcome).await {
        return;
    }
    if !send_text(&mut socket, market.paper_state_frame()).await {
        return;
    }
    if let Some(frame) = market.clock_frame()
        && !send_text(&mut socket, frame).await
    {
        return;
    }
    // Subscribed before the snapshot is taken, for the same reason as the market feed above.
    let settings = state.settings().clone();
    let mut settings_feed = settings.subscribe();
    if !send_text(&mut socket, settings.snapshot_frame().await).await {
        return;
    }
    let mut subs: Vec<Sub> = Vec::new();
    let mut quotes: Option<QuotesSub> = None;

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
                        OutKind::Clock(frame) | OutKind::Paper(frame) => {
                            send_text(&mut socket, frame.clone()).await
                        }
                        // Unsubscribed connections drop the frame here without touching it.
                        OutKind::Quotes(frame) => match quotes.as_ref() {
                            Some(q) if o.seq > q.seq => send_text(&mut socket, frame.clone()).await,
                            _ => true,
                        },
                        OutKind::Resync => {
                            (match quotes.as_mut() {
                                Some(q) => send_quotes_snapshot(&mut socket, &market, q).await,
                                None => true,
                            }) && send_snapshots(&mut socket, &market, &mut subs).await
                        }
                        // Live: a symbol's archived history just went into the engine; a slot
                        // showing it had an empty chart (crate::history).
                        OutKind::Loaded(symbol) => {
                            send_snapshots_of(&mut socket, &market, &mut subs, symbol).await
                        }
                        // Most frames are for symbols no slot shows: dropped here untouched.
                        OutKind::Chart { symbol, .. } if !subs.iter().any(|s| s.symbol == *symbol) => true,
                        // The stock traded out of the price band: it is no longer a candidate,
                        // so every slot showing it stops here and each is told why.
                        OutKind::Chart { symbol, .. } if !market.is_candidate(symbol) => {
                            let gone: Vec<String> = subs.iter().filter(|s| s.symbol == *symbol).map(|s| s.id.clone()).collect();
                            subs.retain(|s| s.symbol != *symbol);
                            let mut ok = true;
                            for id in gone {
                                let frame = chart_error(
                                    "outside_price_band",
                                    "no longer a candidate: price left the configured band",
                                    symbol,
                                    &id,
                                );
                                if !send_text(&mut socket, frame).await {
                                    ok = false;
                                    break;
                                }
                            }
                            ok
                        }
                        // A tape, halt or LULD row: once per symbol, whatever the slots' timeframes.
                        OutKind::Chart { symbol, timeframe: None, json, .. } => {
                            if subs.iter().any(|s| s.symbol == *symbol && o.seq > s.seq) {
                                send_text(&mut socket, json.clone()).await
                            } else {
                                true
                            }
                        }
                        // A candle frame: once per distinct bin among the slots on this chart
                        // that have not already got it in their snapshot.
                        OutKind::Chart { symbol, timeframe: Some(tf), update, json } => {
                            let mut bins: Vec<Price> = Vec::new();
                            for s in subs.iter().filter(|s| s.symbol == *symbol && s.timeframe == *tf && o.seq > s.seq) {
                                if !bins.contains(&s.bin) {
                                    bins.push(s.bin);
                                }
                            }
                            let mut ok = true;
                            for bin in bins {
                                if !send_text(&mut socket, market.rebin(update, json, bin)).await {
                                    ok = false;
                                    break;
                                }
                            }
                            ok
                        }
                    },
                    Err(RecvError::Lagged(n)) => {
                        tracing::debug!(skipped = n, "ws client lagged; resyncing with a snapshot");
                        send_text(&mut socket, market.paper_state_frame()).await
                            && match quotes.as_mut() {
                                Some(q) => send_quotes_snapshot(&mut socket, &market, q).await,
                                None => true,
                            }
                            && send_snapshots(&mut socket, &market, &mut subs).await
                    }
                    Err(RecvError::Closed) => true,
                };
                if !ok {
                    break;
                }
            }
            changed = settings_feed.recv() => {
                let ok = match changed {
                    Ok(frame) => send_text(&mut socket, frame.to_string()).await,
                    Err(RecvError::Lagged(n)) => {
                        tracing::debug!(skipped = n, "ws client lagged on settings; resending the snapshot");
                        send_text(&mut socket, settings.snapshot_frame().await).await
                    }
                    // The store (and its sender) lives as long as the AppState; if it ever went,
                    // recv would return at once forever, so end the session rather than spin.
                    Err(RecvError::Closed) => false,
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
                    Ok(ClientMessage::ChartSubscribe { symbol, timeframe, profile_bin, id }) => {
                        let id = id.unwrap_or_default();
                        let held = subs.iter().position(|s| s.id == id);
                        if id.chars().count() > MAX_CHART_ID {
                            Some(ServerMessage::Error {
                                code: "bad_request",
                                message: "chart id too long",
                            })
                        } else if held.is_none() && subs.len() >= MAX_CHARTS {
                            Some(ServerMessage::Error {
                                code: "too_many_charts",
                                message: "at most 4 charts per connection; unsubscribe one first",
                            })
                        } else {
                            match api::parse_bin(profile_bin, market.default_bin()) {
                                Ok(bin) => {
                                    let mut s = Sub {
                                        id,
                                        symbol: symbol.trim().to_ascii_uppercase(),
                                        timeframe,
                                        bin,
                                        seq: 0,
                                    };
                                    if !send_snapshot(&mut socket, &market, &mut s).await {
                                        break;
                                    }
                                    // The client replaced its copy of this chart with the new
                                    // snapshot, so every slot sharing it is current to its seq.
                                    for other in subs.iter_mut().filter(|o| o.same_chart(&s)) {
                                        other.seq = s.seq;
                                    }
                                    match held {
                                        Some(i) => subs[i] = s,
                                        None => subs.push(s),
                                    }
                                    None
                                }
                                Err(_) => Some(ServerMessage::Error {
                                    code: "bad_request",
                                    message: "invalid profileBin",
                                }),
                            }
                        }
                    }
                    Ok(ClientMessage::ChartUnsubscribe { id }) => {
                        let id = id.unwrap_or_default();
                        subs.retain(|s| s.id != id);
                        None
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
                            Err(ControlError::UnknownDay(d)) => {
                                tracing::debug!(day = %d, "replay.control day: not in the archive");
                                Some(ServerMessage::Error {
                                    code: "unknown_day",
                                    message: "that day is not in the replay archive",
                                })
                            }
                            Err(ControlError::Rejected(m)) => {
                                tracing::warn!(error = %m, "replay.control refused by the driver");
                                Some(ServerMessage::Error {
                                    code: "day_unavailable",
                                    message: "that day could not be opened; the replay did not change",
                                })
                            }
                            Err(ControlError::Failed(_)) => Some(ServerMessage::Error {
                                code: "replay_unavailable",
                                message: "replay is not running",
                            }),
                        }
                    }
                    Ok(ClientMessage::OrderPlace(cmd)) => {
                        market.paper_place(cmd);
                        None
                    }
                    Ok(ClientMessage::OrderModify(cmd)) => match market.paper_modify(cmd) {
                        Ok(()) => None,
                        Err(e) => Some(ServerMessage::Error { code: e.code(), message: e.message() }),
                    },
                    Ok(ClientMessage::OrderCancel(cmd)) => match market.paper_cancel(&cmd.id) {
                        Ok(()) => None,
                        Err(e) => Some(ServerMessage::Error { code: e.code(), message: e.message() }),
                    },
                    Ok(ClientMessage::PaperReset) => {
                        market.paper_reset();
                        None
                    }
                    Ok(ClientMessage::QuotesSubscribe) => {
                        let q = quotes.get_or_insert_with(|| QuotesSub {
                            _guard: market.quotes_subscribe(),
                            seq: 0,
                        });
                        if !send_quotes_snapshot(&mut socket, &market, q).await {
                            break;
                        }
                        None
                    }
                    Ok(ClientMessage::QuotesUnsubscribe) => {
                        quotes = None;
                        None
                    }
                    Ok(ClientMessage::SettingsSet { ns, value, op_id, if_absent }) => {
                        let op = SettingsOp::Set { value, if_absent };
                        if !apply_setting(&mut socket, &settings, &ns, &op_id, op).await {
                            break;
                        }
                        None
                    }
                    Ok(ClientMessage::SettingsPatch { ns, set, remove, op_id }) => {
                        let op = SettingsOp::Patch { set, remove };
                        if !apply_setting(&mut socket, &settings, &ns, &op_id, op).await {
                            break;
                        }
                        None
                    }
                    Err(_) if let Some((ns, op_id)) = settings_frame_ids(&text) => {
                        let frame = settings_rejected(&ns, &op_id, "bad_request", "malformed settings message");
                        if !send_text(&mut socket, frame).await {
                            break;
                        }
                        None
                    }
                    Err(_) if is_paper_command(&text) => Some(ServerMessage::Error {
                        code: "bad_order",
                        message: "malformed or unknown order command",
                    }),
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
