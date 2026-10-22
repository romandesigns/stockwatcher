//! End-to-end parity with the research, on a recorded archive day (needs the research disk, so
//! `#[ignore]`d; run by hand):
//!
//! ```text
//! cargo test -p stockwatcher-server --release --lib burst::replay_parity -- --ignored --nocapture
//! ```
//!
//! Env (defaults are the research machine's paths): `BURST_PARITY_ARCHIVE`
//! (`C:\research\stockwatcher\archive-hist`), `BURST_PARITY_REPORTS`
//! (`C:\research\stockwatcher\reports\burst`), `BURST_PARITY_DAY` (`2026-09-02`, a TEST day),
//! `BURST_PARITY_THREADS` (8).
//!
//! Every symbol of the day goes through the archive reader, `ReplayCore` and the production
//! `ChartEngine` (each print's counted verdict), exactly as the research's `burst::load`, then
//! through a fresh live [`BurstDetector`] checked at every `w + 1 s` with the prints executed by
//! then. Compared with the research's files for the day:
//!
//! 1. the gate passes (`export-test/rows5-<day>.csv`, `gate = 1`): the same `(symbol, w)` set;
//! 2. every feature of every pass, written as the research wrote it (five decimals): equal;
//! 3. the alerts at the frozen 20-a-day threshold (`test/alerts-model-theta-B20.csv`): the same
//!    symbols and times, the scores within 1e-9.
//!
//! It also times the detector alone (per print, per 5 s check) on the whole-market day.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use chart_engine::ChartEngine;
use chart_engine::wire::ChartUpdate;
use market_archive::{Archive, ArchiveConfig, DayData};
use market_events::MarketEvent;
use market_replay::source::{EventSource, EventStream};
use market_replay::{ReplayCore, marks::DEFAULT_MARK_STEP_NS};

use super::*;

struct OneSymbol {
    archive: Archive,
    day: NaiveDate,
    symbol: String,
    live: bool,
}

impl EventSource for OneSymbol {
    fn open(&self) -> Result<Vec<EventStream>, String> {
        let r = if self.live {
            self.archive.live_reader(self.day, &self.symbol, None)
        } else {
            self.archive.reader(self.day, &self.symbol)
        }
        .map_err(|e| e.to_string())?;
        Ok(vec![Box::new(
            r.map(|x| x.map(|v| v.event).map_err(|e| e.to_string())),
        )])
    }
    fn day(&self) -> NaiveDate {
        self.day
    }
    fn symbols(&self) -> Vec<String> {
        vec![self.symbol.clone()]
    }
    fn fixture(&self) -> bool {
        false
    }
    fn label(&self) -> String {
        format!("burst parity {} {}", self.day, self.symbol)
    }
}

/// The research's `num`: how a feature was written to the CSV.
fn num(x: f64) -> String {
    if x.is_nan() {
        String::new()
    } else if x == x.trunc() && x.abs() < 1e9 {
        format!("{}", x as i64)
    } else {
        format!("{x:.5}")
    }
}

/// One symbol-day's counted prints, sorted by execution (the research's `burst::load`).
pub(crate) fn load(
    archive: &Archive,
    day: NaiveDate,
    sym: &str,
    live: bool,
) -> Result<Vec<(Trade, Counted)>, String> {
    let src = Arc::new(OneSymbol {
        archive: archive.clone(),
        day,
        symbol: sym.to_string(),
        live,
    });
    let mut core = ReplayCore::new(src, DEFAULT_MARK_STEP_NS)?;
    let mut chart = ChartEngine::default();
    let mut v = Vec::new();
    let mut items = Vec::new();
    loop {
        items.clear();
        let more = core.step(&mut items)?;
        for it in items.drain(..) {
            match it {
                market_replay::FeedItem::Advance(m) => {
                    chart.advance_clock(m);
                }
                market_replay::FeedItem::Event(e) => {
                    let (ups, _) = chart.apply_revising(&e);
                    if let MarketEvent::Trade(t) = &e
                        && let Some(ChartUpdate::Tape(row)) = ups.first()
                    {
                        v.push((t.clone(), Counted::from_tape(&row.counted)));
                    }
                }
                _ => {}
            }
        }
        if !more {
            break;
        }
    }
    v.sort_by_key(|x| x.0.event_ts.0);
    Ok(v)
}

