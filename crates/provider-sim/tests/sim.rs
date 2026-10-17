//! Golden JSON for every implemented provider-sim route and stream, against the shapes in
//! Alpaca's and FMP's public docs (see docs/architecture/simulator-and-chart.md, Appendix B).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::NaiveDate;
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use market_archive::{Archive, ArchiveConfig, FsyncPolicy};
use market_events::conditions::Tape;
use market_events::session::et;
use market_events::{
    Cancel, CancelAction, Correction, MarketEvent, Price, Quote, Source, Status, StatusKind, Trade,
    UnixNanos,
};
use market_replay::{ClockMode, ClockState, FeedItem, Speed};
use provider_sim::{Sim, SimConfig};
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message;
use tower::ServiceExt;

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 15).unwrap()
}

fn at(h: u32, m: u32, s: u32, ms: u32) -> UnixNanos {
    et(day(), h, m, s, ms).unwrap()
}

fn trade(t: UnixNanos, cents: i64, size: u64, seq: u64, x: &str, c: &[&str]) -> Trade {
    Trade::new("SMPL", t, Price::from_cents(cents), size, seq)
        .with_exchange(x)
        .with_tape(Tape::C)
        .with_source(Source::Fixture)
        .with_conditions(c.iter().copied())
}

/// Arrival order. O is overnight (BOATS), P premarket, the rest regular; E is after the clock.
fn events() -> Vec<MarketEvent> {
    let prev = NaiveDate::from_ymd_opt(2026, 10, 14).unwrap();
    vec![
        trade(
            et(prev, 21, 0, 0, 0).unwrap(),
            300,
            200,
            1,
            "Q",
            &["@", "T"],
        )
        .into(), // O
        trade(at(8, 0, 0, 0), 310, 100, 2, "Q", &["@", "T"]).into(), // P
        trade(at(10, 0, 1, 250), 318, 100, 7, "V", &["@"]).into(),   // A
        MarketEvent::Quote(Quote {
            symbol: "SMPL".into(),
            event_ts: at(10, 0, 2, 0),
            receive_ts: None,
            bid_price: Price::from_cents(317),
            bid_size: 3,
            bid_exchange: "Q".into(),
            ask_price: Price::from_cents(319),
            ask_size: 5,
            ask_exchange: "V".into(),
            conditions: vec!["R".into()],
            tape: Tape::C,
            source: Source::Fixture,
        }),
        trade(at(10, 0, 30, 500), 320, 50, 8, "P", &["@", "I"]).into(), // B odd lot
        trade(at(10, 1, 5, 0), 319, 300, 9, "Z", &["@"]).into(),        // C, cancelled below
        MarketEvent::Cancel(Cancel {
            symbol: "SMPL".into(),
            event_ts: at(10, 1, 20, 0),
            receive_ts: None,
            source: Source::Fixture,
            exchange: "Z".into(),
            tape: Tape::C,
            trade_id: 9,
            price: Price::from_cents(319),
            size: 300,
            action: CancelAction::Cancel,
        }),
        trade(at(10, 1, 30, 0), 321, 200, 10, "K", &["@"]).into(), // D, corrected below
        MarketEvent::Correction(Correction {
            symbol: "SMPL".into(),
            event_ts: at(10, 1, 40, 0),
            receive_ts: None,
            source: Source::Fixture,
            exchange: "K".into(),
            tape: Tape::C,
            original_id: 10,
            original_price: Price::from_cents(321),
            original_size: 200,
            original_conditions: vec!["@".into()],
            corrected_id: 11,
            corrected_price: Price::from_cents(322),
            corrected_size: 200,
            corrected_conditions: vec!["@".into()],
        }),
        MarketEvent::Status(Status {
            symbol: Some("SMPL".into()),
            event_ts: at(10, 2, 0, 0),
            receive_ts: None,
            source: Source::Fixture,
            kind: StatusKind::Halt,
            code: "H".into(),
            reason: "LUDP".into(),
            tape: Tape::C,
        }),
        trade(at(10, 3, 0, 0), 325, 100, 12, "V", &["@"]).into(), // E: after the clock
    ]
}

