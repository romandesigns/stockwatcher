//! How often the Runners entry's early fire (module docs, "Early fire") is NOT confirmed by the
//! 15 s closes, run by hand:
//!
//! ```text
//! CALIB_ROOT=C:\research\stockwatcher\archive-vps CALIB_DAYS=2026-09-22,2026-09-23 \
//!   cargo test -p stockwatcher-server --release --lib zones::early_calib -- --ignored --nocapture
//! ```
//!
//! For each day, every symbol whose raw daily bar touches `$0.10–$25` goes through the archive
//! reader and the production chart engine (each print's counted verdict, the market's own
//! input), into a board of its own with the early fire on, ticked once a second while it trades
//! (as the market ticks it). An early fire is confirmed when the same symbol's 15 s-close run
//! (the reference) marks within the 15 minutes after it (or the slot before); the lead is how
//! long before that run's decision (mark end + 1 s) the early fire came. `CALIB_SYMBOLS=A,B`
//! limits a day to those symbols.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use market_archive::{Archive, ArchiveConfig, DayData};

use super::*;
use crate::rocket::burst::replay_parity::load;

#[derive(Default)]
struct Tally {
    early: usize,
    confirmed: usize,
    unconfirmed: Vec<String>,
    leads_s: Vec<f64>,
    /// 15 s runs with no early fire before them (the guard or the clock held it to the close).
    late_only: usize,
    runs: usize,
}

#[test]
#[ignore = "calibration; needs an archive, run by hand"]
fn early_fire_vs_closes() {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let root = PathBuf::from(env("CALIB_ROOT").expect("CALIB_ROOT"));
    let threads: usize = env("CALIB_THREADS").map_or(4, |v| v.parse().unwrap());
    let only: Option<Vec<String>> =
        env("CALIB_SYMBOLS").map(|v| v.split(',').map(str::to_string).collect());
    let archive = Archive::new(ArchiveConfig {
        root,
        retention_days: 0,
        min_free_bytes: 0,
    });
    let band = (
        RUN_BAND_MICROS.0 as f64 / 1e6,
        RUN_BAND_MICROS.1 as f64 / 1e6,
    );
    let mut all = Tally::default();
    for day in env("CALIB_DAYS").expect("CALIB_DAYS").split(',') {
        let day: NaiveDate = day.parse().unwrap();
        let started = std::time::Instant::now();
        let live = match archive.day_data(day) {
            Some(DayData::Final) => false,
            Some(DayData::Provisional { .. }) => true,
            None => {
                eprintln!("{day}: not in the archive");
                continue;
            }
        };
        let syms: Vec<String> = match &only {
            Some(v) => v.clone(),
            None => {
                let manifest = if live {
                    archive.live_manifest(day)
                } else {
                    archive.manifest(day)
                }
                .unwrap()
                .unwrap();
                let raw = archive.read_raw_bars(day).ok().flatten();
                manifest
                    .symbols
                    .keys()
                    .filter(|s| {
                        let Some(Some(Some(b))) = raw.as_ref().map(|d| d.coverage(s)) else {
                            return true;
                        };
                        let (lo, hi) = (b.l.unwrap_or(b.c), b.h.unwrap_or(b.c));
                        !(lo > band.1 || hi < band.0)
                    })
                    .cloned()
                    .collect()
            }
        };
        let next = AtomicUsize::new(0);
        let tally = Mutex::new(Tally::default());
        std::thread::scope(|sc| {
            for _ in 0..threads {
                sc.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(sym) = syms.get(i) else { break };
                        let Ok(prints) = load(&archive, day, sym, live) else {
                            continue;
                        };
                        if prints.is_empty() {
                            continue;
                        }
                        let mut b = ZoneBoard::new(true);
                        b.set_early(true);
                        let mut got: Vec<Run> = Vec::new();
                        let mut j = 0;
                        let first = prints[0].0.event_ts.0;
                        let mut now = first - first.rem_euclid(SEC_NS) + SEC_NS;
                        while j < prints.len() {
                            // Apply what executed before this second, then tick at it.
                            while j < prints.len() && prints[j].0.event_ts.0 < now {
                                b.apply_trade(&prints[j].0, prints[j].1);
                                j += 1;
                            }
                            b.tick(now);
                            got.extend(b.take_entries().into_iter().map(|(_, r)| r));
                            // Idle: jump to the second after the next print, ticking once at
                            // 20 s so the slots in progress are decided first.
                            if j < prints.len() && prints[j].0.event_ts.0 > now + 20 * SEC_NS {
                                b.tick(now + 20 * SEC_NS);
                                got.extend(b.take_entries().into_iter().map(|(_, r)| r));
                                let e = prints[j].0.event_ts.0;
                                now = e - e.rem_euclid(SEC_NS);
                            }
                            now += SEC_NS;
                        }
                        b.tick(now + 20 * SEC_NS);
                        got.extend(b.take_entries().into_iter().map(|(_, r)| r));
                        let closes: Vec<&Run> = got.iter().filter(|r| !r.early).collect();
                        let mut t = tally.lock().unwrap();
                        t.runs += closes.len();
                        for c in &closes {
                            let decided = c.plus20_ns + SETTLE_NS;
                            if !got.iter().any(|e| {
                                e.early && e.plus20_ns <= decided && e.plus20_ns > c.start_ns
                            }) {
                                t.late_only += 1;
                            }
                        }
                        for e in got.iter().filter(|r| r.early) {
                            t.early += 1;
                            let m = closes.iter().find(|c| {
                                c.plus20_ns >= e.plus20_ns - BAR_NS
                                    && c.plus20_ns <= e.plus20_ns + LEG_SLOTS * BAR_NS
                            });
                            match m {
                                Some(c) => {
                                    t.confirmed += 1;
                                    t.leads_s
                                        .push((c.plus20_ns + SETTLE_NS - e.plus20_ns) as f64 / 1e9);
                                }
                                None => t.unconfirmed.push(format!(
                                    "{sym} {} ${:.4} from ${:.4}",
                                    e.plus20_ns, e.close, e.start_price
                                )),
                            }
                        }
                    }
                });
            }
        });
        let t = tally.into_inner().unwrap();
        report(&format!("{day} ({} symbols)", syms.len()), &t);
        eprintln!("  {:.0} s", started.elapsed().as_secs_f64());
        all.early += t.early;
        all.confirmed += t.confirmed;
        all.unconfirmed.extend(t.unconfirmed);
        all.leads_s.extend(t.leads_s);
        all.late_only += t.late_only;
        all.runs += t.runs;
    }
    report("all days", &all);
}

fn report(what: &str, t: &Tally) {
    let mut l = t.leads_s.clone();
    l.sort_by(f64::total_cmp);
    let q = |p: f64| {
        l.get(((l.len() as f64 - 1.0) * p).round() as usize)
            .copied()
    };
    eprintln!(
        "{what}: {} early fires, {} confirmed by the 15 s closes, {} not ({:.1} %); \
         lead before the 15 s decision median {:?} s, p10 {:?} s, p90 {:?} s; \
         {} 15 s runs, {} with no early fire before them",
        t.early,
        t.confirmed,
        t.unconfirmed.len(),
        100.0 * t.unconfirmed.len() as f64 / t.early.max(1) as f64,
        q(0.5),
        q(0.1),
        q(0.9),
        t.runs,
        t.late_only,
    );
    for u in t.unconfirmed.iter().take(15) {
        eprintln!("  unconfirmed: {u}");
    }
}
