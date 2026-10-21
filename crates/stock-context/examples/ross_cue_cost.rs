//! What the Ross cues cost with the Detection Panel off (`STOCK_CONTEXT=1`, `DETECTION_PANEL=0`):
//! `cargo run --release -p stock-context --example ross_cue_cost -- [symbols] [members] [bars]`.
//!
//! Since 2026-10-21 every Ross row's cue is the detection policy's micro-pullback, which the
//! engine runs for the panel's members only when the Detection Panel is off. This runs one
//! synthetic regular-session stretch twice on otherwise identical engines: once as shipped (the
//! micro-pullback on for members), once with that detector disabled (a policy whose grid does not
//! match, which the detector refuses to run). The difference is the cue's cost: time per barrier
//! (`advance_to`, which decides every symbol and publishes the panel), time per print (`apply`),
//! and heap bytes held at the end (a counting allocator). `members` symbols pass all five pillars
//! and print a repeating impulse / pullback / breakout; the rest print in band without evidence,
//! so they are never members. Synthetic data on this machine: not a production VPS figure.

use chrono::NaiveDate;
use market_events::session::et;
use market_events::{Price, Trade, UnixNanos};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::Instant;
use stock_context::*;

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
}

#[global_allocator]
static A: Counting = Counting;

const S: i64 = 1_000_000_000;
const VOL: Verdict = Verdict {
    ohlc: true,
    volume: true,
};

/// One member's repeating 16-bar shape (cents above a per-symbol base): flat, a 6 % impulse, a
/// two-bar pullback inside 38.2 %, a bar at the trigger, a breakout, then drift back down.
const CYCLE: [[i64; 4]; 16] = [
    [500, 500, 500, 500],
    [500, 500, 500, 500],
    [500, 500, 500, 500],
    [500, 500, 500, 500],
    [500, 510, 500, 510],
    [510, 530, 510, 528],
    [528, 528, 521, 522],
    [522, 524, 520, 521],
    [521, 526, 521, 526],
    [526, 534, 526, 533],
    [533, 533, 530, 531],
    [531, 531, 520, 522],
    [522, 522, 510, 512],
    [512, 512, 505, 506],
    [506, 506, 500, 501],
    [501, 501, 500, 500],
];

struct Run {
    barrier_us: Vec<f64>,
    apply_ns: f64,
    live_bytes: isize,
    setup_entries: u64,
    attention_max: u32,
}

