pub mod admit;
pub mod alerts;
pub mod api;
pub mod auth;
pub mod config;
pub mod context;
pub mod days;
pub mod detection;
pub mod evidence;
pub mod history;
pub mod market;
pub mod observe;
pub mod pressure;
pub mod quotes;
pub mod settings;
pub mod shadow;
pub mod ws;

use std::path::Path;
use std::sync::Arc;

use axum::extract::Request;
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;
use tokio::sync::watch;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

pub use config::Config;
pub use market::{Market, MarketConfig, Markets, Source};
pub use settings::Settings;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Set by build.rs from the GIT_SHA build env ("unknown" for local builds).
pub const COMMIT: &str = env!("STOCKWATCHER_COMMIT");

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

struct Inner {
    api_token: Option<String>,
    markets: Markets,
    /// Synced settings and drawings (settings.rs). Independent of the market: a seek or a day
    /// switch rebuilds the market, never this.
    settings: Settings,
    /// Price alerts on the live market (crate::alerts); idle in tests that do not start them.
    alerts: alerts::Alerts,
    /// Only WebSocket sessions subscribe, so `closed()` resolving means
    /// every session has finished.
    shutdown: watch::Sender<bool>,
}

impl AppState {
    /// No market data (an idle market reports `idle`). Used by tests that don't need it.
    pub fn new(api_token: Option<String>) -> Self {
        Self::with_market(
            api_token,
            Market::idle(market_providers::ClockMode::Replay, "idle", None),
        )
    }

    /// A memory-only settings store: nothing written survives the process (tests).
    pub fn with_market(api_token: Option<String>, market: Market) -> Self {
        Self::with_parts(api_token, market, Settings::in_memory())
    }

    /// One market, in the slot its mode says ([`Markets::single`]); tests.
    pub fn with_parts(api_token: Option<String>, market: Market, settings: Settings) -> Self {
        Self::with_markets(api_token, Markets::single(market), settings)
    }

    /// The production constructor: main.rs starts both markets and opens the store under
    /// `DATA_DIR/settings`.
    pub fn with_markets(api_token: Option<String>, markets: Markets, settings: Settings) -> Self {
        Self::with_alerts(api_token, markets, settings, alerts::Alerts::idle())
    }

    /// With the alerts service (main.rs starts it on the live market and the same store).
    pub fn with_alerts(
        api_token: Option<String>,
        markets: Markets,
        settings: Settings,
        alerts: alerts::Alerts,
    ) -> Self {
        Self(Arc::new(Inner {
            api_token,
            markets,
            settings,
            alerts,
            shutdown: watch::Sender::new(false),
        }))
    }

    pub fn alerts(&self) -> &alerts::Alerts {
        &self.0.alerts
    }

    /// The default market (`MARKET_DATA_MODE`).
    pub fn market(&self) -> &Market {
        self.0.markets.get(None)
    }

    /// The market a client asked for (`hello.source`, `?source=`), else the default.
    pub fn market_for(&self, source: Option<Source>) -> &Market {
        self.0.markets.get(source)
    }

    pub fn markets(&self) -> &Markets {
        &self.0.markets
    }

    pub fn settings(&self) -> &Settings {
        &self.0.settings
    }

    pub fn api_token(&self) -> Option<&str> {
        self.0.api_token.as_deref()
    }

    pub fn shutdown_signal(&self) -> watch::Receiver<bool> {
        self.0.shutdown.subscribe()
    }

    /// Tells every WebSocket session to close, then waits (bounded) until
    /// they have. Upgraded connections aren't covered by axum's graceful
    /// shutdown, so without this they'd be cut mid-frame on exit.
    pub async fn close_sessions(&self, grace: std::time::Duration) {
        self.0.shutdown.send_replace(true);
        let _ = tokio::time::timeout(grace, self.0.shutdown.closed()).await;
    }

