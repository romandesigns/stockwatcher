//! The live market over the archive (crates/server/src/history.rs): what one run records, the
//! next run loads back per symbol, lazily, into the same charts; a closed market shows the last
//! session; history never reaches the paper broker.

use std::path::Path;
use std::time::{Duration, Instant};

use chart_engine::{ChartEngine, Timeframe};
use futures_util::{SinkExt, StreamExt};
use market_archive::{Archive, ArchiveConfig, LiveWriter};
use market_events::{MarketEvent, Price, Source as EventSource, Trade, UnixNanos};
use market_providers::recorder::receive_time;
use market_providers::{
    ChannelFeed, LiveOptions, LiveProvider, MarketDataProvider, RecorderConfig,
};
use market_replay::{ArchiveDaySource, EventSource as _, Merger};
use serde_json::{Value, json};
use stockwatcher_server::{AppState, Market, router};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn day() -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(2026, 10, 15).unwrap()
}

fn et(h: u32, m: u32) -> UnixNanos {
    market_events::session::et(day(), h, m, 0, 0).unwrap()
}

const SYMS: [&str; 2] = ["SMPL", "FXMD"];

fn archive(root: &Path) -> Archive {
    Archive::new(ArchiveConfig {
        root: root.to_path_buf(),
        retention_days: 0,
        min_free_bytes: 0,
    })
}

/// The fixture day (from a scratch archive) in delivery order, stamped with the delivery time as
/// the receive time, as the live provider stamps them.
fn fixture_events(scratch: &Path) -> Vec<MarketEvent> {
    let a = archive(scratch);
    let syms: Vec<String> = SYMS.iter().map(|s| s.to_string()).collect();
    market_archive::fixture::write_fixture_day(&a, day(), &syms, 7).unwrap();
    let source = ArchiveDaySource::new(a, day()).unwrap();
    let mut m = Merger::new(source.open().unwrap());
    let mut out = Vec::new();
    while let Some((d, mut e)) = m.pop() {
        let r = Some(UnixNanos(d));
        match &mut e {
            MarketEvent::Trade(t) => t.receive_ts = r,
            MarketEvent::Quote(q) => q.receive_ts = r,
            MarketEvent::Correction(c) => c.receive_ts = r,
            MarketEvent::Cancel(c) => c.receive_ts = r,
            MarketEvent::Status(s) => s.receive_ts = r,
        }
        out.push(e);
    }
    out
}

/// A live market recording into `root`, as if started at `now`. The sender is the stream.
fn live(root: &Path, now: UnixNanos, preload: bool) -> (Market, mpsc::Sender<MarketEvent>) {
    let (tx, rx) = mpsc::channel(4096);
    let mut rec = RecorderConfig::new(archive(root).config().clone());
    rec.today = Some(day());
    let session = Box::new(LiveProvider::with_feed(
        Box::new(ChannelFeed {
            label: "live test".into(),
            symbols: Vec::new(),
            rx,
        }),
        LiveOptions {
            idle_tick: None,
            record: Some(rec),
            ..LiveOptions::default()
        },
    ))
    .start()
    .unwrap();
    (
        Market::from_live_session(session, archive(root), now, preload),
        tx,
    )
}

/// Waits until the recording holds `n` events of the symbols.
async fn recorded(root: &Path, n: usize) {
    let a = archive(root);
    for _ in 0..1000 {
        let have: usize = SYMS
            .iter()
            .map(|s| {
                a.live_reader(day(), s, None)
                    .map(|r| r.map_while(Result::ok).count())
                    .unwrap_or(0)
            })
            .sum();
        if have == n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the recording never reached {n} events");
}

async fn until(what: &str, f: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !f() {
        assert!(Instant::now() < deadline, "never: {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn serve(market: Market) -> (String, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let app = router(AppState::with_market(None, market), tmp.path(), tmp.path());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("ws://{addr}/ws"), tmp)
}

async fn send(ws: &mut Ws, v: Value) {
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}

async fn next(ws: &mut Ws, pred: impl Fn(&Value) -> bool) -> Value {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Message::Text(t) = ws.next().await.expect("stream ended").unwrap() {
                let v: Value = serde_json::from_str(&t).unwrap();
                if pred(&v) {
                    return v;
                }
            }
        }
    })
    .await
    .expect("timed out waiting for a matching frame")
}

