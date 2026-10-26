use std::path::Path;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use stockwatcher_server::{AppState, router};
use tokio_tungstenite::tungstenite::Message;
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789abcdef0123456789";
const INDEX: &str = "<!doctype html><title>stockwatcher test shell</title>";

/// static/{index.html, assets/app.js} plus a secret file OUTSIDE static/.
fn site() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("static/assets")).unwrap();
    std::fs::write(root.join("static/index.html"), INDEX).unwrap();
    std::fs::write(root.join("static/assets/app.js"), "console.log(1)").unwrap();
    std::fs::write(root.join("secret.txt"), "TOP-SECRET").unwrap();
    std::fs::create_dir_all(root.join("data/updates/desktop")).unwrap();
    std::fs::write(
        root.join("data/updates/desktop/latest.json"),
        r#"{"version":"0.1.7"}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("data/updates/desktop/stockwatcher_0.1.7_x64-setup.exe"),
        "MZ",
    )
    .unwrap();
    std::fs::write(
        root.join("data/updates/desktop/stockwatcher_0.1.7_x64_en-US.msi"),
        "MSI",
    )
    .unwrap();
    std::fs::write(
        root.join("data/updates/desktop/stockwatcher_0.1.7_x64-setup.exe.sig"),
        "SIG",
    )
    .unwrap();
    std::fs::write(root.join("data/db-secret.txt"), "DATA-SECRET").unwrap();
    dir
}

fn app(root: &Path, token: Option<&str>) -> axum::Router {
    router(
        AppState::new(token.map(str::to_owned)),
        &root.join("static"),
        &root.join("data"),
    )
}

async fn get(
    app: axum::Router,
    uri: &str,
    bearer: Option<&str>,
) -> (StatusCode, String, Option<String>) {
    let mut req = Request::get(uri);
    if let Some(t) = bearer {
        req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let res = app.oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
    let status = res.status();
    let cache = res
        .headers()
        .get(header::CACHE_CONTROL)
        .map(|v| v.to_str().unwrap().to_owned());
    let body = res.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned(), cache)
}

#[tokio::test]
async fn health_is_public_and_reports_build() {
    let dir = site();
    let (status, body, cache) = get(app(dir.path(), Some(TOKEN)), "/api/health", None).await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["status"], "ok");
    assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
    assert!(v["commit"].is_string());
    assert_eq!(cache.as_deref(), Some("no-cache"));
}

#[tokio::test]
async fn auth_off_allows_api() {
    let dir = site();
    let (status, body, _) = get(app(dir.path(), None), "/api/version", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"version\""));
}

#[tokio::test]
async fn auth_on_requires_matching_bearer() {
    let dir = site();
    let a = app(dir.path(), Some(TOKEN));
    assert_eq!(
        get(a.clone(), "/api/version", None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        get(
            a.clone(),
            "/api/version",
            Some("wrong-token-0123456789abcdef012345")
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(get(a, "/api/version", Some(TOKEN)).await.0, StatusCode::OK);
}

/// The daily scorecards: behind the token like every other read, and an empty list (never an
/// error) before the maintainer has scored a night.
#[tokio::test]
async fn scorecard_needs_the_token_and_is_empty_before_any_night() {
    let dir = site();
    let a = app(dir.path(), Some(TOKEN));
    assert_eq!(
        get(a.clone(), "/api/scorecard?days=30", None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (status, body, _) = get(a, "/api/scorecard?days=9999", Some(TOKEN)).await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["days"], json!([]));
}

/// The installed desktop app's page is `http://tauri.localhost`, so its API calls are cross-origin:
/// the preflight (no token) must be answered for it, and only for it.
#[tokio::test]
async fn api_allows_the_desktop_origin_only() {
    let dir = site();
    let a = app(dir.path(), Some(TOKEN));
    let preflight = |origin: &'static str| {
        Request::builder()
            .method("OPTIONS")
            .uri("/api/symbols")
            .header(header::ORIGIN, origin)
            .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
            .header(header::ACCESS_CONTROL_REQUEST_HEADERS, "authorization")
            .body(Body::empty())
            .unwrap()
    };
    let res = a
        .clone()
        .oneshot(preflight("http://tauri.localhost"))
        .await
        .unwrap();
    assert!(res.status().is_success());
    assert_eq!(
        res.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
        "http://tauri.localhost"
    );
    let res = a
        .clone()
        .oneshot(preflight("https://evil.example"))
        .await
        .unwrap();
    assert!(
        !res.headers()
            .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN)
    );

    let res = a
        .oneshot(
            Request::get("/api/version")
                .header(header::ORIGIN, "http://tauri.localhost")
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
        "http://tauri.localhost"
    );
}

#[tokio::test]
async fn unknown_api_route_is_json_404_not_the_spa() {
    let dir = site();
    let (status, body, _) = get(app(dir.path(), None), "/api/nope", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("not_found"));
}

#[tokio::test]
async fn spa_fallback_and_cache_headers() {
    let dir = site();
    let a = app(dir.path(), None);

    let (status, body, cache) = get(a.clone(), "/watchlist/deep/link", None).await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, INDEX));
    assert_eq!(cache.as_deref(), Some("no-cache"));

    let (status, body, cache) = get(a.clone(), "/assets/app.js", None).await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "console.log(1)"));
    assert_eq!(
        cache.as_deref(),
        Some("public, max-age=31536000, immutable")
    );

    // A stale chunk URL gets the shell, and must not be cached as immutable.
    let (_, body, cache) = get(a, "/assets/gone-1234.js", None).await;
    assert_eq!(body, INDEX);
    assert_eq!(cache.as_deref(), Some("no-cache"));
}