#[derive(Default)]
struct Totals {
    prints: u64,
    apply_ns: u128,
    ticks: u64,
    tick_ns: u128,
    passes: Vec<Pass>,
    alerts: Vec<BurstAlert>,
    errors: Vec<String>,
    /// Fixture symbols' prints (`BURST_PARITY_FIXTURE=A,B`).
    fixture: BTreeMap<String, Vec<(Trade, Counted)>>,
}

#[test]
#[ignore = "needs the research archive; run by hand"]
fn replay_parity_with_the_research() {
    let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
    let root = PathBuf::from(env(
        "BURST_PARITY_ARCHIVE",
        r"C:\research\stockwatcher\archive-hist",
    ));
    let reports = PathBuf::from(env(
        "BURST_PARITY_REPORTS",
        r"C:\research\stockwatcher\reports\burst",
    ));
    let day: NaiveDate = env("BURST_PARITY_DAY", "2026-09-02").parse().unwrap();
    let threads: usize = env("BURST_PARITY_THREADS", "8").parse().unwrap();
    let archive = Archive::new(ArchiveConfig {
        root: root.clone(),
        retention_days: 0,
        min_free_bytes: 0,
    });
    let live = match archive.day_data(day) {
        Some(DayData::Final) => false,
        Some(DayData::Provisional { sealed: true }) => true,
        _ => panic!("{day} is not replayable"),
    };
    let manifest = if live {
        archive.live_manifest(day)
    } else {
        archive.manifest(day)
    }
    .unwrap()
    .unwrap();
    let raw = archive.read_raw_bars(day).ok().flatten();
    // The research's prefilter (setup-research main.rs `prefilter_out`).
    let syms: Vec<String> = manifest
        .symbols
        .keys()
        .filter(|s| {
            let Some(Some(Some(b))) = raw.as_ref().map(|d| d.coverage(s)) else {
                return true;
            };
            let (lo, hi) = (b.l.unwrap_or(b.c), b.h.unwrap_or(b.c));
            !(lo > 30.0 || hi < 0.12)
        })
        .cloned()
        .collect();
    // Previous close: the latest date of daily-closes.json before the day (research `facts`).
    let closes: HashMap<String, HashMap<String, f64>> =
        serde_json::from_slice(&std::fs::read(root.join("daily-closes.json")).unwrap()).unwrap();
    let prev_date = closes
        .keys()
        .filter(|d| d.as_str() < day.to_string().as_str())
        .max()
        .cloned();
    let prev_close: HashMap<String, f64> = prev_date
        .and_then(|d| closes.get(&d).cloned())
        .unwrap_or_default();
    let adv = facts::adv30(&archive, day);
    eprintln!(
        "{day}: {} symbols ({} prefiltered out), {} closes, {} ADV30",
        syms.len(),
        manifest.symbols.len() - syms.len(),
        prev_close.len(),
        adv.len()
    );
    let t0 = market_events::session::et(day, 4, 0, 0, 0).unwrap().0;
    let next = AtomicUsize::new(0);
    let totals = Mutex::new(Totals::default());
    let fixture_syms: BTreeSet<String> = env("BURST_PARITY_FIXTURE", "")
        .split(',')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    let wall = Instant::now();
    std::thread::scope(|sc| {
        for _ in 0..threads {
            sc.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(sym) = syms.get(i) else { break };
                    let prints = match load(&archive, day, sym, live) {
                        Ok(p) => p,
                        Err(e) => {
                            totals.lock().unwrap().errors.push(format!("{sym}: {e}"));
                            continue;
                        }
                    };
                    let mut d = BurstDetector::new(BurstPolicy::default());
                    d.set_adv(day, adv.clone());
                    d.capture();
                    let pc = |s: &str| prev_close.get(s).copied();
                    let (mut apply_ns, mut tick_ns, mut ticks) = (0u128, 0u128, 0u64);
                    let mut alerts = Vec::new();
                    let mut j = 0;
                    // Every 5 s boundary of the day, checked at w + 1 s with the prints executed
                    // by then (applied in execution order, as the research sorted them).
                    let mut w = t0;
                    while w <= t0 + NSEC * SEC {
                        let now = w + SETTLE_NS;
                        let t = Instant::now();
                        while j < prints.len() && prints[j].0.event_ts.0 <= now {
                            d.apply_trade(&prints[j].0, prints[j].1);
                            j += 1;
                        }
                        apply_ns += t.elapsed().as_nanos();
                        let t = Instant::now();
                        alerts.extend(d.tick(now, &pc));
                        tick_ns += t.elapsed().as_nanos();
                        ticks += 1;
                        w += STEP_S * SEC;
                    }
                    let passes = d.take_captured();
                    let mut tt = totals.lock().unwrap();
                    tt.prints += prints.len() as u64;
                    tt.apply_ns += apply_ns;
                    tt.tick_ns += tick_ns;
                    tt.ticks = tt.ticks.max(ticks);
                    tt.passes.extend(passes);
                    tt.alerts.extend(alerts);
                    if fixture_syms.contains(sym) {
                        tt.fixture.insert(sym.clone(), prints);
                    }
                }
            });
        }
    });
    let tt = totals.into_inner().unwrap();
    assert!(
        tt.errors.is_empty(),
        "{:?}",
        &tt.errors[..tt.errors.len().min(5)]
    );
    eprintln!(
        "replayed in {:.0} s; detector alone: {} prints, {:.0} ns per print (incl. the 5 s filling); \
         5 s checks: {} boundaries, {:.1} µs per boundary summed over all symbols; {} gate passes, {} alerts",
        wall.elapsed().as_secs_f64(),
        tt.prints,
        tt.apply_ns as f64 / tt.prints.max(1) as f64,
        tt.ticks,
        tt.tick_ns as f64 / tt.ticks.max(1) as f64 / 1e3,
        tt.passes.len(),
        tt.alerts.len(),
    );

    // 1-2. Gate passes and features against rows5.
    let rows =
        std::fs::read_to_string(reports.join(format!("export-test/rows5-{day}.csv"))).unwrap();
    let mut lines = rows.lines();
    let header: Vec<&str> = lines.next().unwrap().split(',').collect();
    let col = |n: &str| header.iter().position(|h| *h == n).unwrap();
    let (c_sym, c_t, c_gate, c_f0) = (col("sym"), col("t"), col("gate"), col("ld5"));
    let mut research: BTreeMap<(String, i64), Vec<String>> = BTreeMap::new();
    for l in lines {
        let c: Vec<&str> = l.split(',').collect();
        if c[c_gate] != "1" {
            continue;
        }
        research.insert(
            (c[c_sym].to_string(), c[c_t].parse().unwrap()),
            c[c_f0..c_f0 + NF].iter().map(|s| s.to_string()).collect(),
        );
    }
    let mine: BTreeMap<(String, i64), &Pass> = tt
        .passes
        .iter()
        .map(|p| ((p.symbol.to_string(), p.t_ns), p))
        .collect();
    let rk: BTreeSet<_> = research.keys().cloned().collect();
    let mk: BTreeSet<_> = mine.keys().cloned().collect();
    let only_r: Vec<_> = rk.difference(&mk).take(10).collect();
    let only_m: Vec<_> = mk.difference(&rk).take(10).collect();
    eprintln!(
        "gate passes: research {}, live {}, research only {} (e.g. {only_r:?}), live only {} (e.g. {only_m:?})",
        rk.len(),
        mk.len(),
        rk.difference(&mk).count(),
        mk.difference(&rk).count()
    );
    let mut cells = 0usize;
    let mut diff: BTreeMap<&str, usize> = BTreeMap::new();
    let mut examples = Vec::new();
    for (k, f) in &research {
        let Some(p) = mine.get(k) else { continue };
        for (n, name) in FEATS.iter().enumerate() {
            cells += 1;
            let m = num(p.x[n]);
            if m != f[n] {
                *diff.entry(name).or_default() += 1;
                if examples.len() < 12 {
                    examples.push(format!(
                        "{} {} {name}: research {:?} live {:?} ({:e})",
                        k.0, k.1, f[n], m, p.x[n]
                    ));
                }
            }
        }
    }
    eprintln!("feature cells compared: {cells}; differing per feature: {diff:?}");
    if !tt.fixture.is_empty() {
        // The always-run fixture (`tests.rs` `fixture_replays_like_the_research`): each
        // symbol's counted prints, its outside facts and the research's feature strings.
        let mut s = format!(
            "# burst parity fixture, {day}: research export-test/rows5 + archive-hist prints\n"
        );
        for (sym, prints) in &tt.fixture {
            s += &format!(
                "c,{sym},{},{}\n",
                prev_close
                    .get(sym)
                    .map_or(String::new(), |x| format!("{x:?}")),
                adv.get(sym).map_or(String::new(), |x| format!("{x:?}"))
            );
            for (t, c) in prints {
                s += &format!(
                    "p,{sym},{},{},{},{},{},{}\n",
                    t.event_ts.0,
                    t.price.micros(),
                    t.size,
                    u8::from(c.ohlc),
                    u8::from(c.volume),
                    t.exchange
                );
            }
            for ((rs, t), f) in research.range((sym.clone(), i64::MIN)..=(sym.clone(), i64::MAX)) {
                debug_assert_eq!(rs, sym);
                s += &format!("r,{sym},{t},{}\n", f.join(","));
            }
        }
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/rocket/burst/burst_fixture.csv");
        std::fs::write(&path, s).unwrap();
        eprintln!("fixture written: {}", path.display());
    }
    for e in &examples {
        eprintln!("  {e}");
    }

    // 3. Alerts against the research's alert file at the frozen B20 threshold.
    let af = std::fs::read_to_string(reports.join("test/alerts-model-theta-B20.csv")).unwrap();
    let mut al = af.lines();
    let h: Vec<&str> = al.next().unwrap().split(',').collect();
    let ci = |n: &str| h.iter().position(|x| *x == n).unwrap();
    let (a_day, a_sym, a_alert, a_score) = (ci("day"), ci("sym"), ci("alert_ns"), ci("score"));
    let mut ra: BTreeMap<(String, i64), f64> = BTreeMap::new();
    for l in al {
        let c: Vec<&str> = l.split(',').collect();
        if c[a_day] == day.to_string() {
            ra.insert(
                (c[a_sym].to_string(), c[a_alert].parse().unwrap()),
                c[a_score].parse().unwrap(),
            );
        }
    }
    let ma: BTreeMap<(String, i64), f64> = tt
        .alerts
        .iter()
        .map(|a| ((a.pass.symbol.to_string(), a.at_ns), a.pass.score))
        .collect();
    let mut worst = 0f64;
    for (k, s) in &ra {
        if let Some(m) = ma.get(k) {
            worst = worst.max((m - s).abs());
        }
    }
    eprintln!(
        "alerts: research {} {:?}\n        live     {} {:?}\n        max |score diff| on common alerts {worst:e}",
        ra.len(),
        ra.keys().collect::<Vec<_>>(),
        ma.len(),
        ma.keys().collect::<Vec<_>>()
    );
    assert_eq!(rk, mk, "gate passes differ");
    assert!(diff.is_empty(), "features differ");
    assert_eq!(
        ra.keys().collect::<Vec<_>>(),
        ma.keys().collect::<Vec<_>>(),
        "alerts differ"
    );
    assert!(worst <= 1e-9, "scores differ by {worst:e}");
}
