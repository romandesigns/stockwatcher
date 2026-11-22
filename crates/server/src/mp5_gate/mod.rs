//! rw-mp5-1: the MP5 crossings of every stock (`signals_crossing`, the Detection Panel's scope),
//! gated by "in play or prior runner" (A-G9). Owner approval 2026-11-22 ("everything that has
//! proven better than what's live, push it live"; DEPLOY-PLAN.md, last addendum): on Train
//! (669 days, 2021-10-29..2024-06-28) B0 plus this stream beat B0 by +5.85 pp recall
//! [LB +5.59] and +0.61 pp precision [LB +0.44], at about 64 more alerts a day; 4 of 4 folds. It
//! has NOT had its judge look (H2 + 2025-04..09) yet: if the judge fails it, the switch turns it
//! off (`detectors` → `rw-mp5-1`, below).
//!
//! # What it changes
//!
//! Only who gets a `signals_crossing` notice. The crossings themselves are production's, decided
//! by `crate::cue_notices` from the stock-context engine's `micro_pullback` exactly as before;
//! they exist only with `DETECTION_PANEL=1` (off until the deploy that ships this turns it on:
//! an env change, so a restart after 20:00 ET). Without this gate every one of them (~73 a day in
//! research) would reach every device, since the Signals crossing switch is on by default. With
//! it, each one is held in the alerts task ([`Mp5Hold`]) for 5 s, then:
//!
//! - held `mp5_dedup` when a B0 alert ([`logic::B0_KINDS`]) of the same stock came within 5 s
//!   either way (the research removed those: they are the same move told twice);
//! - held `mp5_gate` when G9 fails ([`logic`] module docs), or the day was not watched whole
//!   (`incomplete`, below);
//! - held `switched_off` when the owner turned `rw-mp5-1` off;
//! - otherwise delivered as any Runners notice (devices, quiet hours, stale, duplicate).
//!
//! Every outcome is ledgered with the gate's facts (`crate::alerts::ledger`, `detector`,
//! `detector_version`, `gate`), sent or held, so the daily report can score the held ones too.
//! Ross crossings (a Ross member's episode) and Runners pullback crossings are untouched: they
//! were live before and are not this stream.
//!
//! # Inputs, all live
//!
//! - **The day's shares per 5-minute bucket** ([`Mp5Live::apply_trade`]): every print the
//!   Runners board counts, with the chart engine's volume verdict, by execution time, 04:00–20:00
//!   ET (the research store's 15 s bars summed per bucket, `tod_agg.vol`).
//! - **The time-of-day baseline**: the same per-bucket figures of the 20 prior sessions, which
//!   this module writes itself at 20:00 ET (`ARCHIVE_ROOT/tod-agg/<date>.tsv`, [`write_day`]) and
//!   reads at 03:30 ET ([`load_table`]). A prior session with no file is rebuilt from the
//!   archive's tape on the loader's thread ([`day_from_archive`], the same chart engine verdicts),
//!   so the first live day after the deploy has its 20 sessions without an owner step, as long
//!   as the archive still holds those days' tape. A day this process did not watch from 04:00 is
//!   written with `complete=0` and never used as a baseline session (it would deflate it).
//! - **c1 and runs90** from `daily-bars/raw/` (the research's `refdata::DailyBars::facts`, the
//!   same files) with the corporate-actions ledger's splits, read at 03:30 ET with the baseline.
//!   runs90 needs a stock's last 91 daily bars: the archive keeps `DAILY_BARS_KEEP_DAYS` dates
//!   (63 by default), so **production needs `DAILY_BARS_KEEP_DAYS >= 92`** (an owner step; the
//!   nightly import backfills the missing dates) or runs90 counts only the pairs it has.
//! - **The security type** from `assets.json` (universe-v1, `crate::n1_live::logic::sec_type`).
//!
//! # A day not watched whole decides nothing
//!
//! As N1 (`crate::n1_live`, "A day not watched whole never alerts"): after a restart mid-day the
//! earlier prints are not replayed into the bucket shares, so rvol_day would be too low all day.
//! Such a day holds every crossing (`mp5_gate`, why `incomplete`), and so does a day whose table
//! (facts and baseline) was not loaded. A deploy after 20:00 ET is watched whole the next day.
//!
//! # Known differences from the research rows (measured in the parity report)
//!
//! - **Decision instant.** Research: the replay's 1 s engine tick after the drain
//!   (`decided_ns`); live: when the alerts task receives the crossing (sub-second later, after
//!   the spread stage). The bucket count `k` and the 5 s window can differ near a boundary.
//! - **Dedup clock.** Research compared replay ticks; live compares the alerts task's receive
//!   times of both notices, and delivers 5 s later than the crossing (the symmetric window).
//! - **Late prints.** Research bars hold every print by execution time, with hindsight; live
//!   holds those that arrived by the decision (+5 s). Prints after 20:00 ET and before 04:00 never
//!   count, as in the store.
//! - **Daily history.** Research read raw daily bars since 2021-01-04; live reads the last
//!   [`FACTS_LOOKBACK`] dates the archive keeps, so a stock with fewer than 91 bars in them has
//!   runs90 over the pairs it has.
//! - **Splits.** Research: the provider's corporate-actions history; live: the ledger the
//!   nightly `swimport maintain` keeps from the same provider's announcements.
//! - **Universe.** Research dropped a crossing on a stock with no in-band 15 s close that day
//!   (cache envelope); live checks the crossing's own price and the type only.

