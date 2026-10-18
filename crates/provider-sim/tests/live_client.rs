//! The real Alpaca stream client (`market_providers::alpaca_stream`) against the simulator's
//! Alpaca-compatible WebSocket, end to end: auth, subscribe, normalization, refusal, reconnect,
//! and live-vs-replay parity of the engine output.
//!
//! Lives here rather than in market-providers because provider-sim already depends on it (a
//! dev-dependency the other way would be a cycle).

use chart_engine::wire::ChartUpdate;
use chart_engine::{ChartEngine, Timeframe};
use chrono::NaiveDate;
use market_archive::fixture::{fixture_symbol, symbol_params};
use market_archive::{Archive, ArchiveConfig, FsyncPolicy};
use market_events::conditions::Tape;
use market_events::session::{SessionKind, et};
use market_events::{
    Cancel, CancelAction, Correction, MarketEvent, Price, Source, Status, StatusKind, Trade,
    UnixNanos, classify,
};
use market_providers::alpaca_stream::{
    AlpacaStream, AlpacaStreamConfig, Backoff, LiveSymbols, ReceiveTime, StreamFeed, StreamStats,
};
use market_providers::import::alpaca::AlpacaCredentials;
use market_providers::{
    ClockMode, ClockState, FeedItem, LinkState, LiveOptions, LiveProvider, MarketDataProvider,
    ProviderSession, ReplayProvider, Speed,
};
use market_replay::{ArchiveDaySource, EventSource, ReplayOptions, day_bounds};
use provider_sim::{Sim, SimConfig};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, watch};

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 15).unwrap()
}

fn at(h: u32, m: u32, s: u32) -> UnixNanos {
    et(day(), h, m, s, 0).unwrap()
}

fn sim_creds() -> AlpacaCredentials {
    AlpacaCredentials {
        key_id: "sim-key".into(),
        secret: "sim-secret".into(),
    }
}

/// The simulator's router on an ephemeral port.
async fn serve(sim: &Sim) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = sim.router();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

/// A TCP forwarder in front of the simulator whose live connections can be cut at once, like a
/// network drop: the listener stays up, so the client can reconnect through it.
struct Proxy {
    addr: SocketAddr,
    cut: broadcast::Sender<()>,
}

async fn proxy(upstream: SocketAddr) -> Proxy {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (cut, _) = broadcast::channel::<()>(4);
    let c = cut.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut down, _)) = listener.accept().await else {
                return;
            };
            let mut stop = c.subscribe();
            tokio::spawn(async move {
                let Ok(mut up) = TcpStream::connect(upstream).await else {
                    return;
                };
                tokio::select! {
                    _ = tokio::io::copy_bidirectional(&mut down, &mut up) => {}
                    _ = stop.recv() => {} // both sockets drop here: an abrupt close
                }
            });
        }
    });
    Proxy { addr, cut }
}

fn fast_backoff() -> Backoff {
    Backoff {
        initial: Duration::from_millis(50),
        max: Duration::from_millis(200),
        stable_after: Duration::ZERO,
    }
}

fn client(
    base: String,
    symbols: LiveSymbols,
    creds: AlpacaCredentials,
    receive: ReceiveTime,
) -> (Box<AlpacaStream>, Arc<StreamStats>) {
    let mut c = AlpacaStreamConfig::new(base, StreamFeed::Sip, symbols, creds);
    c.backoff = fast_backoff();
    c.handshake_timeout = Duration::from_secs(5);
    c.receive_time = receive;
    let s = AlpacaStream::new(c);
    let stats = s.stats();
    (Box::new(s), stats)
}

fn start_live(feed: Box<AlpacaStream>) -> ProviderSession {
    Box::new(LiveProvider::with_feed(
        feed,
        LiveOptions {
            idle_tick: None,
            ..LiveOptions::default()
        },
    ))
    .start()
    .unwrap()
}

async fn next(s: &mut ProviderSession) -> FeedItem {
    tokio::time::timeout(Duration::from_secs(10), s.items.recv())
        .await
        .expect("timed out waiting for a feed item")
        .expect("feed ended")
}

