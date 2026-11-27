//! How long one clock call (`Engine::advance_to`) holds the server's core lock, on a whole
//! market's worth of state, with the evidence a live day delivers between the calls:
//!
//! ```text
//! cargo run --release -p stock-context --example clock_holds -- [symbols] [prints/s] [--budget]
//! ```
//!
//! `--budget` runs the live server's [`ClockBudget::LIVE`]; without it every due symbol is
//! decided in the call that reaches its barrier, as before 2026-11-27.
//!
//! The engine is the production one (pillar policy, Ross Panel, micro-pullback detector). Every
//! symbol has a previous close and a 30-session baseline, a tenth a float and a story; half trade
//! inside the band, half far above it. The run covers 19:30-21:30 ET (prints until 20:00, then
//! the quiet evening the slow holds were seen in) at one clock call per wall second, as the live
//! provider's idle tick makes them, with prints spread over the symbols in between, a market-wide
//! news coverage record every 30 s (one per news poll, as the server delivers them) and the
//! previous closes re-issued for every symbol every 30 minutes (a baseline rebuild). It prints
//! the slowest calls, what preceded each, and the distribution. Synthetic data on this machine:
//! compare before and after on the same machine, never with a production figure.

use chrono::NaiveDate;
use market_events::session::et;
use market_events::{Price, Trade, UnixNanos};
use std::time::Instant;
use stock_context::*;

const SEC: i64 = 1_000_000_000;

