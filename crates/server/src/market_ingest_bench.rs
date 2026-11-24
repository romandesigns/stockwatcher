//! Ingest bench (manual): a recorded live day through the production-configured live market,
//! driven by hand, timing every piece of work that takes the core lock.
//!
//! The live ingest (`ingest`) applies one feed item at a time under `Core`'s mutex, and the
//! background tasks (the rocket decision, the 1 Hz broadcast tick, the detection publish) take
//! the same mutex. Whatever holds it stops the ingest; once the two channels in front of it are
//! full (8192 + 8192 items, under half a second at the open's ~45k events/s) the Alpaca reader
//! waits `lag_grace` (2 s) and then sheds events, and a reader that stops reading long enough is
//! dropped by Alpaca ("407 slow client"). This bench measures each holder on real data so a stall
//! can be pinned on one of them.
//!
//! Run (one test thread; the bench is single-threaded by design: it is the ingest's own order):
//!
//! ```text
//! BENCH_ARCHIVE=<archive root: metadata + the PREVIOUS trading day only>
//! BENCH_FEED=<root holding <day>/live/*.swa of the day to play>
//! BENCH_DAY=2026-11-23 BENCH_FROM=04:00 BENCH_TO=09:33 BENCH_OUT=<csv path>
//! cargo test -p stockwatcher-server --release ingest_bench -- --ignored --nocapture
//! ```
//!
//! The day being played must NOT be under `BENCH_ARCHIVE`: the live history would preload it,
//! i.e. the future. Not measured: the WebSocket fan-out (no clients here) and what the publish
//! steps skip without subscribers, so production's holds are at least these.

use super::*;
use market_archive::ArchiveConfig;
use std::collections::BTreeMap;
use std::io::Write as _;
use std::time::Instant;

const SEC: i64 = 1_000_000_000;

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.trim().is_empty())
}

fn et_ns(day: NaiveDate, hm: &str) -> i64 {
    let mut it = hm.split(':').map(|p| p.parse::<u32>().expect("HH:MM[:SS]"));
    let h = it.next().unwrap();
    let m = it.next().unwrap_or(0);
    let s = it.next().unwrap_or(0);
    market_events::session::et(day, h, m, s, 0)
        .expect("ET time")
        .0
}

fn clock_at(now: i64, events: u64) -> ClockState {
    ClockState {
        mode: ClockMode::Live,
        now: UnixNanos(now),
        speed: market_providers::Speed::Times(1),
        playing: true,
        day: None,
        start: None,
        end: None,
        finished: false,
        events,
    }
}

#[derive(Default, Clone)]
struct Sec {
    events: u64,
    apply_us: u64,
    apply_max_us: u64,
    decide_us: u64,
    tick_us: u64,
    detect_us: u64,
}

