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
//!
//! The scorecard's additions (2026-10-23): `fast.jsonl` (the fast-move rule second by second,
//! with its 30-minute refractory; the one-row-per-symbol and daily caps are the reader's) and
//! `run.jsonl` (the "Run +20%" entries from a board with the per-second early fire on, ticked as
//! the zones early-fire calibration does), each line with its forward outcome from the fire's
//! price. `MOVERS_PROBES=file.jsonl` (lines `{"id","s","t","p"}`, e.g. every live row of the
//! day) adds the same forward outcome per probe to `probes.jsonl`, and the probes' symbols to
//! `bars.csv` even when they did not move 15 %. Outcome: `o` = first +10 % before -5 % within
//! 30 minutes (1), -5 % first (-1; a second touching both counts -1), neither (0); `mx30` /
//! `mx120` = max OHLC high over the next 30 / 120 minutes, relative to the price.

use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use market_archive::{Archive, ArchiveConfig, DayData};

use super::super::burst::replay_parity::load;
use super::super::burst::{self, BurstDetector, BurstPolicy};
use super::super::fast::{self as fastm, FastTape};
use super::super::features::{self, Barrier, Tape};
use super::*;
use crate::zones::ZoneBoard;

const SEC: i64 = 1_000_000_000;
/// 04:00-20:00 ET, seconds.
const SPAN: i64 = 16 * 3600;

