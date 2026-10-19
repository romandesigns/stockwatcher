//! Synced settings over /ws and /api/settings (crates/server/src/settings.rs, ws.rs): a real
//! server on an ephemeral port, real WebSocket clients.

use std::path::Path;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use stockwatcher_server::{AppState, Market, Settings, router};
use tokio_tungstenite::tungstenite::Message;
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789abcdef0123456789";

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Server {
    url: String,
    app: axum::Router,
    task: tokio::task::JoinHandle<()>,
}

/// A server whose settings live in `dir` (None = memory only).
async fn serve(dir: Option<&Path>, token: Option<&str>) -> Server {
    let settings = match dir {
        Some(d) => Settings::open(d).unwrap(),
        None => Settings::in_memory(),
    };
    let market = Market::idle(market_providers::ClockMode::Replay, "idle", None);
    let state = AppState::with_parts(token.map(str::to_owned), market, settings);
    let tmp = std::env::temp_dir();
    let app = router(state, &tmp.join("sw-no-static"), &tmp.join("sw-no-data"));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let served = app.clone();
    let task = tokio::spawn(async move { axum::serve(listener, served).await.unwrap() });
    Server {
        url: format!("ws://{addr}/ws"),
        app,
        task,
    }
}

async fn send(ws: &mut Ws, v: Value) {
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}

/// The next frame matching `pred` (others skipped), within 5 s.
async fn next(ws: &mut Ws, pred: impl Fn(&Value) -> bool) -> Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match ws.next().await.expect("stream ended").unwrap() {
                Message::Text(t) => {
                    let v: Value = serde_json::from_str(&t).unwrap();
                    if pred(&v) {
                        return v;
                    }
                }
                Message::Close(c) => panic!("closed: {c:?}"),
                _ => {}
            }
        }
    })
    .await
    .expect("timed out waiting for a matching frame")
}

fn is(t: &'static str) -> impl Fn(&Value) -> bool {
    move |m| m["type"] == t
}

/// Nothing settings-related arrives within `wait`.
async fn quiet(ws: &mut Ws, wait: Duration) {
    let got = tokio::time::timeout(wait, async {
        loop {
            if let Some(Ok(Message::Text(t))) = ws.next().await {
                let v: Value = serde_json::from_str(&t).unwrap();
                if v["type"]
                    .as_str()
                    .is_some_and(|t| t.starts_with("settings."))
                {
                    return v;
                }
            }
        }
    })
    .await;
    assert!(got.is_err(), "unexpected settings frame: {got:?}");
}

/// Connects, says hello, and returns the socket with the `settings.snapshot` that follows `welcome`.
async fn connect(url: &str, token: Option<&str>) -> (Ws, Value) {
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let mut hello = json!({ "type": "hello", "client": "test" });
    if let Some(t) = token {
        hello["token"] = json!(t);
    }
    send(&mut ws, hello).await;
    next(&mut ws, is("welcome")).await;
    let snap = next(&mut ws, is("settings.snapshot")).await;
    (ws, snap)
}

fn doc<'a>(snap: &'a Value, ns: &str) -> Option<&'a Value> {
    snap["docs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["ns"] == ns)
}

#[tokio::test]
async fn a_change_reaches_every_connection_including_the_writer_with_its_op_id() {
    let s = serve(None, None).await;
    let (mut a, snap_a) = connect(&s.url, None).await;
    let (mut b, _) = connect(&s.url, None).await;
    assert_eq!(snap_a["docs"], json!([]));

    let indicators = json!([{ "id": "ema-9", "kind": "ema", "period": 9 }]);
    send(
        &mut a,
        json!({ "type": "settings.set", "ns": "indicators", "value": indicators, "opId": "a-1" }),
    )
    .await;
    for ws in [&mut a, &mut b] {
        let m = next(ws, is("settings.changed")).await;
        assert_eq!(m["ns"], "indicators");
        assert_eq!(m["rev"], 1);
        assert_eq!(m["opId"], "a-1");
        assert_eq!(m["value"], indicators);
        assert!(m["updatedAt"].as_u64().unwrap() > 0);
    }

    // B answers: last writer wins, rev 2, both hear it.
    send(
        &mut b,
        json!({ "type": "settings.set", "ns": "indicators", "value": [], "opId": "b-1" }),
    )
    .await;
    for ws in [&mut a, &mut b] {
        let m = next(ws, is("settings.changed")).await;
        assert_eq!(
            (m["rev"].clone(), m["opId"].clone(), m["value"].clone()),
            (json!(2), json!("b-1"), json!([]))
        );
    }

    // A third client connecting now (or a reconnect) starts from the snapshot.
    let (_c, snap) = connect(&s.url, None).await;
    let d = doc(&snap, "indicators").unwrap();
    assert_eq!(
        (d["rev"].clone(), d["value"].clone()),
        (json!(2), json!([]))
    );
}