fn main() {
    let args: Vec<u64> = std::env::args()
        .skip(1)
        .filter_map(|a| a.parse().ok())
        .collect();
    let symbols = *args.first().unwrap_or(&13_000) as usize;
    let rate = *args.get(1).unwrap_or(&1_000) as i64;
    let day = NaiveDate::from_ymd_opt(2026, 11, 25).unwrap();
    let prev = NaiveDate::from_ymd_opt(2026, 11, 24).unwrap();
    let pre = et(day, 4, 0, 0, 0).unwrap().nanos();
    let start = et(day, 19, 30, 0, 0).unwrap().nanos();
    let quiet = et(day, 20, 0, 0, 0).unwrap().nanos();
    let end = et(day, 21, 30, 0, 0).unwrap().nanos();

    let mut e = Engine::new(
        Policy::current(),
        RunInfo {
            run_id: "live".into(),
            source: "live".into(),
            fidelity: Fidelity::RecordedLive,
            numerator_coverage: Coverage::Unverified,
        },
    )
    .with_panel_policy(PanelPolicy::v1())
    .with_setup_policy(DetectionPolicy::current());
    let budget = std::env::args().any(|a| a == "--budget");
    if budget {
        e = e.with_clock_budget(ClockBudget::LIVE);
    }
    let names: Vec<String> = (0..symbols).map(|i| format!("S{i:05}")).collect();
    // Half the symbols trade around $3 (inside the band), half around $120 (far above it, as
    // most of the market's prints are).
    let base = |i: usize| {
        if i.is_multiple_of(2) {
            3_000_000
        } else {
            120_000_000
        }
    };
    let close = |e: &mut Engine, i: usize, s: &str, v: u32, at: i64| {
        e.add_prev_close(PrevClose {
            id: format!("c:{s}"),
            version: v,
            instrument: InstrumentId::symbol(s),
            date: prev,
            close_micros: base(i) + i as i64 * 100,
            basis: CloseBasis::SplitAdjusted,
            actions_checked_through: Some(day),
            actions: Default::default(),
            observed_at: at,
        });
    };
    for (i, s) in names.iter().enumerate() {
        let inst = InstrumentId::symbol(s);
        close(&mut e, i, s, 1, pre);
        let mut sessions = Vec::new();
        let mut d = prev;
        while sessions.len() < 30 {
            if market_events::is_trading_day(d) {
                sessions.push(SessionVolume {
                    date: d,
                    volume: 200_000 + i as u64,
                    complete: true,
                    comparable: true,
                });
            }
            d = d.pred_opt().unwrap();
        }
        e.add_daily(DailyBaseline {
            id: format!("d:{s}"),
            version: 1,
            instrument: inst.clone(),
            sessions,
            basis: Default::default(),
            observed_at: pre,
        });
        if i % 10 == 0 {
            e.add_float(FloatObservation {
                id: format!("f:{s}"),
                version: 1,
                instrument: inst.clone(),
                shares: 9_000_000,
                provider: "bench".into(),
                effective_as_of: None,
                observed_at: pre,
                quality: vec![],
            });
            e.add_news(NewsItem {
                id: format!("n:{s}"),
                version: 1,
                instrument: inst,
                event_type: "other".into(),
                source: "bench".into(),
                credible_source: true,
                relevant: true,
                substantive: true,
                authenticity: Authenticity::IssuerAnnounced,
                favorability: Favorability::Favorable,
                published_at: Some(start - 3600 * SEC),
                observed_at: start - 3600 * SEC,
                tag: Tag::Present,
                content_key: format!("bench:{s}"),
                revision_at: Some(start - 3600 * SEC),
                rules: "bench".into(),
                url: None,
                headline: None,
                halt: None,
            });
        }
    }
    let coverage = |e: &mut Engine, n: u64, at: i64| {
        e.add_coverage(NewsCoverage {
            id: format!("cov:{n}"),
            instrument: InstrumentId::market(),
            sources: vec!["bench".into()],
            covered_from: pre - 2 * 86_400 * SEC,
            covered_through: at,
            observed_at: at,
        });
    };

    // Warm up: every symbol trades a little from 15:00, so every one has a date, a price and a
    // ledger before the measured hour.
    let mut seq = 0u64;
    let mut print = |e: &mut Engine, k: u64, at: i64| {
        seq += 1;
        let i = (k as usize * 7919) % symbols;
        let s = &names[i];
        let price = Price::from_micros(base(i) + ((k * 131) % 900_000) as i64);
        let t = Trade::new(s.as_str(), UnixNanos(at), price, 100 + k % 900, seq);
        e.apply(
            &t,
            Verdict {
                ohlc: true,
                volume: true,
            },
            UnixNanos(at + 50_000_000),
        );
    };
    let warm = start - 1800 * SEC;
    for k in 0..(symbols as u64 * 4) {
        print(
            &mut e,
            k,
            warm + (k as i64 * 1800 * SEC) / (symbols as i64 * 4),
        );
    }
    let mut ncov = 0u64;
    coverage(&mut e, ncov, start - SEC);
    e.advance_to(UnixNanos(start));
    e.take_panels();
    e.take_setup_log();

    // (call time in ms, at, what preceded it)
    let mut holds: Vec<(f64, i64, &'static str)> = Vec::new();
    let mut k = 0u64;
    let mut print_max = 0f64;
    let mut now = start;
    while now < end {
        // A second of prints (none after the session ends), then the clock.
        let mut slowest_print = 0f64;
        for j in 0..if now < quiet { rate } else { 0 } {
            let t0 = Instant::now();
            print(&mut e, k, now + j * SEC / rate);
            slowest_print = slowest_print.max(t0.elapsed().as_secs_f64() * 1e3);
            k += 1;
        }
        print_max = print_max.max(slowest_print);
        now += SEC;
        let mut why = "";
        let since = (now - start) / SEC;
        if since % 30 == 7 {
            ncov += 1;
            coverage(&mut e, ncov, now - SEC);
            e.prune_evidence();
            why = "coverage";
        }
        if since % 1800 == 900 {
            for (i, s) in names.iter().enumerate() {
                close(&mut e, i, s, 2 + (since / 1800) as u32, now - SEC);
            }
            e.prune_evidence();
            why = "closes";
        }
        let t0 = Instant::now();
        e.advance_to(UnixNanos(now));
        holds.push((t0.elapsed().as_secs_f64() * 1e3, now, why));
        // The server drains these after each call; leaving them would only grow the logs.
        e.take_panels();
        e.take_setup_log();
    }
    let mut sorted: Vec<f64> = holds.iter().map(|h| h.0).collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let q = |p: f64| sorted[((sorted.len() - 1) as f64 * p) as usize];
    let (entries, bytes) = e.ledger_footprint();
    println!(
        "symbols {symbols}, {rate} prints/s, {} clock calls, ledger {entries} entries / {} MB, budget {}",
        holds.len(),
        bytes / 1_000_000,
        if budget { "LIVE" } else { "none" }
    );
    println!("clock telemetry: {:?}", e.clock_telemetry());
    println!(
        "clock call ms: p50 {:.2}, p99 {:.2}, p99.9 {:.2}, max {:.2}; calls over 100 ms {}, over 300 ms {}",
        q(0.5),
        q(0.99),
        q(0.999),
        sorted[sorted.len() - 1],
        sorted.iter().filter(|&&x| x > 100.0).count(),
        sorted.iter().filter(|&&x| x > 300.0).count()
    );
    println!("slowest print apply (which decides any barrier it passes): {print_max:.2} ms");
    let mut by_why = std::collections::BTreeMap::<&str, (usize, f64, f64)>::new();
    for (ms, _, why) in &holds {
        let w = by_why
            .entry(if why.is_empty() { "-" } else { why })
            .or_default();
        w.0 += 1;
        w.1 += ms;
        w.2 = w.2.max(*ms);
    }
    for (why, (n, sum, max)) in by_why {
        println!(
            "  after {why:<8}: {n:>5} calls, mean {:.2} ms, max {max:.2} ms",
            sum / n as f64
        );
    }
    holds.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    for (ms, at, why) in holds.iter().take(8) {
        let s = (at - start) / SEC;
        println!(
            "  slow: {ms:.1} ms at 19:30 + {}m{:02}s ({})",
            s / 60,
            s % 60,
            if why.is_empty() { "-" } else { why }
        );
    }
}