fn clock(now: UnixNanos) -> ClockState {
    ClockState {
        mode: ClockMode::Replay,
        now,
        speed: Speed::Times(10),
        playing: false,
        day: Some(day()),
        start: None,
        end: None,
        finished: false,
        events: 0,
    }
}

struct Fixture {
    _tmp: tempfile::TempDir,
    sim: Sim,
    clock: watch::Sender<ClockState>,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let archive = Archive::new(ArchiveConfig {
        root: tmp.path().join("archive"),
        retention_days: 0,
        min_free_bytes: 0,
    });
    let mut w = archive.writer(day(), "SMPL", FsyncPolicy::Never).unwrap();
    for e in events() {
        w.append(&e).unwrap();
    }
    archive.commit(day(), w.finalize().unwrap()).unwrap();
    let (tx, rx) = watch::channel(clock(at(10, 2, 30, 0)));
    let sim = Sim::new(
        SimConfig::default(),
        archive,
        day(),
        vec!["SMPL".into()],
        true,
        rx,
    );
    Fixture {
        _tmp: tmp,
        sim,
        clock: tx,
    }
}

async fn get(sim: &Sim, uri: &str, auth: bool) -> (StatusCode, Value, Option<String>) {
    let mut req = Request::get(uri);
    if auth {
        req = req
            .header("APCA-API-KEY-ID", "sim-key")
            .header("APCA-API-SECRET-KEY", "sim-secret");
    }
    let res = sim
        .router()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let fixture = res
        .headers()
        .get("x-stockwatcher-fixture")
        .map(|v| v.to_str().unwrap().to_string());
    let body = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap(), fixture)
}

#[tokio::test]
async fn rest_requires_dummy_credentials() {
    let f = fixture();
    let (s, v, _) = get(&f.sim, "/v2/stocks/SMPL/trades", false).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(
        v,
        json!({"code": 40110000, "message": "request is not authorized"})
    );
    // HTTP Basic works too.
    let res = f
        .sim
        .router()
        .oneshot(
            Request::get("/v2/assets")
                .header("Authorization", "Basic c2ltLWtleTpzaW0tc2VjcmV0")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let (s, _, _) = get(&f.sim, "/v2/stocks/SMPL/quotes", true).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "not implemented routes are 404");
}

#[tokio::test]
async fn trades_golden() {
    let f = fixture();
    let (s, v, _) = get(&f.sim, "/v2/stocks/SMPL/trades", true).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        v,
        json!({
            "trades": [
                {"t":"2026-10-15T12:00:00Z","x":"Q","p":3.1,"s":100,"c":["@","T"],"i":2,"z":"C"},
                {"t":"2026-10-15T14:00:01.250Z","x":"V","p":3.18,"s":100,"c":["@"],"i":7,"z":"C"},
                {"t":"2026-10-15T14:00:30.500Z","x":"P","p":3.2,"s":50,"c":["@","I"],"i":8,"z":"C"},
                {"t":"2026-10-15T14:01:05Z","x":"Z","p":3.19,"s":300,"c":["@"],"i":9,"z":"C","u":"canceled"},
                {"t":"2026-10-15T14:01:30Z","x":"K","p":3.21,"s":200,"c":["@"],"i":10,"z":"C","u":"incorrect"},
                {"t":"2026-10-15T14:01:30Z","x":"K","p":3.22,"s":200,"c":["@"],"i":11,"z":"C","u":"corrected"}
            ],
            "symbol": "SMPL",
            "currency": "USD",
            "next_page_token": null
        })
    );

    // Paging: 4 per page, token continues; desc sort reverses.
    let (_, p1, _) = get(&f.sim, "/v2/stocks/SMPL/trades?limit=4", true).await;
    assert_eq!(p1["trades"].as_array().unwrap().len(), 4);
    let tok = p1["next_page_token"].as_str().unwrap().to_string();
    // Even if the clock moves on, page 2 sees the same set.
    f.clock.send_replace(clock(at(11, 0, 0, 0)));
    let (_, p2, _) = get(
        &f.sim,
        &format!("/v2/stocks/SMPL/trades?limit=4&page_token={tok}"),
        true,
    )
    .await;
    assert_eq!(p2["trades"].as_array().unwrap().len(), 2);
    assert_eq!(p2["next_page_token"], Value::Null);
    let (_, d, _) = get(&f.sim, "/v2/stocks/SMPL/trades?sort=desc&limit=1", true).await;
    assert_eq!(
        d["trades"][0]["i"], 12,
        "after the clock moved, E is visible"
    );

    // BOATS feed: overnight only. Unknown symbol: empty.
    let (_, b, _) = get(
        &f.sim,
        "/v2/stocks/SMPL/trades?feed=boats&start=2026-10-14",
        true,
    )
    .await;
    assert_eq!(
        b["trades"],
        json!([{"t":"2026-10-15T01:00:00Z","x":"Q","p":3.0,"s":200,"c":["@","T"],"i":1,"z":"C"}])
    );
    let (_, u, _) = get(&f.sim, "/v2/stocks/NOPE/trades", true).await;
    assert_eq!(u["trades"], json!([]));
    let (s, e, _) = get(&f.sim, "/v2/stocks/SMPL/trades?limit=0", true).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(e["code"], 40010001);
}

