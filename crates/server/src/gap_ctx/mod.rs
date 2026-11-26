//! v2 research finalists **x-gap-1** ("pre-market new high": a ≥ 10 % gapper makes a new
//! session high on volume before 09:30) and **x-ctx-1** ("day 2": a stock that ran ≥ 30 %
//! yesterday breaks above yesterday's high on volume), live beside production. Owner-approved
//! ship set of 2026-11-22 (`C:/wavystack/outputs/stockwatcher-research-v2/DEPLOY-PLAN.md`, last
//! addendum: "everything that has proven better … push it live"). Each only ADDS notices
//! ([`crate::rocket::NoticeKind::XGap`] / [`crate::rocket::NoticeKind::XCtx`], data kinds
//! `v2.xgap` / `v2.xctx`); nothing production decides changes.
//!
//! # Evidence (Train 2021-10-29..2024-06-28, Validation 2024-07..2025-03; judge pending)
//!
//! - x-gap-1: Train 12.0 a day at 21.7 % GOOD (+10 % before −5 % with RVOL ≥ 2), Δ recall
//!   +1.86 pp over production; Validation +1.9 pp recall, +0.20 pp precision, 22 a day.
//! - x-ctx-1: Train 4.5 a day at 23.2 % GOOD, Δ recall +1.10 pp; Validation +1.2 / +0.12 pp,
//!   8 a day. Mostly complementary to x-gap-1 (+0.85 pp recall over production + x-gap-1).
//!
//! The judge (sealed H2 + 2025-04..09) still runs on the frozen finalists; one it fails is
//! switched off live ([`DETECTORS`]).
//!
//! # Switches (no restart)
//!
//! - **Per item, for everyone** — the synced settings namespace `detectors`
//!   (`crate::alerts::DETECTORS_NS`): `{"x-gap-1": {"on": false}}` holds every notice of that
//!   item at the alerts task (`held: switched_off` in the ledger, both paths), while the detector
//!   keeps deciding, so the kill rules of the deploy plan act at once and a switched-off item's
//!   would-be alerts are still on record. Absent = on.
//! - **Per device / desk** — `xGap` / `xCtx` in Settings → Alerts, as every Runners kind.
//! - `GAP_CTX_LIVE=0` (env, needs a restart) removes the detector entirely.
//!
//! # What it reads (module [`logic`] has every rule with the research's arithmetic)
//!
//! - **Prints**: every print the Runners board counts, with the chart engine's verdict,
//!   04:00–20:00 ET, into 15 s bars on the 04:00 grid ([`logic::BarAcc`]). A bar is decided at
//!   its end + 50 ms on the rocket task's clock.
//! - **Facts** (read at 03:30 ET on a blocking thread, [`load_table`]): C1 and H1 (the prior
//!   session's close and high, put on the day's share basis with the corporate-actions ledger's
//!   splits) and prev_ran (the prior session's high ≥ 1.3 × its own prior close, as runs365
//!   growing overnight) from `daily-bars/raw/`, the last [`RAW_LOOKBACK`] dates before the day;
//!   security types from `assets.json` (`universe-v1`, the excluded types never fire).
//! - **Time-of-day baselines**: the 5- and 20-session mean dollars of each 5-minute bucket per
//!   symbol, from this module's own per-session files `ARCHIVE_ROOT/tod/<date>.json`
//!   ([`TodWriter`], written when the trading date moves on at 20:00 ET, for a day watched
//!   whole). Nothing else in the server
//!   keeps them. Until [`TOD_MIN_SESSIONS`] of the 20 prior sessions have a file the day is
//!   incomplete (nothing is sent); a one-time backfill of 20 sessions from the VPS archive gives
//!   full fidelity from the first day (owner / deploy step, not built here).
//!
//! # A day not watched whole never alerts
//!
//! Both need every print since 04:00 (cumulative dollars, the session high, the trailing 5
//! minutes), so a restart mid-day makes the day incomplete: it keeps deciding (the counts in the
//! day summary) but sends nothing, and writes no ToD file. So does a day whose facts were stale
//! (the newest raw daily bar is not the previous trading day: a C1 a day old would misread every
//! gap), had too few ToD sessions, or never arrived. Facts asked for (03:30 ET, or at a start
//! after that) and still being read when a bar is due make the bars wait, up to
//! [`INPUTS_WAIT_NS`]; they are decided in order once the table lands, as if it had been there
//! ([`STALE_FIRE_NS`] keeps a fire minutes old from being sent). The board ticks this module
//! before 04:00 and after 20:00 ET too (`RocketBoard::tick`), so the 03:30 request and the 20:00
//! ToD cut happen on time. A deploy after 20:00 ET is watched whole the next day. Whether today
//! sends is [`GapCtxLive::readiness`], logged on each change and on `/api/health`
//! (`crate::readiness`).
//!
//! # Known differences from the research stream (parity report:
//! `C:/wavystack/outputs/stockwatcher-research-v2/50-ship/gap-ctx/PARITY.md`)
//!
//! - **Bars.** The research read the cache's bars, built from the store with hindsight
//!   (canceled / incorrect prints dropped) and, for gated symbols, rebuilt from 1 s rows each
//!   rounded to $0.0001 VWAP. Live builds them from the counted prints as they arrive and rounds
//!   the bar's VWAP the same way ([`logic::cache_dollars`]).
//! - **Arrival.** A print reaching the server after its bar was decided (50 ms + the tick after
//!   the bar's end) is not in that decision; its dollars still count for later bars (counted,
//!   `late` in the cost line).
//! - **Universe.** The research archive held a symbol only on days its close was in $0.05–$30;
//!   live sees every symbol. The ToD files apply the same membership by the day's last print.
//! - **prev_ran** needs 367 daily bars to reproduce the research's 365-session window exactly
//!   (a run dropping out of the window on the same step masks a new one there). With the
//!   production default `DAILY_BARS_KEEP_DAYS` = 63 that masking cannot happen, so a few more
//!   symbol-days qualify than in research; set `DAILY_BARS_KEEP_DAYS` ≥ 368 on the VPS.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{Datelike, NaiveDate};
use market_events::session::{SessionKind, previous_trading_day, sessions_for, trading_date};
use market_events::{Trade, UnixNanos};
use serde_json::{Value, json};

