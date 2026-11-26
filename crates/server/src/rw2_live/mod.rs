//! v2 rework-2 finalists **rw2-vwapr-1** ("VWAP reclaim": an in-play stock flushes under VWAP
//! and reclaims it for 30 s) and **rw2-level-1** ("20-day high break": a +10 % gainer breaks its
//! 20-session high on 20× volume and holds it 60 s), live beside production. Owner-approved ship
//! set of 2026-11-22 (`C:/wavystack/outputs/stockwatcher-research-v2/DEPLOY-PLAN.md`, last
//! addendum; `FINALISTS-v2c.md`, both CONFIRMED-TRAIN). Each only ADDS notices
//! ([`crate::rocket::NoticeKind::VwapReclaim`] / [`crate::rocket::NoticeKind::LevelBreak`], data
//! kinds `v2.vwapr` / `v2.level`); nothing production decides changes.
//!
//! # Evidence (Train 2021-10-29..2024-06-28; Validation was spent for both families; the judge's
//! # H2 + 2025-04..09 looks are pending)
//!
//! - rw2-vwapr-1: 18.5 a day at 21.95 % GOOD; union Δ precision +0.21 pp [LB +0.12], Δ recall
//!   +0.85 pp [LB +0.74], lead +64 s, 4/4 folds. Most of the lift is confirmation near a
//!   production alert (verifier); the pre-market slice is flat.
//! - rw2-level-1: 8.8 a day at 21.4 % GOOD; union Δ precision +0.074 pp [LB +0.014], Δ recall
//!   +0.61 pp, 4/4 folds. Thin; its after-hours alerts are 17.6 % GOOD.
//!
//! A finalist the judge fails is switched off live ([`DETECTORS`]).
//!
//! # Switches (no restart)
//!
//! - **Per item, for everyone** — the synced settings namespace `detectors`
//!   (`crate::alerts::DETECTORS_NS`): `{"rw2-vwapr-1": {"on": false}}` holds every notice of that
//!   item at the alerts task (`held: switched_off`, both paths), while the detector keeps
//!   deciding. Absent = on.
//! - **Per device / desk** — `vwapReclaim` / `levelBreak` in Settings → Alerts.
//! - `RW2_LIVE=0` (env, needs a restart) removes the detector entirely.
//!
//! # What it reads (module [`logic`] has every rule with the research's arithmetic)
//!
//! - **Prints**: every print the Runners board counts, with the chart engine's verdict,
//!   04:00–20:00 ET, into 15 s bars with close, high, LOW, shares and dollars
//!   ([`logic::BarAcc`]). A bar is decided at its end + 50 ms on the rocket task's clock; a
//!   pending 30 s hold is decided at its own end + 50 ms even without a bar there.
//! - **Facts** (03:30 ET, blocking thread, [`load_table`]): C1 (the prior session's close on
//!   the day's share basis, as x-gap-1's) and HI20 from `daily-bars/raw/` (the last
//!   [`RAW_LOOKBACK`] dates) and the corporate-actions ledger's splits; security types from
//!   `assets.json` (excluded types never fire).
//! - **HI20 presence**: whether a symbol was in the research archive on each of the 19 prior
//!   sessions. This module writes it per session, `ARCHIVE_ROOT/rw2/<date>.json` (the symbols
//!   with a 15 s close in $0.10–$25 that day, the archive's rule, for a day watched whole); a
//!   session without that file uses the daily-bar proxy [`logic::member`].
//! - **Time-of-day baselines**: x-gap-1's own files `ARCHIVE_ROOT/tod/<date>.json`
//!   (`crate::gap_ctx`, written at the 20:00 roll while `GAP_CTX_LIVE` is on), read with the same
//!   rule ([`crate::gap_ctx::logic::baselines`]). Fewer than
//!   [`crate::gap_ctx::TOD_MIN_SESSIONS`] files: the day is incomplete.
//!
//! # A day not watched whole never alerts
//!
//! VWAP, the session high and the dollars since 04:00 need every print of the day, so a restart
//! mid-day makes the day incomplete: it keeps deciding (the day summary counts) but sends
//! nothing, and writes no presence file. So does a day whose facts came late or were stale, or
//! had too few ToD sessions.
//!
//! # Known differences from the research stream (parity report:
//! `C:/wavystack/outputs/stockwatcher-research-v2/50-ship/rw2/PARITY.md`)
//!
//! - **Bars**, **arrival** and **universe** exactly as `crate::gap_ctx` documents them (the
//!   cache's bars were built with hindsight; a print reaching the server after its bar was
//!   decided is not in that decision; live sees every symbol).
//! - **HI20 value.** The research carried each prior high to the day's basis through its own
//!   per-session factors (snapped to 1 within ±3 %) in log space, so its levels hold float noise
//!   and, around a split or a bad daily high, a factor that is not the split's. Live restates
//!   with the split ledger once. A close exactly AT the level can therefore cross in one and not
//!   the other (the report counts every case).
//! - **HI20 presence** before the presence files exist: the daily-bar proxy (counted).

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{Datelike, NaiveDate};
use market_events::session::{SessionKind, previous_trading_day, sessions_for, trading_date};
use market_events::{Trade, UnixNanos};
use serde_json::{Value, json};

