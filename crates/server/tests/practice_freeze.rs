//! The Practice-page freeze (2026-11-26): a replay command never holds the connection's loop
//! while the replay applies it. (A seek awaited there stopped every frame, ping and probe answer
//! for as long as the rebuild took, so the page declared the socket dead, and the broadcast the
//! loop no longer read overflowed into a resync.) The lagged-socket resync itself is measured on
//! a whole-market day by `scripts/perf/practice-cdp.ts`: a four-symbol fixture never fills the
//! 16,384-frame broadcast.

use std::time::{Duration, Instant};

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
        ("SIM_FIXTURE_SYMBOLS".into(), "SMPL,FXLO,FXMD,FXHI".into()),
        ("REPLAY_START".into(), "10:40".into()),
        ("REPLAY_SPEED".into(), "1".into()),
        ("REPLAY_AUTOPLAY".into(), "0".into()),
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

async fn send(ws: &mut Ws, v: Value) {
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}

/// The next text frame; the server's heartbeat pings are answered on the way, as a client does
/// (a socket silent for three heartbeats is closed).
async fn next_frame(ws: &mut Ws) -> Value {
    loop {
        if let Message::Text(t) = ws.next().await.unwrap().unwrap() {
            let v: Value = serde_json::from_str(&t).unwrap();
            if v["type"] == "ping" {
                send(ws, json!({"type": "pong", "ts": v["ts"]})).await;
            }
            return v;
        }
    }
}

async fn next(ws: &mut Ws, pred: impl Fn(&Value) -> bool) -> Value {
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let v = next_frame(ws).await;
            if pred(&v) {
                return v;
            }
        }
    })
    .await
    .expect("timed out waiting for a frame")
}

async fn connect(url: &str) -> Ws {
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    send(
        &mut ws,
        json!({"type": "hello", "client": "test", "source": "replay"}),
    )
    .await;
    next(&mut ws, |m| m["type"] == "welcome").await;
    ws
}

/// Paused at 10:40 after the startup rebuild.
async fn settled(s: &Stack, ws: &mut Ws) {
    s.replay.control("pause", None).await.unwrap();
    next(ws, |m| {
        m["type"] == "replay.clock" && m["clock"]["playing"] == false
    })
    .await;
}

#[tokio::test]
async fn a_replay_command_does_not_hold_the_connection() {
    let s = stack().await;
    let mut ws = connect(&s.url).await;
    settled(&s, &mut ws).await;
    // A seek to the session's end rebuilds the whole fixture day; the ping right behind it is
    // answered while the replay is still rebuilding, not after.
    let end = s.replay.clock().unwrap().end.unwrap().millis();
    let sent = Instant::now();
    send(
        &mut ws,
        json!({"type": "replay.control", "action": "seek", "value": end - 60_000}),
    )
    .await;
    send(&mut ws, json!({"type": "ping", "ts": 7})).await;
    let mut pong = None;
    loop {
        let m = next(&mut ws, |m| {
            m["type"] == "pong" || m["type"] == "replay.clock"
        })
        .await;
        if m["type"] == "pong" {
            pong = Some(sent.elapsed());
        } else if m["clock"]["now"].as_i64() == Some(end - 60_000) {
            break;
        }
    }
    let rebuilt = sent.elapsed();
    let pong = pong.expect("the ping was answered");
    eprintln!("pong after {pong:?}, rebuild done after {rebuilt:?}");
    // Before the fix the pong came with the rebuild's end (14.28 s of a 14.28 s rebuild); now it
    // takes a round trip (about 50 ms in a debug build).
    assert!(
        pong * 4 < rebuilt,
        "the pong waited for the seek ({pong:?} vs {rebuilt:?})"
    );
}