pub mod logic;

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::NaiveDate;
use market_events::session::{SessionKind, sessions_for, trading_date};
use market_events::{MarketEvent, Trade, UnixNanos};

use crate::quotes::Counted;
pub use logic::{Facts, NB, SymDay, Verdict, Why};

/// Days of raw daily bars read at most (N1's lookback, `crate::n1_live::FACTS_LOOKBACK`).
pub const FACTS_LOOKBACK: usize = 260;
/// When the table is read: 30 minutes before 04:00 ET (after the nightly import), as N1.
pub const FACTS_LEAD_NS: i64 = 30 * 60 * logic::SEC;
/// The day is watched whole when this process saw its first instant by 04:01 ET.
const START_GRACE_NS: i64 = 60 * logic::SEC;
/// Where the per-day bucket figures live, under `ARCHIVE_ROOT`.
pub const TOD_DIR: &str = "tod-agg";
/// Day files kept (the baseline reads 20; the rest are slack for b-good's 20-day figures and a
/// missed night).
pub const TOD_KEEP: usize = 40;
/// The settings switch id (`detectors` namespace, `crate::alerts`).
pub const SWITCH: &str = logic::DETECTOR;

/// One trading day's clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Day {
    pub date: NaiveDate,
    /// 04:00 ET.
    pub t0: i64,
    /// The end of the extended session (20:00 ET, earlier on a half day).
    pub end: i64,
}

pub fn day_of(date: NaiveDate) -> Option<Day> {
    let s = sessions_for(date);
    let t0 = s.iter().find(|(id, _, _)| id.kind == SessionKind::Pre)?.1.0;
    let end = s.iter().map(|(_, _, e)| e.0).max()?;
    Some(Day { date, t0, end })
}

fn ymd(d: NaiveDate) -> u32 {
    d.format("%Y%m%d")
        .to_string()
        .parse()
        .expect("a date is digits")
}

/// The day's gate inputs: facts and baselines per symbol, read once a day ([`load_table`]).
#[derive(Clone, Debug, Default)]
pub struct Table {
    pub date: Option<NaiveDate>,
    pub facts: HashMap<String, Facts>,
    pub base: HashMap<String, Box<[f32; NB]>>,
    /// Baseline sessions used (complete day files strictly before the date, at most 20).
    pub sessions: usize,
    /// Of those, rebuilt from the archive's tape by this load.
    pub backfilled: usize,
    pub raw_dates: usize,
    pub newest_raw: Option<NaiveDate>,
    pub splits: usize,
}

/// The live state, under the core lock beside the Runners board: O(1) per print.
#[derive(Default)]
pub struct Mp5Live {
    on: bool,
    day: Option<Day>,
    started_at: Option<i64>,
    syms: HashMap<String, Box<SymDay>>,
    table: Option<Arc<Table>>,
    table_wanted: Option<NaiveDate>,
    table_asked: Option<NaiveDate>,
    written: Option<NaiveDate>,
    finished: Option<Finished>,
}

/// A day's figures to write ([`write_day`]), taken by the market off the lock.
pub struct Finished {
    pub date: NaiveDate,
    pub complete: bool,
    pub syms: HashMap<String, Box<SymDay>>,
}

impl Mp5Live {
    pub fn new() -> Self {
        Self::default()
    }

    /// Turns the bucket shares on (the live market with the Detection Panel; a replay never does).
    pub fn enable(&mut self) {
        self.on = true;
    }

    pub fn enabled(&self) -> bool {
        self.on
    }

    /// Whether the current day was watched from 04:00 ET.
    pub fn complete(&self) -> bool {
        match (self.day, self.started_at) {
            (Some(d), Some(s)) => s <= d.t0 + START_GRACE_NS,
            _ => false,
        }
    }

