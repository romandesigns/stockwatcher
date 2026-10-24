//! Market REST routes under `/api` (behind the bearer token when one is configured). Each takes
//! `?source=live|replay` (absent = `MARKET_DATA_MODE`): the server runs both markets side by side.
//!
//! - `GET /api/chart/{symbol}/snapshot?timeframe=1m&bin=0.01` → a `chart.snapshot` (with clock)
//! - `GET /api/replay/state` → mode, provider status, day, symbols, clock, and the switchable `days`
//! - `GET /api/replay/alerts?day=YYYY-MM-DD[&symbol=X]` → `{day, alerts: [{t, kind, symbol, price?,
//!   reason?}]}`, the Runners entries the live server logged that day (`crate::replay_alerts`): the
//!   replay bar's alert dots. Always the replay market's archive; an unlogged day is `[]`
//! - `GET /api/symbols` → the symbols the server can chart, and whether the data is fixture data
//! - `GET /api/quotes` → the quote board as a `quotes.snapshot` (the body a `/ws`
//!   `quotes.subscribe` starts with; for tests and debugging, clients use the socket)
//! - `GET /api/settings` → every synced settings document as a `settings.snapshot` (the frame a
//!   `/ws` connection gets after `welcome`; for debugging, clients use the socket)

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chart_engine::Timeframe;
use market_events::Price;
use serde::Deserialize;
use serde_json::json;

use crate::AppState;
use crate::market::{SnapshotError, Source};

#[derive(Deserialize)]
pub struct SnapshotQuery {
    timeframe: Option<String>,
    bin: Option<f64>,
    source: Option<Source>,
}

/// `?source=live|replay` on every market route; absent = the default (`MARKET_DATA_MODE`).
#[derive(Deserialize)]
pub struct SourceQuery {
    source: Option<Source>,
}

fn bad(msg: String) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": "bad_request", "message": msg })),
    )
        .into_response()
}

/// Parses a profile bin width in dollars: positive, at most $100.
pub fn parse_bin(v: Option<f64>, default: Price) -> Result<Price, String> {
    match v {
        None => Ok(default),
        Some(b) => Price::from_f64_rounded(b)
            .filter(|p| p.is_positive() && *p <= Price::from_cents(10_000))
            .ok_or_else(|| format!("invalid profile bin {b}")),
    }
}

pub async fn chart_snapshot(
    State(state): State<AppState>,
    Path(symbol): Path<String>,
    Query(q): Query<SnapshotQuery>,
) -> Response {
    let tf: Timeframe = match q.timeframe.as_deref().unwrap_or("1m").parse() {
        Ok(tf) => tf,
        Err(e) => return bad(e),
    };
    let market = state.market_for(q.source);
    let bin = match parse_bin(q.bin, market.default_bin()) {
        Ok(b) => b,
        Err(e) => return bad(e),
    };
    match market.snapshot(&symbol.to_ascii_uppercase(), tf, bin) {
        Ok(s) => (
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            s.json,
        )
            .into_response(),
        Err(SnapshotError::UnknownSymbol) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "unknown_symbol" })),
        )
            .into_response(),
        Err(SnapshotError::OutsidePriceBand) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "outside_price_band" })),
        )
            .into_response(),
    }
}

pub async fn replay_state(
    State(state): State<AppState>,
    Query(q): Query<SourceQuery>,
) -> Json<serde_json::Value> {
    Json(state.market_for(q.source).state_json())
}

#[derive(Deserialize)]
pub struct ReplayAlertsQuery {
    day: Option<String>,
    symbol: Option<String>,
}

pub async fn replay_alerts(
    State(state): State<AppState>,
    Query(q): Query<ReplayAlertsQuery>,
) -> Response {
    // Parsed into a date before it names a file, so nothing but `YYYY-MM-DD.jsonl` is ever read.
    let Some(day) = q
        .day
        .as_deref()
        .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
    else {
        return bad("day must be YYYY-MM-DD".into());
    };
    let symbol = q.symbol.filter(|s| !s.is_empty());
    let alerts = match state.market_for(Some(Source::Replay)).archive_root() {
        // At most a few MB of small lines, but still file IO: off the async workers.
        Some(root) => tokio::task::spawn_blocking(move || {
            crate::replay_alerts::read_day(&root, day, symbol.as_deref())
        })
        .await
        .unwrap_or_default(),
        None => Vec::new(),
    };
    Json(json!({ "day": day.format("%Y-%m-%d").to_string(), "alerts": alerts })).into_response()
}

/// Same bounded state/history as the opt-in socket. Export is deliberately a research seam,
/// never a claim that a retained ring or unsealed journal is an exhaustive cohort.
pub async fn detection_snapshot(
    State(state): State<AppState>,
    Query(q): Query<SourceQuery>,
) -> Response {
    let (_, _, frame) = state.market_for(q.source).detection_snapshot();
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        frame,
    )
        .into_response()
}

pub async fn symbols(
    State(state): State<AppState>,
    Query(q): Query<SourceQuery>,
) -> Json<serde_json::Value> {
    Json(state.market_for(q.source).symbols_json())
}

pub async fn quotes(State(state): State<AppState>, Query(q): Query<SourceQuery>) -> Response {
    let (_, json) = state.market_for(q.source).quotes_snapshot();
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        json,
    )
        .into_response()
}

pub async fn settings(State(state): State<AppState>) -> Response {
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        state.settings().snapshot_frame().await,
    )
        .into_response()
}
