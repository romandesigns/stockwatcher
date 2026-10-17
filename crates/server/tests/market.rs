//! Server integration: provider → engine → /ws and /api, driven by a fixture replay.

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use stockwatcher_server::{AppState, Market, MarketConfig, router};
use tokio_tungstenite::tungstenite::Message;
use tower::ServiceExt;

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// 2026-10-15 10:40:00 ET (EDT) in epoch ms.
const T1040: i64 = 1_792_075_200_000;

struct Stack {
    _tmp: tempfile::TempDir,
    app: axum::Router,
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
    let (market, _sim) = Market::start(&cfg);
    let dir = tmp.path().join("static");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("index.html"), "<!doctype html>").unwrap();
    let app = router(
        AppState::with_market(None, market),
        &dir,
        &tmp.path().join("data"),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let served = app.clone();
    tokio::spawn(async move { axum::serve(listener, served).await.unwrap() });
    Stack {
        _tmp: tmp,
        app,
        url: format!("ws://{addr}/ws"),
    }
}

async fn connect(url: &str) -> Ws {
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    send(&mut ws, json!({"type": "hello", "client": "test"})).await;
    let w = next(&mut ws, |m| m["type"] == "welcome").await;
    assert_eq!(w["type"], "welcome");
    ws
}

async fn send(ws: &mut Ws, v: Value) {
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}

