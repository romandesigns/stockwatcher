//! Calibration export of the fast-move trigger (module docs), run by hand:
//!
//! ```text
//! FAST_ROOT=C:\research\stockwatcher\archive-vps FAST_DAYS=2026-09-22,2026-09-23 \
//! FAST_OUT=out-dir cargo test -p stockwatcher-server --release --lib rocket::fast::calib \
//!   -- --ignored --nocapture
//! ```
//!
//! For each day, every symbol goes through the archive reader and the production chart engine
//! (each print's counted verdict), then a [`FastTape`] second by second, 04:00–20:00 ET. Every
//! second where a loose version of the rule holds (in band, some window with the held price
//! ≥ 4 % over its low, ≥ 5 trades and ≥ $5k) is written to `<out>/<day>/cand.jsonl` with what
//! the rule reads plus the forward outcome from the last price (`o`: first +10 % before −5 %
//! within 30 minutes = 1, −5 % first = −1, neither = 0; a second that touches both counts −1;
//! `mx`: max high over the 30 minutes, relative), so the constants can be swept offline.
//! `<out>/<day>/running.jsonl` holds each symbol's first running entry ([`running::decide`]), to
//! count the fast trigger's extra symbols. `FAST_SYMBOLS=A,B` limits a day to those symbols.

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::NaiveDate;
use market_archive::{Archive, ArchiveConfig, DayData};

use super::super::burst::replay_parity::load;
use super::super::features::{self, Barrier, Tape};
use super::super::running;
use super::*;

const SEC: i64 = 1_000_000_000;
const SPAN: i64 = 16 * 3600;
const FWD: i64 = 1800;