use crate::n1_live::logic::{SecType, sec_type};
use crate::quotes::Counted;
use crate::rocket::{Notice, NoticeDetail, NoticeKind};

pub mod logic;
use logic::{BarAcc, Daily, Facts, SymDay, TodDay};

/// The detector ids the `detectors` switch namespace is keyed by.
pub const DETECTORS: [&str; 2] = [logic::XGAP_ID, logic::XCTX_ID];
/// Raw daily-bar dates read at most: runs365 on the day AND on the prior session (366 + 1),
/// plus one spare.
pub const RAW_LOOKBACK: usize = 368;
/// Sessions of ToD baseline the day reads (`k = 20`) and the fewest files that make it complete.
pub const TOD_SESSIONS: usize = 20;
pub const TOD_MIN_SESSIONS: usize = 5;
/// Archive membership of a symbol-day for the ToD files (research archive: closes $0.05–$30).
pub const TOD_PRESENT_LO_U: i64 = 50_000;
pub const TOD_PRESENT_HI_U: i64 = 30_000_000;
/// ToD files kept (sessions are ~252 a year; 20 are read).
pub const TOD_KEEP_DAYS: i64 = 120;
/// When the day's facts are read: 30 minutes before 04:00 ET (N1's rule).
pub const FACTS_LEAD_NS: i64 = 30 * logic::MIN;
const START_GRACE_NS: i64 = logic::MIN;
/// How long the day's first decisions wait for a table that was asked for and is still being
/// read (a server started between 03:30 and 04:00, a slow disk). The bars stay open meanwhile
/// and are decided in order when it lands, exactly as if it had been there; past this the day
/// is decided without it and sends nothing (`table_late`). rw2 and b-good use the same bound.
pub const INPUTS_WAIT_NS: i64 = 10 * logic::MIN;
/// A fire decided more than this after its own instant (only while catching up on a table that
/// came late) is recorded but not sent: an alert minutes old is not news.
pub const STALE_FIRE_NS: i64 = 2 * logic::MIN;
const MAX_NOTICES: usize = 256;
const MAX_FIRED: usize = 500;

/// The day's inputs: per symbol its [`SymDay`] (only symbols that can fire), the security types,
/// and what was read.
#[derive(Clone, Debug, Default)]
pub struct Table {
    pub date: Option<NaiveDate>,
    pub syms: HashMap<String, SymDay>,
    /// For the notice text only: C1, and yesterday's run.
    pub facts: HashMap<String, Facts>,
    pub types: HashMap<String, SecType>,
    pub raw_dates: usize,
    pub newest_raw: Option<NaiveDate>,
    /// The newest raw daily bar is not the previous trading day: nothing is sent today.
    pub stale: bool,
    /// ToD session files found among the [`TOD_SESSIONS`] prior sessions.
    pub tod_sessions: usize,
    pub ctx_eligible: usize,
}

impl Table {
    pub fn sec_type(&self, sym: &str) -> SecType {
        self.types
            .get(sym)
            .copied()
            .unwrap_or_else(|| sec_type(sym, None))
    }
}