    fn see(&mut self, t: i64) {
        if self.started_at.is_none() {
            self.started_at = Some(t);
        }
    }

    /// Moves to `date` (a later day only); false for an earlier one. A day left before its 20:00
    /// write (a server up across midnight that never ticked past the end) is written now.
    fn roll(&mut self, date: NaiveDate) -> bool {
        if self.day.is_some_and(|d| d.date == date) {
            return true;
        }
        if self.day.is_some_and(|d| d.date > date) {
            return false;
        }
        let Some(d) = day_of(date) else { return false };
        if let Some(prev) = self.day
            && self.written != Some(prev.date)
            && !self.syms.is_empty()
        {
            self.finished = Some(Finished {
                date: prev.date,
                complete: self.complete(),
                syms: std::mem::take(&mut self.syms),
            });
            self.written = Some(prev.date);
        }
        self.day = Some(d);
        self.syms.clear();
        if self.table.as_ref().is_some_and(|t| t.date != Some(date)) {
            self.table = None;
        }
        true
    }

    /// One print the Runners board counted (`counted` is the chart engine's verdict).
    pub fn apply_trade(&mut self, t: &Trade, counted: Counted) {
        if !self.on {
            return;
        }
        let ts = t.event_ts.0;
        self.see(ts);
        let Some(s) = t.session else { return };
        if s.kind == SessionKind::Overnight || !counted.volume {
            return;
        }
        if !self.roll(s.date) {
            return;
        }
        let d = self.day.expect("rolled");
        if logic::bucket_of(d.t0, ts).is_none() {
            return;
        }
        let px = t.price.micros() as f64 / 1e6;
        match self.syms.get_mut(t.symbol.as_str()) {
            Some(x) => x.add(d.t0, ts, t.size, px),
            None => {
                let mut x = Box::<SymDay>::default();
                x.add(d.t0, ts, t.size, px);
                self.syms.insert(t.symbol.clone(), x);
            }
        }
    }

    /// The clock reached `now` (the rocket task's tick): the table is asked for when due, and a
    /// day past its end is handed over for writing once.
    pub fn tick(&mut self, now: i64) {
        if !self.on {
            return;
        }
        self.see(now);
        let date = trading_date(UnixNanos(now));
        if !self.roll(date) {
            return;
        }
        let d = self.day.expect("rolled");
        if self.table.as_ref().is_none_or(|t| t.date != Some(d.date))
            && self.table_asked != Some(d.date)
            && now >= d.t0 - FACTS_LEAD_NS
        {
            self.table_asked = Some(d.date);
            self.table_wanted = Some(d.date);
        }
        if now >= d.end && self.written != Some(d.date) {
            self.written = Some(d.date);
            self.finished = Some(Finished {
                date: d.date,
                complete: self.complete(),
                // Cloned, not taken: a crossing decided in the last seconds still reads them.
                syms: self.syms.clone(),
            });
        }
    }

    /// The date whose table the market should load now, once ([`load_table`]).
    pub fn take_table_request(&mut self) -> Option<NaiveDate> {
        self.table_wanted.take()
    }

    /// A finished day to write ([`write_day`]).
    pub fn take_finished(&mut self) -> Option<Finished> {
        self.finished.take()
    }

    pub fn set_table(&mut self, table: Arc<Table>) {
        tracing::info!(
            date = ?table.date,
            symbols = table.facts.len(),
            baselines = table.base.len(),
            sessions = table.sessions,
            backfilled = table.backfilled,
            raw_dates = table.raw_dates,
            newest_raw = ?table.newest_raw,
            splits = table.splits,
            "rw-mp5-1: table loaded"
        );
        if table.sessions < logic::BASE_LONG {
            tracing::warn!(
                sessions = table.sessions,
                "rw-mp5-1: fewer than 20 baseline sessions; the 5-session baseline is used (research's rule)"
            );
        }
        self.table = Some(table);
    }

    /// G9 on a crossing of `symbol` at `price`, decided at `a` (the alerts task's receive time):
    /// [`Why::Incomplete`] on a day not watched whole or without its table.
    pub fn verdict(&self, symbol: &str, price: f64, a: i64) -> Verdict {
        let day = self.day.filter(|d| trading_date(UnixNanos(a)) == d.date);
        let table = self
            .table
            .as_deref()
            .filter(|t| day.is_some_and(|d| t.date == Some(d.date)));
        let (Some(d), Some(t), true) = (day, table, self.complete()) else {
            let t0 = day.map_or(a, |d| d.t0);
            let mut v = logic::decide(t0, a, price, None, None, None);
            v.why = Why::Incomplete;
            return v;
        };
        logic::decide(
            d.t0,
            a,
            price,
            t.facts.get(symbol),
            self.syms.get(symbol).map(|s| &s.vol),
            t.base.get(symbol).map(|b| &**b),
        )
    }
}

