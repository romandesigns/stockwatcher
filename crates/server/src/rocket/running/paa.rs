//! Pullback-after-alert export (research only, run by hand;
//! `docs/research/pullback-after-alert/protocol.md`):
//!
//! ```text
//! PAA_ROOT=C:\research\stockwatcher\archive-hist PAA_DAY=2026-05-01 PAA_OUT=dir [PAA_B1=120] \
//!   cargo test -p stockwatcher-server --release --lib running::paa -- --ignored --nocapture
//! ```
//!
//! Triggers are the production Runners paths exactly as `movers` replays them (Running v2's
//! `decide`, the burst detector at its default policy, the fast-move rule second by second, the
//! zones board's "Run +20%" with the early fire): the **first** entry per path per symbol, in the
//! path's own band. R2 is not here: it needs the market-wide RocketBoard (protocol §3, report).
//! The reader picks the first trigger per symbol-day across paths after the per-path daily caps.
//!
//! For every such trigger, every entry rule of protocol §4 is run from the trigger time `t0` to
//! `t0 + 30 min` (B1 only for the delays in `PAA_B1`, seconds). Each entry line carries the entry
//! print's time and price, the stop, the scenario label, forward prices and the post-entry
//! **running extremes**: every print within 30 minutes that set a new high or a new low since the
//! entry (`ext`, `[ms after entry, price]`), and the last print at or before 30 minutes (`last`).
//! That is enough to settle a +10 %/−5 % bracket (or a stop/2R bracket) from any cost-adjusted
//! entry price afterwards, so costs are applied by the reader, not baked in here.
//!
//! Prices are OHLC-counted prints only; volumes are volume-counted prints (the chart engine's
//! verdicts, as production builds bars). Bars are clock-aligned; empty bars stay empty.

use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use market_archive::{Archive, ArchiveConfig, DayData};
use market_events::{Trade, UnixNanos};
use stock_context::detect::Family;
use stock_context::{Coverage, DetectionPolicy, Engine, Fidelity, Policy, RunInfo, Verdict};

use super::super::burst::replay_parity::load;
use super::super::burst::{self, BurstDetector, BurstPolicy};
use super::super::fast::{self as fastm, FastTape};
use super::super::features::{self, Barrier, Tape};
use super::*;
use crate::quotes::Counted;
use crate::zones::ZoneBoard;

const SEC: i64 = 1_000_000_000;
const MIN: i64 = 60 * SEC;
const Q15: i64 = 15 * SEC;
const WINDOW: i64 = 30 * MIN;

/// One OHLC/volume print, flattened.
#[derive(Clone, Copy)]
struct P {
    t: i64,
    p: f64,
    v: f64,
    ohlc: bool,
}

#[derive(Clone, Copy, Default)]
struct Bar {
    o: f64,
    h: f64,
    l: f64,
    c: f64,
    v: f64,
    n: u32,
}

impl Bar {
    fn empty(&self) -> bool {
        self.n == 0
    }
}

/// Clock-aligned bars of width `w` from `t_start`, `nb` of them.
fn bars(ps: &[P], t_start: i64, w: i64, nb: usize) -> Vec<Bar> {
    let mut v = vec![Bar::default(); nb];
    for x in ps {
        let k = (x.t - t_start).div_euclid(w);
        if k < 0 || k as usize >= nb {
            continue;
        }
        let b = &mut v[k as usize];
        if x.ohlc {
            if b.n == 0 {
                b.o = x.p;
                b.h = x.p;
                b.l = x.p;
            }
            b.h = b.h.max(x.p);
            b.l = b.l.min(x.p);
            b.c = x.p;
            b.n += 1;
        }
        b.v += x.v;
    }
    v
}

/// Grid of protocol §5.
#[derive(Clone, Copy)]
struct Cfg {
    k: usize,
    dlo: f64,
    dhi: f64,
    /// 0 VWAP, 1 EMA-9, 2 none.
    s: u8,
    c3: bool,
}