/// Skips marks and returns the next link change.
async fn next_link(s: &mut ProviderSession) -> market_providers::Link {
    loop {
        match next(s).await {
            FeedItem::Link(l) => return l,
            FeedItem::Advance(_) => {}
            other => panic!("expected a link change, got {other:?}"),
        }
    }
}

async fn wait_up(s: &mut ProviderSession) -> market_providers::Link {
    loop {
        let l = next_link(s).await;
        if l.state == LinkState::Up {
            return l;
        }
    }
}

/// Skips marks and returns the next event.
async fn next_event(s: &mut ProviderSession) -> MarketEvent {
    loop {
        match next(s).await {
            FeedItem::Event(e) => return e,
            FeedItem::Advance(_) | FeedItem::Link(_) => {}
            other => panic!("expected an event, got {other:?}"),
        }
    }
}

fn clock(now: UnixNanos) -> ClockState {
    ClockState {
        mode: ClockMode::Replay,
        now,
        speed: Speed::Times(1),
        playing: false,
        day: Some(day()),
        start: None,
        end: None,
        finished: false,
        events: 0,
    }
}

/// A one-symbol simulator over an archive with one trade (the sim needs a day to serve).
fn small_sim() -> (tempfile::TempDir, Sim, watch::Sender<ClockState>) {
    let tmp = tempfile::tempdir().unwrap();
    let archive = Archive::new(ArchiveConfig {
        root: tmp.path().join("archive"),
        retention_days: 0,
        min_free_bytes: 0,
    });
    let mut w = archive.writer(day(), "SMPL", FsyncPolicy::Never).unwrap();
    w.append(&Trade::new("SMPL", at(9, 31, 0), Price::from_cents(300), 100, 1).into())
        .unwrap();
    archive.commit(day(), w.finalize().unwrap()).unwrap();
    let (tx, rx) = watch::channel(clock(at(10, 0, 0)));
    let sim = Sim::new(
        SimConfig::default(),
        archive,
        day(),
        vec!["SMPL".into()],
        true,
        rx,
    );
    (tmp, sim, tx)
}

fn trade(t: UnixNanos, cents: i64, size: u64, seq: u64, x: &str) -> Trade {
    Trade::new("SMPL", t, Price::from_cents(cents), size, seq)
        .with_exchange(x)
        .with_tape(Tape::C)
        .with_source(Source::Fixture)
        .with_conditions(["@"])
}

/// What the archive event becomes after a trip through the SIP stream: the source is the feed's,
/// and the client stamped a receive time. Everything else is unchanged.
fn as_live(e: &MarketEvent, got: &MarketEvent) -> MarketEvent {
    let mut e = e.clone();
    let r = match got {
        MarketEvent::Trade(t) => t.receive_ts,
        MarketEvent::Correction(c) => c.receive_ts,
        MarketEvent::Cancel(c) => c.receive_ts,
        MarketEvent::Status(s) => s.receive_ts,
        MarketEvent::Quote(q) => q.receive_ts,
    };
    assert!(r.is_some(), "receive time stamped");
    match &mut e {
        MarketEvent::Trade(t) => (t.source, t.receive_ts) = (Source::AlpacaSip, r),
        MarketEvent::Correction(c) => (c.source, c.receive_ts) = (Source::AlpacaSip, r),
        MarketEvent::Cancel(c) => (c.source, c.receive_ts) = (Source::AlpacaSip, r),
        MarketEvent::Status(s) => (s.source, s.receive_ts) = (Source::AlpacaSip, r),
        MarketEvent::Quote(q) => (q.source, q.receive_ts) = (Source::AlpacaSip, r),
    }
    e
}