/// The next frame matching `pred` (others are skipped), within 20 s.
async fn next(ws: &mut Ws, pred: impl Fn(&Value) -> bool) -> Value {
    tokio::time::timeout(Duration::from_secs(20), async {
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

async fn get(app: &axum::Router, uri: &str) -> (StatusCode, Value) {
    let res = app
        .clone()
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let s = res.status();
    let b = res.into_body().collect().await.unwrap().to_bytes();
    (s, serde_json::from_slice(&b).unwrap_or(Value::Null))
}

/// Total volume a snapshot accounts for: candles plus volume-only gaps.
fn total_volume(snap: &Value) -> u64 {
    let c: u64 = snap["candles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["v"].as_u64().unwrap())
        .sum();
    let g: u64 = snap["gaps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["v"].as_u64().unwrap())
        .sum();
    c + g
}

async fn wait_for_replay(ws: &mut Ws) {
    // Started (catch-up to 10:40 finished) once a clock at or past 10:40 arrives.
    next(ws, |m| {
        m["type"] == "replay.clock" && m["clock"]["now"].as_i64().unwrap_or(0) >= T1040
    })
    .await;
}

#[tokio::test]
async fn subscribe_gets_snapshot_then_live_updates_while_playing() {
    let s = stack(&[]).await;
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;
    send(
        &mut ws,
        json!({"type":"chart.subscribe","symbol":"SMPL","timeframe":"1m","profileBin":0.01}),
    )
    .await;
    let snap = next(&mut ws, is("chart.snapshot")).await;
    assert_eq!(snap["symbol"], "SMPL");
    assert_eq!(snap["timeframe"], "1m");
    assert!(snap["candles"].as_array().unwrap().len() > 100);
    assert_eq!(snap["clock"]["mode"], "replay");
    assert_eq!(snap["profile"]["anchor"], "2026-10-15/regular");
    // Recent tape follows the snapshot, then live increments for this symbol and timeframe.
    let tape = next(&mut ws, is("tape.trade")).await;
    assert_eq!(tape["symbol"], "SMPL");
    let up = next(&mut ws, is("chart.update")).await;
    assert_eq!(
        (up["symbol"].as_str(), up["timeframe"].as_str()),
        (Some("SMPL"), Some("1m"))
    );
    assert!(up["candle"]["t"].as_i64().unwrap() >= T1040 - 60_000);
    // Nothing for other symbols or timeframes leaks into this subscription.
    for _ in 0..200 {
        let m = next(&mut ws, |m| m["type"] != "replay.clock").await;
        assert_eq!(m["symbol"], "SMPL");
        if let Some(tf) = m.get("timeframe") {
            assert_eq!(tf, "1m");
        }
    }
}

#[tokio::test]
async fn timeframe_switch_mid_replay_keeps_the_same_totals() {
    let s = stack(&[]).await;
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;
    send(&mut ws, json!({"type":"replay.control","action":"pause"})).await;
    let paused = next(&mut ws, |m| {
        m["type"] == "replay.clock" && m["clock"]["playing"] == false
    })
    .await;
    let mut snaps = Vec::new();
    for tf in ["1m", "15s", "30s", "5m"] {
        send(
            &mut ws,
            json!({"type":"chart.subscribe","symbol":"SMPL","timeframe":tf,"profileBin":0.01}),
        )
        .await;
        let snap = next(&mut ws, is("chart.snapshot")).await;
        assert_eq!(snap["timeframe"], tf);
        assert_eq!(snap["clock"]["now"], paused["clock"]["now"], "still paused");
        snaps.push(snap);
    }
    let totals: Vec<u64> = snaps.iter().map(total_volume).collect();
    assert!(totals[0] > 0);
    assert!(totals.iter().all(|t| *t == totals[0]), "{totals:?}");
    for s in &snaps[1..] {
        assert_eq!(
            s["profile"], snaps[0]["profile"],
            "profile independent of timeframe"
        );
        assert_eq!(s["quality"], snaps[0]["quality"]);
    }
    // Switching back mid-replay (playing again) still starts from a full snapshot.
    send(&mut ws, json!({"type":"replay.control","action":"play"})).await;
    send(
        &mut ws,
        json!({"type":"chart.subscribe","symbol":"SMPL","timeframe":"1m","profileBin":0.01}),
    )
    .await;
    let again = next(&mut ws, is("chart.snapshot")).await;
    assert!(total_volume(&again) >= totals[0]);
}

#[tokio::test]
async fn replay_control_round_trips() {
    let s = stack(&[]).await;
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;
    let clock = |m: &Value, k: &str| m["clock"][k].clone();

    send(&mut ws, json!({"type":"replay.control","action":"pause"})).await;
    let c = next(&mut ws, |m| {
        m["type"] == "replay.clock" && m["clock"]["playing"] == false
    })
    .await;
    let t0 = clock(&c, "now").as_i64().unwrap();

    send(
        &mut ws,
        json!({"type":"replay.control","action":"speed","value":2}),
    )
    .await;
    let c = next(&mut ws, |m| {
        m["type"] == "replay.clock" && m["clock"]["speed"] == 2.0
    })
    .await;
    assert_eq!(clock(&c, "playing"), false);

    send(
        &mut ws,
        json!({"type":"replay.control","action":"speed","value":0}),
    )
    .await;
    next(&mut ws, |m| {
        m["type"] == "replay.clock" && m["clock"]["speed"] == 0.0
    })
    .await;

    send(&mut ws, json!({"type":"replay.control","action":"step"})).await;
    let c = next(&mut ws, |m| {
        m["type"] == "replay.clock" && m["clock"]["now"].as_i64() != Some(t0)
    })
    .await;
    let t1 = clock(&c, "now").as_i64().unwrap();
    assert!(t1 > t0);

    send(
        &mut ws,
        json!({"type":"replay.control","action":"stepInterval","value":"5m"}),
    )
    .await;
    let c = next(&mut ws, |m| {
        m["type"] == "replay.clock" && m["clock"]["now"].as_i64().is_some_and(|n| n > t1)
    })
    .await;
    assert_eq!(clock(&c, "now").as_i64().unwrap() % 300_000, 0);

    // Subscribe, then seek: subscribers get a fresh snapshot of the rebuilt state.
    send(
        &mut ws,
        json!({"type":"chart.subscribe","symbol":"SMPL","timeframe":"5m","profileBin":0.01}),
    )
    .await;
    next(&mut ws, is("chart.snapshot")).await;
    let target = T1040 + 20 * 60_000; // 11:00 ET
    send(
        &mut ws,
        json!({"type":"replay.control","action":"seek","value":target}),
    )
    .await;
    let snap = next(&mut ws, is("chart.snapshot")).await;
    assert_eq!(snap["clock"]["now"], target);
    let last = snap["candles"].as_array().unwrap().last().unwrap()["t"]
        .as_i64()
        .unwrap();
    assert!(
        last < target && last >= target - 300_000,
        "rebuilt up to the seek target"
    );

    send(&mut ws, json!({"type":"replay.control","action":"play"})).await;
    next(&mut ws, |m| {
        m["type"] == "replay.clock" && m["clock"]["playing"] == true
    })
    .await;

    send(
        &mut ws,
        json!({"type":"replay.control","action":"speed","value":1.5}),
    )
    .await;
    let e = next(&mut ws, is("error")).await;
    assert_eq!(e["code"], "bad_request");
}

#[tokio::test]
async fn other_profile_bins_are_rebinned() {
    let s = stack(&[]).await;
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;
    send(
        &mut ws,
        json!({"type":"chart.subscribe","symbol":"SMPL","timeframe":"15s","profileBin":0.05}),
    )
    .await;
    let snap = next(&mut ws, is("chart.snapshot")).await;
    assert_eq!(snap["profile"]["bin"], 0.05);
    for r in snap["profile"]["rows"].as_array().unwrap() {
        let cents = (r["p"].as_f64().unwrap() * 100.0).round() as i64;
        assert_eq!(cents % 5, 0);
    }
    let up = next(&mut ws, |m| {
        m["type"] == "chart.update" && !m["profile"].as_array().unwrap().is_empty()
    })
    .await;
    assert_eq!(up["profileBin"], 0.05);
    let cents = (up["profile"][0]["p"].as_f64().unwrap() * 100.0).round() as i64;
    assert_eq!(cents % 5, 0);
}

#[tokio::test]
async fn rest_routes() {
    let s = stack(&[("REPLAY_AUTOPLAY", "0")]).await;
    // Wait until the catch-up finished.
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;

    let (st, v) = get(&s.app, "/api/symbols").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["symbols"], json!(["FXLO", "SMPL"]));
    assert_eq!(v["fixture"], true);
    assert_eq!(v["mode"], "replay");
    assert_eq!(v["day"], "2026-10-15");

    let (st, v) = get(&s.app, "/api/replay/state").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["status"]["state"], "running");
    assert_eq!(v["controllable"], true);
    assert_eq!(v["clock"]["now"], T1040);
    assert_eq!(v["clock"]["playing"], false);

    let (st, v) = get(&s.app, "/api/chart/smpl/snapshot?timeframe=5m&bin=0.02").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["type"], "chart.snapshot");
    assert_eq!(v["timeframe"], "5m");
    assert_eq!(v["profile"]["bin"], 0.02);
    assert!(
        v["candles"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["t"].as_i64().unwrap() < T1040)
    );

    assert_eq!(
        get(&s.app, "/api/chart/NOPE/snapshot").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&s.app, "/api/chart/SMPL/snapshot?timeframe=2m").await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(&s.app, "/api/chart/SMPL/snapshot?bin=-1").await.0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn live_mode_is_not_configured_and_refuses_replay_controls() {
    let s = stack(&[("MARKET_DATA_MODE", "live")]).await;
    let (_, v) = get(&s.app, "/api/replay/state").await;
    assert_eq!(v["mode"], "live");
    assert_eq!(v["status"]["state"], "not_configured");
    assert_eq!(v["controllable"], false);
    let (_, v) = get(&s.app, "/api/symbols").await;
    assert_eq!(v["symbols"], json!([]));
    let mut ws = connect(&s.url).await;
    send(&mut ws, json!({"type":"replay.control","action":"pause"})).await;
    let e = next(&mut ws, is("error")).await;
    assert_eq!(e["code"], "not_replay");
    send(
        &mut ws,
        json!({"type":"chart.subscribe","symbol":"SMPL","timeframe":"1m","profileBin":0.01}),
    )
    .await;
    let e = next(&mut ws, is("error")).await;
    assert_eq!(e["code"], "unknown_symbol");
}

#[tokio::test]
async fn empty_archive_without_autofixture_reports_no_data() {
    let s = stack(&[("SIM_AUTOFIXTURE", "0")]).await;
    let (_, v) = get(&s.app, "/api/symbols").await;
    assert_eq!(v["status"], "no_data");
    assert_eq!(v["symbols"], json!([]));
    // Health is unaffected.
    assert_eq!(get(&s.app, "/api/health").await.0, StatusCode::OK);
}

#[tokio::test]
async fn only_price_band_candidates_are_let_through() {
    // FXHI trades above $25 in the fixture day; SMPL and FXLO are inside $0.10–$25.
    let s = stack(&[
        ("REPLAY_AUTOPLAY", "0"),
        ("SIM_FIXTURE_SYMBOLS", "SMPL,FXLO,FXHI"),
    ])
    .await;
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;

    let (st, v) = get(&s.app, "/api/symbols").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["symbols"], json!(["FXLO", "SMPL"]));
    assert_eq!(v["screened"], json!({"total": 3, "candidates": 2}));
    let band = |k: &str| v["priceBand"][k].as_str().unwrap().parse::<f64>().unwrap();
    assert_eq!((band("min"), band("max")), (0.10, 25.0));

    let (st, body) = get(&s.app, "/api/chart/FXHI/snapshot").await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "outside_price_band");

    send(
        &mut ws,
        json!({"type": "chart.subscribe", "symbol": "FXHI", "timeframe": "1m", "profileBin": 0.01}),
    )
    .await;
    let err = next(&mut ws, is("error")).await;
    assert_eq!(err["code"], "outside_price_band");
}

#[tokio::test]
async fn the_price_band_is_configurable() {
    // A $0.10–$1.00 band keeps only the sub-dollar fixture symbol.
    let s = stack(&[
        ("REPLAY_AUTOPLAY", "0"),
        ("SIM_FIXTURE_SYMBOLS", "SMPL,FXLO,FXHI"),
        ("PRICE_BAND_MAX", "1.00"),
    ])
    .await;
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;
    let (_, v) = get(&s.app, "/api/symbols").await;
    assert_eq!(v["symbols"], json!(["FXLO"]));

    let bad = MarketConfig::from_lookup(|k| match k {
        "PRICE_BAND_MIN" => Some("5".into()),
        "PRICE_BAND_MAX" => Some("1".into()),
        _ => None,
    });
    assert!(bad.is_err(), "an inverted band must be refused at startup");
}

// --- simulated chart trading (docs/architecture/chart-trading-sim.md) -----------------------

/// The next `paper.state` matching `pred`.
async fn paper_state(ws: &mut Ws, pred: impl Fn(&Value) -> bool) -> Value {
    next(ws, |m| m["type"] == "paper.state" && pred(m)).await
}

fn order<'a>(state: &'a Value, id: &str) -> Option<&'a Value> {
    state["orders"].as_array()?.iter().find(|o| o["id"] == id)
}

