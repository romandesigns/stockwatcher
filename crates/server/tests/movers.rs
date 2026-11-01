//! The movers channel on the wire (`movers.subscribe` / `movers.snapshot` / `movers.update`),
//! driven by a fixture replay. Ranking, the diff, the slices and the throttle are tested in
//! crates/server/src/movers.rs.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use stockwatcher_server::{AppState, Market, MarketConfig, Markets, Settings, router};
use tokio_tungstenite::tungstenite::Message;

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Stack {
    _tmp: tempfile::TempDir,
    url: String,
    replay: Market,
}

async fn stack() -> Stack {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("archive").to_string_lossy().into_owned();
    let vars: Vec<(String, String)> = vec![
        ("ARCHIVE_ROOT".into(), root),
        ("ARCHIVE_MIN_FREE_MB".into(), "0".into()),
        ("SIM_AUTOFIXTURE".into(), "1".into()),
        ("SIM_FIXTURE_SYMBOLS".into(), "SMPL,FXLO".into()),
        ("REPLAY_START".into(), "10:40".into()),
        ("REPLAY_SPEED".into(), "60".into()),
    ];
    let cfg = MarketConfig::from_lookup(|k| {
        vars.iter()
            .rev()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.clone())
    })
    .unwrap();
    let (markets, _sim) = Markets::start(&cfg);
    let replay = markets.replay.clone();
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
        replay,
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

async fn frames_for(ws: &mut Ws, kind: &str, wait: Duration) -> Vec<Value> {
    let mut seen = Vec::new();
    let _ = tokio::time::timeout(wait, async {
        while let Some(Ok(m)) = ws.next().await {
            if let Message::Text(t) = m {
                let v: Value = serde_json::from_str(&t).unwrap();
                if v["type"] == kind {
                    seen.push(v);
                }
            }
        }
    })
    .await;
    seen
}

/// A snapshot at once with the three lists, rows that carry whole day lines once the tick has
/// ranked the fixture, then updates newer than it; a bystander never sees a movers frame.
#[tokio::test]
async fn movers_subscribe_gets_the_lists_with_lines_then_updates() {
    let s = stack().await;
    let mut ws = connect(&s.url).await;
    let mut bystander = connect(&s.url).await;
    send(&mut ws, json!({"type": "movers.subscribe"})).await;
    let snap = next(&mut ws, |m| m["type"] == "movers.snapshot").await;
    for k in ["gainers", "bull", "bear", "rows"] {
        assert!(snap[k].is_array(), "{k}: {snap}");
    }
    // The frame's market clock in exchange ns beside the ms `t` (`times.ns.v1`).
    assert_eq!(
        snap["tNs"].as_i64().map(|n| n.div_euclid(1_000_000)),
        snap["t"].as_i64(),
        "{snap}"
    );
    // The fixture replay plays at 60×: within a few ticks some name is up or down on the day.
    let up = next(&mut ws, |m| {
        m["type"] == "movers.update" && m["rows"].as_array().is_some_and(|r| !r.is_empty())
    })
    .await;
    let row = &up["rows"][0];
    assert!(row["price"].as_i64().unwrap() > 0, "{row}");
    assert!(row["chg"].is_number() && row["dv"].is_u64() && row["trades"].as_u64().unwrap() >= 100);
    assert!(row["day_line"].is_array(), "rows carry the day line: {row}");
    // Every row's symbol is in a list (this update's, or the snapshot's when unchanged).
    let listed = |k: &str| {
        up.get(k)
            .or(snap.get(k))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
    };
    let all: Vec<Value> = ["gainers", "bull", "bear"]
        .iter()
        .flat_map(|k| listed(k))
        .collect();
    assert!(all.contains(&row["symbol"]), "{up}");

    // Re-subscribing answers with a fresh snapshot that now has those rows, lines whole.
    send(&mut ws, json!({"type": "movers.subscribe"})).await;
    let snap = next(&mut ws, |m| m["type"] == "movers.snapshot").await;
    let rows = snap["rows"].as_array().unwrap();
    assert!(!rows.is_empty(), "{snap}");
    assert!(
        rows.iter()
            .all(|r| r.get("day_line").is_none() || r["day_line_from"] == 0)
    );

    assert!(
        frames_for(&mut bystander, "movers.update", Duration::from_millis(1500))
            .await
            .is_empty()
    );
    assert_eq!(s.replay.movers_subscribers(), 1);
    send(&mut ws, json!({"type": "movers.unsubscribe"})).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(s.replay.movers_subscribers(), 0);
}
