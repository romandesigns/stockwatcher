//! Exercises the real auth/router/Market boundary, without a live provider or paid request.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use serde_json::Value;
use serde_json::json;
use stockwatcher_server::{AppState, MarketConfig, router};
use tokio_tungstenite::tungstenite::Message;
use tower::ServiceExt;

#[tokio::test]
async fn detection_default_off_and_explicit_source_routes_are_bounded_replacements() {
    assert!(!MarketConfig::default().detection_panel);
    let on = MarketConfig::from_lookup(|k| (k == "DETECTION_PANEL").then(|| "1".into())).unwrap();
    assert!(on.detection_panel);
    assert!(!on.stock_context);
    let dir = tempfile::tempdir().unwrap();
    let app = router(AppState::new(None), dir.path(), dir.path());
    for uri in [
        "/api/detection?source=live",
        "/api/detection?source=replay",
        "/api/detection/export?source=replay",
    ] {
        let response = app
            .clone()
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let frame: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(frame["type"], "detection.snapshot");
        assert_eq!(frame["enabled"], false);
        assert_eq!(frame["alertsEnabled"], false);
        assert_eq!(frame["replacement"], true);
        assert_eq!(frame["transitions"], serde_json::json!([]));
    }
}

#[tokio::test]
async fn socket_opt_in_returns_current_disabled_state_on_every_reconnect_without_alerts() {
    let dir = tempfile::tempdir().unwrap();
    let app = router(AppState::new(None), dir.path(), dir.path());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    for _ in 0..2 {
        let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
            .await
            .unwrap();
        socket
            .send(Message::Text(
                json!({"type":"hello","client":"test","source":"replay"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        socket
            .send(Message::Text(
                json!({"type":"detection.subscribe"}).to_string().into(),
            ))
            .await
            .unwrap();
        let frame = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let Message::Text(text) = socket.next().await.unwrap().unwrap() else {
                    continue;
                };
                let frame: Value = serde_json::from_str(&text).unwrap();
                if frame["type"] == "detection.snapshot" {
                    break frame;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(frame["enabled"], false);
        assert_eq!(frame["alertsEnabled"], false);
        assert_eq!(frame["cursor"], 0);
        assert_eq!(frame["transitions"], json!([]));
        socket
            .send(Message::Text(
                json!({"type":"detection.unsubscribe"}).to_string().into(),
            ))
            .await
            .unwrap();
        socket.close(None).await.unwrap();
    }
    server.abort();
}
