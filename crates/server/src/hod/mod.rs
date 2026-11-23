//! **HOD Momentum scanner** (`hod-1`, owner 2026-11-22): every time a stock in the app's price
//! band prints a new high of day (the session high since 04:00 ET, pre-market included), it is
//! checked against a fixed set of categories (low / medium float × high / medium rel vol × price,
//! a former momentum stock, two squeeze speeds, a 52-week breakout) and, when any matches, the
//! scanner adds a row tagged with every matching category. Our own categories and thresholds, all
//! in `policy-hod-v1.json` ([`logic::Policy`]); the clients show the policy beside the rows.
//!
//! # What it is not
//!
//! Not an alert. It never sends a notice, a push or a desktop notification, and nothing any
//! existing detector decides changes: it reads the same prints as the Runners board (each with
//! the chart engine's verdict) into its own state, and its rows only reach the HOD panel
//! (`hod.subscribe` → `hod.snapshot` / `hod.update`) and its own ledger.
//!
//! # Switches
//!
//! - `HOD_SCANNER=0` (env, a restart) removes it.
//! - The synced `detectors` settings entry `"hod-1"` (`{"hod-1": {"on": false}}`, Settings →
//!   Runners on every client) stops new rows at once for everybody, with no restart ([`follow`]).
//!   Off, new highs are counted but not evaluated; the rows already shown stay.
//! - Which categories a person sees is a per-user client filter (synced settings `hodFilter`);
//!   the server tags every row with every category it matches.
//!
//! # Inputs
//!
//! - **Prints**: OHLC-counted prints make the high, the minute lows and closes; volume-counted
//!   prints make the volume (as the store's bars count them), 04:00 ET to the session end.
//! - **Float**: the stock-context engine's latest public float (`STOCK_CONTEXT=1`; FMP), as
//!   x-float-1. Without it no float category matches (the squeezes still can).
//! - **Previous close**: the quote board's, split-guarded (what the app's "% up" reads), else the
//!   daily bars' close restated by the splits ledger.
//! - **Rel vol, daily rate**: shares since 04:00 over the mean daily (SIP) volume of the 30 prior
//!   sessions, restated onto today's share basis: the stock-context engine's RVOL definition
//!   (`stock_context::evaluate::rvol`, `rvol_daily_sessions` 30), computed here from the same
//!   daily-bar files (`daily-bars/raw/`) for every symbol, not only those the engine watches. The
//!   engine also checks each session is a full comparable day and its split coverage; this does
//!   not (a stated difference).
//! - **Rel vol 5 min**: the last 5 minutes' shares over rw-mp5-1's 20-session time-of-day baseline
//!   for the current 5-minute bucket ([`crate::mp5_gate::Mp5Live::tod_base`]); empty while that
//!   table is not loaded (it needs the Detection Panel's inputs).
//! - **Former momo**: the biggest one-session run (high over the previous close) in the last 90
//!   sessions' daily bars. The VPS keeps `DAILY_BARS_KEEP_DAYS` (63) dates, so live sees ~62
//!   sessions until that is raised to >= 92 (the same owner step rw-mp5-1 documents).
//! - **52-week breakout** needs 252 daily bars: unavailable on the VPS's 63 days, so the category
//!   is off by default and the snapshot says `hi52_available: false` ("needs history").
//! - **Security types** from `assets.json` (universe-v1): an excluded type never gets a row.
//!
//! # A restart mid-day
//!
//! The earlier prints are not replayed into the scanner, so after a restart the high of day and
//! the volume start from the restart: rows can be a new high only since then. The snapshot carries
//! `complete: false` and `since_ns`, and the panel says so. A deploy before 04:00 ET is whole.
//!
//! # Ledger
//!
//! One JSON line per row (`ARCHIVE_ROOT/hod/<date>.jsonl`): the detector id, policy version and
//! hash, the facts every tag was decided on, whether the day was watched whole, and when.
//!
//! # Calibration (advisory)
//!
//! Train only (2021-10-29..2024-06-28, 669 days, the frozen good-v1 label), 12 declared configs:
//! medium rel vol from 3x (start 2x) and a former-momo run of +30 % (start +50 %) caught the most
//! GOOD symbol-days at a GOOD share at least the start's (17.9 % vs 17.3 %), ~185 rows a day for
//! the default categories. `outputs/stockwatcher-research-v2/55-hod/HOD-CALIBRATION.md`.