// ---------------------------------------------------------------------------------------------
// The day files (`ARCHIVE_ROOT/tod-agg/<date>.tsv`).
//
// Line 1: `# tod-agg v1 date=<YYYY-MM-DD> complete=<0|1> source=<live|archive>`; then one line
// per symbol: `SYM<TAB>b:n:vol:dol b:n:vol:dol …`, buckets with volume only, ascending. Plain
// text on purpose (no compression crate in the server): ~5–8 MB a day.

pub fn tod_dir(archive_root: &Path) -> PathBuf {
    archive_root.join(TOD_DIR)
}

fn day_path(dir: &Path, date: NaiveDate) -> PathBuf {
    dir.join(format!("{date}.tsv"))
}

/// Writes one day (temp file + rename) and prunes to [`TOD_KEEP`] files.
pub fn write_day(
    dir: &Path,
    date: NaiveDate,
    complete: bool,
    source: &str,
    syms: &HashMap<String, Box<SymDay>>,
) -> std::io::Result<()> {
    write_day_keep(dir, date, complete, source, syms, TOD_KEEP)
}

/// [`write_day`] keeping the newest `keep` files (the parity harness keeps every one).
pub fn write_day_keep(
    dir: &Path,
    date: NaiveDate,
    complete: bool,
    source: &str,
    syms: &HashMap<String, Box<SymDay>>,
    keep: usize,
) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("{date}.tsv.tmp"));
    {
        let mut w = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        writeln!(
            w,
            "# tod-agg v1 date={date} complete={} source={source}",
            complete as u8
        )?;
        let mut names: Vec<&String> = syms.keys().collect();
        names.sort();
        for s in names {
            let x = &syms[s];
            write!(w, "{s}\t")?;
            let mut first = true;
            for b in 0..NB {
                if x.vol[b] == 0 && x.n[b] == 0 {
                    continue;
                }
                if !first {
                    w.write_all(b" ")?;
                }
                first = false;
                write!(w, "{b}:{}:{}:{}", x.n[b], x.vol[b], x.dol[b])?;
            }
            w.write_all(b"\n")?;
        }
        w.flush()?;
    }
    std::fs::rename(&tmp, day_path(dir, date))?;
    let mut days = file_dates(dir);
    days.sort();
    for d in days.iter().take(days.len().saturating_sub(keep)) {
        let _ = std::fs::remove_file(day_path(dir, *d));
    }
    Ok(())
}

fn file_dates(dir: &Path) -> Vec<NaiveDate> {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| {
                    let n = e.file_name().to_string_lossy().to_string();
                    n.strip_suffix(".tsv")?.parse().ok()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A day's shares per bucket, by symbol.
pub type DayVol = HashMap<String, Box<[u64; NB]>>;

/// One day file's shares per bucket (only the symbols in `want`, when given) and whether it is
/// complete. `None` when there is no readable file.
pub fn read_day_vol(
    dir: &Path,
    date: NaiveDate,
    want: Option<&std::collections::HashSet<String>>,
) -> Option<(bool, DayVol)> {
    let text = std::fs::read_to_string(day_path(dir, date)).ok()?;
    let mut lines = text.lines();
    let head = lines.next()?;
    if !head.starts_with("# tod-agg v1 ") || !head.contains(&format!("date={date} ")) {
        tracing::warn!(%date, "rw-mp5-1: a tod-agg file with an unexpected header; ignored");
        return None;
    }
    let complete = head.contains(" complete=1 ");
    let mut out = HashMap::new();
    for l in lines {
        let Some((s, rest)) = l.split_once('\t') else {
            continue;
        };
        if want.is_some_and(|w| !w.contains(s)) {
            continue;
        }
        let mut v = Box::new([0u64; NB]);
        for cell in rest.split(' ').filter(|c| !c.is_empty()) {
            let mut it = cell.split(':');
            let (Some(b), Some(_n), Some(vol)) = (it.next(), it.next(), it.next()) else {
                continue;
            };
            if let (Ok(b), Ok(vol)) = (b.parse::<usize>(), vol.parse::<u64>())
                && b < NB
            {
                v[b] = vol;
            }
        }
        out.insert(s.to_string(), v);
    }
    Some((complete, out))
}

/// A past day's figures rebuilt from the archive's tape: every `.swa` of the day through a 15 s
/// chart engine (production's verdicts, as `engine_a::tape::Tape::prints` reads them), each
/// volume-counted print by execution time into its bucket. `None` without the day's folder.
pub fn day_from_archive(
    archive: &market_archive::Archive,
    date: NaiveDate,
) -> Option<HashMap<String, Box<SymDay>>> {
    let d = day_of(date)?;
    let rd = std::fs::read_dir(archive.day_dir(date)).ok()?;
    let ext = format!(".{}", market_archive::format::EXT);
    let mut names: Vec<String> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            n.strip_suffix(&ext).map(str::to_string)
        })
        .collect();
    names.sort();
    if names.is_empty() {
        return None;
    }
    let mut out = HashMap::new();
    for sym in names {
        let Ok(r) = archive.reader(date, &sym) else {
            continue;
        };
        let Ok(mut engine) = chart_engine::ChartEngine::new(chart_engine::EngineConfig {
            timeframes: vec![chart_engine::Timeframe::S15],
            ..Default::default()
        }) else {
            continue;
        };
        let mut x = SymDay::default();
        let mut any = false;
        for rec in r {
            let Ok(rec) = rec else { break };
            let e = rec.event;
            let ups = engine.apply(&e);
            if let MarketEvent::Trade(t) = &e
                && let Some(chart_engine::wire::ChartUpdate::Tape(row)) = ups.first()
                && Counted::from_tape(&row.counted).volume
            {
                x.add(d.t0, t.event_ts.0, t.size, t.price.micros() as f64 / 1e6);
                any = true;
            }
        }
        if any {
            out.insert(sym, Box::new(x));
        }
    }
    Some(out)
}