#[tokio::test]
async fn client_authenticates_subscribes_normalizes_and_reconnects() {
    let (_tmp, sim, _clock) = small_sim();
    let upstream = serve(&sim).await;
    let px = proxy(upstream).await;
    let (feed, stats) = client(
        format!("ws://{}", px.addr),
        LiveSymbols::List(vec!["SMPL".into()]),
        sim_creds(),
        ReceiveTime::Wall,
    );
    let mut s = start_live(feed);
    assert_eq!(s.info.mode, ClockMode::Live);
    assert_eq!(s.info.symbols, vec!["SMPL".to_string()]);
    assert!(s.control.is_none());
    assert_eq!(next_link(&mut s).await.state, LinkState::Connecting);
    let up = wait_up(&mut s).await;
    let detail = up.detail.unwrap();
    assert!(
        detail.contains("trades=[SMPL]") && detail.contains("statuses=[SMPL]"),
        "{detail}"
    );

    // Every kind the client subscribes to, in order; the quote is not subscribed and never
    // arrives, and another symbol is filtered by the server.
    let events: Vec<MarketEvent> = vec![
        trade(at(10, 0, 1), 318, 100, 7, "V").into(),
        MarketEvent::Quote(market_events::Quote {
            symbol: "SMPL".into(),
            event_ts: at(10, 0, 2),
            receive_ts: None,
            bid_price: Price::from_cents(317),
            bid_size: 3,
            bid_exchange: "Q".into(),
            ask_price: Price::from_cents(319),
            ask_size: 5,
            ask_exchange: "V".into(),
            conditions: vec![],
            tape: Tape::C,
            source: Source::Fixture,
        }),
        Trade::new("OTHR", at(10, 0, 3), Price::from_cents(500), 1, 1).into(),
        trade(at(10, 1, 5), 319, 300, 9, "Z").into(),
        MarketEvent::Cancel(Cancel {
            symbol: "SMPL".into(),
            event_ts: at(10, 1, 20),
            receive_ts: None,
            source: Source::Fixture,
            exchange: "Z".into(),
            tape: Tape::C,
            trade_id: 9,
            price: Price::from_cents(319),
            size: 300,
            action: CancelAction::Cancel,
        }),
        MarketEvent::Correction(Correction {
            symbol: "SMPL".into(),
            event_ts: at(10, 1, 40),
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
            event_ts: at(10, 2, 0),
            receive_ts: None,
            source: Source::Fixture,
            kind: StatusKind::Halt,
            code: "H".into(),
            reason: "LUDP".into(),
            tape: Tape::C,
        }),
    ];
    for e in &events {
        sim.ingest(&FeedItem::Event(e.clone()));
    }
    for want in [&events[0], &events[3], &events[4], &events[5], &events[6]] {
        let got = next_event(&mut s).await;
        assert_eq!(got, as_live(want, &got));
    }

    // The network drops: the client notices, backs off, re-authenticates and re-subscribes.
    px.cut.send(()).unwrap();
    let down = next_link(&mut s).await;
    assert_eq!(down.state, LinkState::Down, "{down:?}");
    assert!(down.detail.unwrap().contains("reconnecting in"));
    wait_up(&mut s).await;
    let resume = MarketEvent::Status(Status {
        symbol: Some("SMPL".into()),
        event_ts: at(10, 3, 0),
        receive_ts: None,
        source: Source::Fixture,
        kind: StatusKind::Resume,
        code: "T".into(),
        reason: String::new(),
        tape: Tape::C,
    });
    let after: MarketEvent = trade(at(10, 3, 1), 325, 100, 12, "V").into();
    sim.ingest(&FeedItem::Event(resume.clone()));
    sim.ingest(&FeedItem::Event(after.clone()));
    let got = next_event(&mut s).await;
    assert_eq!(got, as_live(&resume, &got));
    let got = next_event(&mut s).await;
    assert_eq!(got, as_live(&after, &got));

    let st = stats.snapshot();
    assert_eq!(st.connects, 2);
    assert_eq!(st.subscribed, 2);
    assert_eq!(st.events, 7);
    assert_eq!((st.dropped, st.malformed, st.errors), (0, 0, 0));
}