#[tokio::test]
async fn drawing_patches_from_two_devices_both_land() {
    let s = serve(None, None).await;
    let (mut a, _) = connect(&s.url, None).await;
    let (mut b, _) = connect(&s.url, None).await;
    let d1 = json!({ "z": 1, "d": { "id": "d1", "kind": "hline", "a": { "t": 1, "p": 2.5 }, "b": { "t": 1, "p": 2.5 } } });
    let d2 = json!({ "z": 2, "d": { "id": "d2", "kind": "vline", "a": { "t": 5, "p": 3.0 }, "b": { "t": 5, "p": 3.0 } } });
    send(&mut a, json!({ "type": "settings.patch", "ns": "drawings:SMPL", "set": { "d1": d1 }, "opId": "a-1" })).await;
    send(&mut b, json!({ "type": "settings.patch", "ns": "drawings:SMPL", "set": { "d2": d2 }, "opId": "b-1" })).await;
    let last = next(&mut a, |m| m["type"] == "settings.changed" && m["rev"] == 2).await;
    assert_eq!(last["value"], json!({ "d1": d1, "d2": d2 }));
    next(&mut b, |m| m["type"] == "settings.changed" && m["rev"] == 2).await;

    send(
        &mut b,
        json!({ "type": "settings.patch", "ns": "drawings:SMPL", "remove": ["d1"], "opId": "b-2" }),
    )
    .await;
    let m = next(&mut a, |m| m["type"] == "settings.changed" && m["rev"] == 3).await;
    assert_eq!(m["value"], json!({ "d2": d2 }));
    assert_eq!(m["opId"], "b-2");
}

#[tokio::test]
async fn migration_seeds_once_and_the_second_client_adopts_the_shared_value() {
    let s = serve(None, None).await;
    let (mut a, _) = connect(&s.url, None).await;
    let (mut b, _) = connect(&s.url, None).await;
    send(&mut a, json!({ "type": "settings.set", "ns": "layers", "value": { "halts": false }, "opId": "a-m", "ifAbsent": true })).await;
    next(&mut b, is("settings.changed")).await;
    next(&mut a, is("settings.changed")).await;

    send(&mut b, json!({ "type": "settings.set", "ns": "layers", "value": { "halts": true }, "opId": "b-m", "ifAbsent": true })).await;
    let reply = next(&mut b, is("settings.changed")).await;
    assert_eq!(
        (
            reply["rev"].clone(),
            reply["opId"].clone(),
            reply["value"].clone()
        ),
        (json!(1), json!("b-m"), json!({ "halts": false }))
    );
    // The refused seed went to the writer alone.
    quiet(&mut a, Duration::from_millis(300)).await;
}

#[tokio::test]
async fn bad_writes_are_rejected_to_the_writer_with_their_op_id() {
    let s = serve(None, None).await;
    let (mut a, _) = connect(&s.url, None).await;
    let (mut b, _) = connect(&s.url, None).await;
    let cases = [
        (
            json!({ "type": "settings.set", "ns": "../x", "value": 1, "opId": "o1" }),
            "bad_request",
        ),
        (
            json!({ "type": "settings.set", "ns": "layers", "opId": "o2" }),
            "bad_request",
        ),
        (
            json!({ "type": "settings.set", "ns": "big", "value": "x".repeat(60_000), "opId": "o3" }),
            "ok",
        ),
    ];
    for (msg, code) in cases {
        let op = msg["opId"].clone();
        send(&mut a, msg).await;
        if code == "ok" {
            next(&mut a, is("settings.changed")).await;
            next(&mut b, is("settings.changed")).await;
            continue;
        }
        let r = next(&mut a, is("settings.rejected")).await;
        assert_eq!((r["code"].as_str().unwrap(), r["opId"].clone()), (code, op));
    }
    send(
        &mut a,
        json!({ "type": "settings.set", "ns": "list", "value": [1], "opId": "o4" }),
    )
    .await;
    next(&mut a, is("settings.changed")).await;
    next(&mut b, is("settings.changed")).await;
    send(
        &mut a,
        json!({ "type": "settings.patch", "ns": "list", "set": { "k": 1 }, "opId": "o5" }),
    )
    .await;
    let r = next(&mut a, is("settings.rejected")).await;
    assert_eq!(
        (r["code"].clone(), r["ns"].clone(), r["opId"].clone()),
        (json!("not_a_map"), json!("list"), json!("o5"))
    );
    quiet(&mut b, Duration::from_millis(300)).await;
}

