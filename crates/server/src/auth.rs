use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use subtle::ConstantTimeEq;

use crate::AppState;

/// Constant-time comparison, so response timing can't be used to guess the
/// token byte by byte. (A length mismatch returns early; that reveals only
/// the length, which the ≥32-char rule already makes useless to know.)
pub fn token_matches(expected: &str, provided: &str) -> bool {
    expected.as_bytes().ct_eq(provided.as_bytes()).into()
}

/// Guards `/api/*` routes other than `/api/health`. Pass-through when the
/// server runs without a token.
pub async fn require_bearer(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let Some(expected) = state.api_token() else {
        return next.run(req).await;
    };
    let provided = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    match provided {
        Some(p) if token_matches(expected, p) => next.run(req).await,
        _ => (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            axum::Json(serde_json::json!({ "error": "unauthorized" })),
        )
            .into_response(),
    }
}