pub mod logic;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{Datelike, NaiveDate};
use market_events::session::{SessionKind, trading_date};
use market_events::{Trade, UnixNanos};
use serde_json::{Value, json};

use crate::mp5_gate::{Day, day_of};
use crate::n1_live::logic::{SecType, sec_type};
use crate::quotes::Counted;
pub use logic::{Facts, Inputs, Policy, Row};

/// The welcome feature a client checks before sending `hod.subscribe` (an older server would
/// answer it with an error).
pub const FEATURE: &str = "hod.v1";
/// Rows kept for the day (newest first); the oldest drop off.
pub const MAX_ROWS: usize = 500;
/// Raw daily-bar dates read at most (the 52-week high's 252 and room).
pub const RAW_LOOKBACK: usize = 260;
/// When the day's facts are read: 30 minutes before 04:00 ET (after the nightly import).
pub const FACTS_LEAD_NS: i64 = 30 * logic::MIN;
/// The day is watched whole when this process saw its first instant by 04:01 ET.
const START_GRACE_NS: i64 = logic::MIN;
/// Where the ledger lives, under `ARCHIVE_ROOT`.
pub const LEDGER_DIR: &str = "hod";

/// The day's facts per symbol and what was read.
#[derive(Clone, Debug, Default)]
pub struct Table {
    pub date: Option<NaiveDate>,
    pub facts: HashMap<String, Facts>,
    pub excluded: HashSet<String>,
    pub raw_dates: usize,
    pub newest_raw: Option<NaiveDate>,
    /// Symbols with a 52-week high (enough daily bars).
    pub hi52: usize,
}

fn day_num(d: NaiveDate) -> i32 {
    d.num_days_from_ce()
}

/// The day's [`Table`] from the archive's raw daily bars, splits ledger and assets. Off the
/// ingest path (a blocking thread).
pub fn load_table(archive: &market_archive::Archive, policy: &Policy, date: NaiveDate) -> Table {
    let dates: Vec<NaiveDate> = archive
        .raw_bar_dates()
        .unwrap_or_default()
        .into_iter()
        .filter(|d| *d < date)
        .collect();
    let tail = &dates[dates.len().saturating_sub(RAW_LOOKBACK)..];
    let mut bars: HashMap<String, Vec<logic::DailyBar>> = HashMap::new();
    let mut read = 0;
    for d in tail {
        let Some(b) = archive.read_raw_bars(*d).ok().flatten() else {
            continue;
        };
        read += 1;
        for (s, bar) in &b.bars {
            bars.entry(s.clone()).or_default().push(logic::DailyBar {
                d: day_num(*d),
                h: bar.h,
                c: bar.c,
                v: bar.v,
            });
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
            tracing::warn!(error = %e, "hod: corporate-actions ledger unreadable; daily facts are raw")
        }
    }
    for v in splits.values_mut() {
        v.sort_by_key(|x| x.0);
        v.dedup_by(|a, b| a.0 == b.0);
    }
    let assets = archive.read_assets().ok().flatten().unwrap_or_default();
    let excluded: HashSet<String> = assets
        .iter()
        .filter(|(s, a)| sec_type(s, Some(&a.name)) == SecType::Excluded)
        .map(|(s, _)| s.clone())
        .collect();
    let day = day_num(date);
    let none: Vec<(i32, f64)> = Vec::new();
    let facts: HashMap<String, Facts> = bars
        .iter()
        .map(|(s, b)| {
            let sp = splits.get(s).unwrap_or(&none);
            (s.clone(), logic::facts_of(policy, b, sp, day))
        })
        .collect();
    let hi52 = facts.values().filter(|f| f.hi52.is_some()).count();
    Table {
        date: Some(date),
        facts,
        excluded,
        raw_dates: read,
        newest_raw: tail.last().copied(),
        hi52,
    }
}

/// The scanner board: lives in the market core, fed under its lock.
#[derive(Debug)]
pub struct HodScanner {
    on: bool,
    /// The `detectors."hod-1"` runtime switch.
    runtime_on: bool,
    policy: Arc<Policy>,
    policy_sha: String,
    day: Option<Day>,
    started_at: Option<i64>,
    syms: HashMap<String, logic::Sym>,
    pending: Vec<String>,
    table: Option<Arc<Table>>,
    table_asked: Option<NaiveDate>,
    table_wanted: Option<NaiveDate>,
    rows: VecDeque<Row>,
    next_id: u64,
    /// Rows added since the day began (`rows` may hold fewer).
    generation: u64,
    published: u64,
    ledger: Vec<(NaiveDate, String)>,
    /// New highs seen / evaluated / rows / skipped by the switch, today.
    counts: [u64; 4],
}