#[tokio::test]
async fn path_traversal_never_escapes_static_dir() {
    let dir = site();
    for uri in [
        "/../secret.txt",
        "/%2e%2e/secret.txt",
        "/assets/..%2f..%2fsecret.txt",
        "/assets/%2e%2e/%2e%2e/secret.txt",
        "/..%5csecret.txt",
    ] {
        let (_, body, _) = get(app(dir.path(), None), uri, None).await;
        assert!(
            !body.contains("TOP-SECRET"),
            "{uri} leaked a file outside static_dir"
        );
    }
}

// --- WebSocket: real server on an ephemeral port ---------------------------

async fn spawn(token: Option<&str>) -> (String, tempfile::TempDir) {
    let dir = site();
    let a = app(dir.path(), token);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, a).await.unwrap() });
    (format!("ws://{addr}/ws"), dir)
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn send(ws: &mut Ws, v: Value) {
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}

async fn recv(ws: &mut Ws) -> Value {
    loop {
        match ws.next().await.expect("stream ended").unwrap() {
            Message::Text(t) => return serde_json::from_str(&t).unwrap(),
            Message::Close(_) => panic!("closed while waiting for a message"),
            _ => {}
        }
    }
}

#[tokio::test]
async fn ws_handshake_with_token_then_ping_pong() {
    let (url, _dir) = spawn(Some(TOKEN)).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    send(
        &mut ws,
        json!({ "type": "hello", "token": TOKEN, "client": "test" }),
    )
    .await;
    let welcome = recv(&mut ws).await;
    assert_eq!(welcome["type"], "welcome");
    assert_eq!(welcome["heartbeatSecs"], 15);
    // The simulated account follows immediately (docs/architecture/chart-trading-sim.md).
    let paper = recv(&mut ws).await;
    assert_eq!(paper["type"], "paper.state");
    assert_eq!(paper["simulated"], true);
    assert_eq!(paper["orders"], json!([]));
    // Then the synced settings (no clock on an idle market; tests/settings.rs covers the rest).
    let settings = recv(&mut ws).await;
    assert_eq!(settings, json!({ "type": "settings.snapshot", "docs": [] }));
    // Then the alerts' Activity (crate::alerts; empty, with the default phone budget).
    let alerts = recv(&mut ws).await;
    assert_eq!(alerts["type"], "alerts.history");
    assert_eq!(alerts["events"], json!([]));
    assert_eq!(alerts["phoneBudget"], 20);

    send(&mut ws, json!({ "type": "ping", "ts": 42 })).await;
    assert_eq!(recv(&mut ws).await, json!({ "type": "pong", "ts": 42 }));
}

/// `runners.notify` (desktop and web Runners notifications) is accepted without a reply, in
/// its full and its minimal form: the next answer is the pong, not an error.
#[tokio::test]
async fn ws_accepts_runners_notify() {
    let (url, _dir) = spawn(None).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    send(&mut ws, json!({ "type": "hello" })).await;
    for want in [
        "welcome",
        "paper.state",
        "settings.snapshot",
        "alerts.history",
    ] {
        assert_eq!(recv(&mut ws).await["type"], want);
    }
    send(&mut ws, json!({ "type": "runners.notify", "new": true, "breakout": false, "burst": true, "running": true, "run": false, "fast": true })).await;
    send(&mut ws, json!({ "type": "runners.notify" })).await;
    send(
        &mut ws,
        json!({ "type": "runners.notify", "enabled": false }),
    )
    .await;
    send(&mut ws, json!({ "type": "ping", "ts": 7 })).await;
    assert_eq!(recv(&mut ws).await, json!({ "type": "pong", "ts": 7 }));
}