#[tokio::test]
async fn no_future_data_leaks() {
    let f = fixture();
    // Asking for an end after the clock is clamped to the clock.
    let (_, v, _) = get(
        &f.sim,
        "/v2/stocks/SMPL/trades?end=2026-10-15T20:00:00Z",
        true,
    )
    .await;
    assert!(v["trades"].as_array().unwrap().iter().all(|t| t["i"] != 12));
    f.clock.send_replace(clock(at(9, 0, 0, 0)));
    let (_, v, _) = get(&f.sim, "/v2/stocks/snapshots?symbols=SMPL", true).await;
    assert_eq!(v["SMPL"]["latestTrade"]["i"], 2);
    assert_eq!(v["SMPL"]["latestQuote"], Value::Null);
    assert_eq!(v["SMPL"]["dailyBar"], Value::Null);
}

#[tokio::test]
async fn bars_golden_one_minute_only() {
    let f = fixture();
    let (s, v, _) = get(&f.sim, "/v2/stocks/SMPL/bars?timeframe=1Min", true).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        v,
        json!({
            "bars": [
                {"t":"2026-10-15T12:00:00Z","o":3.1,"h":3.1,"l":3.1,"c":3.1,"v":100,"n":1,"vw":3.1},
                // A (3.18×100) sets OHLC; the odd lot B adds volume only.
                {"t":"2026-10-15T14:00:00Z","o":3.18,"h":3.18,"l":3.18,"c":3.18,"v":150,"n":2,"vw":3.186667},
                // C cancelled, D corrected 3.21 → 3.22.
                {"t":"2026-10-15T14:01:00Z","o":3.22,"h":3.22,"l":3.22,"c":3.22,"v":200,"n":1,"vw":3.22}
            ],
            "symbol": "SMPL",
            "currency": "USD",
            "next_page_token": null
        })
    );
    for bad in ["15Sec", "5Min", "1Day"] {
        let (s, _, _) = get(
            &f.sim,
            &format!("/v2/stocks/SMPL/bars?timeframe={bad}"),
            true,
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{bad}");
    }
}

#[tokio::test]
async fn snapshots_golden() {
    let f = fixture();
    let (s, v, _) = get(&f.sim, "/v2/stocks/snapshots?symbols=SMPL,NOPE", true).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        v,
        json!({
            "SMPL": {
                "latestTrade": {"t":"2026-10-15T14:01:30Z","x":"K","p":3.21,"s":200,"c":["@"],"i":10,"z":"C"},
                "latestQuote": {"t":"2026-10-15T14:00:02Z","ax":"V","ap":3.19,"as":5,"bx":"Q","bp":3.17,"bs":3,"c":["R"],"z":"C"},
                "minuteBar": {"t":"2026-10-15T14:01:00Z","o":3.22,"h":3.22,"l":3.22,"c":3.22,"v":200,"n":1,"vw":3.22},
                "dailyBar": {"t":"2026-10-15T04:00:00Z","o":3.18,"h":3.22,"l":3.18,"c":3.22,"v":350,"n":3,"vw":3.205714},
                "prevDailyBar": null
            }
        })
    );
}