#[tokio::test]
async fn a_limit_order_fills_during_replay_and_every_connection_hears_it() {
    let s = stack(&[]).await;
    let mut ws = connect(&s.url).await;
    // paper.state comes right after welcome: a flat, empty, clearly simulated account.
    let first = next(&mut ws, |m| m["type"] != "replay.clock").await;
    assert_eq!(first["type"], "paper.state");
    assert_eq!(first["simulated"], true);
    assert_eq!(first["account"]["cash"], 100_000.0);
    assert_eq!(first["orders"], json!([]));
    assert_eq!(first["positions"], json!([]));
    let mut other = connect(&s.url).await;
    wait_for_replay(&mut ws).await;

    // A price-qualifying print tells us where SMPL trades; a buy limit 10 cents above it is
    // traded through by the next print near that level.
    send(
        &mut ws,
        json!({"type":"chart.subscribe","symbol":"SMPL","timeframe":"1m","profileBin":0.01}),
    )
    .await;
    let print = next(&mut ws, |m| {
        m["type"] == "tape.trade" && m["counted"] == "ohlc_and_volume"
    })
    .await;
    let p = print["p"].as_f64().unwrap();
    let limit = ((p + 0.10) * 100.0).ceil() / 100.0;
    send(
        &mut ws,
        json!({"type":"order.place","clientOrderId":"c-1","symbol":"smpl","side":"buy",
               "orderType":"limit","qty":100,"limitPrice":limit}),
    )
    .await;
    let placed = paper_state(&mut ws, |m| !m["orders"].as_array().unwrap().is_empty()).await;
    let o = &placed["orders"][0];
    assert_eq!(o["clientOrderId"], "c-1");
    assert_eq!(o["symbol"], "SMPL");
    assert_eq!(o["limitPrice"], limit);
    assert!(
        o["createdAt"].as_i64().unwrap() >= T1040,
        "virtual clock, not wall clock"
    );
    let id = o["id"].as_str().unwrap().to_string();

    // The state that contains a fill goes out first, then the fill event itself.
    let filled = paper_state(&mut ws, |m| {
        order(m, &id).is_some_and(|o| o["status"] == "filled")
    })
    .await;
    let fill = next(&mut ws, is("paper.fill")).await;
    assert_eq!(fill["simulated"], true);
    assert_eq!(fill["orderId"], id.as_str());
    assert_eq!(
        (fill["side"].as_str(), fill["qty"].as_i64()),
        (Some("buy"), Some(100))
    );
    assert_eq!(fill["price"], limit, "a limit fills at its limit");
    assert_eq!(order(&filled, &id).unwrap()["avgFillPrice"], limit);
    assert_eq!(filled["positions"][0]["symbol"], "SMPL");
    assert_eq!(filled["positions"][0]["qty"], 100);
    let cash = filled["account"]["cash"].as_f64().unwrap();
    assert!((cash - (100_000.0 - 100.0 * limit)).abs() < 1e-6, "{cash}");

    // The other connection (no chart subscription) got the same broadcast. Matched on the state
    // rather than the fill: this socket is not read while the replay plays, so it may lag, and a
    // lagged client is resynchronized with a fresh paper.state, not the missed fill.
    paper_state(&mut other, |m| {
        order(m, &id).is_some_and(|o| o["status"] == "filled")
    })
    .await;

    // Time travel resets the account for everyone.
    send(
        &mut ws,
        json!({"type":"replay.control","action":"seek","value":T1040 + 5 * 60_000}),
    )
    .await;
    for c in [&mut ws, &mut other] {
        let reset = paper_state(c, |m| m["orders"].as_array().unwrap().is_empty()).await;
        assert_eq!(reset["positions"], json!([]));
        assert_eq!(reset["account"]["cash"], 100_000.0);
        assert_eq!(reset["account"]["realizedPnl"], 0.0);
    }
}