#[tokio::test]
async fn refused_credentials_are_reported_and_retried_at_the_slow_pace() {
    let (_tmp, sim, _clock) = small_sim();
    let upstream = serve(&sim).await;
    let (feed, stats) = client(
        format!("ws://{upstream}"),
        LiveSymbols::All,
        AlpacaCredentials {
            key_id: "wrong".into(),
            secret: "wrong".into(),
        },
        ReceiveTime::Wall,
    );
    let mut s = start_live(feed);
    assert!(
        s.info.symbols.is_empty(),
        "* is the whole market, not a list"
    );
    let mut downs = Vec::new();
    while downs.len() < 2 {
        let l = next_link(&mut s).await;
        assert_ne!(l.state, LinkState::Up);
        if l.state == LinkState::Down {
            downs.push(l.detail.unwrap());
        }
    }
    for d in &downs {
        assert!(d.contains("402") && d.contains("ALPACA_API_KEY"), "{d}");
        assert!(!d.contains("wrong"), "never echoes the credentials: {d}");
    }
    let st = stats.snapshot();
    assert!(
        st.connects >= 2 && st.errors >= 2 && st.subscribed == 0,
        "{st:?}"
    );
}

// --- parity ------------------------------------------------------------------

/// Feeds items into an engine and keeps the tape rows it produced (as in market-providers'
/// tests/parity.rs).
struct Consumer {
    engine: ChartEngine,
    tape: Vec<ChartUpdate>,
}

impl Consumer {
    fn new() -> Self {
        Consumer {
            engine: ChartEngine::default(),
            tape: Vec::new(),
        }
    }

    fn take(&mut self, item: &FeedItem) {
        match item {
            FeedItem::Reset(_) => *self = Consumer::new(),
            FeedItem::Event(e) => self.tape.extend(
                self.engine
                    .apply(e)
                    .into_iter()
                    .filter(|u| matches!(u, ChartUpdate::Tape(_))),
            ),
            FeedItem::Advance(t) => {
                self.engine.advance_clock(*t);
            }
            _ => {}
        }
    }
}

/// Whether the simulator's SIP stream carries this event: overnight prints go to BOATS, quotes
/// are not subscribed, and market-wide statuses have no symbol to subscribe to.
fn on_sip(e: &MarketEvent) -> bool {
    let overnight = classify(e.event_ts()).is_some_and(|s| s.kind == SessionKind::Overnight);
    !overnight && !matches!(e, MarketEvent::Quote(_)) && e.symbol().is_some()
}

