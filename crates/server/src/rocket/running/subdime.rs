//! Sub-dime heads-up export (research only, run by hand; `docs/research/subdime-headsup/protocol.md`):
//!
//! ```text
//! SD_ROOT=C:\research\stockwatcher\archive-hist SD_DAY=2026-05-01 SD_OUT=dir [SD_RANDOM=1] \
//!   cargo test -p stockwatcher-server --release --lib [--features subdime] running::subdime -- --ignored --nocapture
//! ```
//!
//! The production Runners paths replayed as `research/fast-0800`'s harness does (Running v2's
//! `decide`, the burst detector at its default policy, the fast-move rule second by second, the
//! zones board's "Run +20%" with the early fire), first fire per path per symbol-day. Built without
//! the `subdime` feature the floors are production's $0.10; with it, $0.05 (protocol "Candidates").
//! Venue-D Form-T prints executed 04:00:00–04:03:00 ET (the TRF report batch, `research/fast-0800`)
//! are dropped before any path sees the tape.
//!
//! `fires.jsonl`: per fire the bracket outcome (+10 % before −5 % within 30 min on OHLC prints
//! after the fire), its return and exit time, the first print at or over $0.10 within 60 min, and
//! the trailing 15-minute high/low range. `random.jsonl` (`SD_RANDOM=1`): three seeded sub-dime
//! print times per symbol-day with at least 30 OHLC prints in [$0.05, $0.10), same fields.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use market_archive::{Archive, ArchiveConfig, DayData};
use market_events::Trade;

use super::super::burst::replay_parity::load;
use super::super::burst::{self, BurstDetector, BurstPolicy};
use super::super::fast::{self as fastm, FastTape};
use super::super::features::{self, Barrier, Tape};
use super::*;
use crate::quotes::Counted;
use crate::zones::ZoneBoard;

const SEC: i64 = 1_000_000_000;
const MIN: i64 = 60 * SEC;

/// OHLC prints as (time, price), in time order.
fn ohlc(prints: &[(Trade, Counted)]) -> Vec<(i64, f64)> {
    prints
        .iter()
        .filter(|(_, c)| c.ohlc)
        .map(|(t, _)| (t.event_ts.0, t.price.micros() as f64 / 1e6))
        .collect()
}

struct Outcome {
    ft: &'static str,
    ret: f64,
    t_exit: i64,
    reach10: Option<i64>,
    range15: Option<f64>,
}

/// Protocol "Outcomes": the bracket from `p` at `t`, the first print at or over $0.10 within 60
/// minutes, and the trailing 15-minute range (prints at or before `t`).
fn outcome(px: &[(i64, f64)], t: i64, p: f64) -> Outcome {
    let i0 = px.partition_point(|x| x.0 <= t);
    let (mut ft, mut ret, mut t_exit) = ("timeout", 0.0, t);
    let mut last = None;
    for &(ts, q) in &px[i0..] {
        if ts > t + 30 * MIN {
            break;
        }
        last = Some((ts, q));
        if q >= p * 1.10 {
            (ft, ret, t_exit) = ("win", 0.10, ts);
            break;
        }
        if q <= p * 0.95 {
            (ft, ret, t_exit) = ("loss", -0.05, ts);
            break;
        }
    }
    if ft == "timeout"
        && let Some((ts, q)) = last
    {
        (ret, t_exit) = (q / p - 1.0, ts);
    }
    let reach10 = px[i0..]
        .iter()
        .take_while(|x| x.0 <= t + 60 * MIN)
        .find(|x| x.1 >= 0.10)
        .map(|x| x.0);
    let lo_i = px.partition_point(|x| x.0 < t - 15 * MIN);
    let win = &px[lo_i..i0];
    let range15 = (!win.is_empty()).then(|| {
        let (mut lo, mut hi) = (f64::MAX, 0f64);
        for &(_, q) in win {
            lo = lo.min(q);
            hi = hi.max(q);
        }
        hi / lo - 1.0
    });
    Outcome { ft, ret, t_exit, reach10, range15 }
}