fn day_num(d: NaiveDate) -> i32 {
    d.num_days_from_ce()
}

/// `ARCHIVE_ROOT/tod/`.
pub fn tod_dir(root: &Path) -> PathBuf {
    root.join("tod")
}

fn tod_path(root: &Path, date: NaiveDate) -> PathBuf {
    tod_dir(root).join(format!("{date}.json"))
}

/// One session's ToD file: `{"v":1,"date","present":[sym…],"agg":{sym:[[bucket,n,vol,dol]…]}}`,
/// `dol` the bucket's `f32` dollars (`f64` sum of its bars' `f32` dollars, as `tod_agg.parquet`),
/// `n` / `vol` the volume-counted prints and shares (for the other v2 items' baselines).
pub fn read_tod(root: &Path, date: NaiveDate) -> Option<TodDay> {
    let raw = std::fs::read(tod_path(root, date)).ok()?;
    let v: Value = serde_json::from_slice(&raw).ok()?;
    if v["date"].as_str() != Some(date.to_string().as_str()) {
        tracing::warn!(%date, "gap/ctx: a ToD file holds another date; ignored");
        return None;
    }
    let mut d = TodDay::default();
    for s in v["present"].as_array()? {
        if let Some(s) = s.as_str() {
            d.present.insert(s.to_owned());
        }
    }
    for (s, rows) in v["agg"].as_object()? {
        let mut out = Vec::new();
        for r in rows.as_array()? {
            let b = r[0].as_u64().and_then(|b| u8::try_from(b).ok())?;
            // An f32 widened to f64 prints and parses back to the same f32.
            out.push((b, r[3].as_f64()? as f32));
        }
        d.agg.insert(s.clone(), out);
    }
    Some(d)
}

/// The day's [`Table`] (module docs), off the ingest path.
pub fn load_table(archive: &market_archive::Archive, date: NaiveDate) -> Table {
    let dates: Vec<NaiveDate> = archive
        .raw_bar_dates()
        .unwrap_or_default()
        .into_iter()
        .filter(|d| *d < date)
        .collect();
    let tail = &dates[dates.len().saturating_sub(RAW_LOOKBACK)..];
    let mut bars: HashMap<String, Vec<Daily>> = HashMap::new();
    let mut read = 0;
    for d in tail {
        let Some(b) = archive.read_raw_bars(*d).ok().flatten() else {
            continue;
        };
        read += 1;
        for (s, bar) in &b.bars {
            // `refdata::DailyBars::load`: h, l and c required.
            if let (Some(h), Some(_), c) = (bar.h, bar.l, bar.c) {
                bars.entry(s.clone()).or_default().push(Daily {
                    day: day_num(*d),
                    h,
                    c,
                });
            }
        }
    }
    let mut splits: HashMap<String, Vec<(i32, f64)>> = HashMap::new();
    match archive.read_action_ledger() {
        Ok(l) => {
            for s in market_archive::splits_from_ledger(&l) {
                splits
                    .entry(s.symbol)
                    .or_default()
                    .push((day_num(s.effective), s.factor));
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "gap/ctx: corporate-actions ledger unreadable; C1 / H1 are raw")
        }
    }
    for v in splits.values_mut() {
        v.sort_by_key(|x| x.0);
        v.dedup_by(|a, b| a.0 == b.0);
    }
    let prev = previous_trading_day(date);
    let newest = tail.last().copied();
    let facts: HashMap<String, Facts> = bars
        .iter()
        .map(|(s, v)| {
            let sp = splits.get(s).map(Vec::as_slice).unwrap_or(&[]);
            (
                s.clone(),
                logic::facts_of(v, sp, day_num(date), Some(day_num(prev))),
            )
        })
        .collect();
    let assets = archive.read_assets().ok().flatten().unwrap_or_default();
    let types: HashMap<String, SecType> = assets
        .iter()
        .map(|(s, a)| (s.clone(), sec_type(s, Some(&a.name))))
        .collect();
    // The 20 sessions before the day, oldest first; a missing file is a session nobody was in.
    let sessions = &dates[dates.len().saturating_sub(TOD_SESSIONS)..];
    let mut found = 0;
    let prior: Vec<TodDay> = sessions
        .iter()
        .map(|d| match read_tod(archive.root(), *d) {
            Some(t) => {
                found += 1;
                t
            }
            None => TodDay::default(),
        })
        .collect();
    let want: Vec<&str> = facts
        .iter()
        .filter(|(s, f)| {
            f.c1.is_some_and(|c| c > 0.0)
                && types
                    .get(s.as_str())
                    .copied()
                    .unwrap_or_else(|| sec_type(s, None))
                    != SecType::Excluded
        })
        .map(|(s, _)| s.as_str())
        .collect();
    let mut bases = logic::baselines(&prior, want.iter().copied());
    let mut syms = HashMap::new();
    let mut ctx_eligible = 0;
    for s in want {
        let sd = SymDay::new(&facts[s], bases.remove(s).unwrap_or_default());
        ctx_eligible += sd.h1_u.is_some() as usize;
        syms.insert(s.to_owned(), sd);
    }
    Table {
        date: Some(date),
        syms,
        facts,
        types,
        raw_dates: read,
        newest_raw: newest,
        stale: newest != Some(prev),
        tod_sessions: found,
        ctx_eligible,
    }
}

