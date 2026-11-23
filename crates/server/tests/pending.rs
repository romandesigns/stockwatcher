//! The News pending cards on the wire (`pending.subscribe` / `pending.snapshot`). The board and its
//! frames are tested in crates/server/src/pending/tests.rs; this is the channel.

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

/// The welcome lists `pending.v1`; `pending.subscribe` answers with a snapshot at once (a market
/// without the feed: disabled, no cards), and asking again answers again; after
/// `pending.unsubscribe` the connection keeps working and gets no pending frame.
#[tokio::test]
async fn pending_subscribe_gets_a_snapshot() {
    let s = stack(&[]).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(&s.url).await.unwrap();
    send(
        &mut ws,
        json!({"type": "hello", "client": "test", "source": "live"}),
    )
    .await;
    let w = next(&mut ws, is("welcome")).await;
    let mut other = connect(&s.url).await;
    send(&mut other, json!({"type": "pending.subscribe"})).await;
    assert_eq!(
        next(&mut other, is("pending.snapshot")).await["enabled"],
        false
    );
    assert!(
        w["features"]
            .as_array()
            .unwrap()
            .contains(&json!("pending.v1")),
        "{w}"
    );
    send(&mut ws, json!({"type": "pending.subscribe"})).await;
    let snap = next(&mut ws, is("pending.snapshot")).await;
    assert_eq!(snap["enabled"], false, "no live feed in a test: off");
    assert_eq!(snap["cards"], json!([]));
    assert!(snap["gen"].is_u64() && snap["grace_secs"].is_i64());
    send(&mut ws, json!({"type": "pending.subscribe"})).await;
    next(&mut ws, is("pending.snapshot")).await;
    send(&mut ws, json!({"type": "pending.unsubscribe"})).await;
    send(&mut ws, json!({"type": "ping", "ts": 1})).await;
    next(&mut ws, is("pong")).await;
    let seen = types_for(&mut ws, Duration::from_millis(300)).await;
    assert!(!seen.iter().any(|t| t.starts_with("pending.")), "{seen:?}");
}