#[tokio::test]
async fn assets_golden() {
    let f = fixture();
    let (_, v, _) = get(
        &f.sim,
        "/v2/assets?status=active&asset_class=us_equity",
        true,
    )
    .await;
    let a = &v[0];
    let id = a["id"].as_str().unwrap();
    assert_eq!(id.len(), 36);
    assert_eq!(id.matches('-').count(), 4);
    let mut a = a.clone();
    a["id"] = json!("<uuid>");
    assert_eq!(
        a,
        json!({
            "id": "<uuid>", "class": "us_equity", "exchange": "NASDAQ", "symbol": "SMPL",
            "name": "SMPL Fixture Inc. (simulator fixture, not a real security)",
            "status": "active", "tradable": true, "marginable": false,
            "maintenance_margin_requirement": 100, "margin_requirement_long": "100",
            "margin_requirement_short": "100", "shortable": false, "easy_to_borrow": false,
            "fractionable": false, "attributes": []
        })
    );
    let (_, none, _) = get(&f.sim, "/v2/assets?asset_class=crypto", true).await;
    assert_eq!(none, json!([]));
}

#[tokio::test]
async fn universe_helper_filters_by_price_at_the_clock() {
    let f = fixture();
    let (_, v, _) = get(&f.sim, "/sim/universe", true).await;
    assert_eq!(v["simulatorOnly"], true);
    assert_eq!(
        v["symbols"],
        json!([{"symbol":"SMPL","price":3.21,"inBand":true}])
    );
    let (_, v, _) = get(&f.sim, "/sim/universe?min=5&max=25", true).await;
    assert_eq!(v["symbols"][0]["inBand"], false);
}

#[tokio::test]
async fn fmp_shares_float_golden_shape_and_fixture_marker() {
    let f = fixture();
    let (s, v, fx) = get(
        &f.sim,
        "/stable/shares-float?symbol=SMPL&apikey=sim-fmp-key",
        false,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(fx.as_deref(), Some("true"));
    let row = v[0].as_object().unwrap();
    let mut keys: Vec<_> = row.keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "date",
            "floatShares",
            "freeFloat",
            "outstandingShares",
            "symbol"
        ]
    );
    assert_eq!(row["symbol"], "SMPL");
    assert_eq!(row["date"], "2026-10-15 00:00:00");
    let out = row["outstandingShares"].as_u64().unwrap();
    let fl = row["floatShares"].as_u64().unwrap();
    assert!(fl < out && fl > out / 2);
    let (_, again, _) = get(
        &f.sim,
        "/stable/shares-float?symbol=SMPL&apikey=sim-fmp-key",
        false,
    )
    .await;
    assert_eq!(v, again, "deterministic");
    let (s, _, _) = get(
        &f.sim,
        "/stable/shares-float?symbol=SMPL&apikey=wrong",
        false,
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (_, empty, _) = get(
        &f.sim,
        "/stable/shares-float?symbol=NOPE&apikey=sim-fmp-key",
        false,
    )
    .await;
    assert_eq!(empty, json!([]));
}

// --- WebSocket ---------------------------------------------------------------

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn serve(sim: &Sim) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = sim.router();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("ws://{addr}")
}

async fn recv(ws: &mut Ws) -> Value {
    loop {
        let m = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
            .await
            .expect("timed out waiting for a frame")
            .expect("stream ended")
            .unwrap();
        match m {
            Message::Text(t) => return serde_json::from_str(&t).unwrap(),
            Message::Close(_) => panic!("closed"),
            _ => {}
        }
    }
}