fn json(day: &str, sym: &str, path: &str, t: i64, p: f64, o: &Outcome) -> String {
    format!(
        "{}\n",
        serde_json::json!({"day": day, "s": sym, "path": path, "t": t, "p": p, "ft": o.ft,
            "ret": o.ret, "t_exit": o.t_exit, "reach10": o.reach10, "range15": o.range15,
            "floor": fastm::BAND.0})
    )
}

/// A small deterministic generator for the random baseline's draws (seeded per symbol-day).
fn lcg(seed: &mut u64) -> u64 {
    *seed = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *seed >> 33
}

#[test]
#[ignore = "export; needs an archive, run by hand"]
fn export_subdime() {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let root = PathBuf::from(env("SD_ROOT").expect("SD_ROOT"));
    let out = PathBuf::from(env("SD_OUT").expect("SD_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    let threads: usize = env("SD_THREADS").map_or(6, |v| v.parse().unwrap());
    let day: NaiveDate = env("SD_DAY").expect("SD_DAY").parse().unwrap();
    let random = env("SD_RANDOM").is_some_and(|v| v == "1");
    let day_s = day.to_string();
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
        .filter(|d| d.as_str() < day_s.as_str())
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
    let syms: Vec<String> = manifest
        .symbols
        .keys()
        .filter(|s| prev_close.get(*s).is_none_or(|c| (0.02..=60.0).contains(c)))
        .cloned()
        .collect();
    let rfacts = facts(&archive, day);
    let adv = burst::facts::adv30(&archive, day);
    let t0d = market_events::session::et(day, 4, 0, 0, 0).unwrap().0;
    let t403 = t0d + 3 * MIN;
    let t1530 = market_events::session::et(day, 15, 30, 0, 0).unwrap().0;
    let s0 = t0d.div_euclid(SEC);
    let span: i64 = 16 * 3600;
    eprintln!("{day}: {} symbols, live={live}, floor={}", syms.len(), fastm::BAND.0);
    let started = std::time::Instant::now();
    let next = AtomicUsize::new(0);
    let wf = Mutex::new(std::io::BufWriter::new(
        std::fs::File::create(out.join("fires.jsonl")).unwrap(),
    ));
    let wr = Mutex::new(std::io::BufWriter::new(
        std::fs::File::create(out.join("random.jsonl")).unwrap(),
    ));
    std::thread::scope(|sc| {
        for _ in 0..threads {
            sc.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(sym) = syms.get(i) else { break };
                    let Ok(all) = load(&archive, day, sym, live) else {
                        continue;
                    };
                    // The TRF report batch never reaches any path (protocol "Data").
                    let prints: Vec<(Trade, Counted)> = all
                        .into_iter()
                        .filter(|(t, _)| {
                            !(t.exchange == "D"
                                && t.conditions.iter().any(|x| x == "T")
                                && (t0d..t403).contains(&t.event_ts.0))
                        })
                        .collect();
                    if prints.is_empty() {
                        continue;
                    }
                    let px = ohlc(&prints);
                    let (mut lo, mut hi) = (f64::MAX, 0f64);
                    for &(_, p) in &px {
                        lo = lo.min(p);
                        hi = hi.max(p);
                    }
                    if px.is_empty() || hi < 0.05 || lo > 30.0 {
                        continue;
                    }

                    if random {
                        let pool: Vec<&(i64, f64)> = px
                            .iter()
                            .filter(|x| (0.05..0.10).contains(&x.1) && (t403..=t1530).contains(&x.0))
                            .collect();
                        if pool.len() >= 30 {
                            let mut seed = sym.bytes().fold(day_s.bytes().fold(7u64, |a, b| a.wrapping_mul(31).wrapping_add(b as u64)), |a, b| {
                                a.wrapping_mul(131).wrapping_add(b as u64)
                            });
                            let mut rl = String::new();
                            for _ in 0..3 {
                                let &&(t, p) = &pool[(lcg(&mut seed) as usize) % pool.len()];
                                rl += &json(&day_s, sym, "random", t, p, &outcome(&px, t, p));
                            }
                            wr.lock().unwrap().write_all(rl.as_bytes()).unwrap();
                        }
                    }
                    if hi / lo < 1.04 {
                        continue;
                    }
                    let mut first: Vec<(&'static str, i64, f64)> = Vec::new();

                    // Running v2.
                    {
                        let fct = rfacts.get(sym);
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
                            if let Some(e) = decide(&x, fct) {
                                first.push(("running", b.at_ns, e.price));
                                break;
                            }
                        }
                    }

                    // Buying burst (default policy).
                    {
                        let mut d = BurstDetector::new(BurstPolicy::default());
                        d.set_adv(day, adv.clone());
                        let pcf = |s: &str| prev_close.get(s).copied();
                        let mut j = 0;
                        let mut wb = t0d;
                        'b: while wb <= t0d + burst::NSEC * SEC {
                            let now = wb + burst::SETTLE_NS;
                            while j < prints.len() && prints[j].0.event_ts.0 <= now {
                                d.apply_trade(&prints[j].0, prints[j].1);
                                j += 1;
                            }
                            if let Some(a) = d.tick(now, &pcf).into_iter().next() {
                                first.push(("burst", a.at_ns, a.price));
                                break 'b;
                            }
                            wb += burst::STEP_S * SEC;
                        }
                    }

                    // Fast move.
                    if hi / lo >= 1.08 {
                        let mut ft = FastTape::default();
                        let mut j = 0;
                        let mut prev_traded = -1i64;
                        for k in 0..span {
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
                            if let Some(e) = ft.inputs(s0 + k).and_then(|x| fastm::decide(&x)) {
                                first.push(("fast", (s0 + k) * SEC + SEC, e.price));
                                break;
                            }
                        }
                    }

                    // Run +20% with the early fire.
                    if hi / lo >= 1.18 {
                        let mut b = ZoneBoard::new(true);
                        b.set_early(true);
                        let mut got: Option<(i64, f64)> = None;
                        let mut j = 0;
                        let f0 = prints[0].0.event_ts.0;
                        let mut now = f0 - f0.rem_euclid(SEC) + SEC;
                        while j < prints.len() && got.is_none() {
                            while j < prints.len() && prints[j].0.event_ts.0 < now {
                                b.apply_trade(&prints[j].0, prints[j].1);
                                j += 1;
                            }
                            b.tick(now);
                            if let Some((_, r)) = b.take_entries().into_iter().next() {
                                got = Some((now, r.close));
                                break;
                            }
                            if j < prints.len() && prints[j].0.event_ts.0 > now + 20 * SEC {
                                b.tick(now + 20 * SEC);
                                if let Some((_, r)) = b.take_entries().into_iter().next() {
                                    got = Some((now + 20 * SEC, r.close));
                                    break;
                                }
                                let e = prints[j].0.event_ts.0;
                                now = e - e.rem_euclid(SEC);
                            }
                            now += SEC;
                        }
                        if got.is_none() {
                            b.tick(now + 20 * SEC);
                            if let Some((_, r)) = b.take_entries().into_iter().next() {
                                got = Some((now + 20 * SEC, r.close));
                            }
                        }
                        if let Some((t, p)) = got {
                            first.push(("run", t, p));
                        }
                    }

                    let mut fl = String::new();
                    for (path, t, p) in &first {
                        fl += &json(&day_s, sym, path, *t, *p, &outcome(&px, *t, *p));
                    }
                    if !fl.is_empty() {
                        wf.lock().unwrap().write_all(fl.as_bytes()).unwrap();
                    }
                }
            });
        }
    });
    wf.lock().unwrap().flush().unwrap();
    wr.lock().unwrap().flush().unwrap();
    eprintln!("done in {:.0} s", started.elapsed().as_secs_f64());
}