/// Reads `date`'s table (module docs, "Inputs"): facts, splits, types, and the baseline from the
/// 20 prior complete day files, rebuilding missing prior sessions from the archive first. Off the
/// ingest path: the market runs it on a blocking thread.
pub fn load_table(archive: &market_archive::Archive, date: NaiveDate) -> Table {
    let splits = match archive.read_action_ledger() {
        Ok(l) => market_archive::splits_from_ledger(&l),
        Err(e) => {
            tracing::warn!(error = %e, "rw-mp5-1: corporate-actions ledger unreadable; c1 and runs90 are not split-restated");
            Vec::new()
        }
    };
    load_table_with(archive, &tod_dir(archive.root()), date, &splits)
}

/// [`load_table`] with the day files under `dir` and the splits given (the parity harness feeds
/// the research's own split history and a scratch directory; live passes the ledger's).
pub fn load_table_with(
    archive: &market_archive::Archive,
    dir: &Path,
    date: NaiveDate,
    splits: &[market_archive::SplitFact],
) -> Table {
    let dates: Vec<NaiveDate> = archive
        .raw_bar_dates()
        .unwrap_or_default()
        .into_iter()
        .filter(|d| *d < date)
        .collect();
    let tail = &dates[dates.len().saturating_sub(FACTS_LOOKBACK)..];
    let mut by_sym: HashMap<String, Vec<logic::DailyBar>> = HashMap::new();
    let mut read = 0;
    for d in tail {
        let Some(b) = archive.read_raw_bars(*d).ok().flatten() else {
            continue;
        };
        read += 1;
        let dn = ymd(*d);
        for (s, bar) in &b.bars {
            if let (Some(h), Some(_l)) = (bar.h, bar.l) {
                by_sym
                    .entry(s.clone())
                    .or_default()
                    .push(logic::DailyBar { d: dn, h, c: bar.c });
            }
        }
    }
    let mut split_by: HashMap<String, Vec<(u32, f64)>> = HashMap::new();
    for s in splits {
        let v = split_by.entry(s.symbol.clone()).or_default();
        let e = ymd(s.effective);
        if !v.iter().any(|(d, _)| *d == e) {
            v.push((e, s.factor));
        }
    }
    for v in split_by.values_mut() {
        v.sort_by_key(|x| x.0);
    }
    let names: HashMap<String, String> = archive
        .read_assets()
        .ok()
        .flatten()
        .unwrap_or_default()
        .into_iter()
        .map(|(s, a)| (s, a.name))
        .collect();
    let day = ymd(date);
    let mut facts: HashMap<String, Facts> = by_sym
        .iter()
        .map(|(s, v)| {
            let sp = split_by.get(s).map_or(&[][..], |x| &x[..]);
            let (c1, runs90) = logic::facts_of(v, sp, day);
            let excluded = crate::n1_live::logic::sec_type(s, names.get(s).map(String::as_str))
                == crate::n1_live::logic::SecType::Excluded;
            (
                s.clone(),
                Facts {
                    c1,
                    runs90,
                    excluded,
                },
            )
        })
        .collect();
    // Excluded types without daily bars too (the research never had them).
    for (s, n) in &names {
        if !facts.contains_key(s)
            && crate::n1_live::logic::sec_type(s, Some(n))
                == crate::n1_live::logic::SecType::Excluded
        {
            facts.insert(
                s.clone(),
                Facts {
                    excluded: true,
                    ..Facts::default()
                },
            );
        }
    }
    // The baseline: the 20 prior sessions are the raw daily-bar dates before the day (the
    // trading calendar the import keeps), each read from its day file or rebuilt from the tape.
    let prior: Vec<NaiveDate> = dates[dates.len().saturating_sub(logic::BASE_LONG)..].to_vec();
    let mut backfilled = 0;
    let mut days: Vec<DayVol> = Vec::new();
    for d in &prior {
        match read_day_vol(dir, *d, None) {
            Some((true, v)) => days.push(v),
            Some((false, _)) => {
                tracing::warn!(date = %d, "rw-mp5-1: day file incomplete; not a baseline session");
            }
            None => {
                let t = std::time::Instant::now();
                match day_from_archive(archive, *d) {
                    Some(x) => {
                        if let Err(e) = write_day(dir, *d, true, "archive", &x) {
                            tracing::warn!(date = %d, error = %e, "rw-mp5-1: rebuilt day not saved");
                        }
                        tracing::info!(date = %d, symbols = x.len(), ms = t.elapsed().as_millis() as u64, "rw-mp5-1: baseline day rebuilt from the archive");
                        backfilled += 1;
                        days.push(x.into_iter().map(|(s, v)| (s, Box::new(v.vol))).collect());
                    }
                    None => {
                        tracing::warn!(date = %d, "rw-mp5-1: no day file and no tape; not a baseline session");
                    }
                }
            }
        }
    }
    let mut all: std::collections::BTreeSet<&String> = std::collections::BTreeSet::new();
    for d in &days {
        all.extend(d.keys());
    }
    let base: HashMap<String, Box<[f32; NB]>> = all
        .into_iter()
        .map(|s| {
            let rows: Vec<Option<&[u64; NB]>> =
                days.iter().map(|d| d.get(s).map(|b| &**b)).collect();
            (s.clone(), Box::new(logic::baseline(&rows)))
        })
        .collect();
    Table {
        date: Some(date),
        facts,
        base,
        sessions: days.len(),
        backfilled,
        raw_dates: read,
        newest_raw: tail.last().copied(),
        splits: splits.len(),
    }
}

