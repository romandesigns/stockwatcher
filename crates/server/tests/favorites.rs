//! The Favorites panel on the wire (`favorites.subscribe` / `favorites.snapshot` /
//! `favorites.update`), driven by a fixture replay. Row building and the list's parse are tested
//! in crates/server/src/favorites.rs.

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

/// A snapshot at once (empty before a list exists), an update with every favorite's row and its
/// whole day line once the list arrives, then nothing while nothing changes; a bystander never
/// sees a favorites frame.
#[tokio::test]
async fn favorites_subscribe_gets_rows_with_lines_then_only_changes() {
    let s = stack().await;
    let mut ws = connect(&s.url).await;
    let mut bystander = connect(&s.url).await;
    s.replay.control("pause", None).await.unwrap();
    next(&mut ws, |m| {
        m["type"] == "replay.clock" && m["clock"]["playing"] == false
    })
    .await;
    send(&mut ws, json!({"type": "favorites.subscribe"})).await;
    let snap = next(&mut ws, |m| m["type"] == "favorites.snapshot").await;
    assert_eq!(snap["symbols"], json!([]), "{snap}");
    assert_eq!(snap["rows"], json!([]));

    // A periodic tick may publish the initial empty board after the snapshot and before the
    // list edit. Exercise that ordering explicitly, then wait for the edit's update rather
    // than assuming the next Favorites frame belongs to it.
    s.replay.publish_favorites();
    s.replay.set_favorites(vec!["SMPL".into(), "NOPE".into()]);
    s.replay.publish_favorites();
    let up = next(&mut ws, |m| {
        m["type"] == "favorites.update" && m["symbols"] == json!(["SMPL", "NOPE"])
    })
    .await;
    assert_eq!(up["symbols"], json!(["SMPL", "NOPE"]));
    let rows = up["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    let smpl = &rows[0];
    assert_eq!(smpl["symbol"], "SMPL");
    assert!(smpl["price"].as_i64().unwrap() > 0, "{smpl}");
    assert!(smpl["high"].as_i64().unwrap() >= smpl["low"].as_i64().unwrap());
    assert!(smpl["volume"].as_u64().unwrap() > 0);
    assert_eq!(
        smpl["day_line_from"], 0,
        "the first update carries the whole line"
    );
    assert!(smpl["day_line"].as_array().unwrap().len() > 2);
    // A ticker that never traded is still listed, with nothing invented.
    assert_eq!(rows[1], json!({"symbol": "NOPE"}));

    // Paused: nothing changed, so the next ticks send nothing.
    s.replay.publish_favorites();
    assert!(
        frames_for(&mut ws, "favorites.update", Duration::from_millis(1500))
            .await
            .is_empty()
    );
    assert!(
        frames_for(
            &mut bystander,
            "favorites.snapshot",
            Duration::from_millis(200)
        )
        .await
        .is_empty()
    );

    // Unsubscribed: a list change reaches nobody.
    send(&mut ws, json!({"type": "favorites.unsubscribe"})).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    s.replay.set_favorites(vec!["FXLO".into()]);
    s.replay.publish_favorites();
    assert!(
        frames_for(&mut ws, "favorites.update", Duration::from_millis(1200))
            .await
            .is_empty()
    );
}
