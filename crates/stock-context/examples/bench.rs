//! A reproducible synthetic workload for the context engine on its own (no server, no I/O):
//! `cargo run --release -p stock-context --example bench -- [symbols] [prints] [--detection]`
//! (`--detection`: the same workload with the EXPERIMENTAL detection families on).
//!
//! Prints are spread evenly over the regular session across `symbols` tickers, every symbol has
//! a previous close and a 30-session baseline, a tenth have float and news. It measures the time
//! of each `apply` (which includes deciding any barriers it passes), so the tail percentiles are
//! the barriers. Synthetic data on this machine: not a production VPS figure.

use chrono::NaiveDate;
use market_events::session::et;
use market_events::{Price, Trade, UnixNanos};
use std::time::Instant;
use stock_context::*;

fn main() {
    let args: Vec<u64> = std::env::args()
        .skip(1)
        .filter_map(|a| a.parse().ok())
        .collect();
    let symbols = *args.first().unwrap_or(&3_000) as usize;
    let prints = *args.get(1).unwrap_or(&1_000_000);
    let day = NaiveDate::from_ymd_opt(2026, 10, 16).unwrap();
    let prev = NaiveDate::from_ymd_opt(2026, 10, 15).unwrap();
    let open = et(day, 9, 30, 0, 0).unwrap().nanos();
    let close = et(day, 16, 0, 0, 0).unwrap().nanos();
    let pre = et(day, 4, 0, 0, 0).unwrap().nanos();

    let detection = std::env::args().any(|a| a == "--detection");
    let mut e = Engine::new(
        Policy::v1(),
        RunInfo {
            run_id: "bench".into(),
            source: "replay".into(),
            fidelity: Fidelity::SyntheticFixture,
            numerator_coverage: Coverage::Complete,
        },
    );
    if detection {
        e = e.with_detection_policy(DetectionPolicy::v0());
    }
    let names: Vec<String> = (0..symbols).map(|i| format!("S{i:05}")).collect();
    for (i, s) in names.iter().enumerate() {
        let inst = InstrumentId::symbol(s);
        e.add_prev_close(PrevClose {
            id: format!("c:{s}"),
            version: 1,
            instrument: inst.clone(),
            date: prev,
            close_micros: 3_000_000 + i as i64 * 1_000,
            basis: CloseBasis::SplitAdjusted,
            actions_checked_through: Some(day),
            actions: Default::default(),
            observed_at: pre,
        });
        let mut sessions = Vec::new();
        let mut d = prev;
        while sessions.len() < 30 {
            if market_events::is_trading_day(d) {
                sessions.push(SessionVolume {
                    date: d,
                    volume: if i % 10 == 0 {
                        2_000
                    } else {
                        200_000 + i as u64
                    },
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
                published_at: Some(pre),
                observed_at: pre,
                tag: Tag::Present,
                content_key: format!("bench:{s}"),
                revision_at: Some(pre),
                rules: "bench".into(),
            });
        }
    }

    let step = (close - open) / prints as i64;
    let mut lat = Vec::with_capacity(prints as usize);
    let mut decisions = 0usize;
    let started = Instant::now();
    for k in 0..prints {
        let s = &names[(k as usize * 7919) % symbols];
        let at = open + k as i64 * step;
        let price = Price::from_micros(3_000_000 + ((k * 131) % 900_000) as i64);
        let t = Trade::new(s.as_str(), UnixNanos(at), price, 100 + k % 900, k);
        let t0 = Instant::now();
        decisions += e
            .apply(
                &t,
                Verdict {
                    ohlc: true,
                    volume: true,
                },
                UnixNanos(at + 50_000_000),
            )
            .len();
        lat.push(t0.elapsed().as_nanos() as u64);
    }
    decisions += e.advance_to(UnixNanos(close + 60_000_000_000)).len();
    let total = started.elapsed();
    let panels = e.take_panels();
    let members = e.panel().map_or(0, |p| p.rows.len());
    let inside: u64 = lat.iter().sum();
    let spikes: Vec<u64> = lat.iter().copied().filter(|&x| x > 100_000).collect();
    lat.sort_unstable();
    let q = |p: f64| lat[((lat.len() - 1) as f64 * p) as usize];
    println!(
        "symbols {symbols}, prints {prints}, decisions {decisions}, panel snapshots {}, members at close {members}",
        panels.len()
    );
    println!(
        "time inside apply {:.2?} of {:.2?} wall; {} calls over 100 us (barriers) took {:.2?}",
        std::time::Duration::from_nanos(inside),
        total,
        spikes.len(),
        std::time::Duration::from_nanos(spikes.iter().sum())
    );
    println!(
        "total {:.2?} ({:.0} prints/s)",
        total,
        prints as f64 / total.as_secs_f64()
    );
    println!(
        "apply latency: p50 {} ns, p95 {} ns, p99 {} ns, max {} ns",
        q(0.50),
        q(0.95),
        q(0.99),
        lat[lat.len() - 1]
    );
    if detection {
        let t = e.detection_telemetry();
        println!(
            "detection: episodes {} (active {}), transitions {} (dropped {}: nothing takes them here)",
            t.episodes_opened, t.episodes_active, t.transitions_logged, t.transitions_dropped
        );
    }
}
