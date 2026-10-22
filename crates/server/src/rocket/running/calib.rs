//! Calibration of the running entry (module docs), run by hand:
//!
//! ```text
//! CALIB_ROOT=C:\research\stockwatcher\archive-vps CALIB_DAYS=2026-09-22,2026-09-23 \
//! CALIB_OUT=out-dir cargo test -p stockwatcher-server --release --lib running::calib \
//!   -- --ignored --nocapture
//! ```
//!
//! For each day, every symbol whose raw daily bar touches the band goes through the archive
//! reader, the production chart engine (each print's counted verdict) and a tape of its own,
//! barrier by barrier. Every barrier where the rule's loose version holds (a fresh session high,
//! ≥ $25k in 5 minutes, ≥ 4 % off the 60-minute low, in band) is written as a candidate line with
//! what the rule reads, so the constants can be swept offline without replaying again:
//! `<out>/<day>.jsonl`. `CALIB_SYMBOLS=A,B` limits a day to those symbols.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use market_archive::{Archive, ArchiveConfig, DayData};

use super::super::burst::replay_parity::load;
use super::super::features::{self, Barrier, Tape};
use super::*;

#[test]
#[ignore = "calibration; needs an archive, run by hand"]
fn calibrate() {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let root = PathBuf::from(env("CALIB_ROOT").expect("CALIB_ROOT"));
    let out = PathBuf::from(env("CALIB_OUT").expect("CALIB_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    let threads: usize = env("CALIB_THREADS").map_or(3, |v| v.parse().unwrap());
    let only: Option<Vec<String>> =
        env("CALIB_SYMBOLS").map(|v| v.split(',').map(str::to_string).collect());
    let archive = Archive::new(ArchiveConfig {
        root,
        retention_days: 0,
        min_free_bytes: 0,
    });
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
                        !(lo > BAND.1 || hi < BAND.0)
                    })
                    .cloned()
                    .collect()
            }
        };
        let facts = facts(&archive, day);
        let next = AtomicUsize::new(0);
        let lines = Mutex::new(Vec::<String>::new());
        std::thread::scope(|sc| {
            for _ in 0..threads {
                sc.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(sym) = syms.get(i) else { break };
                        let Ok(prints) = load(&archive, day, sym, live) else {
                            continue;
                        };
                        let f = facts.get(sym);
                        let mut tape = Tape::default();
                        let (t0, ord) = tape.day(day);
                        let mut j = 0;
                        let mut mine = Vec::new();
                        for slot in 0..features::NSLOT {
                            let end = t0 + (slot + 1) * features::BAR_NS;
                            while j < prints.len() && prints[j].0.event_ts.0 < end {
                                tape.apply_trade(&prints[j].0, prints[j].1);
                                j += 1;
                            }
                            let Some(&ix) = tape.index.get(sym.as_str()) else {
                                continue;
                            };
                            let b = Barrier::new(t0, ord, slot);
                            let s = &mut tape.syms[ix];
                            if !features::settle(s, &b) {
                                continue;
                            }
                            let Some(x) = features::run_inputs(s, &b) else {
                                continue;
                            };
                            if !(x.fresh_high || x.fresh_high60)
                                || x.dollars5 < 25_000.0
                                || !(BAND.0..=BAND.1).contains(&x.price)
                                || x.base_low.is_nan()
                                || x.base_low <= 0.0
                                || x.price / x.base_low - 1.0 < 0.04
                            {
                                continue;
                            }
                            mine.push(
                                serde_json::json!({
                                    "s": sym, "t": b.at_ns, "p": x.price, "lo": x.base_low,
                                    "d5": x.dollars5, "v30": x.vol30,
                                    "fs": x.fresh_high, "fh": x.fresh_high60,
                                    "vol": f.and_then(|f| f.vol), "adv": f.and_then(|f| f.adv),
                                })
                                .to_string(),
                            );
                        }
                        lines.lock().unwrap().extend(mine);
                    }
                });
            }
        });
        let lines = lines.into_inner().unwrap();
        std::fs::write(out.join(format!("{day}.jsonl")), lines.join("\n")).unwrap();
        eprintln!(
            "{day}: {} symbols, {} candidate barriers, {:.0} s",
            syms.len(),
            lines.len(),
            started.elapsed().as_secs_f64()
        );
    }
}