/// One symbol's session aggregates: bucket → (volume-counted prints, shares, `f64` dollars),
/// and the day's last OHLC print (micro-dollars) for the archive membership.
type TodSym = (HashMap<u8, (u64, u64, f64)>, i64);

/// One session's ToD aggregates, built from every counted print of the day.
#[derive(Clone, Debug, Default)]
pub struct TodWriter {
    syms: HashMap<Arc<str>, TodSym>,
}

impl TodWriter {
    fn add_bar(&mut self, sym: &Arc<str>, b: &BarAcc) {
        let e = self.syms.entry(sym.clone()).or_default();
        let k = u8::try_from(b.slot / logic::BUCKET_SLOTS).unwrap_or(u8::MAX);
        let a = e.0.entry(k).or_default();
        a.0 += u64::from(b.n);
        a.1 += b.vol;
        a.2 += f64::from(b.dollars);
        if b.c > 0 {
            e.1 = b.c;
        }
    }

    /// The file's JSON for `date` (membership and excluded types applied).
    pub fn to_json(&self, date: NaiveDate, types: impl Fn(&str) -> SecType) -> Value {
        let mut present = Vec::new();
        let mut agg = serde_json::Map::new();
        let mut names: Vec<&Arc<str>> = self.syms.keys().collect();
        names.sort();
        for s in names {
            let (buckets, last) = &self.syms[s];
            if !(TOD_PRESENT_LO_U..=TOD_PRESENT_HI_U).contains(last)
                || types(s) == SecType::Excluded
            {
                continue;
            }
            present.push(Value::from(s.to_string()));
            let mut rows: Vec<(&u8, &(u64, u64, f64))> = buckets.iter().collect();
            rows.sort_by_key(|(b, _)| **b);
            agg.insert(
                s.to_string(),
                Value::from(
                    rows.into_iter()
                        .map(|(b, (n, v, d))| json!([b, n, v, f64::from(*d as f32)]))
                        .collect::<Vec<_>>(),
                ),
            );
        }
        json!({ "v": 1, "date": date.to_string(), "present": present, "agg": agg })
    }
}

/// Writes a session's ToD file (temp + rename) and prunes files older than [`TOD_KEEP_DAYS`].
pub fn write_tod(root: &Path, date: NaiveDate, v: &Value) -> std::io::Result<PathBuf> {
    let dir = tod_dir(root);
    std::fs::create_dir_all(&dir)?;
    let p = tod_path(root, date);
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(v).expect("json"))?;
    std::fs::rename(&tmp, &p)?;
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if let Some(d) = n
                .strip_suffix(".json")
                .and_then(|s| s.parse::<NaiveDate>().ok())
                && (date - d).num_days() > TOD_KEEP_DAYS
            {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    Ok(p)
}

/// One fire's facts (the notice's [`NoticeDetail::GapCtx`], its text and its ledger fields).
#[derive(Clone, Debug, PartialEq)]
pub struct GapCtxNotice {
    /// `x-gap-1` or `x-ctx-1` ([`DETECTORS`]).
    pub detector: &'static str,
    pub version: &'static str,
    /// x-gap: the gap over C1. x-ctx: how far above H1 the close is.
    pub gap: f64,
    /// Dollars since 04:00, the last 5 minutes', and their ratio to the usual (None = a zero
    /// baseline: never traded in that bucket).
    pub cum: f64,
    pub trail: f64,
    pub rvol: Option<f64>,
    /// 20 or 5: which baseline was used.
    pub base_kind: u8,
    /// x-gap: the earlier session high. x-ctx: H1 (dollars).
    pub level: f64,
    pub c1: Option<f64>,
    /// Yesterday's run (x-ctx).
    pub prev_run: Option<f64>,
    /// `pre`, `regular` or `after` (ET, of the decision instant).
    pub session: &'static str,
    /// The close print's arrival at this server (epoch ns), when the feed stamps it.
    pub arrival_ns: Option<i64>,
    /// When the server decided it (the rocket task's clock).
    pub emitted_ns: i64,
}

