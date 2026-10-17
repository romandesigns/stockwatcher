//! Market REST routes under `/api` (behind the bearer token when one is configured):
//!
//! - `GET /api/chart/{symbol}/snapshot?timeframe=1m&bin=0.01` → a `chart.snapshot` (with clock)
//! - `GET /api/replay/state` → mode, provider status, day, symbols, clock
//! - `GET /api/symbols` → the symbols the server can chart, and whether the data is fixture data

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chart_engine::Timeframe;
use market_events::Price;
use serde::Deserialize;
use serde_json::json;

use crate::AppState;
use crate::market::SnapshotError;

#[derive(Deserialize)]
pub struct SnapshotQuery {
    timeframe: Option<String>,
    bin: Option<f64>,
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
    let market = state.market();
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

pub async fn replay_state(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(state.market().state_json())
}

pub async fn symbols(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(state.market().symbols_json())
}
