//! Post-alert timing export (research only, run by hand;
//! `docs/research/post-alert-timing/protocol.md`):
//!
//! ```text
//! PAT_ROOT=C:\research\stockwatcher\archive-hist PAT_DAY=2026-05-01 PAT_OUT=dir \
//!   PAT_SYMS=AAA,BBB [PAT_THREADS=8] \
//!   cargo test -p stockwatcher-server --release --lib running::pat -- --ignored --nocapture
//! ```
//!
//! For each named symbol it writes `<SYM>.bin`: one fixed 60-byte little-endian record per second
//! of 04:00–20:00 ET that had a counted print (seconds without one are absent). Record:
//! `sec u32` (seconds since 04:00 ET), `o h l c f64` (OHLC-counted prints only; NaN when the second
//! had none), `n u32` (OHLC-counted prints), `v f64` and `pv f64` (volume-counted shares and
//! price × shares), `nv u32` (volume-counted prints). The chart engine's verdicts decide what
//! counts, exactly as production builds bars; the triggers themselves come from `paa.rs`.
//! Everything else (bars, VWAP, EMA, brackets, fills against quotes) is the reader's.

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use market_archive::{Archive, ArchiveConfig, DayData};

use super::super::burst::replay_parity::load;
use super::*;

const SEC: i64 = 1_000_000_000;
const SPAN: usize = 16 * 3600;

#[derive(Clone, Copy)]
struct S {
    o: f64,
    h: f64,
    l: f64,
    c: f64,
    n: u32,
    v: f64,
    pv: f64,
    nv: u32,
}

impl Default for S {
    fn default() -> S {
        S { o: f64::NAN, h: f64::NAN, l: f64::NAN, c: f64::NAN, n: 0, v: 0.0, pv: 0.0, nv: 0 }
    }
}

#[test]
#[ignore = "export; needs an archive, run by hand"]
fn export_pat() {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let root = PathBuf::from(env("PAT_ROOT").expect("PAT_ROOT"));
    let out = PathBuf::from(env("PAT_OUT").expect("PAT_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    let threads: usize = env("PAT_THREADS").map_or(8, |v| v.parse().unwrap());
    let day: NaiveDate = env("PAT_DAY").expect("PAT_DAY").parse().unwrap();
    let syms: Vec<String> = env("PAT_SYMS")
        .expect("PAT_SYMS")
        .split(',')
        .map(str::to_string)
        .collect();
    let archive = Archive::new(ArchiveConfig { root, retention_days: 0, min_free_bytes: 0 });
    let live = match archive.day_data(day) {
        Some(DayData::Final) => false,
        Some(DayData::Provisional { .. }) => true,
        None => panic!("{day}: not in the archive"),
    };
    let t04 = market_events::session::et(day, 4, 0, 0, 0).unwrap().0;
    let next = AtomicUsize::new(0);
    std::thread::scope(|sc| {
        for _ in 0..threads {
            sc.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(sym) = syms.get(i) else { break };
                    let Ok(prints) = load(&archive, day, sym, live) else {
                        eprintln!("{sym}: load failed");
                        continue;
                    };
                    let mut v: Vec<S> = vec![S::default(); SPAN];
                    let mut used = vec![false; SPAN];
                    for (t, c) in &prints {
                        let k = (t.event_ts.0 - t04).div_euclid(SEC);
                        if k < 0 || k as usize >= SPAN || !(c.ohlc || c.volume) {
                            continue;
                        }
                        let k = k as usize;
                        let p = t.price.micros() as f64 / 1e6;
                        let s = &mut v[k];
                        used[k] = true;
                        if c.ohlc {
                            if s.n == 0 {
                                s.o = p;
                                s.h = p;
                                s.l = p;
                            }
                            s.h = s.h.max(p);
                            s.l = s.l.min(p);
                            s.c = p;
                            s.n += 1;
                        }
                        if c.volume {
                            s.v += t.size as f64;
                            s.pv += p * t.size as f64;
                            s.nv += 1;
                        }
                    }
                    let mut buf = Vec::with_capacity(60 * used.iter().filter(|x| **x).count());
                    for k in 0..SPAN {
                        if !used[k] {
                            continue;
                        }
                        let s = v[k];
                        buf.extend_from_slice(&(k as u32).to_le_bytes());
                        for x in [s.o, s.h, s.l, s.c] {
                            buf.extend_from_slice(&x.to_le_bytes());
                        }
                        buf.extend_from_slice(&s.n.to_le_bytes());
                        buf.extend_from_slice(&s.v.to_le_bytes());
                        buf.extend_from_slice(&s.pv.to_le_bytes());
                        buf.extend_from_slice(&s.nv.to_le_bytes());
                    }
                    let tmp = out.join(format!("{sym}.bin.tmp"));
                    let mut f = std::fs::File::create(&tmp).unwrap();
                    f.write_all(&buf).unwrap();
                    drop(f);
                    std::fs::rename(&tmp, out.join(format!("{sym}.bin"))).unwrap();
                }
            });
        }
    });
}