async fn connect(url: &str) -> Ws {
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    send(
        &mut ws,
        json!({"type":"hello","client":"test","source":"live"}),
    )
    .await;
    next(&mut ws, |m| m["type"] == "welcome").await;
    ws
}

/// The same events fed to one engine with the live ingest's marks: what a market that never
/// restarted would show.
fn eager(events: &[MarketEvent]) -> ChartEngine {
    let mut e = ChartEngine::default();
    let mut mark = i64::MIN;
    for ev in events {
        let m = receive_time(ev).0.div_euclid(1_000_000_000) * 1_000_000_000;
        if m > mark {
            mark = m;
            e.advance_clock(UnixNanos(m));
        }
        e.apply(ev);
    }
    e
}

fn candles(snap: &Value) -> Value {
    json!({"candles": snap["candles"], "gaps": snap["gaps"], "profile": snap["profile"], "halts": snap["halts"]})
}

/// The owner's case. The server ran all morning recording into the archive; a deploy restarts it
/// at 11:00; the stream goes on. A dashboard subscribes to a chart: an empty one at once (its
/// history is loading), then the real one, equal to what a server that never restarted shows,
/// afternoon prints included, and the paper account never saw the history.
#[tokio::test(flavor = "multi_thread")]
async fn after_a_restart_a_chart_loads_its_day_from_the_archive_and_matches_an_unbroken_run() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("archive");
    let events = fixture_events(&tmp.path().join("scratch"));
    let split = events
        .iter()
        .position(|e| receive_time(e) >= et(11, 0))
        .unwrap();
    let cut = events
        .iter()
        .position(|e| receive_time(e) >= et(12, 0))
        .unwrap();

    // The morning, recorded.
    let (before, tx) = live(&root, et(4, 0), true);
    for e in &events[..split] {
        tx.send(e.clone()).await.unwrap();
    }
    drop(tx);
    until("the first run ended", || {
        before.state_json()["status"]["state"] == "stopped"
    })
    .await;
    before.checkpoint_quotes();
    drop(before);
    recorded(&root, split).await;
    let a = archive(&root);
    until("the quote rows were saved", || {
        a.read_live_quotes(day()).ok().flatten().is_some()
    })
    .await;

    // The restart at 11:00, no background preload: only what a chart asks for loads.
    let started = Instant::now();
    let (after, tx) = live(&root, et(11, 0), false);
    assert!(started.elapsed() < Duration::from_secs(2));
    let st = after.state_json();
    assert_eq!(st["status"]["state"], "running", "up at once: {st}");
    assert_eq!(st["history"]["days"], json!(["2026-10-15"]));
    assert_eq!(st["history"]["pending"], 2, "nothing loaded yet");
    // The board is back from the saved rows: both are candidates before anything loaded.
    let mut syms = after.symbols();
    syms.sort();
    assert_eq!(syms, ["FXMD", "SMPL"]);
    let (url, _srv) = serve(after.clone()).await;

    // The stream carries on: 11:00 to 12:00, while neither symbol is loaded.
    for e in &events[split..cut] {
        tx.send(e.clone()).await.unwrap();
    }
    until("the afternoon was ingested", || {
        after.state_json()["seq"].as_u64() >= Some(1 + (cut - split) as u64)
    })
    .await;

    let mut ws = connect(&url).await;
    send(
        &mut ws,
        json!({"type":"chart.subscribe","id":"d0","symbol":"FXMD","timeframe":"1m","profileBin":0.01}),
    )
    .await;
    let first = next(&mut ws, |m| m["type"] == "chart.snapshot").await;
    assert_eq!(
        first["candles"],
        json!([]),
        "loading: an empty chart, not a refusal"
    );
    let loaded = next(&mut ws, |m| {
        m["type"] == "chart.snapshot" && !m["candles"].as_array().unwrap().is_empty()
    })
    .await;
    assert_eq!(
        after.state_json()["history"]["pending"],
        1,
        "only FXMD loaded"
    );

    // What an unbroken run shows for FXMD at 12:00.
    let reference = eager(&events[..cut]);
    let want = reference.snapshot("FXMD", Timeframe::M1, None).unwrap();
    let want = serde_json::to_value(&want).unwrap();
    assert_eq!(candles(&loaded), candles(&want));

    // More prints after the load go straight into the loaded chart.
    for e in &events[cut..] {
        tx.send(e.clone()).await.unwrap();
    }
    drop(tx);
    until("the rest was ingested", || {
        after.state_json()["status"]["state"] == "stopped"
    })
    .await;
    // SMPL loads on request too; then both equal the unbroken run over the whole day.
    assert!(after.load_history("SMPL").is_some());
    let full = eager(&events);
    for sym in SYMS {
        let (s, _) = (after.snapshot(sym, Timeframe::M5, Price::from_cents(1)), ());
        let got: Value = serde_json::from_str(&s.unwrap().json).unwrap();
        let want = serde_json::to_value(full.snapshot(sym, Timeframe::M5, None).unwrap()).unwrap();
        assert_eq!(candles(&got), candles(&want), "{sym}");
    }
    let paper: Value = serde_json::from_str(&after.paper_state_frame()).unwrap();
    assert_eq!(paper["orders"], json!([]));
    assert_eq!(paper["fills"], json!([]));
}

