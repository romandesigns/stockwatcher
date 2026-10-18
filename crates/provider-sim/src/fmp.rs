//! FMP `GET /stable/shares-float?symbol=&apikey=`. FIXTURE VALUES: the simulator has no float
//! data; it returns deterministic synthetic numbers per symbol, and every response carries the
//! header `x-stockwatcher-fixture: true` so nothing can mistake them for FMP data.
//!
//! Shape (FMP "stable" API, as documented for Shares Float; the docs page refused automated
//! fetching on 2026-10-17, so re-check the field list against a real response before relying on
//! it): `[{"symbol","date","freeFloat","floatShares","outstandingShares"}]`.

use crate::Sim;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

#[derive(Deserialize)]
pub struct FloatQuery {
    symbol: Option<String>,
    apikey: Option<String>,
}

fn fixture_header(mut r: Response) -> Response {
    r.headers_mut()
        .insert("x-stockwatcher-fixture", HeaderValue::from_static("true"));
    r
}

pub async fn shares_float(State(sim): State<Sim>, Query(q): Query<FloatQuery>) -> Response {
    let key_ok = q
        .apikey
        .as_deref()
        .is_some_and(|k| bool::from(k.as_bytes().ct_eq(sim.config().fmp_key.as_bytes())));
    if !key_ok {
        return fixture_header(
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({"Error Message": "Invalid API KEY. This is the stockwatcher simulator: use SIM_FMP_KEY."})),
            )
                .into_response(),
        );
    }
    let Some(symbol) = q.symbol.map(|s| s.trim().to_ascii_uppercase()) else {
        return fixture_header(
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"Error Message": "symbol is required"})),
            )
                .into_response(),
        );
    };
    let day = sim.day_info();
    if !day.symbols.contains(&symbol) {
        return fixture_header(Json(json!([])).into_response());
    }
    let h = Sha256::digest(symbol.as_bytes());
    let outstanding: u64 = 20_000_000 + u64::from(h[0]) * 700_000;
    let free_pct = 60.0 + f64::from(h[1] % 35) + f64::from(h[2] % 100) / 100.0;
    let float_shares = (outstanding as f64 * free_pct / 100.0).round() as u64;
    fixture_header(
        Json(json!([{
            "symbol": symbol,
            "date": format!("{} 00:00:00", day.day.format("%Y-%m-%d")),
            "freeFloat": free_pct,
            "floatShares": float_shares,
            "outstandingShares": outstanding,
        }]))
        .into_response(),
    )
}
