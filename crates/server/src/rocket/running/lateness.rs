//! Runners lateness export (research only, run by hand;
//! `docs/research/post-alert-timing/protocol.md` §9–§10):
//!
//! ```text
//! LAT_ROOT=C:\research\stockwatcher\archive-hist LAT_DAY=2026-05-01 LAT_OUT=dir \
//!   LAT_ALERTS=alerts.jsonl [LAT_THREADS=8] \
//!   cargo test -p stockwatcher-server --release --lib running::lateness -- --ignored --nocapture
//! ```
//!
//! For **every** symbol of the day (the same band pre-filter as `paa.rs`) it builds clock-aligned
//! 1-min bars from 04:00 to 20:00 ET (prices from OHLC-counted prints, volume and trade count from
//! volume-counted prints, empty bars stay empty) and writes each completed bar that meets the
//! protocol's *causal early flag* — volume ≥ 5× the median of the 30 previous bars (empty bars
//! count as 0), close ≥ +3 % over the previous non-empty close, ≥ 5 trades — to `fires.jsonl`.
//! The trailing median is written beside each fire, so a reader can apply a stricter variant.
//!
//! For the symbols named in `LAT_ALERTS` (`{"s","t0","p0","path"}` per line: the P1 first alerts)
//! it also writes `alerts.jsonl`: the day's bars, the session's first print, the number of OHLC
//! prints before `t0`, the print-exact retrospective low `L*` in `[t0 − 60 min, t0]` (latest print
//! at the minimum) and high `P*` in `(t0, t0 + 60 min]`, and the scorecard-style follow-through
//! from the first print at or after `t0` (+10 % before −5 % within 30 min, no costs).
//! Everything retrospective is computed by the reader and labelled there; nothing here decides.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use market_archive::{Archive, ArchiveConfig, DayData};

use super::super::burst::replay_parity::load;
use super::*;

const SEC: i64 = 1_000_000_000;
const MIN: i64 = 60 * SEC;
const NB: usize = 16 * 60;

#[derive(Clone, Copy, Default)]
struct Bar {
    o: f64,
    h: f64,
    l: f64,
    c: f64,
    v: f64,
    n: u32,
}