/// With the background preload every symbol loads by itself, and the charts equal an unbroken
/// run's at every timeframe.
#[tokio::test(flavor = "multi_thread")]
async fn the_background_preload_loads_every_symbol_to_the_same_charts() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("archive");
    let events = fixture_events(&tmp.path().join("scratch"));
    let split = events
        .iter()
        .position(|e| receive_time(e) >= et(11, 0))
        .unwrap();
    let (before, tx) = live(&root, et(4, 0), true);
    for e in &events[..split] {
        tx.send(e.clone()).await.unwrap();
    }
    drop(tx);
    until("the first run ended", || {
        before.state_json()["status"]["state"] == "stopped"
    })
    .await;
    drop(before);
    recorded(&root, split).await;

    let (after, _tx) = live(&root, et(11, 0), true);
    until("every symbol loaded", || {
        after.state_json()["history"]["pending"] == 0
    })
    .await;
    let reference = eager(&events[..split]);
    for sym in SYMS {
        for tf in Timeframe::ALL {
            let got: Value =
                serde_json::from_str(&after.snapshot(sym, tf, Price::from_cents(1)).unwrap().json)
                    .unwrap();
            let want = serde_json::to_value(reference.snapshot(sym, tf, None).unwrap()).unwrap();
            assert_eq!(candles(&got), candles(&want), "{sym} {tf}");
            assert_eq!(got["quality"], want["quality"], "{sym} {tf}");
        }
    }
}