impl Cfg {
    fn id(&self) -> String {
        format!(
            "{}_k{}_d{:.0}-{:.0}_{}",
            if self.c3 { "C3" } else { "C2" },
            self.k,
            self.dlo * 100.0,
            self.dhi * 100.0,
            ["vwap", "ema9", "none"][self.s as usize]
        )
    }
}

fn grid() -> Vec<Cfg> {
    let mut g = Vec::new();
    for c3 in [false, true] {
        for k in [2, 3, 4] {
            for (dlo, dhi) in [(0.15, 0.50), (0.15, 0.65), (0.25, 0.65)] {
                for s in 0..3 {
                    g.push(Cfg { k, dlo, dhi, s, c3 });
                }
            }
        }
    }
    g
}

/// Day context for one symbol: prints, 1-min and 15 s bars from 04:00, VWAP and EMA-9 at every
/// minute boundary (index m = boundary `t04 + m·MIN`, computed from bars ending at or before it).
struct Day {
    /// OHLC prints only (times and prices), for entry/outcome scans.
    ot: Vec<i64>,
    op: Vec<f64>,
    t04: i64,
    m1: Vec<Bar>,
    s15: Vec<Bar>,
    vwap: Vec<f64>,
    ema9: Vec<f64>,
}

const NMIN: usize = 16 * 60 + 2;

impl Day {
    fn new(ps: Vec<P>, t04: i64) -> Day {
        let m1 = bars(&ps, t04, MIN, NMIN);
        let s15 = bars(&ps, t04, Q15, NMIN * 4);
        let mut vwap = vec![f64::NAN; NMIN + 1];
        let mut ema9 = vec![f64::NAN; NMIN + 1];
        let (mut pv, mut vv) = (0.0, 0.0);
        let mut j = 0;
        let mut closes = 0usize;
        let mut sum9 = 0.0;
        let mut e = f64::NAN;
        let alpha = 2.0 / 10.0;
        for m in 0..=NMIN {
            let end = t04 + m as i64 * MIN;
            while j < ps.len() && ps[j].t < end {
                if ps[j].v > 0.0 {
                    pv += ps[j].p * ps[j].v;
                    vv += ps[j].v;
                }
                j += 1;
            }
            vwap[m] = if vv > 0.0 { pv / vv } else { f64::NAN };
            if m >= 1 {
                let b = m1[m - 1];
                if !b.empty() {
                    closes += 1;
                    if closes <= 9 {
                        sum9 += b.c;
                        if closes == 9 {
                            e = sum9 / 9.0;
                        }
                    } else {
                        e = alpha * b.c + (1.0 - alpha) * e;
                    }
                }
            }
            ema9[m] = e;
        }
        let (ot, op): (Vec<i64>, Vec<f64>) =
            ps.iter().filter(|x| x.ohlc).map(|x| (x.t, x.p)).unzip();
        Day {
            ot,
            op,
            t04,
            m1,
            s15,
            vwap,
            ema9,
        }
    }

    /// Index of the first OHLC print at or after `t`.
    fn first_at(&self, t: i64) -> usize {
        self.ot.partition_point(|&x| x < t)
    }

    fn minute(&self, t: i64) -> i64 {
        (t - self.t04).div_euclid(MIN)
    }
}

/// The 1-min pullback structure as of boundary minute index `m` (bars ending ≤ boundary).
struct Pull {
    h: f64,
    l: f64,
    hb: usize,
    /// Non-empty bars after `hb`, before `m`.
    bars: Vec<usize>,
    lp: f64,
    x: f64,
    imp_vol: f64,
    pb_vol: f64,
}

