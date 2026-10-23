//! Missed-movers export (run by hand): one archive day through the production chart engine and
//! the current detectors, per symbol, with nothing restarted:
//!
//! ```text
//! MOVERS_ROOT=C:\research\stockwatcher\archive-vps MOVERS_DAY=2026-10-22 MOVERS_OUT=dir \
//!   cargo test -p stockwatcher-server --release --lib running::movers -- --ignored --nocapture
//! ```
//!
//! Writes `<out>/bars.csv` (15 s bars of the symbols that moved: OHLC from OHLC-counted prints,
//! volume/trades/dollars from volume-counted ones), `running.jsonl` (every barrier where
//! `running::decide` holds), `loose.jsonl` (calib's loose candidates), `burst.jsonl` (alerts at
//! the default policy and each symbol's best pass) and `zones.jsonl` (the volume-shelf board's
//! forward log).

use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use market_archive::{Archive, ArchiveConfig, DayData};

use super::super::burst::replay_parity::load;
use super::super::burst::{self, BurstDetector, BurstPolicy};
use super::super::features::{self, Barrier, Tape};
use super::*;
use crate::zones::ZoneBoard;

const SEC: i64 = 1_000_000_000;

#[test]
#[ignore = "export; needs an archive, run by hand"]
fn export_movers() {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let root = PathBuf::from(env("MOVERS_ROOT").expect("MOVERS_ROOT"));
    let out = PathBuf::from(env("MOVERS_OUT").expect("MOVERS_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    let threads: usize = env("MOVERS_THREADS").map_or(4, |v| v.parse().unwrap());
    let day: NaiveDate = env("MOVERS_DAY").expect("MOVERS_DAY").parse().unwrap();
    let only: Option<Vec<String>> =
        env("MOVERS_SYMBOLS").map(|v| v.split(',').map(str::to_string).collect());
    let archive = Archive::new(ArchiveConfig {
        root: root.clone(),
        retention_days: 0,
        min_free_bytes: 0,
    });
    let live = match archive.day_data(day) {
        Some(DayData::Final) => false,
        Some(DayData::Provisional { .. }) => true,
        None => panic!("{day}: not in the archive"),
    };
    let closes: HashMap<String, HashMap<String, f64>> =
        serde_json::from_slice(&std::fs::read(root.join("daily-closes.json")).unwrap()).unwrap();
    let prev_date = closes
        .keys()
        .filter(|d| d.as_str() < day.to_string().as_str())
        .max()
        .cloned();
    let prev_close: HashMap<String, f64> = prev_date
        .clone()
        .and_then(|d| closes.get(&d).cloned())
        .unwrap_or_default();
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
            manifest
                .symbols
                .keys()
                .filter(|s| prev_close.get(*s).is_none_or(|c| (0.04..=60.0).contains(c)))
                .cloned()
                .collect()
        }
    };
    let rfacts = facts(&archive, day);
    let adv = burst::facts::adv30(&archive, day);
    eprintln!(
        "{day}: {} symbols, prev close date {prev_date:?}, {} running facts, {} ADV30",
        syms.len(),
        rfacts.len(),
        adv.len()
    );
    let t0 = market_events::session::et(day, 4, 0, 0, 0).unwrap().0;
    let started = std::time::Instant::now();
    let next = AtomicUsize::new(0);
    let files = [
        "bars.csv",
        "running.jsonl",
        "loose.jsonl",
        "burst.jsonl",
        "zones.jsonl",
    ];
    let w: Vec<Mutex<std::io::BufWriter<std::fs::File>>> = files
        .iter()
        .map(|f| {
            Mutex::new(std::io::BufWriter::new(
                std::fs::File::create(out.join(f)).unwrap(),
            ))
        })
        .collect();
    writeln!(w[0].lock().unwrap(), "s,k,o,h,l,c,v,n,d").unwrap();
    let done = AtomicUsize::new(0);
    std::thread::scope(|sc| {
        for _ in 0..threads {
            sc.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(sym) = syms.get(i) else { break };
                    let n = done.fetch_add(1, Ordering::Relaxed);
                    if n.is_multiple_of(1000) {
                        eprintln!("  {n} symbols, {:.0} s", started.elapsed().as_secs_f64());
                    }
                    let Ok(prints) = load(&archive, day, sym, live) else {
                        continue;
                    };
                    if prints.is_empty() {
                        continue;
                    }
                    // Quick skip: nothing in a wide band.
                    let (mut lo, mut hi) = (f64::MAX, 0f64);
                    for (t, c) in &prints {
                        if c.ohlc {
                            let p = t.price.micros() as f64 / 1e6;
                            lo = lo.min(p);
                            hi = hi.max(p);
                        }
                    }
                    if hi < 0.08 || lo > 30.0 {
                        continue;
                    }
                    let pc = prev_close.get(sym.as_str()).copied();

                    // 1. 15 s bars.
                    // MOVERS_LITE: running candidates (for every symbol) and shelves only.
                    let lite = env("MOVERS_LITE").is_some();
                    let moved = hi / lo >= 1.15 || pc.is_some_and(|c| hi / c >= 1.25);
                    if moved && !lite {
                        let mut bars: Vec<[f64; 8]> = Vec::new();
                        let mut ks: Vec<i64> = Vec::new();
                        for (t, c) in &prints {
                            let k = (t.event_ts.0 - t0).div_euclid(features::BAR_NS);
                            if !(0..features::NSLOT).contains(&k) || !(c.ohlc || c.volume) {
                                continue;
                            }
                            if ks.last() != Some(&k) {
                                ks.push(k);
                                bars.push([f64::NAN, f64::NAN, f64::NAN, f64::NAN, 0.0, 0.0, 0.0, 0.0]);
                            }
                            let b = bars.last_mut().unwrap();
                            let p = t.price.micros() as f64 / 1e6;
                            if c.ohlc {
                                if b[0].is_nan() {
                                    b[0] = p;
                                    b[1] = p;
                                    b[2] = p;
                                }
                                b[1] = b[1].max(p);
                                b[2] = b[2].min(p);
                                b[3] = p;
                            }
                            if c.volume {
                                b[4] += t.size as f64;
                                b[5] += 1.0;
                                b[6] += t.size as f64 * p;
                            }
                        }
                        let mut s = String::new();
                        let f = |x: f64| if x.is_nan() { String::new() } else { format!("{x}") };
                        for (k, b) in ks.iter().zip(&bars) {
                            s += &format!(
                                "{sym},{k},{},{},{},{},{},{},{:.0}\n",
                                f(b[0]),
                                f(b[1]),
                                f(b[2]),
                                f(b[3]),
                                b[4],
                                b[5],
                                b[6]
                            );
                        }
                        w[0].lock().unwrap().write_all(s.as_bytes()).unwrap();
                    }

                    // 2. Running entry (calib's loop).
                    let fct = rfacts.get(sym);
                    let mut tape = Tape::default();
                    let (tt0, ord) = tape.day(day);
                    let mut j = 0;
                    let mut run_lines = String::new();
                    let mut loose_lines = String::new();
                    for slot in 0..features::NSLOT {
                        let end = tt0 + (slot + 1) * features::BAR_NS;
                        while j < prints.len() && prints[j].0.event_ts.0 < end {
                            tape.apply_trade(&prints[j].0, prints[j].1);
                            j += 1;
                        }
                        let Some(&ix) = tape.index.get(sym.as_str()) else {
                            continue;
                        };
                        let b = Barrier::new(tt0, ord, slot);
                        let s = &mut tape.syms[ix];
                        if !features::settle(s, &b) {
                            continue;
                        }
                        let Some(x) = features::run_inputs(s, &b) else {
                            continue;
                        };
                        if let Some(e) = decide(&x, fct) {
                            run_lines += &format!(
                                "{}\n",
                                serde_json::json!({"s": sym, "t": b.at_ns, "p": e.price,
                                    "lo": e.base_low, "gain": e.gain, "req": e.req,
                                    "d5": e.dollars5, "rvol30": e.rvol30})
                            );
                        }
                        if !(x.fresh_high || x.fresh_high60)
                            || x.dollars5 < 25_000.0
                            || !(BAND.0..=BAND.1).contains(&x.price)
                            || x.base_low.is_nan()
                            || x.base_low <= 0.0
                            || x.price / x.base_low - 1.0 < 0.04
                        {
                            continue;
                        }
                        loose_lines += &format!(
                            "{}\n",
                            serde_json::json!({
                                "s": sym, "t": b.at_ns, "p": x.price, "lo": x.base_low,
                                "d5": x.dollars5, "v30": x.vol30,
                                "fs": x.fresh_high, "fh": x.fresh_high60,
                                "vol": fct.and_then(|f| f.vol), "adv": fct.and_then(|f| f.adv),
                            })
                        );
                    }
                    drop(tape);
                    if !run_lines.is_empty() {
                        w[1].lock().unwrap().write_all(run_lines.as_bytes()).unwrap();
                    }
                    if !loose_lines.is_empty() && (moved || lite) {
                        w[2].lock().unwrap().write_all(loose_lines.as_bytes()).unwrap();
                    }

                    // 3. Burst detector over the whole day (no restart).
                    if !lite {
                    let mut d = BurstDetector::new(BurstPolicy::default());
                    d.set_adv(day, adv.clone());
                    d.capture();
                    let pcf = |s: &str| prev_close.get(s).copied();
                    let mut alerts = Vec::new();
                    let mut best: Option<(f64, i64, f64)> = None;
                    let dump = env("MOVERS_PASSES").is_some();
                    let mut passes = String::new();
                    let mut j = 0;
                    let mut wb = t0;
                    while wb <= t0 + burst::NSEC * SEC {
                        let now = wb + burst::SETTLE_NS;
                        while j < prints.len() && prints[j].0.event_ts.0 <= now {
                            d.apply_trade(&prints[j].0, prints[j].1);
                            j += 1;
                        }
                        alerts.extend(d.tick(now, &pcf));
                        if (wb - t0) % (60 * SEC) == 0 {
                            for p in d.take_captured() {
                                if dump {
                                    passes += &format!(
                                        "{}\n",
                                        serde_json::json!({"s": sym, "t": p.t_ns, "p": p.last,
                                            "score": p.score, "rate_x": p.rate_x,
                                            "new_highs": p.new_highs, "sweeps": p.sweeps})
                                    );
                                }
                                if best.is_none_or(|b| p.score > b.0) {
                                    best = Some((p.score, p.t_ns, p.last));
                                }
                            }
                        }
                        wb += burst::STEP_S * SEC;
                    }
                    for p in d.take_captured() {
                        if best.is_none_or(|b| p.score > b.0) {
                            best = Some((p.score, p.t_ns, p.last));
                        }
                    }
                    let mut bl = String::new();
                    for a in &alerts {
                        bl += &format!(
                            "{}\n",
                            serde_json::json!({"kind": "alert", "s": sym, "t": a.at_ns,
                                "p": a.price, "score": a.pass.score})
                        );
                    }
                    if let Some((sc_, t, p)) = best {
                        bl += &format!(
                            "{}\n",
                            serde_json::json!({"kind": "best", "s": sym, "t": t, "p": p, "score": sc_})
                        );
                    }
                    if !passes.is_empty() {
                        std::fs::write(out.join(format!("passes-{sym}.jsonl")), &passes).unwrap();
                    }
                    if !bl.is_empty() {
                        w[3].lock().unwrap().write_all(bl.as_bytes()).unwrap();
                    }
                    }

                    // 4. Volume shelves.
                    let mut zb = ZoneBoard::new(true);
                    zb.enable_log(HashSet::new());
                    let mut j = 0;
                    for slot in 0..crate::zones::NSLOT {
                        let end = t0 + (slot + 1) * crate::zones::BAR_NS;
                        while j < prints.len() && prints[j].0.event_ts.0 < end {
                            zb.apply_trade(&prints[j].0, prints[j].1);
                            j += 1;
                        }
                        zb.tick(end + crate::zones::SETTLE_NS);
                    }
                    let end = t0 + crate::zones::NSLOT * crate::zones::BAR_NS;
                    zb.tick(end + crate::zones::HORIZON_NS + 2 * crate::zones::BAR_NS);
                    let zl: String = zb.take_log().into_iter().map(|(_, l)| l + "\n").collect();
                    if !zl.is_empty() {
                        w[4].lock().unwrap().write_all(zl.as_bytes()).unwrap();
                    }
                }
            });
        }
    });
    for f in &w {
        f.lock().unwrap().flush().unwrap();
    }
    eprintln!("done in {:.0} s", started.elapsed().as_secs_f64());
}