fn evidence(e: &mut Engine, s: &str, member: bool, day: NaiveDate, prev: NaiveDate, pre: i64) {
    let inst = InstrumentId::symbol(s);
    e.add_prev_close(PrevClose {
        id: format!("c:{s}"),
        version: 1,
        instrument: inst.clone(),
        date: prev,
        close_micros: 4_000_000,
        basis: CloseBasis::SplitAdjusted,
        actions_checked_through: Some(day),
        actions: Default::default(),
        observed_at: pre,
    });
    if !member {
        return;
    }
    let mut sessions = Vec::new();
    let mut d = prev;
    while sessions.len() < 30 {
        if market_events::is_trading_day(d) {
            sessions.push(SessionVolume {
                date: d,
                volume: 100,
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
    e.add_float(FloatObservation {
        id: format!("f:{s}"),
        version: 1,
        instrument: inst.clone(),
        shares: 5_000_000,
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

fn run(symbols: usize, members: usize, bars: i64, cue: bool) -> Run {
    let day = NaiveDate::from_ymd_opt(2026, 10, 16).unwrap();
    let prev = NaiveDate::from_ymd_opt(2026, 10, 15).unwrap();
    let pre = et(day, 4, 0, 0, 0).unwrap().nanos();
    let open = et(day, 9, 30, 0, 0).unwrap().nanos();
    let base = LIVE.load(Ordering::Relaxed);
    let mut e = Engine::new(
        Policy::v1(),
        RunInfo {
            run_id: "bench".into(),
            source: "replay".into(),
            fidelity: Fidelity::SyntheticFixture,
            numerator_coverage: Coverage::Complete,
        },
    );
    if !cue {
        // The same policy on a grid the engine does not use: the detector refuses to run
        // (`grid_mismatch`), so every cue is `watching` and nothing else changes.
        let off = DETECTION_V2.replacen("\"bar_secs\": 15", "\"bar_secs\": 30", 1);
        e = e.with_setup_policy(DetectionPolicy::parse(&off).unwrap());
    }
    let names: Vec<String> = (0..symbols).map(|i| format!("S{i:05}")).collect();
    for (i, s) in names.iter().enumerate() {
        evidence(&mut e, s, i < members, day, prev, pre);
    }
    let mut barrier_us = Vec::with_capacity(bars as usize);
    let (mut apply_total, mut applies) = (0u128, 0u64);
    let mut seq = 0u64;
    let mut setup_entries = 0u64;
    let mut attention_max = 0;
    for k in 0..bars {
        let e0 = open + k * 15 * S;
        let end = e0 + 15 * S;
        // Members: four prints making the cycle's bar (offset per member so they do not move in
        // lockstep). Everyone else: one in-band print per bar.
        for (i, s) in names.iter().enumerate() {
            let prints: Vec<(i64, i64, u64)> = if i < members {
                let c = CYCLE[((k + i as i64 * 3) % 16) as usize];
                let p = |x: i64| x * 10_000 + i as i64 * 1_000;
                vec![
                    (3, p(c[0]), 100),
                    (5, p(c[1]), 100),
                    (7, p(c[2]), 100),
                    (12, p(c[3]), 200),
                ]
            } else {
                vec![(
                    3 + (i as i64 % 9),
                    3_000_000 + (i as i64 % 500) * 1_000,
                    100,
                )]
            };
            for (off, px, size) in prints {
                seq += 1;
                let exec = e0 + off * S + (i as i64 % 1_000) * 1_000;
                let t = Trade::new(
                    s.as_str(),
                    UnixNanos(exec),
                    Price::from_micros(px),
                    size,
                    seq,
                );
                let at = Instant::now();
                e.apply(&t, VOL, UnixNanos(exec + 100_000_000));
                apply_total += at.elapsed().as_nanos();
                applies += 1;
            }
        }
        let at = Instant::now();
        e.advance_to(UnixNanos(end + 2 * S));
        barrier_us.push(at.elapsed().as_secs_f64() * 1e6);
        // The server drains every tick (`ContextBoard::drain_outputs`).
        setup_entries += e.take_setup_log().len() as u64;
        for p in e.take_panels() {
            attention_max = attention_max.max(p.attention);
        }
    }
    let live_bytes = LIVE.load(Ordering::Relaxed) - base;
    drop(e);
    Run {
        barrier_us,
        apply_ns: apply_total as f64 / applies as f64,
        live_bytes,
        setup_entries,
        attention_max,
    }
}

fn pct(v: &[f64], p: f64) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    s[((s.len() - 1) as f64 * p).round() as usize]
}

fn main() {
    let args: Vec<i64> = std::env::args()
        .skip(1)
        .filter_map(|a| a.parse().ok())
        .collect();
    let symbols = *args.first().unwrap_or(&3_000) as usize;
    let members = *args.get(1).unwrap_or(&10) as usize;
    let bars = *args.get(2).unwrap_or(&240);
    println!("{symbols} symbols, {members} Ross members, {bars} barriers of 15 s");
    // Alternate the two so drift on this machine hits both alike; keep the medians.
    let (mut on, mut off) = (Vec::new(), Vec::new());
    for _ in 0..5 {
        off.push(run(symbols, members, bars, false));
        on.push(run(symbols, members, bars, true));
    }
    let med = |rs: &[Run], f: &dyn Fn(&Run) -> f64| {
        let mut v: Vec<f64> = rs.iter().map(f).collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[v.len() / 2]
    };
    for (name, rs) in [("cue off", &off), ("cue on ", &on)] {
        println!(
            "{name}: barrier p50 {:.1} us, p99 {:.1} us, mean {:.1} us; apply {:.0} ns/print; held {:.1} KiB; setup log {} entries; max attention {}",
            med(rs, &|r| pct(&r.barrier_us, 0.5)),
            med(rs, &|r| pct(&r.barrier_us, 0.99)),
            med(rs, &|r| r.barrier_us.iter().sum::<f64>()
                / r.barrier_us.len() as f64),
            med(rs, &|r| r.apply_ns),
            med(rs, &|r| r.live_bytes as f64) / 1024.0,
            rs[0].setup_entries,
            rs[0].attention_max,
        );
    }
    let d = |f: &dyn Fn(&Run) -> f64| med(&on, f) - med(&off, f);
    println!(
        "added by the cue: barrier p50 {:+.1} us, mean {:+.1} us; apply {:+.0} ns/print; held {:+.1} KiB ({:+.1} KiB per member)",
        d(&|r| pct(&r.barrier_us, 0.5)),
        d(&|r| r.barrier_us.iter().sum::<f64>() / r.barrier_us.len() as f64),
        d(&|r| r.apply_ns),
        d(&|r| r.live_bytes as f64) / 1024.0,
        d(&|r| r.live_bytes as f64) / 1024.0 / members.max(1) as f64,
    );
    println!(
        "peak heap over all runs: {:.1} MiB",
        PEAK.load(Ordering::Relaxed) as f64 / 1048576.0
    );
}