#[tokio::test]
async fn ws_rejects_bad_token() {
    let (url, _dir) = spawn(Some(TOKEN)).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    send(&mut ws, json!({ "type": "hello", "token": "nope" })).await;
    let err = recv(&mut ws).await;
    assert_eq!(err["type"], "error");
    assert_eq!(err["code"], "unauthorized");
    match ws.next().await {
        Some(Ok(Message::Close(Some(frame)))) => assert_eq!(u16::from(frame.code), 4401),
        other => panic!("expected close frame, got {other:?}"),
    }
}

#[tokio::test]
async fn ws_without_auth_needs_no_token() {
    let (url, _dir) = spawn(None).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    send(&mut ws, json!({ "type": "hello" })).await;
    assert_eq!(recv(&mut ws).await["type"], "welcome");
}

#[tokio::test]
async fn ws_first_message_must_be_hello() {
    let (url, _dir) = spawn(None).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    send(&mut ws, json!({ "type": "ping", "ts": 1 })).await;
    let err = recv(&mut ws).await;
    assert_eq!(err["code"], "bad_request");
}

#[test]
fn short_token_is_refused_without_echoing_it() {
    let err = stockwatcher_server::Config::from_lookup(|k| {
        (k == "STOCKWATCHER_API_TOKEN").then(|| "short-secret".to_owned())
    })
    .unwrap_err();
    assert!(!err.contains("short-secret"));
    let ok = stockwatcher_server::Config::from_lookup(|_| None).unwrap();
    assert!(ok.api_token.is_none());
    assert_eq!(ok.port, 8790);
}

async fn get_full(app: axum::Router, uri: &str) -> (StatusCode, String, String, Option<String>) {
    let res = app
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let header_str = |h| {
        res.headers()
            .get(h)
            .map(|v: &axum::http::HeaderValue| v.to_str().unwrap().to_owned())
    };
    let ct = header_str(header::CONTENT_TYPE).unwrap_or_default();
    let cache = header_str(header::CACHE_CONTROL);
    let body = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        ct,
        String::from_utf8_lossy(&body).into_owned(),
        cache,
    )
}

#[tokio::test]
async fn updates_are_public_typed_and_uncached() {
    let dir = site();
    // Token set: /updates must still work without one (the updater can't send it).
    let a = app(dir.path(), Some(TOKEN));
    let cases = [
        ("latest.json", "application/json"),
        (
            "stockwatcher_0.1.7_x64-setup.exe",
            "application/vnd.microsoft.portable-executable",
        ),
        ("stockwatcher_0.1.7_x64_en-US.msi", "application/x-msi"),
        (
            "stockwatcher_0.1.7_x64-setup.exe.sig",
            "text/plain; charset=utf-8",
        ),
    ];
    for (file, want) in cases {
        let (status, ct, _, cache) = get_full(a.clone(), &format!("/updates/desktop/{file}")).await;
        assert_eq!(status, StatusCode::OK, "{file}");
        assert_eq!(ct, want, "{file}");
        assert_eq!(cache.as_deref(), Some("no-cache"), "{file}");
    }
    let (_, _, body, _) = get_full(a, "/updates/desktop/latest.json").await;
    assert_eq!(body, r#"{"version":"0.1.7"}"#);
}

#[tokio::test]
async fn updates_never_list_fall_back_or_escape() {
    let dir = site();
    let a = app(dir.path(), None);
    for uri in [
        "/updates",
        "/updates/",
        "/updates/desktop",
        "/updates/desktop/",
        "/updates/desktop/missing.json",
    ] {
        let (status, _, body, _) = get_full(a.clone(), uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        assert!(
            !body.contains("latest.json") && !body.contains("stockwatcher test shell"),
            "{uri}"
        );
    }
    for uri in [
        "/updates/../db-secret.txt",
        "/updates/%2e%2e/db-secret.txt",
        "/updates/desktop/..%2f..%2fdb-secret.txt",
        "/updates/desktop/%2e%2e/%2e%2e/secret.txt",
    ] {
        let (_, _, body, _) = get_full(a.clone(), uri).await;
        assert!(
            !body.contains("SECRET"),
            "{uri} leaked a file outside updates/"
        );
    }
    // Read-only: anything but GET/HEAD is refused.
    let res = a
        .oneshot(
            Request::put("/updates/desktop/latest.json")
                .body(Body::from("x"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED);
}

/// One server per DATA_DIR: while one holds the lock a second is refused, and once it is gone
/// (the process exited) the next one starts.
#[test]
fn a_second_server_cannot_take_the_data_dir() {
    let dir = tempfile::tempdir().unwrap();
    let first = stockwatcher_server::lock_data_dir(dir.path()).unwrap();
    let err = stockwatcher_server::lock_data_dir(dir.path()).unwrap_err();
    assert!(
        err.to_string().contains("another stockwatcher server"),
        "{err}"
    );
    drop(first);
    assert!(stockwatcher_server::lock_data_dir(dir.path()).is_ok());
}