/// A row for the cost line and the day summary.
#[derive(Clone, Debug, PartialEq)]
pub struct Fired {
    pub symbol: String,
    pub t: i64,
    pub price: f64,
    pub detector: &'static str,
    pub notified: bool,
}

#[derive(Clone, Copy, Debug)]
struct Day {
    date: NaiveDate,
    /// 04:00 ET, epoch ns.
    t0: i64,
}

fn day_of(date: NaiveDate) -> Option<Day> {
    let s = sessions_for(date);
    let t0 = s.iter().find(|(id, _, _)| id.kind == SessionKind::Pre)?.1.0;
    Some(Day { date, t0 })
}

fn session_of(t: i64, t0: i64) -> &'static str {
    let r = t - t0;
    if r < 330 * logic::MIN {
        "pre"
    } else if r < 720 * logic::MIN {
        "regular"
    } else {
        "after"
    }
}

/// A symbol's live state: the bars not decided yet and the detectors' running state.
#[derive(Debug)]
struct SymLive {
    open: Vec<BarAcc>,
    /// The last slot decided (−1: none).
    decided: i64,
    /// The decision instant this symbol is queued for in `due` (`i64::MAX`: none), so a symbol
    /// is in the queue once.
    queued: i64,
    state: logic::Sym,
}

#[derive(Clone, Copy, Debug, Default)]
struct Counts {
    bars: u64,
    gap_fires: usize,
    ctx_fires: usize,
    notified: usize,
    late_prints: u64,
    /// Fires decided over [`STALE_FIRE_NS`] after their instant (a late table): not sent.
    stale: usize,
}

/// The detectors' live state (module docs). Fed under the core lock with the Runners board:
/// O(1) per print but a rare late one, and per tick only the symbols with a bar due.
#[derive(Default)]
pub struct GapCtxLive {
    day: Option<Day>,
    started_at: Option<i64>,
    table: Option<Arc<Table>>,
    table_wanted: Option<NaiveDate>,
    table_asked: Option<NaiveDate>,
    table_late: bool,
    /// The first decision instant that waited for the asked-for table ([`INPUTS_WAIT_NS`]).
    wait_since: Option<i64>,
    readiness: crate::readiness::Tracker,
    syms: HashMap<Arc<str>, SymLive>,
    due: BinaryHeap<Reverse<(i64, Arc<str>)>>,
    tod: TodWriter,
    tod_cut: bool,
    tod_out: Option<(NaiveDate, Value)>,
    counts: Counts,
    fired: Vec<Fired>,
    notices: Vec<Notice>,
}

impl GapCtxLive {
    pub fn new() -> Self {
        Self::default()
    }

    /// The day's facts of one symbol (C1, and the pre-open Watch card's recent +50 % day), once
    /// the table for the trading date is loaded.
    pub fn facts(&self, sym: &str) -> Option<&Facts> {
        self.table.as_ref()?.facts.get(sym)
    }

    /// The trading date the loaded facts are for.
    pub fn facts_date(&self) -> Option<NaiveDate> {
        self.table.as_ref()?.date
    }

    /// Whether today was watched from 04:00 with its facts in time, fresh, and enough ToD.
    pub fn complete(&self) -> bool {
        let Some(d) = self.day else { return false };
        let watched = self.started_at.is_some_and(|s| s <= d.t0 + START_GRACE_NS);
        let table_ok = self.table.as_ref().is_some_and(|t| {
            t.date == Some(d.date) && !t.stale && t.tod_sessions >= TOD_MIN_SESSIONS
        });
        watched && !self.table_late && table_ok
    }