fn pull(d: &Day, t0: i64, m: usize) -> Option<Pull> {
    // Live recordings carry overnight prints before 04:00; the bars start at 04:00 (A2).
    let from = (t0 - 5 * MIN).max(d.t04);
    let end = d.t04 + m as i64 * MIN;
    // H, and the latest print holding it, over [t0 - 5 min, end).
    let i0 = d.first_at(from);
    let i1 = d.first_at(end);
    if i1 <= i0 {
        return None;
    }
    let (mut h, mut ht) = (f64::MIN, 0);
    for i in i0..i1 {
        if d.op[i] >= h {
            h = d.op[i];
            ht = d.ot[i];
        }
    }
    // L: low of [t0 - 5 min, t0); with no print there, the last price before it.
    let j1 = d.first_at(t0);
    let l = if j1 > i0 {
        d.op[i0..j1].iter().cloned().fold(f64::MAX, f64::min)
    } else if i0 > 0 {
        d.op[i0 - 1]
    } else {
        return None;
    };
    let hb = d.minute(ht) as usize;
    let bars: Vec<usize> = ((hb + 1)..m).filter(|&k| !d.m1[k].empty()).collect();
    let (mut lp, mut x, mut pv) = (f64::MAX, f64::MIN, 0.0);
    for &k in &bars {
        lp = lp.min(d.m1[k].l);
        x = x.max(d.m1[k].h);
        pv += d.m1[k].v;
    }
    let pb_vol = if bars.is_empty() {
        f64::NAN
    } else {
        pv / bars.len() as f64
    };
    let fm = d.minute(from).max(0) as usize;
    let imp: Vec<usize> = (fm..=hb).filter(|&k| !d.m1[k].empty()).collect();
    let imp_vol = if imp.is_empty() {
        f64::NAN
    } else {
        imp.iter().map(|&k| d.m1[k].v).sum::<f64>() / imp.len() as f64
    };
    Some(Pull {
        h,
        l,
        hb,
        bars,
        lp,
        x,
        imp_vol,
        pb_vol,
    })
}

fn armed(d: &Day, pl: &Pull, c: &Cfg, m: usize) -> bool {
    if pl.bars.len() < c.k || pl.h <= pl.l {
        return false;
    }
    let depth = (pl.h - pl.lp) / (pl.h - pl.l);
    if !(c.dlo..=c.dhi).contains(&depth) {
        return false;
    }
    if !(pl.pb_vol < pl.imp_vol) {
        return false;
    }
    match c.s {
        0 => d.vwap[m].is_finite() && pl.lp >= d.vwap[m],
        1 => d.ema9[m].is_finite() && pl.lp >= d.ema9[m],
        _ => true,
    }
}

/// Median range of the non-empty 15 s bars inside the pullback (from the first pullback bar's
/// start to the boundary).
fn pull_med15(d: &Day, pl: &Pull, m: usize) -> f64 {
    let Some(&first) = pl.bars.first() else {
        return f64::NAN;
    };
    let mut r: Vec<f64> = (first * 4..m * 4)
        .filter(|&q| !d.s15[q].empty())
        .map(|q| d.s15[q].h - d.s15[q].l)
        .collect();
    if r.is_empty() {
        return f64::NAN;
    }
    r.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = r.len();
    if n % 2 == 1 {
        r[n / 2]
    } else {
        (r[n / 2 - 1] + r[n / 2]) / 2.0
    }
}

/// The last 4 completed 15 s bars before `t`.
fn last4(d: &Day, t: i64) -> Option<[Bar; 4]> {
    let q = (t - d.t04).div_euclid(Q15); // the slot `t` is in; completed are q-4..q-1
    if q < 4 {
        return None;
    }
    let q = q as usize;
    Some([d.s15[q - 4], d.s15[q - 3], d.s15[q - 2], d.s15[q - 1]])
}

fn c3_ok(d: &Day, pl: &Pull, m: usize, t: i64) -> bool {
    let Some(b) = last4(d, t) else { return false };
    if b.iter().any(Bar::empty) {
        return false;
    }
    let med = pull_med15(d, pl, m);
    let mean = b.iter().map(|x| x.h - x.l).sum::<f64>() / 4.0;
    med.is_finite() && mean <= med
}