fn ms(d: std::time::Duration) -> u64 {
    d.as_micros() as u64
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "manual ingest bench on a recorded day; see the module docs"]
async fn ingest_bench() {
    let (Some(hist), Some(feed_root), Some(day)) =
        (env("BENCH_ARCHIVE"), env("BENCH_FEED"), env("BENCH_DAY"))
    else {
        eprintln!("set BENCH_ARCHIVE, BENCH_FEED and BENCH_DAY");
        return;
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(
            env("RUST_LOG").unwrap_or("warn".into()),
        ))
        .try_init();
    let day: NaiveDate = day.parse().expect("BENCH_DAY");
    let from = et_ns(day, &env("BENCH_FROM").unwrap_or("04:00".into()));
    let to = et_ns(day, &env("BENCH_TO").unwrap_or("09:33".into()));
    let out = env("BENCH_OUT").unwrap_or("ingest-bench.csv".into());

    // 1. The day's events in arrival order (receive time, then each file's own order).
    let t0 = Instant::now();
    let feed = Archive::new(ArchiveConfig {
        root: feed_root.into(),
        ..ArchiveConfig::default()
    });
    let dir = feed.live_dir(day);
    let mut symbols: Vec<String> = std::fs::read_dir(&dir)
        .expect("feed live dir")
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            n.strip_suffix(".swa").map(str::to_string)
        })
        .collect();
    symbols.sort();
    let mut events: Vec<(i64, u32, u32, MarketEvent)> = Vec::new();
    for (si, s) in symbols.iter().enumerate() {
        let Ok(r) = feed.live_reader(day, s, None) else {
            continue;
        };
        for (i, ev) in r.enumerate() {
            let Ok(ev) = ev else { break };
            let at = market_providers::recorder::receive_time(&ev.event).0;
            if at >= from && at < to {
                events.push((at, si as u32, i as u32, ev.event));
            }
        }
    }
    events.sort_by_key(|e| (e.0, e.1, e.2));
    eprintln!(
        "bench: {} events from {} symbols in {:.1}s",
        events.len(),
        symbols.len(),
        t0.elapsed().as_secs_f64()
    );

    // 2. The live market as `Market::start` configures it in production (`.env`: STOCK_CONTEXT=1,
    // DETECTION_PANEL=1; everything else at its default), without its background tasks.
    let mut cfg = MarketConfig {
        stock_context: true,
        detection_panel: true,
        meta_check: Duration::ZERO,
        ..MarketConfig::default()
    };
    cfg.provider.archive.root = hist.into();
    let archive = Archive::new(cfg.provider.archive.clone());
    let market = Market::build(
        ClockMode::Live,
        Some(ProviderInfo {
            mode: ClockMode::Live,
            label: "ingest bench".into(),
            day: None,
            symbols: Vec::new(),
            fixture: false,
        }),
        ProviderStatus {
            state: "running",
            detail: None,
        },
        None,
        cfg.band,
        ArchiveMeta {
            assets: load_assets(&archive),
            closes: Arc::new(load_closes(&archive)),
            days: None,
            files: Some(MetaFiles::new(archive.clone())),
            follow_latest: false,
        },
    );
    market.configure_features(&cfg, ClockMode::Live, &archive, None);
    // Production restarted at 01:11 ET: the previous day's history was loaded long before 04:00.
    let t1 = Instant::now();
    loop {
        let left = market.0.core.lock().expect("core lock").unloaded.len();
        if left == 0 || t1.elapsed() > Duration::from_secs(1800) {
            eprintln!(
                "bench: history loaded ({left} left) in {:.0}s",
                t1.elapsed().as_secs_f64()
            );
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    tokio::time::sleep(Duration::from_secs(5)).await;

    // 3. Play it, as the live forwarder and the background tasks would: a mark at every new
    // receive second, the provider's clock once a second, then the rocket decision, the 1 Hz
    // tick and the detection publish, each timed.
    let mut marks =
        market_replay::marks::MarkClock::new(market_replay::marks::DEFAULT_MARK_STEP_NS);
    let mut secs: BTreeMap<i64, Sec> = BTreeMap::new();
    let mut cur_sec = i64::MIN;
    let mut n = 0u64;
    let mut tick_n = 0u64;
    let play = Instant::now();
    for (at, _, _, e) in events {
        let s = at.div_euclid(SEC);
        if s != cur_sec {
            if cur_sec != i64::MIN {
                // The second `cur_sec` is over: what the background tasks do at its end.
                let row = secs.entry(cur_sec).or_default();
                market.apply(FeedItem::Clock(clock_at((cur_sec + 1) * SEC, n)));
                let t = Instant::now();
                market.decide_rocket();
                row.decide_us += ms(t.elapsed());
                let t = Instant::now();
                market.publish_quotes();
                market.flush_chart_zones();
                market.publish_pressure();
                market.broadcast_rocket();
                market.drain_context();
                market.publish_context();
                market.publish_favorites();
                market.publish_movers();
                market.publish_symbols(tick_n);
                tick_n += 1;
                row.tick_us += ms(t.elapsed());
                let t = Instant::now();
                market.publish_detection();
                row.detect_us += ms(t.elapsed());
            }
            cur_sec = s;
        }
        let row = secs.entry(s).or_default();
        let t = Instant::now();
        if let Some(m) = marks.mark(at) {
            market.apply(FeedItem::Advance(m));
        }
        market.apply(FeedItem::Event(e));
        let us = ms(t.elapsed());
        row.events += 1;
        row.apply_us += us;
        row.apply_max_us = row.apply_max_us.max(us);
        n += 1;
    }
    eprintln!(
        "bench: played {n} events in {:.1}s",
        play.elapsed().as_secs_f64()
    );

    let mut f = std::fs::File::create(&out).expect("csv");
    writeln!(
        f,
        "et,events,apply_ms,apply_max_ms,decide_ms,tick_ms,detect_ms,busy_ms"
    )
    .unwrap();
    let mut worst: Vec<(u64, String)> = Vec::new();
    for (s, r) in &secs {
        let t = chrono::DateTime::from_timestamp(*s, 0)
            .unwrap()
            .with_timezone(&New_York)
            .format("%H:%M:%S")
            .to_string();
        let busy = r.apply_us + r.decide_us + r.tick_us + r.detect_us;
        let line = format!(
            "{t},{},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1}",
            r.events,
            r.apply_us as f64 / 1e3,
            r.apply_max_us as f64 / 1e3,
            r.decide_us as f64 / 1e3,
            r.tick_us as f64 / 1e3,
            r.detect_us as f64 / 1e3,
            busy as f64 / 1e3
        );
        writeln!(f, "{line}").unwrap();
        worst.push((busy, line));
    }
    worst.sort_by_key(|a| std::cmp::Reverse(a.0));
    eprintln!("bench: worst seconds (et,events,apply,apply_max,decide,tick,detect,busy ms):");
    for (_, l) in worst.iter().take(25) {
        eprintln!("  {l}");
    }
}