use crate::gap_ctx::logic::TodDay;
use crate::n1_live::logic::{SecType, sec_type};
use crate::quotes::Counted;
use crate::rocket::{Notice, NoticeDetail, NoticeKind};

pub mod logic;
use logic::{BarAcc, Daily, SymDay};

/// The detector ids the `detectors` switch namespace is keyed by.
pub const DETECTORS: [&str; 2] = [logic::VWAPR_ID, logic::LEVEL_ID];
/// Raw daily-bar dates read at most: C1 (the last bar before the day) and HI20's 20 columns
/// (19 prior sessions and the bar before each), with room for symbols that skip sessions. The
/// production archive keeps 63 (`DAILY_BARS_KEEP_DAYS`).
pub const RAW_LOOKBACK: usize = 64;
/// Presence files kept (19 are read).
pub const MEMBER_KEEP_DAYS: i64 = 60;
/// When the day's facts are read: 30 minutes before 04:00 ET (N1's rule).
pub const FACTS_LEAD_NS: i64 = 30 * logic::MIN;
const START_GRACE_NS: i64 = logic::MIN;
const MAX_NOTICES: usize = 256;
const MAX_FIRED: usize = 500;

/// The day's inputs: per symbol its [`SymDay`] (only symbols that can fire), the security types,
/// and what was read.
#[derive(Clone, Debug, Default)]
pub struct Table {
    pub date: Option<NaiveDate>,
    pub syms: HashMap<String, SymDay>,
    pub types: HashMap<String, SecType>,
    pub raw_dates: usize,
    pub newest_raw: Option<NaiveDate>,
    /// The newest raw daily bar is not the previous trading day: nothing is sent today.
    pub stale: bool,
    /// ToD session files found among the 20 prior sessions.
    pub tod_sessions: usize,
    /// Presence files found among HI20's 19 prior sessions (the rest used the daily-bar proxy).
    pub member_files: usize,
    pub level_eligible: usize,
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

/// `ARCHIVE_ROOT/rw2/`.
pub fn member_dir(root: &Path) -> PathBuf {
    root.join("rw2")
}

fn member_path(root: &Path, date: NaiveDate) -> PathBuf {
    member_dir(root).join(format!("{date}.json"))
}

/// One session's presence file: `{"v":1,"date","kept":[sym…]}`.
pub fn read_member(root: &Path, date: NaiveDate) -> Option<HashSet<String>> {
    let raw = std::fs::read(member_path(root, date)).ok()?;
    let v: Value = serde_json::from_slice(&raw).ok()?;
    if v["date"].as_str() != Some(date.to_string().as_str()) {
        tracing::warn!(%date, "rw2: a presence file holds another date; ignored");
        return None;
    }
    Some(
        v["kept"]
            .as_array()?
            .iter()
            .filter_map(|s| s.as_str().map(str::to_owned))
            .collect(),
    )
}

/// Writes a session's presence file (temp + rename) and prunes files older than
/// [`MEMBER_KEEP_DAYS`].
pub fn write_member(root: &Path, date: NaiveDate, v: &Value) -> std::io::Result<PathBuf> {
    let dir = member_dir(root);
    std::fs::create_dir_all(&dir)?;
    let p = member_path(root, date);
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(v).expect("json"))?;
    std::fs::rename(&tmp, &p)?;
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if let Some(d) = n
                .strip_suffix(".json")
                .and_then(|s| s.parse::<NaiveDate>().ok())
                && (date - d).num_days() > MEMBER_KEEP_DAYS
            {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    Ok(p)
}

/// The 19 trading sessions before `date`, oldest first (HI20's prior columns).
fn prior_sessions(date: NaiveDate) -> Vec<NaiveDate> {
    let mut v = Vec::with_capacity(logic::HI20_COLUMNS - 1);
    let mut d = date;
    for _ in 0..logic::HI20_COLUMNS - 1 {
        d = previous_trading_day(d);
        v.push(d);
    }
    v.reverse();
    v
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
            if let (Some(h), Some(l), c) = (bar.h, bar.l, bar.c) {
                bars.entry(s.clone()).or_default().push(Daily {
                    day: day_num(*d),
                    h,
                    l,
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
            tracing::warn!(error = %e, "rw2: corporate-actions ledger unreadable; C1 / HI20 are raw")
        }
    }
    for v in splits.values_mut() {
        v.sort_by_key(|x| x.0);
        v.dedup_by(|a, b| a.0 == b.0);
    }
    let prev = previous_trading_day(date);
    let newest = tail.last().copied();
    let assets = archive.read_assets().ok().flatten().unwrap_or_default();
    let types: HashMap<String, SecType> = assets
        .iter()
        .map(|(s, a)| (s.clone(), sec_type(s, Some(&a.name))))
        .collect();
    let type_of = |s: &str| types.get(s).copied().unwrap_or_else(|| sec_type(s, None));
    // HI20's prior sessions and their presence files.
    let prior = prior_sessions(date);
    let members: HashMap<i32, HashSet<String>> = prior
        .iter()
        .filter_map(|d| read_member(archive.root(), *d).map(|m| (day_num(*d), m)))
        .collect();
    let prior_n: Vec<i32> = prior.iter().map(|d| day_num(*d)).collect();
    // The ToD baselines (x-gap-1's files, the 20 sessions before the day).
    let sessions = &dates[dates.len().saturating_sub(20)..];
    let mut found = 0;
    let tod: Vec<TodDay> = sessions
        .iter()
        .map(|d| match crate::gap_ctx::read_tod(archive.root(), *d) {
            Some(t) => {
                found += 1;
                t
            }
            None => TodDay::default(),
        })
        .collect();
    let today = day_num(date);
    let mut c1s: HashMap<&str, f64> = HashMap::new();
    for (s, v) in &bars {
        if type_of(s) == SecType::Excluded {
            continue;
        }
        let k = v.partition_point(|b| b.day < today);
        if let Some(last) = v[..k].last() {
            let sp = splits.get(s).map(Vec::as_slice).unwrap_or(&[]);
            let c1 = last.c * logic::factor(sp, last.day, today);
            if c1.is_finite() && c1 > 0.0 {
                c1s.insert(s.as_str(), c1);
            }
        }
    }
    let mut bases = crate::gap_ctx::logic::baselines(&tod, c1s.keys().copied());
    let mut syms = HashMap::new();
    let mut level_eligible = 0;
    for (s, c1) in c1s {
        let v = &bars[s];
        let sp = splits.get(s).map(Vec::as_slice).unwrap_or(&[]);
        let hi20 = logic::hi20_of(v, sp, &prior_n, today, |j| match members.get(&j) {
            Some(m) => m.contains(s),
            None => logic::member(v, j),
        });
        level_eligible += hi20.is_some() as usize;
        let b = bases.remove(s).unwrap_or_default();
        syms.insert(
            s.to_owned(),
            SymDay {
                c1: Some(c1),
                hi20_u: hi20.map(|h| h * 1e6),
                base: (0..logic::NBUCKETS as i64)
                    .map(|k| b.of_bucket(k))
                    .collect(),
            },
        );
    }
    Table {
        date: Some(date),
        syms,
        types,
        raw_dates: read,
        newest_raw: newest,
        stale: newest != Some(prev),
        tod_sessions: found,
        member_files: members.len(),
        level_eligible,
    }
}

/// One fire's facts (the notice's [`NoticeDetail::Rw2`], its text and its ledger fields).
#[derive(Clone, Debug, PartialEq)]
pub struct Rw2Notice {
    /// `rw2-vwapr-1` or `rw2-level-1` ([`DETECTORS`]).
    pub detector: &'static str,
    pub version: &'static str,
    /// vwapr: the segment high over C1. level: the close over C1.
    pub gain: f64,
    /// Dollars since 04:00 at the deciding bar, the trigger bar's last 5 minutes and their ratio
    /// to the usual (None = a zero baseline: never traded in that bucket).
    pub cum: f64,
    pub trail: f64,
    pub rvol: Option<f64>,
    /// vwapr: the VWAP reclaimed (dollars). level: HI20 (dollars).
    pub level: f64,
    pub c1: Option<f64>,
    /// vwapr: how far the flush's low went under VWAP (fraction).
    pub flush_depth: Option<f64>,
    /// When the reclaim / the cross bar ended (epoch ns).
    pub trigger_ns: i64,
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
    match logic::session_part(t, t0) {
        0 => "pre",
        1 => "regular",
        _ => "after",
    }
}

fn decision_ns(d: Day, slot: i64) -> i64 {
    d.t0 + (slot + 1) * logic::SLOT + logic::DECIDE_NS
}

/// The last slot whose decision instant is at or before `now` (−1: none yet).
fn done_slot(d: Day, now: i64) -> i64 {
    let r = now - d.t0 - logic::DECIDE_NS;
    if r < logic::SLOT {
        -1
    } else {
        r / logic::SLOT - 1
    }
}

/// A symbol's live state: the bars not decided yet and the detectors' running state.
#[derive(Debug)]
struct SymLive {
    open: Vec<BarAcc>,
    /// The last slot decided (−1: none).
    decided: i64,
    /// The decision instant this symbol is queued for in `due` (`i64::MAX`: none).
    queued: i64,
    state: logic::Sym,
    /// A decided bar closed in $0.10–$25 today (the presence file).
    kept: bool,
    /// The last decided bars' (slot, close print's execution ns, its arrival ns): a vwapr fire
    /// is at the close of its hold's last PRINTED bar, which may be a few slots back.
    recent: std::collections::VecDeque<(i64, i64, i64)>,
}

#[derive(Clone, Copy, Debug, Default)]
struct Counts {
    bars: u64,
    vwapr_fires: usize,
    level_fires: usize,
    notified: usize,
    late_prints: u64,
    /// Fires decided over `gap_ctx::STALE_FIRE_NS` after their instant (a late table): not sent.
    stale: usize,
}

/// The detectors' live state (module docs). Fed under the core lock with the Runners board:
/// O(1) per print but a rare late one, and per tick only the symbols with a decision due.
#[derive(Default)]
pub struct Rw2Live {
    day: Option<Day>,
    started_at: Option<i64>,
    table: Option<Arc<Table>>,
    table_wanted: Option<NaiveDate>,
    table_asked: Option<NaiveDate>,
    table_late: bool,
    /// The first decision instant that waited for the asked-for table
    /// (`crate::gap_ctx::INPUTS_WAIT_NS`).
    wait_since: Option<i64>,
    readiness: crate::readiness::Tracker,
    syms: HashMap<Arc<str>, SymLive>,
    due: BinaryHeap<Reverse<(i64, Arc<str>)>>,
    member_cut: bool,
    member_out: Option<(NaiveDate, Value)>,
    counts: Counts,
    fired: Vec<Fired>,
    notices: Vec<Notice>,
}

impl Rw2Live {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether today was watched from 04:00 with its facts in time, fresh, and enough ToD.
    pub fn complete(&self) -> bool {
        let Some(d) = self.day else { return false };
        let watched = self.started_at.is_some_and(|s| s <= d.t0 + START_GRACE_NS);
        let table_ok = self.table.as_ref().is_some_and(|t| {
            t.date == Some(d.date) && !t.stale && t.tod_sessions >= crate::gap_ctx::TOD_MIN_SESSIONS
        });
        watched && !self.table_late && table_ok
    }

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
            if !self.member_cut {
                self.member_cut = true;
                self.cut_member(prev);
            }
            let c = self.counts;
            tracing::info!(
                day = %prev.date,
                bars = c.bars,
                vwapr_fires = c.vwapr_fires,
                level_fires = c.level_fires,
                notified = c.notified,
                complete = self.complete(),
                late_prints = c.late_prints,
                "rw2: day summary"
            );
        }
        self.day = Some(d);
        self.syms.clear();
        self.due.clear();
        self.member_cut = false;
        self.counts = Counts::default();
        self.fired.clear();
        self.table_late = false;
        self.wait_since = None;
        if self.table.as_ref().is_some_and(|t| t.date != Some(date)) {
            self.table = None;
        }
        true
    }