/// Protocol §8 scenario label at an entry at `t`, from the pullback as of the boundary before it.
fn scenario(d: &Day, t0: i64, t: i64) -> &'static str {
    let q = (t - d.t04).div_euclid(Q15);
    if q < 8 {
        return "D";
    }
    if (q as usize - 8..q as usize).any(|k| d.s15[k].empty()) {
        return "D";
    }
    let m = d.minute(t) as usize;
    let Some(pl) = pull(d, t0, m) else {
        return "none";
    };
    if pl.bars.is_empty() {
        return "none";
    }
    let n = pl.bars.len();
    let tail: Vec<usize> = pl.bars[n.saturating_sub(3)..].to_vec();
    let mut lower = 0;
    for (i, &k) in tail.iter().enumerate() {
        let prev = if n - tail.len() + i == 0 {
            pl.hb
        } else {
            pl.bars[n - tail.len() + i - 1]
        };
        if d.m1[k].h < d.m1[prev].h {
            lower += 1;
        }
    }
    let range = pl.x - pl.lp;
    let decline = d.m1[pl.hb].c - d.m1[*pl.bars.last().unwrap()].c;
    if lower >= 2 && range > 0.0 && decline >= 0.5 * range {
        return "A";
    }
    let med = pull_med15(d, &pl, m);
    if let Some(b) = last4(d, t)
        && med.is_finite()
        && b.iter().all(|x| x.h - x.l < med)
    {
        return "C";
    }
    "B"
}

struct Ent {
    rule: String,
    i: usize,
    stop: f64,
}

/// The entry line: price path after the entry print `i` (module docs).
fn line(d: &Day, sym: &str, path: &str, t0: i64, p0: f64, e: &Ent) -> String {
    let (te, pe) = (d.ot[e.i], d.op[e.i]);
    let mut ext = Vec::new();
    let (mut hi, mut lo) = (pe, pe);
    let mut last: Option<(i64, f64)> = None;
    let mut fwd = [f64::NAN; 5];
    let marks = [MIN, 3 * MIN, 5 * MIN, 10 * MIN, 15 * MIN];
    for j in e.i + 1..d.ot.len() {
        let (t, p) = (d.ot[j], d.op[j]);
        if t > te + WINDOW {
            break;
        }
        for (f, &mk) in fwd.iter_mut().zip(&marks) {
            if t <= te + mk {
                *f = p;
            }
        }
        if p > hi || p < lo {
            hi = hi.max(p);
            lo = lo.min(p);
            ext.push(serde_json::json!([(t - te) / 1_000_000, p]));
        }
        last = Some((t, p));
    }
    for f in fwd.iter_mut() {
        if f.is_nan() {
            *f = pe;
        }
    }
    let n_after = last.is_some();
    let last = last.map(|(t, p)| serde_json::json!([(t - te) / 1_000_000, p]));
    format!(
        "{}\n",
        serde_json::json!({
            "s": sym, "path": path, "t0": t0, "p0": p0, "rule": e.rule, "te": te, "pe": pe,
            "stop": if e.stop.is_finite() { serde_json::json!(e.stop) } else { serde_json::Value::Null },
            "scen": scenario(d, t0, te), "fwd": fwd, "ext": ext, "last": last, "after": n_after,
        })
    )
}

/// MP5 confirmations (reason `full_post_arm_bar_closed_above_trigger`) for the day: (observed
/// at, invalidation price), from the product's detection engine fed every print unchanged.
fn mp5(sym: &str, prints: &[(Trade, Counted)]) -> Vec<(i64, f64)> {
    let mut e = Engine::new(
        Policy::current(),
        RunInfo {
            run_id: "replay:paa".into(),
            source: "replay".into(),
            fidelity: Fidelity::RecordedLive,
            numerator_coverage: Coverage::Complete,
        },
    )
    .with_detection_policy(DetectionPolicy::current());
    let mut inval: HashMap<String, f64> = HashMap::new();
    let mut out = Vec::new();
    let mut take = |e: &mut Engine, out: &mut Vec<(i64, f64)>| {
        for t in e.take_detection_transitions() {
            if t.family != Family::MicroPullback || t.symbol != sym {
                continue;
            }
            if let Some(l) = &t.level
                && l.kind == "invalidation"
            {
                inval.insert(t.episode_id.clone(), l.price as f64 / 1e6);
            }
            if t.reason == "full_post_arm_bar_closed_above_trigger" {
                let stop = inval.get(&t.episode_id).copied().unwrap_or(f64::NAN);
                out.push((t.observed_at, stop));
            }
        }
    };
    for (t, c) in prints {
        e.advance_to(t.event_ts);
        take(&mut e, &mut out);
        let admit = t.receive_ts.unwrap_or(t.event_ts);
        e.apply(
            t,
            Verdict {
                ohlc: c.ohlc,
                volume: c.volume,
            },
            admit,
        );
        take(&mut e, &mut out);
    }
    if let Some((t, _)) = prints.last() {
        e.advance_to(UnixNanos(t.event_ts.0 + 5 * MIN));
        take(&mut e, &mut out);
    }
    out
}