#[tokio::test]
async fn rejections_show_up_in_state_and_bad_commands_get_errors() {
    let s = stack(&[("REPLAY_AUTOPLAY", "0")]).await;
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;

    send(
        &mut ws,
        json!({"type":"order.place","symbol":"SMPL","side":"buy","orderType":"limit",
               "qty":10,"limitPrice":3.385}),
    )
    .await;
    let st = paper_state(&mut ws, |m| !m["orders"].as_array().unwrap().is_empty()).await;
    assert_eq!(st["orders"][0]["status"], "rejected");
    assert_eq!(st["orders"][0]["rejectReason"], "sub_penny");

    // Unknown means not in the market's symbol list at all.
    send(
        &mut ws,
        json!({"type":"order.place","symbol":"NOPE","side":"sell","orderType":"market","qty":1}),
    )
    .await;
    let st = paper_state(&mut ws, |m| m["orders"].as_array().unwrap().len() == 2).await;
    assert_eq!(st["orders"][1]["rejectReason"], "unknown_symbol");

    // Malformed (bad side) and unknown commands: error bad_order, no state change.
    send(
        &mut ws,
        json!({"type":"order.place","symbol":"SMPL","side":"hold","orderType":"market","qty":1}),
    )
    .await;
    assert_eq!(next(&mut ws, is("error")).await["code"], "bad_order");
    send(&mut ws, json!({"type":"order.replace","id":"o-1"})).await;
    assert_eq!(next(&mut ws, is("error")).await["code"], "bad_order");

    // Modifying or cancelling a finished order: not_open.
    send(&mut ws, json!({"type":"order.modify","id":"o-1","qty":5})).await;
    assert_eq!(next(&mut ws, is("error")).await["code"], "not_open");
    send(&mut ws, json!({"type":"order.cancel","id":"o-1"})).await;
    assert_eq!(next(&mut ws, is("error")).await["code"], "not_open");

    // A working order can be modified and cancelled; each change is broadcast.
    send(
        &mut ws,
        json!({"type":"order.place","symbol":"SMPL","side":"buy","orderType":"stop",
               "qty":10,"stopPrice":24.0}),
    )
    .await;
    paper_state(&mut ws, |m| {
        order(m, "o-3").is_some_and(|o| o["status"] == "open")
    })
    .await;
    send(
        &mut ws,
        json!({"type":"order.modify","id":"o-3","stopPrice":24.5}),
    )
    .await;
    paper_state(&mut ws, |m| {
        order(m, "o-3").is_some_and(|o| o["stopPrice"] == 24.5)
    })
    .await;
    send(&mut ws, json!({"type":"order.cancel","id":"o-3"})).await;
    paper_state(&mut ws, |m| {
        order(m, "o-3").is_some_and(|o| o["status"] == "cancelled")
    })
    .await;

    send(&mut ws, json!({"type":"paper.reset"})).await;
    paper_state(&mut ws, |m| m["orders"].as_array().unwrap().is_empty()).await;
}

