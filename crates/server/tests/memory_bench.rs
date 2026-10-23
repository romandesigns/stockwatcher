//! What a whole-market day costs the LIVE market in heap: the chart engine, the quote board, the
//! Halt watcher, stock context and Runners with buying bursts, all fed through the real ingest.
//!
//! Synthetic, shaped like the US tape (assumed, not measured: ~40M prints a day over ~12k
//! symbols, Zipf-skewed, a U-shaped regular session, a third at sub-penny TRF prices). It reports
//! the **live heap** (bytes held, from a counting allocator), not RSS: the gap between the two in
//! production is the allocator's, which this cannot see. Run on demand, in release:
//!
//! `MEM_PRINTS=40000000 cargo test --release -p stockwatcher-server --test memory_bench -- --ignored --nocapture`

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::{Duration, Instant};

use market_archive::{Archive, ArchiveConfig};
use market_events::{MarketEvent, Price, Source, Trade, UnixNanos};
use market_providers::{ChannelFeed, LiveOptions, LiveProvider, MarketDataProvider};
use stockwatcher_server::Market;
use tokio::sync::mpsc;

struct Counting;
static LIVE: AtomicIsize = AtomicIsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        LIVE.fetch_add(l.size() as isize, Ordering::Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        LIVE.fetch_sub(l.size() as isize, Ordering::Relaxed);
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        LIVE.fetch_add(new as isize - l.size() as isize, Ordering::Relaxed);
        unsafe { System.realloc(p, l, new) }
    }
}

#[global_allocator]
static A: Counting = Counting;

const SEC: i64 = 1_000_000_000;

fn env(k: &str, d: u64) -> u64 {
    std::env::var(k)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(d)
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Relative print intensity at `m` minutes after 04:00 ET.
fn intensity(m: i64) -> f64 {
    match m {
        0..330 => 0.08,
        330..360 => 4.0,
        360..420 => 2.0,
        420..690 => 1.0,
        690..720 => 3.0,
        720..960 => 0.06,
        _ => 0.0,
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn measure_live_market_heap() {
    let prints = env("MEM_PRINTS", 10_000_000);
    let n = env("MEM_SYMBOLS", 12_000) as usize;
    let day = chrono::NaiveDate::from_ymd_opt(2026, 10, 15).unwrap();
    let t0 = market_events::session::et(day, 4, 0, 0, 0).unwrap().0;
    let tmp = tempfile::tempdir().unwrap();
    let archive = Archive::new(ArchiveConfig {
        root: tmp.path().join("archive"),
        retention_days: 0,
        min_free_bytes: 0,
    });
    let base = LIVE.load(Ordering::Relaxed);
    let (tx, rx) = mpsc::channel(8192);
    let session = Box::new(LiveProvider::with_feed(
        Box::new(ChannelFeed {
            label: "memory bench".into(),
            symbols: Vec::new(),
            rx,
        }),
        LiveOptions {
            idle_tick: None,
            ..LiveOptions::default()
        },
    ))
    .start()
    .unwrap();
    let m = Market::from_live_session(session, archive, UnixNanos(t0), false);
    // Each feature can be switched off to measure what it costs (1 = on, the production default):
    // MEM_CONTEXT, MEM_ROCKET, MEM_FAST (RUNNERS_FAST), MEM_ZONES (VOLUME_SHELVES), MEM_BURST.
    let on = |k: &str| env(k, 1) != 0;
    m.set_stock_context(on("MEM_CONTEXT"));
    m.set_rocket_watch(
        on("MEM_ROCKET"),
        stockwatcher_server::rocket::Policy {
            fast: on("MEM_FAST"),
            ..Default::default()
        },
    );
    m.set_volume_shelves(on("MEM_ZONES"));
    if on("MEM_BURST") {
        m.enable_burst(
            stockwatcher_server::rocket::burst::BurstPolicy::default(),
            None,
        );
    }

    let w: Vec<f64> = (0..n).map(|i| 1.0 / (i as f64 + 8.0).powf(1.15)).collect();
    let mut cum = Vec::with_capacity(n);
    let mut acc = 0.0;
    for x in &w {
        acc += x;
        cum.push(acc);
    }
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    // Log-uniform $0.50–$40: most of the tape the band-limited boards care about.
    let mut px: Vec<f64> = (0..n)
        .map(|_| (0.5f64.ln() + rng.unit() * (40f64 / 0.5).ln()).exp())
        .collect();
    let names: Vec<String> = (0..n).map(|i| format!("S{i:05}")).collect();
    let total_int: f64 = (0..960).map(intensity).sum();
    let exch = ["V", "Q", "N", "P", "D", "K", "Z", "D", "D"];
    let checkpoints = [330i64, 360, 390, 420, 480, 600, 720, 960];
    let started = Instant::now();
    let mut seq = 0u64;
    for minute in 0..960i64 {
        let per_sec = (prints as f64 * intensity(minute) / total_int / 60.0).round() as u64;
        for s in 0..60 {
            let sec_ts = t0 + (minute * 60 + s) * SEC;
            // The market clock follows the synthetic day (the provider's own clock is the
            // wall's), so the once-a-wall-second tick decides slots and reads fast moves.
            m.bench_set_clock(UnixNanos(sec_ts));
            for k in 0..per_sec {
                let u = rng.unit() * acc;
                let i = cum.partition_point(|c| *c < u).min(n - 1);
                px[i] = (px[i] * (1.0 + (rng.unit() - 0.5) * 0.002)).clamp(0.05, 2000.0);
                let ex = exch[(rng.next() % exch.len() as u64) as usize];
                let price = if ex == "D" {
                    Price::from_micros(((px[i] * 10_000.0).round() as i64) * 100)
                } else {
                    Price::from_cents((px[i] * 100.0).round() as i64)
                };
                let r = rng.next();
                let size = if r.is_multiple_of(3) {
                    1 + r % 99
                } else {
                    100 * (1 + r % 5)
                };
                seq += 1;
                let ts = UnixNanos(sec_ts + (k as i64 * SEC) / per_sec.max(1) as i64);
                let mut t = Trade::new(names[i].clone(), ts, price, size, seq);
                t.source = Source::AlpacaSip;
                t.exchange = ex.into();
                t.conditions = if size < 100 {
                    vec!["@".into(), "I".into()]
                } else {
                    vec!["@".into()]
                };
                t.receive_ts = Some(UnixNanos(ts.0 + 3_000_000));
                tx.send(MarketEvent::Trade(t)).await.unwrap();
            }
        }
        if checkpoints.contains(&(minute + 1)) {
            // Let the ingest catch up to the checkpoint before reading the heap.
            // (the feed channel drained, then the provider's own queue and the ingest behind it).
            while tx.capacity() < tx.max_capacity() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            tokio::time::sleep(Duration::from_millis(1500)).await;
            eprintln!(
                "{:02}:{:02} ET  prints {:>10}  live heap {:6.2} GB  ({:.0} s)",
                4 + (minute + 1) / 60,
                (minute + 1) % 60,
                seq,
                (LIVE.load(Ordering::Relaxed) - base) as f64 / 1e9,
                started.elapsed().as_secs_f64()
            );
            eprintln!("          {}", m.memory_report());
        }
    }
}
