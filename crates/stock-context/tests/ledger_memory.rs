//! What the admission ledgers cost for a whole market, measured on a synthetic hour.
//!
//! The engine sees every print of the market (`STOCK_CONTEXT=1` feeds the whole SIP stream), and
//! each symbol's ledger keeps an hour of them, up to `LEDGER_MAX`. So the peak is the busiest
//! hour of the day summed over every symbol. The generator is the one the live-history benchmark
//! uses (`crates/server/tests/live_history.rs`): symbol index `N·u⁴`, which puts ~11% of prints on
//! the busiest of 6,700 symbols, roughly the skew of a real day.
//!
//! `cargo test -p stock-context --release --test ledger_memory -- --ignored --nocapture`

use market_events::session::et;
use market_events::{Price, Source, Trade, UnixNanos};
use std::time::Instant;
use stock_context::engine::{LEDGER_MAX, LEDGER_WINDOW_SECS};
use stock_context::*;

const SEC: i64 = 1_000_000_000;

fn day() -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(2026, 10, 15).expect("valid")
}

fn engine() -> Engine {
    Engine::new(
        Policy::current(),
        RunInfo {
            run_id: "bench".into(),
            source: "live".into(),
            fidelity: Fidelity::RecordedLive,
            numerator_coverage: Coverage::Unverified,
        },
    )
}

/// Feeds `n` prints over one hour from `start` across `symbols`, skewed like the market.
fn hour(e: &mut Engine, start: i64, n: u64, symbols: u64) {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let both = Verdict {
        ohlc: true,
        volume: true,
    };
    for i in 0..n {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let u = (x >> 11) as f64 / (1u64 << 53) as f64;
        let s = ((symbols as f64) * u.powi(4)) as u64 % symbols;
        let exec = start + (3_600 * SEC / n as i64) * i as i64;
        let cents = 100 + (s as i64 * 37) % 2400;
        let mut t = Trade::new(
            format!("S{s:04}"),
            UnixNanos(exec),
            Price::from_cents(cents),
            100,
            52_983_525_029_461 + i,
        )
        .with_source(Source::AlpacaSip)
        .with_exchange(["V", "Q", "N", "P", "D", "K"][(x % 6) as usize]);
        t.receive_ts = Some(UnixNanos(exec + 3_000_000));
        e.apply(&t, both, UnixNanos(exec + 4_000_000));
    }
}

/// Σ min(cap, n·p_k) for the generator's distribution: the entries an hour of `n` prints leaves.
fn analytic_entries(n: f64, symbols: u64, cap: f64) -> f64 {
    let m = symbols as f64;
    (0..symbols)
        .map(|k| {
            let p = ((k + 1) as f64 / m).powf(0.25) - (k as f64 / m).powf(0.25);
            (n * p).min(cap)
        })
        .sum()
}

#[test]
#[ignore = "measurement"]
fn measure_ledger_memory_for_a_whole_market_hour() {
    let symbols = 6_700;
    let n = 2_000_000;
    let start = et(day(), 9, 30, 0, 0).expect("valid").nanos();
    let mut e = engine();
    let t0 = Instant::now();
    hour(&mut e, start, n, symbols);
    let fed = t0.elapsed();
    let (entries, bytes) = e.ledger_footprint();
    let per = bytes as f64 / entries.max(1) as f64;
    println!(
        "fed {n} prints / {symbols} symbols in {:.1}s; ledger {entries} entries, {:.1} MB, {per:.0} B/entry (allocations)",
        fed.as_secs_f64(),
        bytes as f64 / 1e6
    );
    println!(
        "analytic entries for this hour: {:.0} (cap {})",
        analytic_entries(
            n as f64 * LEDGER_WINDOW_SECS as f64 / 3600.0,
            symbols,
            LEDGER_MAX as f64
        ),
        LEDGER_MAX
    );
    for h in [4e6, 8e6, 12e6] {
        let a = analytic_entries(
            h * LEDGER_WINDOW_SECS as f64 / 3600.0,
            symbols,
            LEDGER_MAX as f64,
        );
        println!(
            "busiest hour {:.0}M prints: {:.2}M entries, ~{:.0} MB at {per:.0} B/entry",
            h / 1e6,
            a / 1e6,
            a * per / 1e6
        );
    }
    // Barrier cost with the ledgers full: each 15 s advance decides a barrier.
    let end = start + 3_600 * SEC;
    let t1 = Instant::now();
    let mut k = 0;
    while k < 8 {
        k += 1;
        e.advance_to(UnixNanos(end + k * 15 * SEC + 2 * SEC));
    }
    println!(
        "8 idle barriers after the hour: {:.1} ms each",
        t1.elapsed().as_secs_f64() * 1e3 / 8.0
    );
    // Overnight: the clock moves on with no prints.
    e.advance_to(UnixNanos(end + 6 * 3_600 * SEC));
    let (entries2, bytes2) = e.ledger_footprint();
    println!(
        "six idle hours later: {entries2} entries, {:.1} MB",
        bytes2 as f64 / 1e6
    );
}
