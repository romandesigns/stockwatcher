//! Rocket watch on the wire (`rocket.subscribe` / `rocket.snapshot` / `rocket.update`), driven by
//! a fixture replay. The board itself (features, policy, outcomes, forward log) is tested in
//! crates/server/src/rocket/tests.rs.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use stockwatcher_server::{AppState, MarketConfig, Markets, Settings, router};
use tokio_tungstenite::tungstenite::Message;

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Stack {
    _tmp: tempfile::TempDir,
    url: String,
}

async fn stack(extra: &[(&str, &str)]) -> Stack {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("archive").to_string_lossy().into_owned();
    let mut vars: Vec<(String, String)> = vec![
        ("ARCHIVE_ROOT".into(), root),
        ("ARCHIVE_MIN_FREE_MB".into(), "0".into()),
        ("SIM_AUTOFIXTURE".into(), "1".into()),
        ("SIM_FIXTURE_SYMBOLS".into(), "SMPL,FXLO".into()),
        ("REPLAY_START".into(), "10:40".into()),
        ("REPLAY_SPEED".into(), "60".into()),
    ];
    vars.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    let cfg = MarketConfig::from_lookup(|k| {
        vars.iter()
            .rev()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.clone())
    })
    .unwrap();
    let (markets, _sim) = Markets::start(&cfg);
    let dir = tmp.path().join("static");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("index.html"), "<!doctype html>").unwrap();
    let app = router(
        AppState::with_markets(None, markets, Settings::in_memory()),
        &dir,
        &tmp.path().join("data"),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Stack {
        _tmp: tmp,
        url: format!("ws://{addr}/ws"),
    }
}

async fn connect(url: &str) -> Ws {
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    send(&mut ws, json!({"type": "hello", "client": "test"})).await;
    next(&mut ws, |m| m["type"] == "welcome").await;
    ws
}

async fn send(ws: &mut Ws, v: Value) {
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}

async fn next(ws: &mut Ws, pred: impl Fn(&Value) -> bool) -> Value {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            match ws.next().await.unwrap().unwrap() {
                Message::Text(t) => {
                    let v: Value = serde_json::from_str(&t).unwrap();
                    if pred(&v) {
                        return v;
                    }
                }
                _ => continue,
            }
        }
    })
    .await
    .expect("timed out waiting for a frame")
}

async fn types_for(ws: &mut Ws, wait: Duration) -> Vec<String> {
    let mut seen = Vec::new();
    let _ = tokio::time::timeout(wait, async {
        while let Some(Ok(m)) = ws.next().await {
            if let Message::Text(t) = m {
                let v: Value = serde_json::from_str(&t).unwrap();
                seen.push(v["type"].as_str().unwrap_or_default().to_string());
            }
        }
    })
    .await;
    seen
}

fn is(t: &'static str) -> impl Fn(&Value) -> bool {
    move |m| m["type"] == t
}

/// On by default: a snapshot at once with the strategy and the track record, then whole-panel
/// updates while the replay plays; unsubscribed connections never see a rocket frame, and
/// unsubscribing stops them.
#[tokio::test]
async fn rocket_subscribe_gets_a_snapshot_then_whole_panel_updates() {
    let s = stack(&[]).await;
    let mut ws = connect(&s.url).await;
    let mut bystander = connect(&s.url).await;
    send(&mut ws, json!({"type": "rocket.subscribe"})).await;
    let snap = next(&mut ws, is("rocket.snapshot")).await;
    assert_eq!(snap["enabled"], true, "{snap}");
    // The production scorer since 2026-10-22: the bake-off's frozen precursors-R0, with its
    // frozen threshold (fixed mode).
    assert_eq!(snap["strategy"]["id"], "precursors-R0");
    assert_eq!(snap["strategy"]["placeholder"], false);
    assert_eq!(snap["mode"], "fixed");
    assert_eq!(snap["budget"], 10);
    assert!(snap["rows"].is_array());
    assert!(snap["days"].is_array());
    assert!(snap["today"]["alerts"].is_u64());

    let up = next(&mut ws, is("rocket.update")).await;
    assert_eq!(up["enabled"], true);
    assert!(up["t"].is_i64());
    assert!(up["rows"].is_array());

    let seen = types_for(&mut bystander, Duration::from_millis(1500)).await;
    assert!(!seen.iter().any(|t| t.starts_with("rocket.")), "{seen:?}");

    send(&mut ws, json!({"type": "rocket.unsubscribe"})).await;
    send(&mut ws, json!({"type": "ping", "ts": 7})).await;
    next(&mut ws, |m| m["type"] == "pong" && m["ts"] == 7).await;
    let seen = types_for(&mut ws, Duration::from_millis(2500)).await;
    assert!(!seen.iter().any(|t| t.starts_with("rocket.")), "{seen:?}");
}

/// `ROCKET_WATCH=0`: the snapshot says "not enabled", never an empty panel.
#[tokio::test]
async fn rocket_watch_off_answers_not_enabled() {
    let s = stack(&[("ROCKET_WATCH", "0")]).await;
    let mut ws = connect(&s.url).await;
    send(&mut ws, json!({"type": "rocket.subscribe"})).await;
    let snap = next(&mut ws, is("rocket.snapshot")).await;
    assert_eq!(snap["enabled"], false, "{snap}");
    assert_eq!(snap["rows"], json!([]));
    let seen = types_for(&mut ws, Duration::from_millis(2500)).await;
    assert!(!seen.iter().any(|t| t == "rocket.update"), "{seen:?}");
}