impl Default for HodScanner {
    fn default() -> Self {
        Self::new()
    }
}

fn sha256_hex(s: &str) -> String {
    use sha2::Digest as _;
    let d = sha2::Sha256::digest(s.as_bytes());
    d.iter().map(|b| format!("{b:02x}")).collect()
}

impl HodScanner {
    pub fn new() -> Self {
        HodScanner {
            on: false,
            runtime_on: true,
            policy: Arc::new(Policy::shipped()),
            policy_sha: sha256_hex(logic::POLICY_JSON),
            day: None,
            started_at: None,
            syms: HashMap::new(),
            pending: Vec::new(),
            table: None,
            table_asked: None,
            table_wanted: None,
            rows: VecDeque::new(),
            next_id: 1,
            generation: 0,
            published: 0,
            ledger: Vec::new(),
            counts: [0; 4],
        }
    }

    /// Turns it on (live market with `HOD_SCANNER`, the default; a replay never does).
    pub fn enable(&mut self) {
        self.on = true;
    }

    pub fn enabled(&self) -> bool {
        self.on
    }

    pub fn set_runtime_on(&mut self, on: bool) {
        self.runtime_on = on;
    }

    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    fn see(&mut self, ns: i64) {
        self.started_at.get_or_insert(ns);
    }

    /// The day was watched from 04:00 (+1 min grace).
    pub fn complete(&self) -> bool {
        match (self.day, self.started_at) {
            (Some(d), Some(s)) => s <= d.t0 + START_GRACE_NS,
            _ => false,
        }
    }

    /// Moves to `date`: a new day clears every symbol and row. False when no day can be had.
    fn roll(&mut self, date: NaiveDate) -> bool {
        if self.day.is_some_and(|d| d.date == date) {
            return true;
        }
        if self.day.is_some_and(|d| d.date > date) {
            return false;
        }
        let Some(d) = day_of(date) else {
            return false;
        };
        if let Some(old) = self.day {
            tracing::info!(
                date = %old.date,
                highs = self.counts[0], evaluated = self.counts[1], rows = self.counts[2], switched_off = self.counts[3],
                complete = self.complete(),
                "hod: day summary"
            );
        }
        self.day = Some(d);
        self.syms.clear();
        self.pending.clear();
        self.rows.clear();
        self.generation = 0;
        self.published = 0;
        self.counts = [0; 4];
        true
    }

    /// One print with the chart engine's verdict. O(1): one map lookup and a ring slot.
    pub fn apply_trade(&mut self, t: &Trade, counted: Counted) {
        if !self.on {
            return;
        }
        let ns = t.event_ts.0;
        self.see(ns);
        let Some(s) = t.session else { return };
        if s.kind == SessionKind::Overnight || (!counted.ohlc && !counted.volume) {
            return;
        }
        if !self.roll(s.date) {
            return;
        }
        let d = self.day.expect("rolled");
        if ns < d.t0 || ns >= d.end {
            return;
        }
        let px = t.price.micros() as f64 / 1e6;
        let cooldown = self.policy.cooldown_secs * logic::SEC;
        let regular = s.kind == SessionKind::Regular;
        let sym = match self.syms.get_mut(t.symbol.as_str()) {
            Some(x) => x,
            None => self.syms.entry(t.symbol.clone()).or_default(),
        };
        if sym.apply(
            d.t0,
            ns,
            px,
            t.size,
            counted.ohlc,
            counted.volume,
            regular,
            cooldown,
        ) {
            self.counts[0] += 1;
            self.pending.push(t.symbol.clone());
        }
    }