    /// Writes out the live market's recording (crate::history): the last step of a clean
    /// shutdown, so a deploy loses none of the day's recorded events.
    pub async fn close_recording(&self, grace: std::time::Duration) {
        self.0.markets.live.close_recording(grace).await;
    }
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok", "version": VERSION, "commit": COMMIT }))
}

async fn version() -> Json<serde_json::Value> {
    Json(json!({ "version": VERSION, "commit": COMMIT }))
}

async fn api_not_found() -> Response {
    (StatusCode::NOT_FOUND, Json(json!({ "error": "not_found" }))).into_response()
}

/// Hashed Vite assets are immutable; everything else (index.html, API) must
/// revalidate so a deploy is picked up on the next load. A missing
/// /assets/* file falls back to index.html, which must NOT be cached as
/// immutable under that URL, hence the content-type check.
async fn cache_headers(req: Request, next: Next) -> Response {
    let hashed_asset = req.uri().path().starts_with("/assets/");
    let mut res = next.run(req).await;
    let is_html = res
        .headers()
        .get(header::CONTENT_TYPE)
        .is_some_and(|ct| ct.as_bytes().starts_with(b"text/html"));
    let value = if hashed_asset && !is_html && res.status().is_success() {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    res.headers_mut()
        .entry(header::CACHE_CONTROL)
        .or_insert(HeaderValue::from_static(value));
    res
}

/// Content types for desktop update artifacts, set explicitly rather than
/// trusting mime_guess, which varies across versions for .msi and .sig.
async fn update_content_type(req: Request, next: Next) -> Response {
    let ext = req
        .uri()
        .path()
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase());
    let mut res = next.run(req).await;
    let ct = match ext.as_deref() {
        Some("json") => "application/json",
        Some("exe") => "application/vnd.microsoft.portable-executable",
        Some("msi") => "application/x-msi",
        Some("sig") => "text/plain; charset=utf-8",
        _ => return res,
    };
    if res.status().is_success() {
        res.headers_mut()
            .insert(header::CONTENT_TYPE, HeaderValue::from_static(ct));
    }
    res
}

/// `static_dir` holds the built web app; `data_dir` is the persistent
/// volume, whose `updates/` subtree publish-updates.sh fills.
pub fn router(state: AppState, static_dir: &Path, data_dir: &Path) -> Router {
    let protected = Router::new()
        .route("/version", get(version))
        .route("/chart/{symbol}/snapshot", get(api::chart_snapshot))
        .route("/replay/state", get(api::replay_state))
        .route("/detection", get(api::detection_snapshot))
        .route("/detection/export", get(api::detection_snapshot))
        .route("/symbols", get(api::symbols))
        .route("/quotes", get(api::quotes))
        .route("/settings", get(api::settings))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_bearer,
        ));
    let api = Router::new()
        .route("/health", get(health))
        .merge(protected)
        .fallback(api_not_found);

    // ServeDir rejects `..` and absolute-path components itself, so a
    // traversal attempt never leaves static_dir; it just gets the SPA shell.
    let spa = ServeDir::new(static_dir).fallback(ServeFile::new(static_dir.join("index.html")));

    // Public and read-only on purpose: the Tauri updater can't send the
    // access token. ServeDir only answers GET/HEAD, never lists a
    // directory, and a missing file is a plain 404 (not the SPA shell, which
    // the updater would try to parse as a manifest).
    let updates = Router::new()
        .fallback_service(
            ServeDir::new(data_dir.join("updates")).append_index_html_on_directories(false),
        )
        .layer(middleware::from_fn(update_content_type));

    Router::new()
        .nest("/api", api)
        .nest("/updates", updates)
        // nest() doesn't match the bare trailing slash; keep it off the SPA.
        .route("/updates/", get(api_not_found))
        .route("/ws", get(ws::upgrade))
        .fallback_service(spa)
        .layer(middleware::from_fn(cache_headers))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