/// Forward outcome from `p` after second `k` (module docs): `(o, ot, mx30, mx120)`.
fn forward(shi: &[f64], slo: &[f64], k: i64, p: f64) -> (i32, i64, f64, f64) {
    let (up, dn) = (p * 1.10, p * 0.95);
    let (mut o, mut ot) = (0, 0);
    let (mut m30, mut m120) = (p, p);
    for f in (k + 1).max(0)..(k + 1 + 7200).min(SPAN) {
        let fu = f as usize;
        if shi[fu].is_nan() {
            continue;
        }
        m120 = m120.max(shi[fu]);
        if f <= k + 1800 {
            m30 = m30.max(shi[fu]);
            if o == 0 {
                if slo[fu] <= dn {
                    (o, ot) = (-1, f - k);
                } else if shi[fu] >= up {
                    (o, ot) = (1, f - k);
                }
            }
        }
    }
    (o, ot, m30 / p - 1.0, m120 / p - 1.0)
}

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
    let s0 = t0.div_euclid(SEC);
    let mut probes: HashMap<String, Vec<serde_json::Value>> = HashMap::new();
    if let Some(f) = env("MOVERS_PROBES") {
        for l in std::fs::read_to_string(f).unwrap().lines() {
            let v: serde_json::Value = serde_json::from_str(l).unwrap();
            probes
                .entry(v["s"].as_str().unwrap().to_string())
                .or_default()
                .push(v);
        }
    }
    let mut syms = syms;
    for s in probes.keys() {
        if !syms.contains(s) {
            syms.push(s.clone());
        }
    }
    let started = std::time::Instant::now();
    let next = AtomicUsize::new(0);
    let files = [
        "bars.csv",
        "running.jsonl",
        "loose.jsonl",
        "burst.jsonl",
        "zones.jsonl",
        "fast.jsonl",
        "run.jsonl",
        "probes.jsonl",
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
                    let probe = probes.get(sym.as_str());
                    if (hi < 0.08 || lo > 30.0) && probe.is_none() {
                        continue;
                    }
                    // Per-second OHLC highs/lows for the forward outcomes.
                    let mut shi = vec![f64::NAN; SPAN as usize];
                    let mut slo = vec![f64::NAN; SPAN as usize];
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
                    if let Some(v) = probe {
                        let mut pl = String::new();
                        for x in v {
                            let t = x["t"].as_i64().unwrap();
                            let p = x["p"].as_f64().unwrap();
                            // The alert's second has passed by the time it shows.
                            let k = t.div_euclid(SEC) - s0 - 1;
                            let (o, ot, m30, m120) = forward(&shi, &slo, k, p);
                            // The newest print at or before the probe (the trigger's proxy for
                            // trade-to-phone latency when the source logs no exec time).
                            let i = prints.partition_point(|(tr, _)| tr.event_ts.0 <= t);
                            let last_ns = i.checked_sub(1).map(|i| prints[i].0.event_ts.0);
                            pl += &format!(
                                "{}\n",
                                serde_json::json!({"id": x["id"], "s": sym, "t": t, "p": p,
                                    "o": o, "ot": ot, "mx30": m30, "mx120": m120,
                                    "last_ns": last_ns})
                            );
                        }
                        w[7].lock().unwrap().write_all(pl.as_bytes()).unwrap();
                    }
                    let pc = prev_close.get(sym.as_str()).copied();

                    // 1. 15 s bars.
                    // MOVERS_LITE: running candidates (for every symbol) and shelves only.
                    let lite = env("MOVERS_LITE").is_some();
                    let moved = hi / lo >= 1.15 || pc.is_some_and(|c| hi / c >= 1.25);
                    if (moved || probe.is_some()) && !lite {
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

                    // 3b. Fast move, second by second (the live tick's reading, 30-min refractory).
                    if hi / lo >= 1.08 {
                        let mut ft = FastTape::default();
                        let mut j = 0;
                        let mut prev_traded = -1i64;
                        let mut fired: Option<i64> = None;
                        let mut fl = String::new();
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
                            if prev_traded < 0 || k - prev_traded > 2 {
                                continue;
                            }
                            let Some(e) = ft.inputs(s0 + k).and_then(|x| fastm::decide(&x))
                            else {
                                continue;
                            };
                            let at = (s0 + k) * SEC;
                            if fired.is_some_and(|f| at - f < fastm::REFRACTORY_NS) {
                                continue;
                            }
                            fired = Some(at);
                            let (o, ot, m30, m120) = forward(&shi, &slo, k, e.price);
                            fl += &format!(
                                "{}\n",
                                serde_json::json!({"s": sym, "t": at + SEC, "p": e.price,
                                    "secs": e.secs, "gain": e.gain, "o": o, "ot": ot,
                                    "mx30": m30, "mx120": m120})
                            );
                        }
                        if !fl.is_empty() {
                            w[5].lock().unwrap().write_all(fl.as_bytes()).unwrap();
                        }
                    }

                    // 3c. "Run +20%" entries with the early fire (zones early_calib's loop).
                    if hi / lo >= 1.18 {
                        let mut b = ZoneBoard::new(true);
                        b.set_early(true);
                        let mut got = Vec::new();
                        let mut j = 0;
                        let first = prints[0].0.event_ts.0;
                        let mut now = first - first.rem_euclid(SEC) + SEC;
                        while j < prints.len() {
                            while j < prints.len() && prints[j].0.event_ts.0 < now {
                                b.apply_trade(&prints[j].0, prints[j].1);
                                j += 1;
                            }
                            b.tick(now);
                            got.extend(b.take_entries().into_iter().map(|(_, r)| (now, r)));
                            if j < prints.len() && prints[j].0.event_ts.0 > now + 20 * SEC {
                                b.tick(now + 20 * SEC);
                                got.extend(
                                    b.take_entries().into_iter().map(|(_, r)| (now + 20 * SEC, r)),
                                );
                                let e = prints[j].0.event_ts.0;
                                now = e - e.rem_euclid(SEC);
                            }
                            now += SEC;
                        }
                        b.tick(now + 20 * SEC);
                        got.extend(b.take_entries().into_iter().map(|(_, r)| (now + 20 * SEC, r)));
                        let mut rl = String::new();
                        for (at, r) in got {
                            // `at`: the tick that produced it (the row's time live).
                            let (o, ot, m30, m120) =
                                forward(&shi, &slo, at.div_euclid(SEC) - s0 - 1, r.close);
                            rl += &format!(
                                "{}\n",
                                serde_json::json!({"s": sym, "t": at, "p": r.close,
                                    "start_ns": r.start_ns, "sp": r.start_price,
                                    "plus20_ns": r.plus20_ns, "early": r.early,
                                    "o": o, "ot": ot, "mx30": m30, "mx120": m120})
                            );
                        }
                        if !rl.is_empty() {
                            w[6].lock().unwrap().write_all(rl.as_bytes()).unwrap();
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