/// A weekend: nothing trades, and the dashboard shows the last session the archive has,
/// labelled with its date, instead of "No live trades yet". A fixture day is never that session.
#[tokio::test(flavor = "multi_thread")]
async fn a_closed_market_shows_the_last_session_labelled_with_its_date() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("archive");
    let a = archive(&root);
    let thursday = day();
    let friday = chrono::NaiveDate::from_ymd_opt(2026, 10, 16).unwrap();
    // Friday is only a fixture day (the replay's synthetic practice data): skipped...
    market_archive::fixture::write_fixture_day(&a, friday, &["FXLO".to_string()], 7).unwrap();
    // ...so Thursday as the import left it (final, SIP) is the last session. Built from the
    // fixture generator's prints, relabelled as SIP market data.
    let scratch = archive(&tmp.path().join("scratch"));
    market_archive::fixture::write_fixture_day(
        &scratch,
        thursday,
        &["SMPL".to_string(), "FXMD".to_string()],
        7,
    )
    .unwrap();
    for sym in ["SMPL", "FXMD"] {
        let mut w = a
            .writer(thursday, sym, market_archive::FsyncPolicy::Never)
            .unwrap();
        for r in scratch.reader(thursday, sym).unwrap() {
            let mut e = r.unwrap().event;
            match &mut e {
                MarketEvent::Trade(t) => t.source = EventSource::AlpacaSip,
                MarketEvent::Quote(q) => q.source = EventSource::AlpacaSip,
                MarketEvent::Correction(c) => c.source = EventSource::AlpacaSip,
                MarketEvent::Cancel(c) => c.source = EventSource::AlpacaSip,
                MarketEvent::Status(s) => s.source = EventSource::AlpacaSip,
            }
            w.append(&e).unwrap();
        }
        a.commit(thursday, w.finalize().unwrap()).unwrap();
    }
    let sunday_noon = market_events::session::et(
        chrono::NaiveDate::from_ymd_opt(2026, 10, 18).unwrap(),
        12,
        0,
        0,
        0,
    )
    .unwrap();
    let (m, _tx) = live(&root, sunday_noon, true);
    until("Thursday loaded", || {
        m.state_json()["history"]["pending"] == 0
    })
    .await;
    let st = m.symbols_json();
    assert_eq!(
        st["session"],
        json!({"day": "2026-10-15", "current": false, "provisional": false})
    );
    let mut syms: Vec<String> = serde_json::from_value(st["symbols"].clone()).unwrap();
    syms.sort();
    assert_eq!(syms, ["FXMD", "SMPL"], "no fixture symbol: {st}");
    let snap: Value = serde_json::from_str(
        &m.snapshot("SMPL", Timeframe::M5, Price::from_cents(1))
            .unwrap()
            .json,
    )
    .unwrap();
    assert!(snap["candles"].as_array().unwrap().len() > 50);
    assert_eq!(snap["session"]["id"], "2026-10-15/after");

    // A provisional (recorded) last session says so.
    let tmp2 = tempfile::tempdir().unwrap();
    let root2 = tmp2.path().join("archive");
    let a2 = archive(&root2);
    let mut w = LiveWriter::open(&a2, friday, 0, "test").unwrap();
    let t = market_events::session::et(friday, 15, 59, 0, 0).unwrap();
    let mut tr = Trade::new("SMPL", t, Price::from_cents(512), 100, 1);
    tr.source = EventSource::AlpacaSip;
    tr.receive_ts = Some(t);
    w.append(&MarketEvent::Trade(tr)).unwrap();
    w.finish().unwrap();
    let (m2, _tx2) = live(&root2, sunday_noon, true);
    until("loaded", || m2.state_json()["history"]["pending"] == 0).await;
    assert_eq!(
        m2.symbols_json()["session"],
        json!({"day": "2026-10-16", "current": false, "provisional": true})
    );
    assert_eq!(m2.symbols(), ["SMPL"]);
}

