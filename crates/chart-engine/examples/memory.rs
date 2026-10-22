//! What a whole-market day costs the chart engine in heap, measured with a counting allocator.
//!
//! Synthetic, shaped like the US tape the live server takes (assumed, not measured: ~40M prints a
//! day over ~12k symbols, Zipf-skewed, a U-shaped regular session, a third of the prints at
//! sub-penny TRF prices). It reports the live heap (bytes the program holds, not RSS) through the
//! day, so the growth can be split into what is retained per trade (a time window) and what is
//! retained per interval (the day's candles) or per exact price (the session profiles).
//!
//! ```text
//! cargo run --release -p chart-engine --example memory            # 40M prints
//! MEM_PRINTS=10000000 MEM_SYMBOLS=12000 cargo run --release -p chart-engine --example memory
//! ```

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};

use chart_engine::{ChartEngine, EngineConfig};
use market_events::{MarketEvent, Price, Source, Trade, UnixNanos};

struct Counting;
static LIVE: AtomicIsize = AtomicIsize::new(0);
static PEAK: AtomicIsize = AtomicIsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let n = LIVE.fetch_add(l.size() as isize, Ordering::Relaxed) + l.size() as isize;
        PEAK.fetch_max(n, Ordering::Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        LIVE.fetch_sub(l.size() as isize, Ordering::Relaxed);
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        let d = new as isize - l.size() as isize;
        let n = LIVE.fetch_add(d, Ordering::Relaxed) + d;
        PEAK.fetch_max(n, Ordering::Relaxed);
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

fn gb(b: isize) -> f64 {
    b as f64 / 1e9
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
        0..330 => 0.08,   // pre-market
        330..360 => 4.0,  // first half hour
        360..420 => 2.0,  // rest of the first hour and a half
        420..690 => 1.0,  // midday
        690..720 => 3.0,  // last half hour
        720..960 => 0.06, // after hours
        _ => 0.0,
    }
}

fn main() {
    let prints = env("MEM_PRINTS", 40_000_000);
    let n = env("MEM_SYMBOLS", 12_000) as usize;
    let day = chrono::NaiveDate::from_ymd_opt(2026, 10, 15).unwrap();
    let t0 = market_events::session::et(day, 4, 0, 0, 0).unwrap().0;

    // Zipf-like activity, a log-uniform price per symbol.
    let w: Vec<f64> = (0..n).map(|i| 1.0 / (i as f64 + 8.0).powf(1.15)).collect();
    let mut cum = Vec::with_capacity(n);
    let mut acc = 0.0;
    for x in &w {
        acc += x;
        cum.push(acc);
    }
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut px: Vec<f64> = (0..n)
        .map(|_| (0.5f64.ln() + rng.unit() * (400f64 / 0.5).ln()).exp())
        .collect();
    let names: Vec<String> = (0..n).map(|i| format!("S{i:05}")).collect();
    let total_int: f64 = (0..960).map(intensity).sum();

    let base = LIVE.load(Ordering::Relaxed);
    let mut engine = ChartEngine::new(EngineConfig::default()).unwrap();
    let checkpoints = [330i64, 360, 390, 420, 480, 600, 720, 960];
    let mut seq = 0u64;
    let exch = ["V", "Q", "N", "P", "D", "K", "Z", "D", "D"];
    for minute in 0..960i64 {
        let per_min = prints as f64 * intensity(minute) / total_int;
        let per_sec = (per_min / 60.0).round() as u64;
        for s in 0..60 {
            let sec_ts = t0 + (minute * 60 + s) * SEC;
            for k in 0..per_sec {
                let u = rng.unit() * acc;
                let i = cum.partition_point(|c| *c < u).min(n - 1);
                // A small random walk; a third of the prints at a sub-penny (TRF) price.
                let step = 1.0 + (rng.unit() - 0.5) * 0.002;
                px[i] = (px[i] * step).clamp(0.05, 2000.0);
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
                let _ = engine.apply(&MarketEvent::Trade(t));
            }
            let _ = engine.advance_clock(UnixNanos(sec_ts + SEC));
        }
        if checkpoints.contains(&(minute + 1)) {
            let live = LIVE.load(Ordering::Relaxed) - base;
            let retained: usize = names.iter().map(|s| engine.retained_trades(s)).sum();
            eprintln!(
                "{:02}:{:02} ET  seq {:>10}  heap {:6.2} GB  peak {:6.2} GB  retained trades {:>9}",
                4 + (minute + 1) / 60,
                (minute + 1) % 60,
                seq,
                gb(live),
                gb(PEAK.load(Ordering::Relaxed) - base),
                retained
            );
        }
    }
    // What the day left behind: candles per timeframe and distinct exact prices in the profiles.
    let mut candles = 0usize;
    let mut ticks = 0usize;
    let tick = Price::from_micros(1);
    for s in &names {
        for tf in chart_engine::Timeframe::ALL {
            candles += engine.snapshot(s, tf, None).map_or(0, |x| x.candles.len());
        }
        for session in engine.sessions(s) {
            ticks += engine.profile_rows(s, session, tick).map_or(0, |r| r.len());
        }
    }
    eprintln!("end of day: {candles} candles (all timeframes), {ticks} distinct profile prices");
}