    /// The rocket task's tick: asks for the day's facts when due, then decides every pending new
    /// high against `inputs(symbol)` (read only for those).
    pub fn tick(&mut self, now: i64, inputs: impl Fn(&str) -> Inputs) {
        if !self.on {
            return;
        }
        self.see(now);
        if !self.roll(trading_date(UnixNanos(now))) {
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
        if self.pending.is_empty() {
            return;
        }
        let table = self.table.clone().filter(|t| t.date == Some(d.date));
        let complete = self.complete();
        let empty = Facts::default();
        for name in std::mem::take(&mut self.pending) {
            let Some(s) = self.syms.get_mut(&name) else {
                continue;
            };
            let Some((ns, px)) = s.pending.take() else {
                continue;
            };
            if !self.runtime_on {
                self.counts[3] += 1;
                continue;
            }
            if table.as_ref().is_some_and(|t| t.excluded.contains(&name)) {
                continue;
            }
            self.counts[1] += 1;
            let facts = table
                .as_ref()
                .and_then(|t| t.facts.get(&name))
                .unwrap_or(&empty);
            let inp = inputs(&name);
            let Some(mut row) = logic::evaluate(&self.policy, &name, s, d.t0, ns, px, facts, &inp)
            else {
                continue;
            };
            s.last_row = Some(ns);
            row.id = self.next_id;
            self.next_id += 1;
            self.counts[2] += 1;
            self.generation += 1;
            self.ledger.push((
                d.date,
                ledger_line(
                    &row,
                    &self.policy,
                    &self.policy_sha,
                    d.date,
                    complete,
                    now,
                    facts,
                ),
            ));
            self.rows.push_front(row);
            if self.rows.len() > MAX_ROWS {
                self.rows.pop_back();
            }
        }
    }

    pub fn take_table_request(&mut self) -> Option<NaiveDate> {
        self.table_wanted.take()
    }

    pub fn set_table(&mut self, t: Table) {
        tracing::info!(
            date = ?t.date, symbols = t.facts.len(), raw_dates = t.raw_dates, newest_raw = ?t.newest_raw,
            with_hi52 = t.hi52, excluded = t.excluded.len(),
            "hod: daily facts loaded"
        );
        self.table = Some(Arc::new(t));
    }

    pub fn take_ledger(&mut self) -> Vec<(NaiveDate, String)> {
        std::mem::take(&mut self.ledger)
    }

    fn hi52_available(&self) -> bool {
        self.table.as_ref().is_some_and(|t| t.hi52 > 0)
    }

    fn head(&self, kind: &str) -> serde_json::Map<String, Value> {
        let mut m = serde_json::Map::new();
        m.insert("type".into(), json!(kind));
        m.insert("enabled".into(), json!(self.on));
        m.insert("on".into(), json!(self.runtime_on));
        m.insert("date".into(), json!(self.day.map(|d| d.date.to_string())));
        m.insert("gen".into(), json!(self.generation));
        m.insert("complete".into(), json!(self.complete()));
        m.insert("since_ns".into(), json!(self.started_at));
        m.insert("hi52_available".into(), json!(self.hi52_available()));
        m.insert(
            "facts_loaded".into(),
            json!(
                self.table
                    .as_ref()
                    .is_some_and(|t| self.day.is_some_and(|d| t.date == Some(d.date)))
            ),
        );
        m
    }

    /// `hod.snapshot`: the policy and every row kept today.
    pub fn snapshot_frame(&self) -> String {
        let mut m = self.head("hod.snapshot");
        m.insert("policy".into(), policy_wire(&self.policy));
        m.insert(
            "rows".into(),
            Value::Array(self.rows.iter().map(row_wire).collect()),
        );
        Value::Object(m).to_string()
    }

    /// `hod.update` with the rows added since the last one (`prev_gen` → `gen`), or `None` when
    /// nothing was added. A client whose `gen` is not `prev_gen` (or whose date differs) asks for
    /// a snapshot.
    pub fn take_update(&mut self) -> Option<String> {
        if self.generation == self.published {
            return None;
        }
        let n = ((self.generation - self.published) as usize).min(self.rows.len());
        let mut m = self.head("hod.update");
        m.insert("prev_gen".into(), json!(self.published));
        m.insert(
            "rows".into(),
            Value::Array(self.rows.iter().take(n).map(row_wire).collect()),
        );
        self.published = self.generation;
        Some(Value::Object(m).to_string())
    }

    /// The cost line's entry.
    pub fn cost(&self) -> Value {
        json!({"syms": self.syms.len(), "rows": self.rows.len(), "counts": self.counts})
    }
}

fn round(x: f64, k: i32) -> f64 {
    let p = 10f64.powi(k);
    (x * p).round() / p
}

fn opt(x: Option<f64>, k: i32) -> Value {
    x.filter(|v| v.is_finite())
        .map_or(Value::Null, |v| json!(round(v, k)))
}

pub fn row_wire(r: &Row) -> Value {
    json!({
        "id": r.id,
        "t": r.ns / 1_000_000,
        "sym": r.sym,
        "price": round(r.price, 4),
        "vol": r.vol,
        "float": r.float,
        "rvol": opt(r.rvol, 2),
        "rvol5": opt(r.rvol5, 2),
        "gap": opt(r.gap, 4),
        "chg": round(r.chg, 4),
        "chg5": opt(r.chg5, 4),
        "chg10": opt(r.chg10, 4),
        "cats": r.cat_ids(),
    })
}

fn policy_wire(p: &Policy) -> Value {
    json!({
        "id": p.id,
        "version": p.version,
        "band": [p.band.min, p.band.max],
        "cooldown_secs": p.cooldown_secs,
        "float_low_max": p.float.low_max,
        "float_medium_max": p.float.medium_max,
        "rvol_high_min": p.rvol.high_min,
        "rvol_medium_min": p.rvol.medium_min,
        "rvol_sessions": p.rvol.sessions,
        "rvol_base_min": p.rvol.base_min,
        "price_high_min": p.price_high_min,
        "momo_run_min": p.momo.run_min,
        "momo_sessions": p.momo.sessions,
        "categories": p.categories.iter().map(|c| json!({
            "id": c.id, "name": c.name, "default_on": c.default_on, "needs_history": c.needs_history,
        })).collect::<Vec<_>>(),
    })
}

fn ledger_line(
    r: &Row,
    p: &Policy,
    sha: &str,
    date: NaiveDate,
    complete: bool,
    now: i64,
    f: &Facts,
) -> String {
    json!({
        "v": 1,
        "detector": logic::DETECTOR,
        "policy_version": p.version,
        "policy_sha256": sha,
        "date": date.to_string(),
        "id": r.id,
        "ns": r.ns,
        "decided_ns": now,
        "sym": r.sym,
        "price": r.price,
        "cats": r.cat_ids(),
        "vol": r.vol,
        "float": r.float,
        "float_source": "stock_context_public_float",
        "rvol": r.rvol,
        "vol_base": r.vol_base,
        "rvol5": r.rvol5,
        "gap": r.gap,
        "chg": r.chg,
        "chg5": r.chg5,
        "chg10": r.chg10,
        "c1": r.c1,
        "runmax": r.runmax,
        "hi52": f.hi52,
        "complete": complete,
    })
    .to_string()
}

/// `ARCHIVE_ROOT/hod/<date>.jsonl`.
pub fn ledger_path(root: &Path, date: NaiveDate) -> PathBuf {
    root.join(LEDGER_DIR).join(format!("{date}.jsonl"))
}

/// Appends ledger lines (a blocking call: run it off the lock).
pub fn append_ledger(root: &Path, lines: &[(NaiveDate, String)]) -> std::io::Result<()> {
    std::fs::create_dir_all(root.join(LEDGER_DIR))?;
    let mut i = 0;
    while i < lines.len() {
        let d = lines[i].0;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(ledger_path(root, d))?;
        let mut buf = String::new();
        while i < lines.len() && lines[i].0 == d {
            buf.push_str(&lines[i].1);
            buf.push('\n');
            i += 1;
        }
        f.write_all(buf.as_bytes())?;
    }
    Ok(())
}

/// Whether the `detectors` settings document has `hod-1` on (absent = on).
pub fn switch_on(v: Option<&Value>) -> bool {
    match v.and_then(|v| v.get(logic::DETECTOR)) {
        Some(Value::Bool(b)) => *b,
        Some(Value::Object(e)) => e.get("on").and_then(Value::as_bool).unwrap_or(true),
        _ => true,
    }
}

/// Follows the `detectors` settings entry `hod-1` into the live market (no restart).
pub fn follow(settings: crate::settings::Settings, market: crate::market::Market) {
    const NS: &str = crate::alerts::detectors::NS;
    tokio::spawn(async move {
        let mut rx = settings.subscribe();
        let apply = |v: Option<Value>| market.set_hod_runtime_on(switch_on(v.as_ref()));
        apply(settings.get(NS).await.map(|(_, v)| v));
        loop {
            match rx.recv().await {
                Ok(frame) => {
                    let ours = serde_json::from_str::<Value>(&frame)
                        .is_ok_and(|v| v["type"] == "settings.changed" && v["ns"] == NS);
                    if ours {
                        apply(settings.get(NS).await.map(|(_, v)| v));
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    apply(settings.get(NS).await.map(|(_, v)| v));
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });
}