#[tokio::test]
async fn a_bracket_fills_its_entry_then_one_exit_and_cancels_the_other() {
    let s = stack(&[]).await;
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;
    send(
        &mut ws,
        json!({"type":"chart.subscribe","symbol":"SMPL","timeframe":"1m","profileBin":0.01}),
    )
    .await;
    let print = next(&mut ws, |m| {
        m["type"] == "tape.trade" && m["counted"] == "ohlc_and_volume"
    })
    .await;
    let p = print["p"].as_f64().unwrap();
    // A market entry fills on the next print near p. Its take-profit, a sell limit 10 cents
    // under that, is traded through by the very same print but must wait for the one after;
    // the stop-loss 30 cents under is out of reach, so it is the one the OCO cancels.
    let cents = |x: f64| (x * 100.0).round() / 100.0;
    let (tp, sl) = (cents(p - 0.10), cents(p - 0.30));
    send(
        &mut ws,
        json!({"type":"order.place","clientOrderId":"c-b","symbol":"SMPL","side":"buy",
               "orderType":"market","qty":100,"bracket":{"takeProfit":tp,"stopLoss":sl}}),
    )
    .await;
    let placed = paper_state(&mut ws, |m| m["orders"].as_array().unwrap().len() == 3).await;
    let ids: Vec<String> = placed["orders"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["id"].as_str().unwrap().to_string())
        .collect();
    let [entry, tp_id, sl_id] = [&ids[0], &ids[1], &ids[2]];
    let group = placed["orders"][0]["ocoGroup"].clone();
    assert!(group.as_str().unwrap().starts_with("b-"));
    for (o, role, status) in [
        (entry, "entry", "open"),
        (tp_id, "take_profit", "pending"),
        (sl_id, "stop_loss", "pending"),
    ] {
        let o = order(&placed, o).unwrap();
        assert_eq!((&o["role"], &o["status"]), (&json!(role), &json!(status)));
        assert_eq!(o["ocoGroup"], group);
    }
    assert_eq!(order(&placed, tp_id).unwrap()["parentId"], entry.as_str());
    assert_eq!(order(&placed, tp_id).unwrap()["limitPrice"], tp);
    assert_eq!(order(&placed, sl_id).unwrap()["stopPrice"], sl);

    // The entry's fill opens both exits, and fills neither of them on that print.
    let opened = paper_state(&mut ws, |m| {
        order(m, entry).is_some_and(|o| o["status"] == "filled")
    })
    .await;
    assert_eq!(order(&opened, tp_id).unwrap()["status"], "open");
    assert_eq!(order(&opened, sl_id).unwrap()["status"], "open");
    assert_eq!(opened["positions"][0]["qty"], 100);

    // The next print fills the take-profit at its limit and cancels the stop-loss.
    let closed = paper_state(&mut ws, |m| {
        order(m, tp_id).is_some_and(|o| o["status"] == "filled")
    })
    .await;
    let sib = order(&closed, sl_id).unwrap();
    assert_eq!(
        (&sib["status"], &sib["rejectReason"]),
        (&json!("cancelled"), &Value::Null)
    );
    assert_eq!(closed["positions"], json!([]), "flat");
    let fill = next(&mut ws, is("paper.fill")).await;
    assert_eq!(fill["orderId"], tp_id.as_str());
    assert_eq!(
        (fill["side"].as_str(), fill["price"].as_f64()),
        (Some("sell"), Some(tp))
    );

    // A bracket out of order is refused when modified, and the order stays as it was.
    send(&mut ws, json!({"type":"paper.reset"})).await;
    paper_state(&mut ws, |m| m["orders"].as_array().unwrap().is_empty()).await;
    send(
        &mut ws,
        json!({"type":"order.place","symbol":"SMPL","side":"buy","orderType":"limit",
               "qty":1,"limitPrice":1.00,"bracket":{"takeProfit":1.50,"stopLoss":0.50}}),
    )
    .await;
    let st = paper_state(&mut ws, |m| m["orders"].as_array().unwrap().len() == 3).await;
    let tp_id = st["orders"][1]["id"].as_str().unwrap().to_string();
    send(
        &mut ws,
        json!({"type":"order.modify","id":tp_id,"limitPrice":0.40}),
    )
    .await;
    assert_eq!(next(&mut ws, is("error")).await["code"], "bad_bracket");
}