#[test]
#[ignore = "calibration; needs an archive, run by hand"]
fn export_fast() {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let root = PathBuf::from(env("FAST_ROOT").expect("FAST_ROOT"));
    let out0 = PathBuf::from(env("FAST_OUT").expect("FAST_OUT"));
    let threads: usize = env("FAST_THREADS").map_or(3, |v| v.parse().unwrap());
    let only: Option<Vec<String>> =
        env("FAST_SYMBOLS").map(|v| v.split(',').map(str::to_string).collect());
    let archive = Archive::new(ArchiveConfig {
        root,
        retention_days: 0,
        min_free_bytes: 0,
    });
    for day in env("FAST_DAYS").expect("FAST_DAYS").split(',') {
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
        let out = out0.join(day.to_string());
        std::fs::create_dir_all(&out).unwrap();
        let syms: Vec<String> = match &only {
            Some(v) => v.clone(),
            None => {
                let m = if live {
                    archive.live_manifest(day)
                } else {
                    archive.manifest(day)
                }
                .unwrap()
                .unwrap();
                m.symbols.keys().cloned().collect()
            }
        };
        let rfacts = running::facts(&archive, day);
        let t0 = market_events::session::et(day, 4, 0, 0, 0).unwrap().0;
        let s0 = t0.div_euclid(SEC);
        let next = AtomicUsize::new(0);
        let wc = Mutex::new(std::io::BufWriter::new(
            std::fs::File::create(out.join("cand.jsonl")).unwrap(),
        ));
        let wr = Mutex::new(std::io::BufWriter::new(
            std::fs::File::create(out.join("running.jsonl")).unwrap(),
        ));
        std::thread::scope(|sc| {
            for _ in 0..threads {
                sc.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(sym) = syms.get(i) else { break };
                        let Ok(prints) = load(&archive, day, sym, live) else {
                            continue;
                        };
                        let (mut lo, mut hi) = (f64::MAX, 0f64);
                        for (t, c) in &prints {
                            if c.ohlc {
                                let p = t.price.micros() as f64 / 1e6;
                                lo = lo.min(p);
                                hi = hi.max(p);
                            }
                        }
                        if hi < 0.10 || lo > 25.0 || hi / lo < 1.04 {
                            continue;
                        }
                        let fct = rfacts.get(sym.as_str());
                        let vol = fct.and_then(|f| f.vol);
                        let adv = fct.and_then(|f| f.adv);

                        // Per-second highs/lows for the forward outcome.
                        let n = SPAN as usize;
                        let mut shi = vec![f64::NAN; n];
                        let mut slo = vec![f64::NAN; n];
                        for (t, c) in &prints {
                            let k = t.event_ts.0.div_euclid(SEC) - s0;
                            if !c.ohlc || !(0..SPAN).contains(&k) {
                                continue;
                            }
                            let p = t.price.micros() as f64 / 1e6;
                            let k = k as usize;
                            if shi[k].is_nan() || p > shi[k] {
                                shi[k] = p;
                            }
                            if slo[k].is_nan() || p < slo[k] {
                                slo[k] = p;
                            }
                        }

                        let mut ft = FastTape::default();
                        let mut j = 0;
                        let mut lines = String::new();
                        let mut prev_traded = -1i64;
                        for k in 0..SPAN {
                            let end = (s0 + k + 1) * SEC;
                            let mut any = false;
                            while j < prints.len() && prints[j].0.event_ts.0 < end {
                                let (t, c) = &prints[j];
                                ft.apply(
                                    t.event_ts.0,
                                    t.price.micros() as f64 / 1e6,
                                    t.size,
                                    c.ohlc,
                                    c.volume,
                                );
                                any = true;
                                j += 1;
                            }
                            if any {
                                prev_traded = k;
                            }
                            // Nothing changes 2 s after the last print except windows emptying.
                            if prev_traded < 0 || k - prev_traded > 2 {
                                continue;
                            }
                            let Some(x) = ft.inputs(s0 + k) else { continue };
                            if !(BAND.0..=BAND.1).contains(&x.held) {
                                continue;
                            }
                            let loose = x.win.iter().any(|w| {
                                w.low > 0.0
                                    && x.held / w.low - 1.0 >= 0.04
                                    && w.trades >= 5
                                    && w.dollars >= 5_000.0
                            });
                            if !loose {
                                continue;
                            }
                            // Forward outcome from the last price.
                            let p = x.price;
                            let (up, dn) = (p * 1.10, p * 0.95);
                            let mut o = 0;
                            let mut ot = 0;
                            let mut mx = p;
                            for f in (k + 1)..(k + 1 + FWD).min(SPAN) {
                                let f = f as usize;
                                if shi[f].is_nan() {
                                    continue;
                                }
                                mx = mx.max(shi[f]);
                                if o == 0 {
                                    if slo[f] <= dn {
                                        o = -1;
                                        ot = f as i64 - k;
                                    } else if shi[f] >= up {
                                        o = 1;
                                        ot = f as i64 - k;
                                    }
                                }
                            }
                            let w = |i: usize| {
                                let v = &x.win[i];
                                serde_json::json!([v.low, v.trades, (v.dollars).round()])
                            };
                            lines += &format!(
                                "{}\n",
                                serde_json::json!({"s": sym, "t": end, "p": p, "h": x.held,
                                    "w15": w(0), "w30": w(1), "w60": w(2),
                                    "vol": vol, "adv": adv, "o": o, "ot": ot, "mx": mx / p - 1.0})
                            );
                        }
                        if !lines.is_empty() {
                            wc.lock().unwrap().write_all(lines.as_bytes()).unwrap();
                        }

                        // The first running entry, the board's barrier loop.
                        if hi / lo >= 1.08 {
                            let mut tape = Tape::default();
                            let (tt0, ord) = tape.day(day);
                            let mut j = 0;
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
                                if let Some(e) = running::decide(&x, fct) {
                                    writeln!(
                                        wr.lock().unwrap(),
                                        "{}",
                                        serde_json::json!({"s": sym, "t": b.at_ns, "p": e.price})
                                    )
                                    .unwrap();
                                    break;
                                }
                            }
                        }
                    }
                });
            }
        });
        wc.lock().unwrap().flush().unwrap();
        wr.lock().unwrap().flush().unwrap();
        eprintln!(
            "{day}: {} symbols in {:.0} s",
            syms.len(),
            started.elapsed().as_secs_f64()
        );
    }
}