fn median(xs: &[f64]) -> f64 {
    let mut v = xs.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

#[derive(serde::Deserialize)]
struct Alert {
    s: String,
    t0: i64,
    p0: f64,
    path: String,
}

#[test]
#[ignore = "export; needs an archive, run by hand"]
fn export_lateness() {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let root = PathBuf::from(env("LAT_ROOT").expect("LAT_ROOT"));
    let out = PathBuf::from(env("LAT_OUT").expect("LAT_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    let threads: usize = env("LAT_THREADS").map_or(8, |v| v.parse().unwrap());
    let day: NaiveDate = env("LAT_DAY").expect("LAT_DAY").parse().unwrap();
    let mut alerts: HashMap<String, Vec<Alert>> = HashMap::new();
    if let Some(p) = env("LAT_ALERTS") {
        for l in std::fs::read_to_string(p).unwrap().lines() {
            if l.trim().is_empty() {
                continue;
            }
            let a: Alert = serde_json::from_str(l).unwrap();
            alerts.entry(a.s.clone()).or_default().push(a);
        }
    }
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
    let prev_close: HashMap<String, f64> = closes
        .keys()
        .filter(|d| d.as_str() < day.to_string().as_str())
        .max()
        .and_then(|d| closes.get(d).cloned())
        .unwrap_or_default();
    let manifest = if live {
        archive.live_manifest(day)
    } else {
        archive.manifest(day)
    }
    .unwrap()
    .unwrap();
    let mut syms: Vec<String> = manifest
        .symbols
        .keys()
        .filter(|s| prev_close.get(*s).is_none_or(|c| (0.04..=60.0).contains(c)))
        .cloned()
        .collect();
    for s in alerts.keys() {
        if !syms.contains(s) {
            syms.push(s.clone());
        }
    }
    let t04 = market_events::session::et(day, 4, 0, 0, 0).unwrap().0;
    let t20 = t04 + NB as i64 * MIN;
    eprintln!("{day}: {} symbols, {} alert symbols, live={live}", syms.len(), alerts.len());
    let started = std::time::Instant::now();
    let next = AtomicUsize::new(0);
    let wf = Mutex::new(std::io::BufWriter::new(
        std::fs::File::create(out.join("fires.jsonl")).unwrap(),
    ));
    let wa = Mutex::new(std::io::BufWriter::new(
        std::fs::File::create(out.join("alerts.jsonl")).unwrap(),
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
                    // (t, price, ohlc, vol-counted size) in the 04:00–20:00 window.
                    let ps: Vec<(i64, f64, bool, f64)> = prints
                        .iter()
                        .filter(|(t, c)| {
                            (c.ohlc || c.volume) && t.event_ts.0 >= t04 && t.event_ts.0 < t20
                        })
                        .map(|(t, c)| {
                            (
                                t.event_ts.0,
                                t.price.micros() as f64 / 1e6,
                                c.ohlc,
                                if c.volume { t.size as f64 } else { 0.0 },
                            )
                        })
                        .collect();
                    if ps.is_empty() {
                        continue;
                    }
                    let mut bars = vec![Bar::default(); NB];
                    for &(t, p, ohlc, v) in &ps {
                        let b = &mut bars[((t - t04) / MIN) as usize];
                        if ohlc {
                            if b.o == 0.0 {
                                b.o = p;
                                b.h = p;
                                b.l = p;
                            }
                            b.h = b.h.max(p);
                            b.l = b.l.min(p);
                            b.c = p;
                        }
                        if v > 0.0 {
                            b.v += v;
                            b.n += 1;
                        }
                    }
                    // Causal early flag on completed bars (needs 30 earlier bars, so from 04:30).
                    let vols: Vec<f64> = bars.iter().map(|b| b.v).collect();
                    let mut prev_c = 0.0;
                    let mut fl = String::new();
                    for k in 0..NB {
                        let b = bars[k];
                        if k >= 30 && b.o > 0.0 && prev_c > 0.0 && b.n >= 5 {
                            let med = median(&vols[k - 30..k]);
                            if b.v >= 5.0 * med && b.c >= 1.03 * prev_c {
                                fl += &format!(
                                    "{}\n",
                                    serde_json::json!({"s": sym, "t": t04 + (k as i64 + 1) * MIN,
                                        "c": b.c, "pc": prev_c, "v": b.v, "med": med, "n": b.n})
                                );
                            }
                        }
                        if b.o > 0.0 {
                            prev_c = b.c;
                        }
                    }
                    if !fl.is_empty() {
                        wf.lock().unwrap().write_all(fl.as_bytes()).unwrap();
                    }
                    let Some(al) = alerts.get(sym) else { continue };
                    let oh: Vec<(i64, f64)> =
                        ps.iter().filter(|x| x.2).map(|x| (x.0, x.1)).collect();
                    let mut ol = String::new();
                    for a in al {
                        let t0 = a.t0;
                        let first = oh.first().copied();
                        let n_before = oh.iter().filter(|x| x.0 < t0).count();
                        let mut lstar: Option<(i64, f64)> = None;
                        let mut pstar: Option<(i64, f64)> = None;
                        for &(t, p) in &oh {
                            if t >= t0 - 60 * MIN && t <= t0 && lstar.is_none_or(|l| p <= l.1) {
                                lstar = Some((t, p));
                            }
                            if t > t0 && t <= t0 + 60 * MIN && pstar.is_none_or(|h| p > h.1) {
                                pstar = Some((t, p));
                            }
                        }
                        // Follow-through from the first print at or after t0.
                        let mut ft = "noprint";
                        let mut entry: Option<(i64, f64)> = None;
                        if let Some(j) = oh.iter().position(|x| x.0 >= t0) {
                            let (te, pe) = oh[j];
                            entry = Some((te, pe));
                            ft = "timeout";
                            for &(t, p) in &oh[j + 1..] {
                                if t > te + 30 * MIN {
                                    break;
                                }
                                if p >= pe * 1.10 {
                                    ft = "win";
                                    break;
                                }
                                if p <= pe * 0.95 {
                                    ft = "loss";
                                    break;
                                }
                            }
                        }
                        let bo: Vec<[f64; 6]> = bars
                            .iter()
                            .map(|b| [b.o, b.h, b.l, b.c, b.v, b.n as f64])
                            .collect();
                        ol += &format!(
                            "{}\n",
                            serde_json::json!({"s": sym, "t0": t0, "p0": a.p0, "path": a.path,
                                "t04": t04, "first": first, "n_before": n_before,
                                "lstar": lstar, "pstar": pstar, "entry": entry, "ft": ft,
                                "bars": bo})
                        );
                    }
                    wa.lock().unwrap().write_all(ol.as_bytes()).unwrap();
                }
            });
        }
    });
    wf.lock().unwrap().flush().unwrap();
    wa.lock().unwrap().flush().unwrap();
    eprintln!("done in {:.0} s", started.elapsed().as_secs_f64());
}