    /// Whether the decisions due by `now` wait for the day's table: asked for, not landed, and
    /// the first decision that waited is less than `gap_ctx::INPUTS_WAIT_NS` old. The bars and
    /// holds stay open and are decided in order once it lands (`logic::Sym::push` advances the
    /// holds itself, so a batch decides exactly what one-by-one would).
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
        now - since < crate::gap_ctx::INPUTS_WAIT_NS
    }

    /// Whether today's fires are sent, and if not why ([`crate::readiness`]).
    pub fn readiness(&self) -> crate::readiness::Readiness {
        use crate::gap_ctx::{TOD_MIN_SESSIONS, TOD_SESSIONS};
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
                "facts, HI20 and ToD baselines not loaded yet (read from 03:30 ET)",
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
                        kept: false,
                        recent: std::collections::VecDeque::new(),
                    },
                );
                k
            }
        };
        let sym = self.syms.get_mut(&key).expect("inserted");
        if k <= sym.decided {
            // Its bar was already decided: the dollars and shares still count for later bars
            // (VWAP, the running sums); that bar's prices stay as decided.
            self.counts.late_prints += 1;
            if p.vol {
                let mut b = BarAcc::new(k);
                b.add(&p, arrival);
                let bar = b.bar();
                sym.state.add_late(k, bar.dollars, bar.vol);
            }
            return;
        }
        let i = sym.open.partition_point(|b| b.slot < k);
        if sym.open.get(i).is_none_or(|b| b.slot != k) {
            sym.open.insert(i, BarAcc::new(k));
            let t = decision_ns(d, k);
            if t < sym.queued {
                sym.queued = t;
                self.due.push(Reverse((t, key.clone())));
            }
        }
        sym.open[i].add(&p, arrival);
    }

    /// The clock reached `now`: every decision whose instant has come is made; the day's table
    /// is asked for when due; the presence file is cut when the trading date moves on (20:00).
    pub fn tick(&mut self, now: i64) {
        self.see(now);
        let date = trading_date(UnixNanos(now));
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
        self.readiness.note("rw2", "rw2-vwapr-1,rw2-level-1", &r);
    }

    /// Every decision of day `d` due by `now`, unless the day's table is still on its way
    /// ([`Self::waiting`]).
    fn decide_due(&mut self, now: i64, d: Day) {
        if self.waiting(now, d) {
            return;
        }
        while let Some(Reverse((t, _))) = self.due.peek() {
            if *t > now {
                break;
            }
            let Reverse((t, key)) = self.due.pop().expect("peeked");
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
            tracing::warn!(day = %d.date, "rw2: a bar was due before the day's facts were loaded; nothing is sent today");
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
            if decision_ns(d, b.slot) > now {
                break;
            }
            sym.open.remove(0);
            sym.decided = b.slot;
            self.counts.bars += 1;
            sym.kept |= b.c > 0 && logic::in_band(b.c);
            if b.c > 0 {
                sym.recent.push_back((b.slot, b.c_ns, b.c_arrival));
                if sym.recent.len() > 8 {
                    sym.recent.pop_front();
                }
            }
            let (v, l) = sym.state.push(&b.bar(), sd, d.t0);
            fires.extend(v.into_iter().map(|f| (logic::VWAPR_ID, f)));
            fires.extend(l.map(|f| (logic::LEVEL_ID, f)));
        }
        // Holds whose end has passed without a bar at their last slot.
        let held = sym.state.advance(done_slot(d, now), sd, d.t0);
        fires.extend(held.into_iter().map(|f| (logic::VWAPR_ID, f)));
        let fires: Vec<_> = fires
            .into_iter()
            .map(|(det, f)| {
                let (c_ns, arr) = sym
                    .recent
                    .iter()
                    .find(|r| r.0 == f.close_slot)
                    .map_or((0, 0), |r| (r.1, r.2));
                (det, f, c_ns, arr)
            })
            .collect();
        let next = [
            sym.open.first().map(|b| decision_ns(d, b.slot)),
            sym.state.next_due_slot().map(|s| decision_ns(d, s)),
        ]
        .into_iter()
        .flatten()
        .min();
        if let Some(t) = next
            && t < sym.queued
        {
            sym.queued = t;
            self.due.push(Reverse((t, key.clone())));
        }
        for (det, f, c_ns, arr) in fires {
            self.fire(key, det, &f, c_ns, arr, now, d, complete, sd);
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
        sd: &SymDay,
    ) {
        let vwapr = det == logic::VWAPR_ID;
        if vwapr {
            self.counts.vwapr_fires += 1;
        } else {
            self.counts.level_fires += 1;
        }
        let detail = Rw2Notice {
            detector: det,
            version: if vwapr {
                logic::VWAPR_VERSION
            } else {
                logic::LEVEL_VERSION
            },
            gain: f.gain,
            cum: f.cum,
            trail: f.trail,
            rvol: f.rvol.is_finite().then_some(f.rvol),
            level: f.level / 1e6,
            c1: sd.c1,
            flush_depth: f.flush_depth.is_finite().then_some(f.flush_depth),
            trigger_ns: decision_ns(d, f.trigger_slot) - logic::DECIDE_NS,
            session: session_of(f.t, d.t0),
            arrival_ns: (arrival > 0).then_some(arrival),
            emitted_ns: now,
        };
        let price = f.close_u as f64 / 1e6;
        let stale = now - f.t > crate::gap_ctx::STALE_FIRE_NS;
        tracing::info!(
            symbol = %key, detector = det, t_ns = f.t, emitted_ns = now, price, gain = f.gain,
            cum = f.cum, rvol = f.rvol, complete, stale, "rw2: fire"
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
            let (kind, tag) = if vwapr {
                // Once per VWAP segment (pre / regular), so the segment keeps it unique.
                (
                    NoticeKind::VwapReclaim,
                    if f.trigger_slot >= logic::OPEN_SLOT {
                        "vwapr-r"
                    } else {
                        "vwapr-p"
                    },
                )
            } else {
                (NoticeKind::LevelBreak, "level")
            };
            self.notices.push(Notice {
                spread_bp: None,
                no_quote: false,
                kind,
                alert_id: format!("{}-{key}-{tag}", d.date),
                symbol: key.to_string(),
                at_ns: f.t,
                price,
                from_flag_pct: None,
                reasons: Vec::new(),
                since_start_min: None,
                exec_ns: (close_ns > 0).then_some(close_ns),
                detail: Some(NoticeDetail::Rw2(detail)),
            });
        }
    }

    /// 20:00 ET: the session's presence file (the symbols that closed a 15 s bar in $0.10–$25,
    /// excluded types left out), when the day was watched whole.
    fn cut_member(&mut self, d: Day) {
        for s in self.syms.values_mut() {
            for b in s.open.drain(..) {
                s.kept |= b.c > 0 && logic::in_band(b.c);
            }
        }
        if !self.watched_whole() {
            tracing::warn!(day = %d.date, "rw2: day not watched whole; no presence file for it");
            return;
        }
        let table = self.table.clone();
        let mut kept: Vec<&str> = self
            .syms
            .iter()
            .filter(|(k, s)| {
                s.kept
                    && match table.as_deref() {
                        Some(t) => t.sec_type(k),
                        None => sec_type(k, None),
                    } != SecType::Excluded
            })
            .map(|(k, _)| &**k)
            .collect();
        kept.sort_unstable();
        self.member_out = Some((
            d.date,
            json!({ "v": 1, "date": d.date.to_string(), "kept": kept }),
        ));
    }

    /// The finished session's presence file for the market to write (once, after 20:00).
    pub fn take_member(&mut self) -> Option<(NaiveDate, Value)> {
        self.member_out.take()
    }

    /// The date whose table the market should load now, once ([`load_table`]).
    pub fn take_table_request(&mut self) -> Option<NaiveDate> {
        self.table_wanted.take()
    }

    pub fn set_table(&mut self, t: Table) {
        tracing::info!(
            date = ?t.date,
            symbols = t.syms.len(),
            level_eligible = t.level_eligible,
            raw_dates = t.raw_dates,
            newest_raw = ?t.newest_raw,
            stale = t.stale,
            tod_sessions = t.tod_sessions,
            member_files = t.member_files,
            "rw2: facts and baselines loaded"
        );
        if t.stale {
            tracing::warn!(date = ?t.date, newest_raw = ?t.newest_raw, "rw2: the newest daily bar is not the previous trading day; nothing is sent today");
        }
        if t.tod_sessions < crate::gap_ctx::TOD_MIN_SESSIONS {
            tracing::warn!(date = ?t.date, tod_sessions = t.tod_sessions, "rw2: too few ToD sessions; nothing is sent today");
        }
        self.table = Some(Arc::new(t));
    }

    pub fn take_notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.notices)
    }

    pub fn fired(&self) -> &[Fired] {
        &self.fired
    }

    /// The cost line's `rw2` entry.
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
                "level_eligible": t.level_eligible, "raw_dates": t.raw_dates,
                "newest_raw": t.newest_raw.map(|d| d.to_string()), "stale": t.stale,
                "tod_sessions": t.tod_sessions, "member_files": t.member_files,
            })),
            "bars": c.bars,
            "vwapr_fires": c.vwapr_fires,
            "level_fires": c.level_fires,
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
        NoticeKind::VwapReclaim => Some(logic::VWAPR_ID),
        NoticeKind::LevelBreak => Some(logic::LEVEL_ID),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