/// One data message: frames are arrays and may coalesce several messages.
async fn one(ws: &mut Ws, q: &mut std::collections::VecDeque<Value>) -> Value {
    while q.is_empty() {
        q.extend(recv(ws).await.as_array().unwrap().iter().cloned());
    }
    q.pop_front().unwrap()
}

async fn send(ws: &mut Ws, v: Value) {
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}

async fn connect(base: &str, path: &str) -> Ws {
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("{base}{path}"))
        .await
        .unwrap();
    assert_eq!(
        recv(&mut ws).await,
        json!([{"T":"success","msg":"connected"}])
    );
    ws
}

#[tokio::test]
async fn sip_stream_protocol_and_message_shapes() {
    let f = fixture();
    let base = serve(&f.sim).await;

    // Wrong key: 402 then close.
    let mut bad = connect(&base, "/v2/sip").await;
    send(
        &mut bad,
        json!({"action":"auth","key":"real-looking","secret":"nope"}),
    )
    .await;
    assert_eq!(
        recv(&mut bad).await,
        json!([{"T":"error","code":402,"msg":"auth failed"}])
    );

    let mut ws = connect(&base, "/v2/sip").await;
    send(&mut ws, json!({"action":"subscribe","trades":["SMPL"]})).await;
    assert_eq!(
        recv(&mut ws).await,
        json!([{"T":"error","code":401,"msg":"not authenticated"}])
    );
    ws.send(Message::Text("not json".into())).await.unwrap();
    assert_eq!(
        recv(&mut ws).await,
        json!([{"T":"error","code":400,"msg":"invalid syntax"}])
    );
    send(
        &mut ws,
        json!({"action":"auth","key":"sim-key","secret":"sim-secret"}),
    )
    .await;
    assert_eq!(
        recv(&mut ws).await,
        json!([{"T":"success","msg":"authenticated"}])
    );
    send(
        &mut ws,
        json!({"action":"auth","key":"sim-key","secret":"sim-secret"}),
    )
    .await;
    assert_eq!(
        recv(&mut ws).await,
        json!([{"T":"error","code":403,"msg":"already authenticated"}])
    );
    send(
        &mut ws,
        json!({"action":"subscribe","trades":["smpl"],"quotes":["SMPL"],"bars":["*"],"statuses":["*"]}),
    )
    .await;
    assert_eq!(
        recv(&mut ws).await,
        json!([{"T":"subscription","trades":["SMPL"],"quotes":["SMPL"],"bars":["*"],"updatedBars":[],"dailyBars":[],"statuses":["*"],"lulds":[],"corrections":["SMPL"],"cancelErrors":["SMPL"]}])
    );

    let mut boats = connect(&base, "/v1beta1/boats").await;
    send(
        &mut boats,
        json!({"action":"auth","key":"sim-key","secret":"sim-secret"}),
    )
    .await;
    recv(&mut boats).await;
    send(&mut boats, json!({"action":"subscribe","trades":["*"]})).await;
    recv(&mut boats).await;

    let mut q = std::collections::VecDeque::new();
    // Feed the replay items as the server's ingest task would.
    let ev = events();
    let feed = |item: FeedItem| f.sim.ingest(&item);
    feed(FeedItem::Advance(UnixNanos(ev[0].event_ts().nanos())));
    feed(FeedItem::Event(ev[0].clone())); // overnight: BOATS only
    assert_eq!(
        one(&mut boats, &mut q).await,
        json!({"T":"t","S":"SMPL","i":1,"x":"Q","p":3.0,"s":200,"c":["@","T"],"t":"2026-10-15T01:00:00Z","z":"C"})
    );
    for e in &ev[2..4] {
        feed(FeedItem::Event(e.clone())); // A trade, then the quote
    }
    assert_eq!(
        one(&mut ws, &mut q).await,
        json!({"T":"t","S":"SMPL","i":7,"x":"V","p":3.18,"s":100,"c":["@"],"t":"2026-10-15T14:00:01.250Z","z":"C"})
    );
    assert_eq!(
        one(&mut ws, &mut q).await,
        json!({"T":"q","S":"SMPL","bx":"Q","bp":3.17,"bs":3,"ax":"V","ap":3.19,"as":5,"c":["R"],"t":"2026-10-15T14:00:02Z","z":"C"})
    );
    feed(FeedItem::Event(ev[4].clone())); // odd lot
    one(&mut ws, &mut q).await;
    feed(FeedItem::Advance(at(10, 1, 0, 0))); // the 10:00 minute closes
    assert_eq!(
        one(&mut ws, &mut q).await,
        json!({"T":"b","S":"SMPL","o":3.18,"h":3.18,"l":3.18,"c":3.18,"v":150,"n":2,"vw":3.186667,"t":"2026-10-15T14:00:00Z"})
    );
    feed(FeedItem::Event(ev[5].clone())); // C
    one(&mut ws, &mut q).await;
    feed(FeedItem::Event(ev[6].clone())); // cancel of C
    assert_eq!(
        one(&mut ws, &mut q).await,
        json!({"T":"x","S":"SMPL","i":9,"x":"Z","p":3.19,"s":300,"a":"C","t":"2026-10-15T14:01:20Z","z":"C"})
    );
    feed(FeedItem::Event(ev[7].clone())); // D
    one(&mut ws, &mut q).await;
    feed(FeedItem::Event(ev[8].clone())); // correction of D
    assert_eq!(
        one(&mut ws, &mut q).await,
        json!({"T":"c","S":"SMPL","x":"K","oi":10,"op":3.21,"os":200,"oc":["@"],"ci":11,"cp":3.22,"cs":200,"cc":["@"],"t":"2026-10-15T14:01:40Z","z":"C"})
    );
    feed(FeedItem::Advance(at(10, 2, 0, 0)));
    feed(FeedItem::Event(ev[9].clone())); // halt
    // The 10:01 bar and the status arrive in order (possibly coalesced into one frame).
    let got = vec![one(&mut ws, &mut q).await, one(&mut ws, &mut q).await];
    assert_eq!(
        got,
        vec![
            json!({"T":"b","S":"SMPL","o":3.22,"h":3.22,"l":3.22,"c":3.22,"v":200,"n":1,"vw":3.22,"t":"2026-10-15T14:01:00Z"}),
            json!({"T":"s","S":"SMPL","sc":"H","sm":"Trading Halt","rc":"LUDP","rm":"Volatility Trading Pause","t":"2026-10-15T14:02:00Z","z":"C"}),
        ]
    );

    // Unsubscribe echoes the remaining lists.
    send(
        &mut ws,
        json!({"action":"unsubscribe","quotes":["SMPL"],"bars":["*"]}),
    )
    .await;
    assert_eq!(
        recv(&mut ws).await,
        json!([{"T":"subscription","trades":["SMPL"],"quotes":[],"bars":[],"updatedBars":[],"dailyBars":[],"statuses":["*"],"lulds":[],"corrections":["SMPL"],"cancelErrors":["SMPL"]}])
    );
}

#[tokio::test]
async fn news_stream_accepts_subscriptions_and_header_auth() {
    let f = fixture();
    let base = serve(&f.sim).await;
    let mut req = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(
        format!("{base}/v1beta1/news"),
    )
    .unwrap();
    req.headers_mut()
        .insert("APCA-API-KEY-ID", "sim-key".parse().unwrap());
    req.headers_mut()
        .insert("APCA-API-SECRET-KEY", "sim-secret".parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    assert_eq!(
        recv(&mut ws).await,
        json!([{"T":"success","msg":"connected"}])
    );
    assert_eq!(
        recv(&mut ws).await,
        json!([{"T":"success","msg":"authenticated"}])
    );
    send(&mut ws, json!({"action":"subscribe","news":["*"]})).await;
    assert_eq!(
        recv(&mut ws).await,
        json!([{"T":"subscription","news":["*"]}])
    );
    send(&mut ws, json!({"action":"subscribe","trades":["SMPL"]})).await;
    assert_eq!(
        recv(&mut ws).await,
        json!([{"T":"error","code":400,"msg":"invalid syntax"}])
    );
}