#[tokio::test]
async fn values_survive_a_server_restart() {
    let dir = tempfile::tempdir().unwrap();
    let settings_dir = dir.path().join("settings");
    let s = serve(Some(&settings_dir), None).await;
    let (mut a, _) = connect(&s.url, None).await;
    send(&mut a, json!({ "type": "settings.set", "ns": "chartSettings", "value": { "chartType": "line" }, "opId": "a-1" })).await;
    send(&mut a, json!({ "type": "settings.patch", "ns": "drawings:FXLO", "set": { "d1": { "z": 1 } }, "opId": "a-2" })).await;
    next(&mut a, |m| m["opId"] == "a-2").await;
    drop(a);
    s.task.abort();

    let s = serve(Some(&settings_dir), None).await;
    let (_b, snap) = connect(&s.url, None).await;
    assert_eq!(
        doc(&snap, "chartSettings").unwrap()["value"],
        json!({ "chartType": "line" })
    );
    assert_eq!(
        doc(&snap, "drawings:FXLO").unwrap()["value"],
        json!({ "d1": { "z": 1 } })
    );
    assert_eq!(doc(&snap, "drawings:FXLO").unwrap()["rev"], 1);
}

async fn get(app: &axum::Router, bearer: Option<&str>) -> (StatusCode, Value) {
    let mut req = Request::get("/api/settings");
    if let Some(t) = bearer {
        req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let res = app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let body = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

#[tokio::test]
async fn the_rest_view_and_the_socket_need_the_token() {
    let s = serve(None, Some(TOKEN)).await;
    assert_eq!(get(&s.app, None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(
        get(&s.app, Some("wrong-token-0123456789abcdef012345"))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );

    // No settings before the token is accepted: a bad hello gets only the rejection.
    let (mut ws, _) = tokio_tungstenite::connect_async(&s.url).await.unwrap();
    send(&mut ws, json!({ "type": "hello", "token": "nope" })).await;
    let err = next(&mut ws, |_| true).await;
    assert_eq!(err["code"], "unauthorized");

    let (mut a, _) = connect(&s.url, Some(TOKEN)).await;
    send(&mut a, json!({ "type": "settings.set", "ns": "drawingSettings", "value": { "version": 1, "hidden": ["rect"] }, "opId": "a-1" })).await;
    next(&mut a, is("settings.changed")).await;
    let (status, body) = get(&s.app, Some(TOKEN)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["type"], "settings.snapshot");
    assert_eq!(
        doc(&body, "drawingSettings").unwrap()["value"]["hidden"],
        json!(["rect"])
    );
}

/// The alerts' runtime state is the server's (crate::alerts::model::STATE_NS): a client write
/// is refused and nothing changes; a malformed push token is refused too.
#[tokio::test]
async fn clients_cannot_write_the_alert_state_or_register_junk_tokens() {
    let s = serve(None, None).await;
    let (mut ws, _) = connect(&s.url, None).await;
    next(&mut ws, is("alerts.history")).await;
    send(
        &mut ws,
        json!({ "type": "settings.patch", "ns": "alerts.state", "set": { "a1": { "status": "armed", "basis": 1 } }, "opId": "x-1" }),
    )
    .await;
    let r = next(&mut ws, is("settings.rejected")).await;
    assert_eq!(r["ns"], "alerts.state");
    assert_eq!(r["code"], "bad_request");
    send(
        &mut ws,
        json!({ "type": "settings.set", "ns": "alerts.state", "value": {}, "opId": "x-2" }),
    )
    .await;
    assert_eq!(next(&mut ws, is("settings.rejected")).await["opId"], "x-2");
    // The rules namespace itself is an ordinary synced namespace.
    send(&mut ws, json!({ "type": "settings.patch", "ns": "alerts", "set": { "a1": { "symbol": "XLAB" } }, "opId": "x-3" })).await;
    assert_eq!(next(&mut ws, is("settings.changed")).await["ns"], "alerts");
    send(
        &mut ws,
        json!({ "type": "push.register", "token": "not-a-token" }),
    )
    .await;
    assert_eq!(next(&mut ws, is("error")).await["code"], "bad_request");
    send(&mut ws, json!({ "type": "push.register", "token": "ExponentPushToken[abc]", "platform": "android" })).await;
    send(&mut ws, json!({ "type": "ping", "ts": 7 })).await;
    // A valid token has no direct reply: the next frame is the pong.
    let v = next(&mut ws, |v| v["type"] == "pong" || v["type"] == "error").await;
    assert_eq!(v["type"], "pong");
    s.task.abort();
}