/// How fast the market comes back on a whole-market recording: startup to `running`, the first
/// chart of the busiest symbol, and the full background load. Synthetic (6,700 symbols, skewed
/// like the real market, a third odd lots) written straight into the archive's live layout.
/// Run on demand, in release:
///
/// `HISTORY_BENCH_EVENTS=5000000 cargo test --release -p stockwatcher-server --test live_history -- --ignored --nocapture`
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn measure_history_load() {
    let n: u64 = std::env::var("HISTORY_BENCH_EVENTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1_000_000);
    // Events per recorder flush: the recorder flushes every second, and an average whole-market
    // second is ~1,700 prints (assumed: ~40M a day over 6.5 h; see the docs).
    let per_flush: u64 = std::env::var("HISTORY_BENCH_FLUSH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1_700);
    let symbols = 6_700u64;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("archive");
    let a = archive(&root);
    let start = et(4, 0);
    let span = 7 * 3600 * 1_000_000_000i64;
    let written = Instant::now();
    let mut w = LiveWriter::open(&a, day(), 0, "bench").unwrap();
    let mut rows: std::collections::HashMap<String, market_archive::LiveQuote> =
        std::collections::HashMap::new();
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    for i in 0..n {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let u = (x >> 11) as f64 / (1u64 << 53) as f64;
        let s = ((symbols as f64) * u.powi(4)) as u64 % symbols;
        let ts = UnixNanos(start.0 + (span / n as i64) * i as i64);
        let cents = 100 + (s as i64 * 37) % 2400 + ((x >> 20) % 7) as i64;
        let size = if x.is_multiple_of(3) {
            1 + x % 99
        } else {
            100 * (1 + x % 5)
        };
        let sym = format!("S{s:04}");
        let mut t = Trade::new(sym.clone(), ts, Price::from_cents(cents), size, i + 1);
        t.source = EventSource::AlpacaSip;
        t.exchange = ["V", "Q", "N", "P", "D", "K"][(x % 6) as usize].into();
        t.conditions = if size < 100 {
            vec!["@".into(), "I".into()]
        } else {
            vec!["@".into()]
        };
        t.receive_ts = Some(UnixNanos(ts.0 + 3_000_000));
        let r = rows
            .entry(sym.clone())
            .or_insert(market_archive::LiveQuote {
                symbol: sym,
                day: day(),
                last: None,
                last_ts: 0,
                first: None,
                open: None,
                volume: 0,
                trades: 0,
            });
        r.volume += size;
        r.trades += 1;
        if size >= 100 {
            r.last = Some(t.price);
            r.last_ts = ts.0;
            r.first.get_or_insert(t.price);
        }
        w.append(&MarketEvent::Trade(t)).unwrap();
        // The recorder's cadence in simulated seconds: due frames (64 KiB) every second, and
        // every symbol's frame at the 10 s age limit (a wall-clock age would never be reached by
        // a benchmark running faster than the market).
        if (i + 1).is_multiple_of(per_flush * 10) {
            w.flush().unwrap();
        } else if (i + 1).is_multiple_of(per_flush) {
            w.flush_due().unwrap();
        }
    }
    w.finish().unwrap();
    a.write_live_quotes(day(), &rows.into_values().collect::<Vec<_>>())
        .unwrap();
    let size = |a: &Archive| -> u64 {
        std::fs::read_dir(a.live_dir(day()))
            .unwrap()
            .filter_map(|e| e.ok()?.metadata().ok())
            .map(|m| m.len())
            .sum()
    };
    let bytes = size(&a);
    let took = written.elapsed().as_secs_f64();
    eprintln!(
        "recording: {n} events, {symbols} symbols, {per_flush} events a simulated second: {:.1} MB ({:.1} B/event), written in {took:.1}s ({:.0} events/s)",
        bytes as f64 / 1e6,
        bytes as f64 / n as f64,
        n as f64 / took
    );

    let t0 = Instant::now();
    let (m, _tx) = live(&root, et(11, 0), true);
    let up = t0.elapsed();
    let st = m.state_json();
    eprintln!(
        "startup: running after {:.1} ms with {} candidates, {} symbols to load",
        up.as_secs_f64() * 1e3,
        st["symbols"].as_array().map_or(0, |a| a.len()),
        st["history"]["symbols"]
    );
    // The busiest symbol (S0000 by construction), as a chart would ask for it: a cold one, so
    // measure on a second market without preload to keep the background thread out of it.
    let (cold, _tx2) = live(&root, et(11, 0), false);
    let t1 = Instant::now();
    let read = cold.load_history("S0000").unwrap();
    eprintln!(
        "first chart: S0000 ({read} events) loaded in {:.0} ms",
        t1.elapsed().as_secs_f64() * 1e3
    );
    drop(cold);
    let deadline = Instant::now() + Duration::from_secs(3600);
    while m.state_json()["history"]["pending"] != 0 {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    eprintln!(
        "background: all {} symbols loaded {:.1} s after startup ({:.0} events/s)",
        st["history"]["symbols"],
        t0.elapsed().as_secs_f64(),
        n as f64 / t0.elapsed().as_secs_f64()
    );
    drop(m);
    let t2 = Instant::now();
    a.seal_live(day()).unwrap();
    let sealed = size(&a);
    eprintln!(
        "sealed (one frame per file): {:.1} MB ({:.1} B/event) in {:.1} s",
        sealed as f64 / 1e6,
        sealed as f64 / n as f64,
        t2.elapsed().as_secs_f64()
    );
}