/// Neutralizes what legitimately differs between the two paths (the feed's source label, and the
/// receive time the live client stamps), so the rest can be compared exactly.
fn identity(e: &MarketEvent) -> MarketEvent {
    let mut e = e.clone();
    match &mut e {
        MarketEvent::Trade(t) => (t.source, t.receive_ts) = (Source::Fixture, None),
        MarketEvent::Correction(c) => (c.source, c.receive_ts) = (Source::Fixture, None),
        MarketEvent::Cancel(c) => (c.source, c.receive_ts) = (Source::Fixture, None),
        MarketEvent::Status(s) => (s.source, s.receive_ts) = (Source::Fixture, None),
        MarketEvent::Quote(q) => (q.source, q.receive_ts) = (Source::Fixture, None),
    }
    e
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_prints_via_the_live_client_and_via_replay_give_identical_engine_output() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = Archive::new(ArchiveConfig {
        root: tmp.path().join("archive"),
        retention_days: 0,
        min_free_bytes: 0,
    });
    let syms: Vec<String> = ["SMPL", "FXLO", "FXMD"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    // The fixture day, minus recorded receive times. Alpaca's wire carries none: a live client's
    // receive time is its own clock, so the prints the wire can deliver identically to both
    // paths are the ones without. (Late *arrival* is a property of when the frame comes, which
    // production stamps from the wall clock; tests/parity.rs in market-providers covers it with
    // recorded receive times.) Arrival order, duplicates, out-of-order prints, the halt, the
    // correction and the cancel all stay.
    for (i, sym) in syms.iter().enumerate() {
        let (seed, scale) = symbol_params(7, i, sym);
        let mut w = archive.writer(day(), sym, FsyncPolicy::Never).unwrap();
        for mut e in fixture_symbol(day(), sym, seed, scale).unwrap() {
            match &mut e {
                MarketEvent::Trade(t) => t.receive_ts = None,
                MarketEvent::Quote(q) => q.receive_ts = None,
                MarketEvent::Correction(c) => c.receive_ts = None,
                MarketEvent::Cancel(c) => c.receive_ts = None,
                MarketEvent::Status(s) => s.receive_ts = None,
            }
            w.append(&e).unwrap();
        }
        archive.commit(day(), w.finalize().unwrap()).unwrap();
    }
    let source: Arc<dyn EventSource> =
        Arc::new(ArchiveDaySource::new(archive.clone(), day()).unwrap());
    let end = day_bounds(day()).unwrap().1;

    // Replay at max speed drives both the reference engine and the simulator.
    let mut replay = Box::new(ReplayProvider::new(
        source,
        ReplayOptions {
            speed: Speed::Max,
            ..ReplayOptions::default()
        },
    ))
    .start()
    .unwrap();
    let sim = Sim::new(
        SimConfig::default(),
        archive,
        day(),
        syms.clone(),
        true,
        replay.clock.clone(),
    );
    let upstream = serve(&sim).await;

    // The live side: the real client over the simulator's SIP stream, whole market, with the
    // receive time set to the print's own time so the engine clock follows the prints.
    let (feed, stats) = client(
        format!("ws://{upstream}"),
        LiveSymbols::All,
        sim_creds(),
        ReceiveTime::EventTime,
    );
    let mut live = start_live(feed);
    wait_up(&mut live).await;
    let received = Arc::new(AtomicU64::new(0));
    let b = Arc::new(Mutex::new(Consumer::new()));
    let live_events = Arc::new(Mutex::new(Vec::<MarketEvent>::new()));
    {
        let (received, b, live_events) = (received.clone(), b.clone(), live_events.clone());
        tokio::spawn(async move {
            while let Some(item) = live.items.recv().await {
                if let FeedItem::Link(l) = &item {
                    assert_eq!(l.state, LinkState::Up, "the link must not drop: {l:?}");
                }
                b.lock().unwrap().take(&item);
                if let FeedItem::Event(e) = item {
                    live_events.lock().unwrap().push(e);
                    received.fetch_add(1, Ordering::SeqCst);
                }
            }
        });
    }

    let mut a = Consumer::new();
    let mut sent_events = Vec::new();
    let mut rebuilding = false;
    while let Some(item) = replay.items.recv().await {
        // The simulator drops what a rebuild re-emits; the reference must see the same.
        match &item {
            FeedItem::Reset(_) => rebuilding = true,
            FeedItem::Clock(_) => rebuilding = false,
            _ => {}
        }
        let visible = match &item {
            FeedItem::Event(e) => on_sip(e) && !rebuilding,
            _ => true,
        };
        if visible {
            a.take(&item);
        }
        if let FeedItem::Event(e) = &item
            && visible
        {
            sent_events.push(e.clone());
        }
        sim.ingest(&item);
        // Keep the simulator's broadcast (and our channel) from overflowing: its 407 "slow
        // client" is correct behaviour, but not what this test is about.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while sent_events.len() as u64 > received.load(Ordering::SeqCst) + 500 {
            assert!(tokio::time::Instant::now() < deadline, "live side stalled");
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        if matches!(&item, FeedItem::Clock(c) if c.finished) {
            break;
        }
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while (sent_events.len() as u64) > received.load(Ordering::SeqCst) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "live side never caught up"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(sent_events.len() > 50_000, "{}", sent_events.len());

    // Event for event: what the client normalized is what the archive holds.
    let got = live_events.lock().unwrap().clone();
    assert_eq!(got.len(), sent_events.len());
    for (i, (l, r)) in got.iter().zip(&sent_events).enumerate() {
        assert_eq!(identity(l), identity(r), "event {i}");
        assert_eq!(l.event_ts(), r.event_ts());
    }

    let mut b = std::mem::replace(&mut *b.lock().unwrap(), Consumer::new());
    a.engine.advance_clock(end);
    b.engine.advance_clock(end);
    // Tape rows say how every print was counted; report the first difference, not 60k rows.
    if let Some(i) = (0..a.tape.len().max(b.tape.len())).find(|&i| a.tape.get(i) != b.tape.get(i)) {
        panic!(
            "tape row {i} differs:
replay {:?}
live   {:?}",
            a.tape.get(i),
            b.tape.get(i)
        );
    }
    for sym in &syms {
        for tf in Timeframe::ALL {
            assert_eq!(
                a.engine.snapshot(sym, tf, None),
                b.engine.snapshot(sym, tf, None),
                "{sym} {tf}"
            );
        }
        assert_eq!(a.engine.quality(sym), b.engine.quality(sym));
        for sess in a.engine.sessions(sym) {
            assert_eq!(
                a.engine.profile(sym, sess, None),
                b.engine.profile(sym, sess, None)
            );
        }
        // Non-vacuous: the cancel and correction paths went over the wire too.
        let q: Vec<_> = b.engine.quality(sym).into_iter().map(|(_, q)| q).collect();
        assert_eq!(q.iter().map(|q| q.cancels_applied).sum::<u64>(), 1, "{sym}");
        assert_eq!(
            q.iter().map(|q| q.corrections_applied).sum::<u64>(),
            1,
            "{sym}"
        );
    }
    let st = stats.snapshot();
    assert_eq!((st.connects, st.dropped, st.malformed), (1, 0, 0), "{st:?}");
    assert_eq!(st.events, sent_events.len() as u64);
}

#[tokio::test]
async fn a_lagging_consumer_costs_events_not_the_connection() {
    let (_tmp, sim, _clock) = small_sim();
    let upstream = serve(&sim).await;
    let mut c = AlpacaStreamConfig::new(
        format!("ws://{upstream}"),
        StreamFeed::Sip,
        LiveSymbols::All,
        sim_creds(),
    );
    c.channel = 16;
    c.lag_grace = Duration::from_millis(100);
    let mut feed = AlpacaStream::new(c);
    let stats = feed.stats();
    let mut link = market_providers::LiveFeed::link(&mut feed).unwrap();
    let mut rx = market_providers::LiveFeed::connect(Box::new(feed)).unwrap();
    let until = |link: &mut watch::Receiver<market_providers::Link>, f: fn(&str) -> bool| {
        let mut link = link.clone();
        async move {
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    {
                        let l = link.borrow_and_update();
                        if l.state == LinkState::Up && l.detail.as_deref().is_some_and(f) {
                            return;
                        }
                    }
                    link.changed().await.unwrap();
                }
            })
            .await
            .expect("link never reached the expected state")
        }
    };
    until(&mut link, |d| d.contains("subscribed")).await;

    // Nobody reads: 16 fit, the 17th waits out the grace, the rest are shed without waiting.
    for seq in 1..=200 {
        sim.ingest(&FeedItem::Event(
            trade(at(10, 0, 1), 318, 100, seq, "V").into(),
        ));
    }
    until(&mut link, |d| d.contains("lagging")).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while stats.snapshot().events < 200 {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(stats.snapshot().dropped, 184);
    for want in 1..=16 {
        let Some(MarketEvent::Trade(t)) = rx.recv().await else {
            panic!("expected a trade")
        };
        assert_eq!(t.source_seq, want, "the oldest events are the ones kept");
    }
    // The consumer is back: the next event goes through and the gap is reported.
    sim.ingest(&FeedItem::Event(
        trade(at(10, 0, 2), 318, 100, 201, "V").into(),
    ));
    let Some(MarketEvent::Trade(t)) = rx.recv().await else {
        panic!("expected a trade")
    };
    assert_eq!(t.source_seq, 201);
    until(&mut link, |d| d.contains("184 events were dropped")).await;
    let st = stats.snapshot();
    assert_eq!((st.connects, st.subscribed, st.errors), (1, 1, 0), "{st:?}");
}