/// Every rule's entry for the trigger `(t0, p0)`.
fn entries(d: &Day, t0: i64, b1: &[i64], mp: &[(i64, f64)], g: &[Cfg]) -> Vec<Ent> {
    let mut v = Vec::new();
    let i0 = d.first_at(t0);
    let iend = d.first_at(t0 + WINDOW + 1);
    let within = |i: usize| i < iend && i < d.ot.len();
    // B0
    if within(i0) {
        v.push(Ent {
            rule: "B0".into(),
            i: i0,
            stop: f64::NAN,
        });
    }
    // B1
    for &dl in b1 {
        let i = d.first_at(t0 + dl * SEC);
        if within(i) {
            v.push(Ent {
                rule: format!("B1_{dl}"),
                i,
                stop: f64::NAN,
            });
        }
    }
    // B2: the first completed red 1-min bar that ends after t0.
    {
        let mut m = d.minute(t0) as usize;
        while m < NMIN && d.t04 + (m as i64 + 1) * MIN <= t0 + WINDOW {
            let b = d.m1[m];
            if !b.empty() && b.c < b.o {
                let end = d.t04 + (m as i64 + 1) * MIN;
                let mut i = d.first_at(end);
                while within(i) && d.op[i] <= b.h {
                    i += 1;
                }
                if within(i) {
                    v.push(Ent {
                        rule: "B2".into(),
                        i,
                        stop: b.l,
                    });
                }
                break;
            }
            m += 1;
        }
    }
    // B3: after 2 min, the first print above the post-t0 high.
    {
        let mut hi = f64::MIN;
        let mut i = i0;
        while within(i) {
            if d.ot[i] >= t0 + 2 * MIN && hi > f64::MIN && d.op[i] > hi {
                v.push(Ent {
                    rule: "B3".into(),
                    i,
                    stop: f64::NAN,
                });
                break;
            }
            hi = hi.max(d.op[i]);
            i += 1;
        }
    }
    // C1: the first MP5 confirmation in [t0, t0 + 30 min].
    if let Some(&(at, stop)) = mp.iter().find(|(at, _)| *at >= t0 && *at <= t0 + WINDOW) {
        let i = d.first_at(at);
        if within(i) {
            v.push(Ent {
                rule: "C1".into(),
                i,
                stop,
            });
        }
    }
    // C2 / C3 grid.
    let m_first = d.minute(t0).max(0) as usize; // boundary at or before t0
    let mut pulls: Vec<(usize, Option<Pull>)> = Vec::new();
    let mut m = m_first;
    while m < NMIN && d.t04 + m as i64 * MIN <= t0 + WINDOW {
        pulls.push((m, pull(d, t0, m)));
        m += 1;
    }
    for c in g {
        let mut got = None;
        'outer: for (m, pl) in &pulls {
            let Some(pl) = pl else { continue };
            if !armed(d, pl, c, *m) {
                continue;
            }
            let bstart = (d.t04 + *m as i64 * MIN).max(t0);
            let bend = d.t04 + (*m as i64 + 1) * MIN;
            let mut i = d.first_at(bstart);
            while within(i) && d.ot[i] < bend {
                if d.op[i] > pl.x && (!c.c3 || c3_ok(d, pl, *m, d.ot[i])) {
                    got = Some(Ent {
                        rule: c.id(),
                        i,
                        stop: pl.lp,
                    });
                    break 'outer;
                }
                i += 1;
            }
        }
        if let Some(e) = got {
            v.push(e);
        }
    }
    v
}

