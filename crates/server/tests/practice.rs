//! Practice notices on the wire (`practice.notice`, crate::practice): a replay connection (the
//! practice page) receives them; a live connection never does. Deterministic: every wait is for a
//! frame (the paused clock, the notice, a pong), never a fixed sleep that has to be long enough.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use stockwatcher_server::rocket::{Notice, NoticeKind};
use stockwatcher_server::{AppState, Market, MarketConfig, Markets, Settings, router};
use tokio_tungstenite::tungstenite::Message;

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Stack {
    _tmp: tempfile::TempDir,
    url: String,
    replay: Market,
    live: Market,
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
    let live = markets.live.clone();
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
        live,
    }
}

async fn connect(url: &str, source: &str) -> (Ws, Value) {
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    send(
        &mut ws,
        json!({"type": "hello", "client": "test", "source": source}),
    )
    .await;
    let welcome = next(&mut ws, |m| m["type"] == "welcome").await;
    (ws, welcome)
}

async fn send(ws: &mut Ws, v: Value) {
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}

async fn next(ws: &mut Ws, pred: impl Fn(&Value) -> bool) -> Value {
    tokio::time::timeout(Duration::from_secs(30), async {
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

/// Every frame up to the pong for `ts`: what the server sent this connection before it answered.
async fn until_pong(ws: &mut Ws, ts: i64) -> Vec<Value> {
    send(ws, json!({"type": "ping", "ts": ts})).await;
    let mut seen = Vec::new();
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Message::Text(t) = ws.next().await.unwrap().unwrap() {
                let v: Value = serde_json::from_str(&t).unwrap();
                if v["type"] == "pong" && v["ts"] == ts {
                    return;
                }
                seen.push(v);
            }
        }
    })
    .await
    .expect("timed out waiting for the pong");
    seen
}

fn notice(kind: NoticeKind, symbol: &str) -> Notice {
    Notice {
        kind,
        alert_id: format!("2026-11-17-{symbol}-1"),
        symbol: symbol.into(),
        at_ns: 1_763_395_200_000_000_000,
        price: 2.41,
        from_flag_pct: Some(0.0),
        reasons: vec!["$120k in 5 min".into()],
        since_start_min: None,
        exec_ns: None,
        detail: None,
        spread_bp: None,
        no_quote: false,
    }
}

#[tokio::test]
async fn a_replay_connection_gets_practice_notices_and_a_live_one_never_does() {
    let s = stack().await;
    let (mut practice, welcome) = connect(&s.url, "replay").await;
    let (mut live, _) = connect(&s.url, "live").await;
    assert!(
        welcome["features"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f == "practice.notice.v1"),
        "{welcome}"
    );
    // The replay's initial catch-up is a rebuild, which offers nothing: wait for its end (the
    // paused clock comes after the rebuild's closing clock in the ordered stream).
    s.replay.control("pause", None).await.unwrap();
    next(&mut practice, |m| {
        m["type"] == "replay.clock" && m["clock"]["playing"] == false
    })
    .await;

    s.replay.test_offer_notices(vec![
        notice(NoticeKind::New, "PRAC"),
        notice(NoticeKind::Burst, "BRST"),
    ]);
    let first = next(&mut practice, |m| m["type"] == "practice.notice").await;
    assert_eq!(first["practice"], true, "{first}");
    assert_eq!(first["symbol"], "PRAC");
    assert!(first["title"].as_str().is_some_and(|t| !t.is_empty()));
    let second = next(&mut practice, |m| m["type"] == "practice.notice").await;
    assert_eq!(second["symbol"], "BRST");

    // The live connection: nothing practice-shaped before its pong, and a live notice offered on
    // the live market never comes back as a practice frame either.
    s.live
        .test_offer_notices(vec![notice(NoticeKind::New, "LIVE")]);
    let seen = until_pong(&mut live, 41).await;
    assert!(
        !seen.iter().any(|m| m["type"] == "practice.notice"),
        "{seen:?}"
    );
    // ...and the practice connection gets no practice frame for the live market's notice.
    let seen = until_pong(&mut practice, 42).await;
    assert!(
        !seen
            .iter()
            .any(|m| m["type"] == "practice.notice" && m["symbol"] == "LIVE"),
        "{seen:?}"
    );
}

/// The practice page reads every dashboard panel from the replay market: each panel's
/// subscription is answered with its snapshot on a replay connection (HOD Momentum and News
/// pending answer too, as "not enabled" on a replay: the page labels them "Not in practice yet").
#[tokio::test]
async fn every_dashboard_panel_answers_on_a_replay_connection() {
    let s = stack().await;
    let (mut ws, _) = connect(&s.url, "replay").await;
    send(
        &mut ws,
        json!({"type": "chart.subscribe", "symbol": "SMPL", "timeframe": "1m", "profileBin": 0.01, "id": "d0"}),
    )
    .await;
    next(&mut ws, |m| m["type"] == "chart.snapshot").await;
    for (sub, snap) in [
        ("rocket.subscribe", "rocket.snapshot"),
        ("ross.subscribe", "ross.snapshot"),
        ("pressure.subscribe", "pressure.snapshot"),
        ("favorites.subscribe", "favorites.snapshot"),
        ("movers.subscribe", "movers.snapshot"),
        ("symbols.subscribe", "symbols.snapshot"),
        ("detection.subscribe", "detection.snapshot"),
        ("hod.subscribe", "hod.snapshot"),
        ("pending.subscribe", "pending.snapshot"),
    ] {
        send(&mut ws, json!({"type": sub})).await;
        let got = next(&mut ws, |m| m["type"] == snap).await;
        if snap == "hod.snapshot" || snap == "pending.snapshot" {
            // Live only until their evidence reads go through the replay clock (Phase 3).
            assert_eq!(got["enabled"], false, "{got}");
        }
    }
}