    /// Whether the day was watched whole (what the ToD file needs: not the facts).
    fn watched_whole(&self) -> bool {
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

    fn roll(&mut self, date: NaiveDate) -> bool {
        if self.day.is_some_and(|d| d.date == date) {
            return true;
        }
        if self.day.is_some_and(|d| d.date > date) {
            return false;
        }
        let Some(d) = day_of(date) else { return false };
        if let Some(prev) = self.day {
            if !self.tod_cut {
                self.tod_cut = true;
                self.cut_tod(prev);
            }
            let c = self.counts;
            tracing::info!(
                day = %prev.date,
                bars = c.bars,
                x_gap_fires = c.gap_fires,
                x_ctx_fires = c.ctx_fires,
                notified = c.notified,
                complete = self.complete(),
                late_prints = c.late_prints,
                "gap/ctx: day summary"
            );
        }
        self.day = Some(d);
        self.syms.clear();
        self.due.clear();
        self.tod = TodWriter::default();
        self.tod_cut = false;
        self.counts = Counts::default();
        self.fired.clear();
        self.table_late = false;
        self.wait_since = None;
        if self.table.as_ref().is_some_and(|t| t.date != Some(date)) {
            self.table = None;
        }
        true
    }

    /// Whether the decisions due by `now` wait for the day's table: it was asked for, has not
    /// landed, and the first decision that waited is less than [`INPUTS_WAIT_NS`] old. Nothing is
    /// lost by waiting: the bars stay open and are decided in order once it lands (the rocket
    /// task is woken then: `Market::load_gap_ctx`).
    fn waiting(&mut self, now: i64, d: Day) -> bool {
        if self.table_late
            || self.table_asked != Some(d.date)
            || self.table.as_ref().is_some_and(|t| t.date == Some(d.date))
        {
            return false;
        }
        let Some(Reverse((t, _))) = self.due.peek() else {
            return false;
        };
        if *t > now {
            return false;
        }
        let since = *self.wait_since.get_or_insert(*t);
        now - since < INPUTS_WAIT_NS
    }

    /// Whether today's fires are sent, and if not why ([`crate::readiness`]).
    pub fn readiness(&self) -> crate::readiness::Readiness {
        use crate::readiness::Readiness;
        let Some(d) = self.day else {
            return Readiness::waiting(None, "no trading day seen yet");
        };
        let date = Some(d.date);
        if !self.watched_whole() {
            return Readiness::silent(date, "the day was not watched from 04:00 ET (a restart)");
        }
        if self.table_late {
            return Readiness::silent(
                date,
                "the day's facts were not loaded in time for its first bar",
            );
        }
        let Some(t) = self.table.as_ref().filter(|t| t.date == date) else {
            return Readiness::waiting(
                date,
                "facts and ToD baselines not loaded yet (read from 03:30 ET)",
            );
        };
        if t.stale {
            return Readiness::silent(
                date,
                format!(
                    "the newest daily bar ({}) is not the previous trading day",
                    t.newest_raw.map_or("none".into(), |d| d.to_string())
                ),
            );
        }
        if t.tod_sessions < TOD_MIN_SESSIONS {
            return Readiness::silent(
                date,
                format!(
                    "{} ToD sessions on file of the last {TOD_SESSIONS} (needs {TOD_MIN_SESSIONS})",
                    t.tod_sessions
                ),
            );
        }
        Readiness::ready(date)
    }

    /// One print the Runners board counted. O(1) (a late print walks a few bars).
    pub fn apply_trade(&mut self, t: &Trade, counted: Counted) {
        let ts = t.event_ts.0;
        self.see(ts);
        let Some(s) = t.session else { return };
        if s.kind == SessionKind::Overnight || !(counted.ohlc || counted.volume) {
            return;
        }
        if !self.roll(s.date) {
            return;
        }
        let d = self.day.expect("rolled");
        if ts < d.t0 {
            return;
        }
        let k = (ts - d.t0) / logic::SLOT;
        if k >= logic::NSLOT {
            return;
        }
        let p = logic::Print {
            ns: ts,
            px: t.price.micros(),
            size: t.size,
            ohlc: counted.ohlc,
            vol: counted.volume,
        };
        let arrival = t.receive_ts.map_or(0, |r| r.0);
        let key: Arc<str> = match self.syms.get_key_value(t.symbol.as_str()) {
            Some((k, _)) => k.clone(),
            None => {
                let k: Arc<str> = Arc::from(t.symbol.as_str());
                self.syms.insert(
                    k.clone(),
                    SymLive {
                        open: Vec::new(),
                        decided: -1,
                        queued: i64::MAX,
                        state: logic::Sym::new(),
                    },
                );
                k
            }
        };
        let sym = self.syms.get_mut(&key).expect("inserted");
        if k <= sym.decided {
            // Its bar was already decided: the dollars still count for later bars and the
            // session's ToD; the close / high of that bar stay as decided.
            self.counts.late_prints += 1;
            if p.vol {
                let mut b = BarAcc::new(k);
                b.add(&p, arrival);
                sym.state.add_late(k, b.bar().dollars);
                self.tod.add_bar(&key, &BarAcc { c: 0, ..b });
            }
            return;
        }
        let i = sym.open.partition_point(|b| b.slot < k);
        if sym.open.get(i).is_none_or(|b| b.slot != k) {
            sym.open.insert(i, BarAcc::new(k));
            let t = d.t0 + (k + 1) * logic::SLOT + logic::DECIDE_NS;
            if t < sym.queued {
                sym.queued = t;
                self.due.push(Reverse((t, key.clone())));
            }
        }
        sym.open[i].add(&p, arrival);
    }

    /// The clock reached `now`: every bar whose decision instant has come is decided; the day's
    /// table is asked for when due; the ToD file is cut when the trading date moves on (20:00 ET:
    /// from then on `trading_date` is the next day's overnight session).
    pub fn tick(&mut self, now: i64) {
        self.see(now);
        let date = trading_date(UnixNanos(now));
        // The day's last bar (19:59:45–20:00:00) is due at 20:00:00.05, when the date has already
        // moved on: decide what is due on the old day first, then cut its file.
        if let Some(old) = self.day
            && date > old.date
        {
            self.decide_due(now, old);
        }
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
        self.decide_due(now, d);
        let r = self.readiness();
        self.readiness.note("gap/ctx", "x-gap-1,x-ctx-1", &r);
    }

    /// Every bar of day `d` whose decision instant is at or before `now`, unless the day's
    /// table is still on its way ([`Self::waiting`]).
    fn decide_due(&mut self, now: i64, d: Day) {
        if self.waiting(now, d) {
            return;
        }
        while let Some(Reverse((t, _))) = self.due.peek() {
            if *t > now {
                break;
            }
            let Reverse((t, key)) = self.due.pop().expect("peeked");
            // An entry superseded by an earlier one for the same symbol.
            match self.syms.get_mut(&key) {
                Some(s) if s.queued == t => s.queued = i64::MAX,
                _ => continue,
            }
            self.decide(&key, now, d);
        }
    }

    fn decide(&mut self, key: &Arc<str>, now: i64, d: Day) {
        let table = self.table.clone().filter(|t| t.date == Some(d.date));
        if table.is_none() && !self.table_late {
            self.table_late = true;
            tracing::warn!(day = %d.date, "gap/ctx: a bar was due before the day's facts were loaded; nothing is sent today");
        }
        let complete = self.complete();
        let empty = SymDay::default();
        let Some(sym) = self.syms.get_mut(key) else {
            return;
        };
        let sd = table
            .as_deref()
            .filter(|t| t.sec_type(key) != SecType::Excluded)
            .and_then(|t| t.syms.get(&**key))
            .unwrap_or(&empty);
        let mut fires = Vec::new();
        while let Some(b) = sym.open.first().copied() {
            if d.t0 + (b.slot + 1) * logic::SLOT + logic::DECIDE_NS > now {
                break;
            }
            sym.open.remove(0);
            sym.decided = b.slot;
            self.counts.bars += 1;
            self.tod.add_bar(key, &b);
            let (g, c) = sym.state.push(&b.bar(), sd, d.t0);
            for (det, f) in [(logic::XGAP_ID, g), (logic::XCTX_ID, c)] {
                if let Some(f) = f {
                    fires.push((det, f, b.c_ns, b.c_arrival));
                }
            }
        }
        if let Some(b) = sym.open.first() {
            let t = d.t0 + (b.slot + 1) * logic::SLOT + logic::DECIDE_NS;
            if t < sym.queued {
                sym.queued = t;
                self.due.push(Reverse((t, key.clone())));
            }
        }
        for (det, f, c_ns, arr) in fires {
            self.fire(key, det, &f, c_ns, arr, now, d, complete, table.as_deref());
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn fire(
        &mut self,
        key: &Arc<str>,
        det: &'static str,
        f: &logic::Fire,
        close_ns: i64,
        arrival: i64,
        now: i64,
        d: Day,
        complete: bool,
        table: Option<&Table>,
    ) {
        let gap = det == logic::XGAP_ID;
        if gap {
            self.counts.gap_fires += 1;
        } else {
            self.counts.ctx_fires += 1;
        }
        let facts = table.and_then(|t| t.facts.get(&**key));
        let base_kind = table
            .and_then(|t| t.syms.get(&**key))
            .map_or(0, |s| s.base.kind());
        let detail = GapCtxNotice {
            detector: det,
            version: if gap {
                logic::XGAP_VERSION
            } else {
                logic::XCTX_VERSION
            },
            gap: f.gap,
            cum: f.cum,
            trail: f.trail,
            rvol: f.rvol.is_finite().then_some(f.rvol),
            base_kind,
            level: f.level_u as f64 / 1e6,
            c1: facts.and_then(|x| x.c1),
            prev_run: facts.and_then(|x| x.prev_run),
            session: session_of(f.t, d.t0),
            arrival_ns: (arrival > 0).then_some(arrival),
            emitted_ns: now,
        };
        let price = f.close_u as f64 / 1e6;
        let stale = now - f.t > STALE_FIRE_NS;
        tracing::info!(
            symbol = %key, detector = det, t_ns = f.t, emitted_ns = now, price, gap = f.gap,
            cum = f.cum, rvol = f.rvol, complete, stale, "gap/ctx: fire"
        );
        let send = complete && !stale;
        if self.fired.len() < MAX_FIRED {
            self.fired.push(Fired {
                symbol: key.to_string(),
                t: f.t,
                price,
                detector: det,
                notified: send,
            });
        }
        if complete && stale {
            self.counts.stale += 1;
        }
        if !send {
            return;
        }
        self.counts.notified += 1;
        if self.notices.len() < MAX_NOTICES {
            let (kind, tag) = if gap {
                (NoticeKind::XGap, "xgap")
            } else {
                (NoticeKind::XCtx, "xctx")
            };
            self.notices.push(Notice {
                spread_bp: None,
                no_quote: false,
                kind,
                // Once a symbol-day per detector, so this is unique (`Notice::event_id`).
                alert_id: format!("{}-{key}-{tag}", d.date),
                symbol: key.to_string(),
                at_ns: f.t,
                price,
                from_flag_pct: None,
                reasons: Vec::new(),
                since_start_min: None,
                exec_ns: Some(close_ns),
                detail: Some(NoticeDetail::GapCtx(detail)),
            });
        }
    }

    /// 20:00 ET: every open bar into the aggregates, then the session's ToD file, when the day
    /// was watched whole (a partial day would read as a quiet session to the next 20).
    fn cut_tod(&mut self, d: Day) {
        for (k, s) in self.syms.iter_mut() {
            for b in s.open.drain(..) {
                self.tod.add_bar(k, &b);
            }
        }
        if !self.watched_whole() {
            tracing::warn!(day = %d.date, "gap/ctx: day not watched whole; no ToD file for it");
            return;
        }
        let table = self.table.clone();
        let v = self.tod.to_json(d.date, |s| match table.as_deref() {
            Some(t) => t.sec_type(s),
            None => sec_type(s, None),
        });
        self.tod_out = Some((d.date, v));
    }

    /// The finished session's ToD file for the market to write (once, after 20:00).
    pub fn take_tod(&mut self) -> Option<(NaiveDate, Value)> {
        self.tod_out.take()
    }

    /// The date whose table the market should load now, once ([`load_table`]).
    pub fn take_table_request(&mut self) -> Option<NaiveDate> {
        self.table_wanted.take()
    }

    pub fn set_table(&mut self, t: Table) {
        tracing::info!(
            date = ?t.date,
            symbols = t.syms.len(),
            ctx_eligible = t.ctx_eligible,
            raw_dates = t.raw_dates,
            newest_raw = ?t.newest_raw,
            stale = t.stale,
            tod_sessions = t.tod_sessions,
            "gap/ctx: facts and baselines loaded"
        );
        if t.stale {
            tracing::warn!(date = ?t.date, newest_raw = ?t.newest_raw, "gap/ctx: the newest daily bar is not the previous trading day; nothing is sent today");
        }
        if t.tod_sessions < TOD_MIN_SESSIONS {
            tracing::warn!(date = ?t.date, tod_sessions = t.tod_sessions, "gap/ctx: too few ToD sessions; nothing is sent today");
        }
        self.table = Some(Arc::new(t));
    }

    pub fn take_notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.notices)
    }

    pub fn fired(&self) -> &[Fired] {
        &self.fired
    }

    /// The cost line's `gap_ctx` entry.
    pub fn cost(&self) -> Value {
        let c = self.counts;
        json!({
            "day": self.day.map(|d| d.date.to_string()),
            "complete": self.complete(),
            "symbols": self.syms.len(),
            "open_bars": self.syms.values().map(|s| s.open.len()).sum::<usize>(),
            "due": self.due.len(),
            "table": self.table.as_ref().map(|t| json!({
                "date": t.date.map(|d| d.to_string()), "symbols": t.syms.len(),
                "ctx_eligible": t.ctx_eligible, "raw_dates": t.raw_dates,
                "newest_raw": t.newest_raw.map(|d| d.to_string()), "stale": t.stale,
                "tod_sessions": t.tod_sessions,
            })),
            "bars": c.bars,
            "x_gap_fires": c.gap_fires,
            "x_ctx_fires": c.ctx_fires,
            "notified": c.notified,
            "late_prints": c.late_prints,
            "stale": c.stale,
            "readiness": self.readiness().to_json(),
        })
    }
}

/// The detector id of a notice kind, for the `detectors` switch and the ledger.
pub fn detector_of(kind: NoticeKind) -> Option<&'static str> {
    match kind {
        NoticeKind::XGap => Some(logic::XGAP_ID),
        NoticeKind::XCtx => Some(logic::XCTX_ID),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