#[test]
#[ignore = "export; needs an archive, run by hand"]
fn export_paa() {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let root = PathBuf::from(env("PAA_ROOT").expect("PAA_ROOT"));
    let out = PathBuf::from(env("PAA_OUT").expect("PAA_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    let threads: usize = env("PAA_THREADS").map_or(6, |v| v.parse().unwrap());
    let day: NaiveDate = env("PAA_DAY").expect("PAA_DAY").parse().unwrap();
    let b1: Vec<i64> = env("PAA_B1")
        .map(|v| v.split(',').map(|x| x.parse().unwrap()).collect())
        .unwrap_or_default();
    let only: Option<Vec<String>> =
        env("PAA_SYMBOLS").map(|v| v.split(',').map(str::to_string).collect());
    // research/fast-0800: a Fast-move variant ("" = production rule unchanged). "drop_d_t" feeds
    // the fast tape no venue-D Form-T print reported in the window after a FINRA TRF opening
    // (its batch of late reports: 08:00 until 2026-03-27, 04:00 from 2026-03-30); "blackout"
    // makes no fast decision in that window. PAA_FAST_WIN = "HH:MM" (default 08:00) and
    // PAA_FAST_WIN_S = its length in seconds (default 120).
    let fast_variant = env("PAA_FAST_VARIANT").unwrap_or_default();
    let (wh, wm) = env("PAA_FAST_WIN")
        .and_then(|v| {
            v.split_once(':')
                .map(|(h, m)| (h.parse().unwrap(), m.parse().unwrap()))
        })
        .unwrap_or((8, 0));
    let win_s: i64 = env("PAA_FAST_WIN_S").map_or(120, |v| v.parse().unwrap());
    let t800 = market_events::session::et(day, wh, wm, 0, 0).unwrap().0;
    let in_800 = |ts: i64| (t800..t800 + win_s * SEC).contains(&ts);
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
    let g = grid();
    eprintln!(
        "{day}: {} symbols, prev close {prev_date:?}, {} running facts, {} ADV30, live={live}",
        syms.len(),
        rfacts.len(),
        adv.len()
    );
    let t0d = market_events::session::et(day, 4, 0, 0, 0).unwrap().0;
    let s0 = t0d.div_euclid(SEC);
    let span: i64 = 16 * 3600;
    let last_trigger = market_events::session::et(day, 15, 30, 0, 0).unwrap().0;
    let started = std::time::Instant::now();
    let next = AtomicUsize::new(0);
    let wt = Mutex::new(std::io::BufWriter::new(
        std::fs::File::create(out.join("triggers.jsonl")).unwrap(),
    ));
    let we = Mutex::new(std::io::BufWriter::new(
        std::fs::File::create(out.join("entries.jsonl")).unwrap(),
    ));
    let wf = Mutex::new(std::io::BufWriter::new(
        std::fs::File::create(out.join("fires.jsonl")).unwrap(),
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
                    if prints.is_empty() {
                        continue;
                    }
                    let (mut lo, mut hi) = (f64::MAX, 0f64);
                    for (t, c) in &prints {
                        if c.ohlc {
                            let p = t.price.micros() as f64 / 1e6;
                            lo = lo.min(p);
                            hi = hi.max(p);
                        }
                    }
                    if hi < 0.08 || lo > 30.0 || hi / lo < 1.04 {
                        continue;
                    }
                    // path -> first (t, p)
                    let mut first: Vec<(&'static str, i64, f64)> = Vec::new();

                    // Running v2 (movers' loop).
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

                    // Buying burst (movers' loop, default policy, no restart).
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
                            for a in d.tick(now, &pcf) {
                                first.push(("burst", a.at_ns, a.price));
                                break 'b;
                            }
                            wb += burst::STEP_S * SEC;
                        }
                    }

                    // Fast move (movers' loop).
                    if hi / lo >= 1.08 {
                        let mut ft = FastTape::default();
                        let mut j = 0;
                        let mut prev_traded = -1i64;
                        for k in 0..span {
                            let end = (s0 + k + 1) * SEC;
                            let mut any = false;
                            while j < prints.len() && prints[j].0.event_ts.0 < end {
                                let (t, c) = &prints[j];
                                if fast_variant == "drop_d_t"
                                    && t.exchange == "D"
                                    && t.conditions.iter().any(|x| x == "T")
                                    && in_800(t.event_ts.0)
                                {
                                    j += 1;
                                    continue;
                                }
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
                            if fast_variant == "blackout" && in_800(end) {
                                continue;
                            }
                            if let Some(e) = ft.inputs(s0 + k).and_then(|x| fastm::decide(&x)) {
                                first.push(("fast", (s0 + k) * SEC + SEC, e.price));
                                break;
                            }
                        }
                    }

                    // Run +20% with the early fire (movers' loop).
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

                    if first.is_empty() {
                        continue;
                    }
                    let mut tl = String::new();
                    for (path, t, p) in &first {
                        tl += &format!(
                            "{}\n",
                            serde_json::json!({"s": sym, "path": path, "t": t, "p": p})
                        );
                    }
                    wt.lock().unwrap().write_all(tl.as_bytes()).unwrap();
                    // research/fast-0800: each path's first fire with its follow-through (+10 %
                    // before -5 % within 30 min on OHLC prints after the fire, no costs).
                    let mut fl = String::new();
                    for (path, t, p) in &first {
                        let mut out_ft = "timeout";
                        for (x, c) in prints.iter() {
                            let ts = x.event_ts.0;
                            if ts <= *t || !c.ohlc {
                                continue;
                            }
                            if ts > t + 30 * 60 * SEC {
                                break;
                            }
                            let q = x.price.micros() as f64 / 1e6;
                            if q >= p * 1.10 {
                                out_ft = "win";
                                break;
                            }
                            if q <= p * 0.95 {
                                out_ft = "loss";
                                break;
                            }
                        }
                        fl += &format!(
                            "{}
",
                            serde_json::json!({"day": day.to_string(), "s": sym, "path": path, "t": t, "p": p, "ft": out_ft, "variant": fast_variant})
                        );
                    }
                    wf.lock().unwrap().write_all(fl.as_bytes()).unwrap();

                    // Entry rules for every in-window first trigger.
                    let todo: Vec<&(&str, i64, f64)> = first
                        .iter()
                        .filter(|(_, t, p)| *t <= last_trigger && (0.10..=25.0).contains(p))
                        .collect();
                    if todo.is_empty() {
                        continue;
                    }
                    let ps: Vec<P> = prints
                        .iter()
                        .filter(|(_, c)| c.ohlc || c.volume)
                        .map(|(t, c)| P {
                            t: t.event_ts.0,
                            p: t.price.micros() as f64 / 1e6,
                            v: if c.volume { t.size as f64 } else { 0.0 },
                            ohlc: c.ohlc,
                        })
                        .collect();
                    let d = Day::new(ps, t0d);
                    let mp = mp5(sym, &prints);
                    let mut el = String::new();
                    let mut seen = HashSet::new();
                    for (path, t0, p0) in todo {
                        if !seen.insert((*t0, *path)) {
                            continue;
                        }
                        for e in entries(&d, *t0, &b1, &mp, &g) {
                            el += &line(&d, sym, path, *t0, *p0, &e);
                        }
                    }
                    we.lock().unwrap().write_all(el.as_bytes()).unwrap();
                }
            });
        }
    });
    wt.lock().unwrap().flush().unwrap();
    wf.lock().unwrap().flush().unwrap();
    we.lock().unwrap().flush().unwrap();
    eprintln!("done in {:.0} s", started.elapsed().as_secs_f64());
}