// ---------------------------------------------------------------------------------------------
// The alerts task's side: the 5 s hold, the switch and the verdict.

/// What the alerts task decides on a released crossing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outcome {
    Deliver(Verdict),
    /// A B0 alert of the stock within 5 s.
    Dedup,
    Gate(Verdict),
    SwitchedOff,
}

/// The crossings parked in the alerts task ([`logic::Dedup`]) and today's counts.
#[derive(Default)]
pub struct Mp5Hold {
    dedup: logic::Dedup<(crate::rocket::Notice, i64)>,
}

impl Mp5Hold {
    /// Whether this module gates `n` (a Signals crossing).
    pub fn gates(n: &crate::rocket::Notice) -> bool {
        n.kind == crate::rocket::NoticeKind::SignalsCrossing
    }

    /// Every notice the alerts task receives, at `now_ns`: a B0 kind is remembered; a Signals
    /// crossing is parked (returns true: do not deliver it now).
    pub fn observe(&mut self, n: &crate::rocket::Notice, now_ns: i64) -> bool {
        if Self::gates(n) {
            self.dedup.park(&n.symbol, now_ns, (n.clone(), now_ns));
            return true;
        }
        if logic::is_b0_kind(n.kind.as_str()) {
            self.dedup.note_b0(&n.symbol, now_ns);
        }
        false
    }

    /// The parked crossings whose window closed by `now_ns`, with their decision instant and
    /// whether a B0 alert came within 5 s.
    pub fn release(&mut self, now_ns: i64) -> Vec<((crate::rocket::Notice, i64), bool)> {
        self.dedup.release(now_ns)
    }

    pub fn pending(&self) -> usize {
        self.dedup.pending()
    }
}

/// The ledger's extra fields for a gated crossing (`crate::alerts::ledger`).
pub fn ledger_fields(o: &Outcome, a_ns: i64, t0: Option<i64>) -> serde_json::Value {
    let f = |x: f64| {
        if x.is_finite() {
            serde_json::json!(x)
        } else if x.is_nan() {
            serde_json::json!("nan")
        } else {
            serde_json::json!(if x > 0.0 { "inf" } else { "-inf" })
        }
    };
    let gate = match o {
        Outcome::Deliver(v) | Outcome::Gate(v) => serde_json::json!({
            "chg": f(v.chg), "rvol_day": f(v.rvol_day), "runs90": v.runs90,
            "completed_buckets": v.completed, "why": v.why.code(), "pass": v.why.pass(),
        }),
        Outcome::Dedup => serde_json::json!({"why": "b0_within_5s", "pass": false}),
        Outcome::SwitchedOff => serde_json::json!({"why": "switched_off", "pass": false}),
    };
    let session = t0.map(|t0| {
        // good_label.session_of: pre before 09:30, regular to 16:00, after past it.
        let m = (a_ns - t0).div_euclid(60 * logic::SEC);
        if m < 330 {
            "pre"
        } else if m < 720 {
            "regular"
        } else {
            "after"
        }
    });
    serde_json::json!({
        "detector": logic::DETECTOR,
        "detector_version": format!("{}@{}", logic::CONFIG_SHA256, &logic::CODE_COMMIT[..8]),
        "decided_ns": a_ns,
        "session": session,
        "gate": gate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use logic::SEC;

    fn table(date: NaiveDate) -> Arc<Table> {
        let mut base = Box::new([0f32; NB]);
        base[0] = 100.0;
        let mut facts = HashMap::new();
        facts.insert(
            "AAA".to_string(),
            Facts {
                c1: Some(1.0),
                runs90: 0,
                excluded: false,
            },
        );
        Arc::new(Table {
            date: Some(date),
            facts,
            base: [("AAA".to_string(), base)].into_iter().collect(),
            sessions: 20,
            ..Table::default()
        })
    }

    fn print(sym: &str, ns: i64, px: f64, size: u64) -> Trade {
        Trade::new(
            sym,
            UnixNanos(ns),
            market_events::Price::from_f64_rounded(px).unwrap(),
            size,
            1,
        )
    }

    const BOTH: Counted = Counted {
        ohlc: true,
        volume: true,
    };

    #[test]
    fn a_day_watched_whole_decides_on_its_own_buckets_and_table() {
        let date = NaiveDate::from_ymd_opt(2026, 11, 23).unwrap();
        let d = day_of(date).unwrap();
        let mut m = Mp5Live::new();
        m.tick(d.t0 - FACTS_LEAD_NS);
        assert_eq!(m.take_table_request(), None, "off until enabled");
        m.enable();
        m.tick(d.t0 - FACTS_LEAD_NS);
        assert_eq!(m.take_table_request(), Some(date));
        m.set_table(table(date));
        m.apply_trade(&print("AAA", d.t0 + 10 * SEC, 1.2, 400), BOTH);
        // Not volume-counted, before 04:00 or after 20:00: never in a bucket.
        let ohlc_only = Counted {
            ohlc: true,
            volume: false,
        };
        m.apply_trade(&print("AAA", d.t0 + 20 * SEC, 1.2, 999), ohlc_only);
        m.apply_trade(&print("AAA", d.end + SEC, 1.2, 999), BOTH);
        // Bucket 0 is still running at 04:04: no completed bucket, rvol +inf (research rule).
        let v = m.verdict("AAA", 1.2, d.t0 + 240 * SEC);
        assert_eq!(v.rvol_day, f64::INFINITY);
        let v = m.verdict("AAA", 1.2, d.t0 + 300 * SEC + 1);
        assert_eq!((v.why, v.rvol_day, v.completed), (Why::InPlay, 4.0, 1));
        assert_eq!(m.verdict("AAA", 1.05, d.t0 + 300 * SEC + 1).why, Why::Gate);
        assert_eq!(m.verdict("ZZZ", 1.05, d.t0 + 300 * SEC + 1).why, Why::Gate);
        m.tick(d.end);
        let f = m.take_finished().expect("written at the end");
        assert!(f.complete);
        assert_eq!(f.syms["AAA"].vol[0], 400);
        assert_eq!(f.syms["AAA"].n[0], 1);
        m.tick(d.end + SEC);
        assert!(m.take_finished().is_none(), "once");
    }

    #[test]
    fn a_day_not_watched_whole_or_without_its_table_holds_everything() {
        let date = NaiveDate::from_ymd_opt(2026, 11, 23).unwrap();
        let d = day_of(date).unwrap();
        let mut late = Mp5Live::new();
        late.enable();
        late.tick(d.t0 + 3600 * SEC);
        late.set_table(table(date));
        late.apply_trade(&print("AAA", d.t0 + 3600 * SEC, 1.2, 400), BOTH);
        assert_eq!(
            late.verdict("AAA", 1.2, d.t0 + 7200 * SEC).why,
            Why::Incomplete
        );
        late.tick(d.end);
        assert!(!late.take_finished().unwrap().complete);
        let mut none = Mp5Live::new();
        none.enable();
        none.tick(d.t0 - SEC);
        assert_eq!(
            none.verdict("AAA", 1.2, d.t0 + 600 * SEC).why,
            Why::Incomplete
        );
    }

    #[test]
    fn day_files_round_trip_and_only_complete_ones_are_baseline_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let date = NaiveDate::from_ymd_opt(2026, 11, 20).unwrap();
        let mut x = SymDay::default();
        x.add(0, 5, 1234, 2.5);
        x.add(0, 300 * SEC + 1, 7, 2.5);
        let syms: HashMap<String, Box<SymDay>> = [("AAA".to_string(), Box::new(x))].into();
        write_day(dir.path(), date, false, "live", &syms).unwrap();
        let (complete, v) = read_day_vol(dir.path(), date, None).unwrap();
        assert!(!complete);
        assert_eq!((v["AAA"][0], v["AAA"][1]), (1234, 7));
        write_day(dir.path(), date, true, "archive", &syms).unwrap();
        assert!(read_day_vol(dir.path(), date, None).unwrap().0);
        let want: std::collections::HashSet<String> = ["BBB".to_string()].into();
        assert!(
            read_day_vol(dir.path(), date, Some(&want))
                .unwrap()
                .1
                .is_empty()
        );
        // Pruned to TOD_KEEP files, oldest first.
        for i in 0..(TOD_KEEP + 3) {
            let d = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap() + chrono::Days::new(i as u64);
            write_day(dir.path(), d, true, "live", &syms).unwrap();
        }
        let mut left = file_dates(dir.path());
        left.sort();
        assert_eq!(left.len(), TOD_KEEP);
        assert_eq!(left.last(), Some(&date));
    }

    #[test]
    fn the_hold_parks_signals_crossings_and_remembers_only_b0_kinds() {
        use crate::rocket::{Notice, NoticeKind};
        let n = |kind, sym: &str| Notice {
            spread_bp: None,
            no_quote: false,
            kind,
            alert_id: format!("{sym}-1"),
            symbol: sym.into(),
            at_ns: 0,
            price: 1.0,
            from_flag_pct: None,
            reasons: Vec::new(),
            since_start_min: None,
            exec_ns: None,
            detail: None,
        };
        let mut h = Mp5Hold::default();
        assert!(h.observe(&n(NoticeKind::SignalsCrossing, "AAA"), 10 * SEC));
        assert!(h.observe(&n(NoticeKind::SignalsCrossing, "BBB"), 10 * SEC));
        assert!(h.observe(&n(NoticeKind::SignalsCrossing, "CCC"), 10 * SEC));
        // A B0 kind, a Ross crossing and a Breakout (not B0) on the three stocks.
        assert!(!h.observe(&n(NoticeKind::PullbackCrossing, "AAA"), 12 * SEC));
        assert!(!h.observe(&n(NoticeKind::RossCrossing, "BBB"), 12 * SEC));
        assert!(!h.observe(&n(NoticeKind::Breakout, "CCC"), 12 * SEC));
        assert!(h.release(15 * SEC).is_empty());
        let r: Vec<(String, i64, bool)> = h
            .release(15 * SEC + 1)
            .into_iter()
            .map(|((n, a), dup)| (n.symbol, a, dup))
            .collect();
        assert_eq!(
            r,
            vec![
                ("AAA".into(), 10 * SEC, true),
                ("BBB".into(), 10 * SEC, false),
                ("CCC".into(), 10 * SEC, false)
            ]
        );
        assert_eq!(h.pending(), 0);
    }

    #[test]
    fn the_ledger_fields_name_the_detector_and_keep_non_finite_numbers() {
        let v = logic::decide(0, 10 * SEC, 1.2, None, None, None);
        let x = ledger_fields(&Outcome::Gate(v), 10 * SEC, Some(0));
        assert_eq!(x["detector"], "rw-mp5-1");
        assert!(
            x["detector_version"]
                .as_str()
                .unwrap()
                .starts_with(logic::CONFIG_SHA256)
        );
        assert_eq!(x["session"], "pre");
        assert_eq!(x["gate"]["chg"], "nan");
        assert_eq!(x["gate"]["rvol_day"], "inf");
        assert_eq!(x["gate"]["why"], "gate");
        let y = ledger_fields(&Outcome::Dedup, 12 * 3600 * SEC, Some(0));
        assert_eq!(
            (y["session"].clone(), y["gate"]["why"].clone()),
            ("after".into(), "b0_within_5s".into())
        );
    }
}
