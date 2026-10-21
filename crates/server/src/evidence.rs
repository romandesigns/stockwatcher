//! Evidence for stock context (`STOCK_CONTEXT=1`): the stored records the trades alone cannot
//! give, read from the archive volume and handed to every context board ([`crate::context`]).
//!
//! # What, from where
//!
//! Everything comes from `ARCHIVE_ROOT`, the volume the other processes write; the server never
//! calls a provider:
//!
//! | Records | Written by | Read with |
//! |---|---|---|
//! | float observations | `swimport evidence` (`evidence/float/`) | `evidence_load::float_observations` |
//! | news items | `swimport evidence` (`evidence/news/`) | `evidence_load::news_items` |
//! | news coverage, market-wide | `swimport evidence` (`evidence/news-coverage/`) | `evidence_load::market_news_coverage` |
//! | previous close + daily baseline | `swimport maintain` (`daily-bars/`, `corporate-actions/`) | `baseline::load` |
//!
//! # When
//!
//! [`EvidenceLoader::refresh`] runs at startup and then every `CONTEXT_EVIDENCE_CHECK_SECS`
//! (15 s, [`DEFAULT_CHECK_SECS`] says why) on a blocking thread, never on the market path: trade
//! processing never waits for disk.
//! The append-only files are followed with [`EvidenceTail`] (only bytes appended since the last
//! call are parsed); baselines are rebuilt only when their inputs changed on disk (a raw date
//! file, the split snapshots, or a new announcement check for the session date: the maintainer
//! makes one every 30 minutes in the session). The new records go into the [`EvidenceLog`] and
//! from there to the board under the core lock, which is only in-memory work. A replay day switch
//! loads the new day synchronously before the rebuild (the same precedent as the archive
//! metadata), and a seek's new board is seeded from the log in memory.
//!
//! # Causality
//!
//! Every record keeps its own `observed_at` (a float's retrieval, a news version's first
//! sighting, a search's completion, the latest retrieval a baseline rests on). Nothing here
//! changes it: the engine joins evidence as of each decision's cutoff, so a record observed after
//! a decision can never reach it, whenever it was loaded. A record loaded later than it was
//! observed simply misses the live decisions in between (they are already made); a rebuild that
//! has it from the start may then decide those bars differently. That is the honest difference
//! between what was known and what was loaded, never a backdating.
//!
//! Baselines are computed **as of** a time, and nothing retrieved after it is used: live, the
//! wall clock at the refresh; a replay, the session date's first announcement check (03:30 ET,
//! or the day's start when there is none), so a replayed day sees the pre-session evidence a live
//! run would have had and not the later re-checks. Raw date files are refreshed every night,
//! so for an older replayed day the records usually carry an `observed_at` after that day and are
//! invisible to it: the rule working, not a gap.
//!
//! # Bounded memory
//!
//! Only the current session date is held: records observed from two days before its first
//! session on (a float is stale after a day, news after 24 h, and a coverage run must reach a
//! full freshness window back), up to the day's end for a replay. A replay day switch reads the
//! files again for the new window; live only trims as its date moves forward. Baselines are built only for symbols whose previous close
//! was at or below the band's top (above it a stock can only reach the band by falling, which
//! fails the gain pillar), and the log keeps only the latest previous close and baseline per id.
//! News coverage is one market-wide record per search, not one per symbol. The engine drops what
//! no later decision can read ([`stock_context::EvidenceStore::prune`]).

use crate::market::{Inner, Market};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use market_archive::baseline::{self, Inputs};
use market_archive::evidence_load::{
    float_observations, market_news_coverage, news_coverage, news_items, scoped_symbols,
};
use market_archive::{
    Archive, CoveragePolicy, EvidenceTail, FloatRecord, NewsCoverageRecord, NewsVersionRecord,
};
use market_events::session::{previous_trading_day, sessions_for, trading_date};
use market_events::{Price, UnixNanos};
use market_providers::ClockMode;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration as StdDuration, SystemTime};
use stock_context::{DailyBaseline, FloatObservation, NewsCoverage, NewsItem, PrevClose};

/// Default period of the evidence check (`CONTEXT_EVIDENCE_CHECK_SECS`), chosen from the news
/// pillar's arithmetic rather than for cost (a check that finds nothing is a few `stat`s). A
/// market search covers through its poll time minus the collector's 30 s settle, polls are 60 s
/// apart, and pillars-v2 lets a decision's cutoff run 120 s past `covered_through`. Just before a
/// record is delivered, the latest one the board has is up to `60 + 30 + check` seconds behind
/// the clock: with 30 s that is the whole 120 s and a stock without news would flicker between
/// FAIL and UNKNOWN; 15 s leaves a quarter-minute of margin.
pub const DEFAULT_CHECK_SECS: u64 = 15;

/// How far before the session date's first session records are kept.
const LOOKBACK: Duration = Duration::days(2);

/// The session date's evidence as delivered so far, each record stamped with a delivery
/// sequence so a board can take only what it has not got ([`EvidenceLog::since`]).
#[derive(Default)]
pub struct EvidenceLog {
    /// Identifies this log: a new session date starts a new one, and a board holding another
    /// generation's cursor takes everything again.
    generation: u64,
    day: Option<NaiveDate>,
    seq: u64,
    floats: Vec<(u64, FloatObservation)>,
    news: Vec<(u64, NewsItem)>,
    coverage: Vec<(u64, NewsCoverage)>,
    /// The latest per record id: every announcement check re-issues every symbol's records, and
    /// only the latest can win a join (the engine prunes the older ones too).
    closes: HashMap<String, (u64, PrevClose)>,
    baselines: HashMap<String, (u64, DailyBaseline)>,
}

/// Records a board has not got yet, cloned out of the log ([`EvidenceLog::since`]).
#[derive(Debug, Default)]
pub struct EvidenceBatch {
    pub generation: u64,
    /// The log's sequence this batch brings the board up to.
    pub through: u64,
    pub floats: Vec<FloatObservation>,
    pub news: Vec<NewsItem>,
    pub coverage: Vec<NewsCoverage>,
    pub closes: Vec<PrevClose>,
    pub baselines: Vec<DailyBaseline>,
}

impl EvidenceBatch {
    pub fn len(&self) -> usize {
        self.floats.len()
            + self.news.len()
            + self.coverage.len()
            + self.closes.len()
            + self.baselines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A board's place in the log: `(generation, sequence)`.
pub type Cursor = (u64, u64);

impl EvidenceLog {
    pub fn day(&self) -> Option<NaiveDate> {
        self.day
    }

    /// Records held (logs and tests).
    pub fn len(&self) -> usize {
        self.floats.len()
            + self.news.len()
            + self.coverage.len()
            + self.closes.len()
            + self.baselines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// What a board at `cursor` has not got; `None` when it is up to date. A cursor of another
    /// generation (or none) gets everything.
    pub fn since(&self, cursor: Option<Cursor>) -> Option<EvidenceBatch> {
        let after = match cursor {
            Some((g, s)) if g == self.generation => {
                if s >= self.seq {
                    return None;
                }
                s
            }
            _ => 0,
        };
        fn pick<'a, T: Clone + 'a>(v: impl Iterator<Item = &'a (u64, T)>, after: u64) -> Vec<T> {
            v.filter(|(s, _)| *s > after)
                .map(|(_, r)| r.clone())
                .collect()
        }
        Some(EvidenceBatch {
            generation: self.generation,
            through: self.seq,
            floats: pick(self.floats.iter(), after),
            news: pick(self.news.iter(), after),
            coverage: pick(self.coverage.iter(), after),
            closes: pick(self.closes.values(), after),
            baselines: pick(self.baselines.values(), after),
        })
    }

    fn next(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }
}

/// What one refresh found (one log line).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RefreshReport {
    pub day: Option<NaiveDate>,
    pub floats: usize,
    pub news: usize,
    pub coverage: usize,
    pub closes: usize,
    pub baselines: usize,
    /// Baselines were rebuilt this time (their inputs changed).
    pub rebuilt: bool,
    /// Symbols the rebuild looked at.
    pub universe: usize,
    pub errors: Vec<String>,
}

impl RefreshReport {
    pub fn new_records(&self) -> usize {
        self.floats + self.news + self.coverage + self.closes + self.baselines
    }
}

/// `(name, length, mtime)` of every file a baseline rebuild reads, for change detection.
type Stamp = Vec<(String, u64, Option<SystemTime>)>;

/// Reads the archive's evidence for one session date at a time (module docs).
pub struct EvidenceLoader {
    archive: Archive,
    /// The band's top: baselines are built for previous closes at or below it.
    band_max: Price,
    live: bool,
    tail: EvidenceTail,
    day: Option<NaiveDate>,
    /// `[from, through]` (UTC ns) of the records kept for `day`.
    window: (i64, i64),
    floats_raw: Vec<FloatRecord>,
    news_raw: Vec<NewsVersionRecord>,
    coverage_raw: Vec<NewsCoverageRecord>,
    baseline_stamp: Option<Stamp>,
    /// Content hashes of the records put in the log for `day`: a recomputation repeats them.
    delivered: HashSet<u64>,
    generations: u64,
}

fn ns(t: DateTime<Utc>) -> i64 {
    UnixNanos::from_utc(t).map_or(i64::MAX, |n| n.0)
}

/// `[first session start, last session end]` of `day` (UTC ns).
fn day_bounds(day: NaiveDate) -> (i64, i64) {
    let s = sessions_for(day);
    match (s.first(), s.last()) {
        (Some(a), Some(b)) => (a.1.nanos(), b.2.nanos()),
        // Not a trading day (callers pass trading dates): its UTC calendar day.
        _ => {
            let start = day.and_hms_opt(0, 0, 0).expect("valid").and_utc();
            (ns(start), ns(start + Duration::days(1)))
        }
    }
}

fn content_key<T: Serialize>(kind: &str, r: &T) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    kind.hash(&mut h);
    serde_json::to_vec(r)
        .expect("evidence serializes")
        .hash(&mut h);
    h.finish()
}

/// The records of `v` whose content was not delivered yet today (and marks them delivered).
fn keep_new<T: Serialize>(kind: &str, v: Vec<T>, delivered: &mut HashSet<u64>) -> Vec<T> {
    v.into_iter()
        .filter(|r| delivered.insert(content_key(kind, r)))
        .collect()
}

fn dir_stamp(dir: &std::path::Path, out: &mut Stamp) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.filter_map(|e| e.ok()) {
        let m = e.metadata().ok();
        out.push((
            e.path().to_string_lossy().into_owned(),
            m.as_ref().map_or(0, |m| m.len()),
            m.and_then(|m| m.modified().ok()),
        ));
    }
}

impl EvidenceLoader {
    pub fn new(archive: Archive, band_max: Price, live: bool) -> Self {
        EvidenceLoader {
            archive,
            band_max,
            live,
            tail: EvidenceTail::default(),
            day: None,
            window: (0, 0),
            floats_raw: Vec::new(),
            news_raw: Vec::new(),
            coverage_raw: Vec::new(),
            baseline_stamp: None,
            delivered: HashSet::new(),
            generations: 0,
        }
    }

    /// Reads what is new for session date `day` as of `now` and appends it to `log` (a new day
    /// replaces the log). Disk I/O and parsing happen with no lock held; `log` is locked only to
    /// append. Never fails: what could not be read is in the report and retried next time.
    pub fn refresh(
        &mut self,
        day: NaiveDate,
        now: DateTime<Utc>,
        log: &std::sync::Mutex<EvidenceLog>,
    ) -> RefreshReport {
        let mut rep = RefreshReport {
            day: Some(day),
            ..RefreshReport::default()
        };
        let new_day = self.day != Some(day);
        if new_day {
            let (start, end) = day_bounds(day);
            let from = ns(start_minus(start));
            if self.day.is_some() && (!self.live || from < self.window.0) {
                // A replay day switch: the records the new day needs were read under the old
                // window and dropped (the tail never hands a line out twice), so read the files
                // again from the new window's start. Live only ever moves forward and keeps
                // everything newer than its window's start, so it just trims.
                self.tail = EvidenceTail::default();
                self.floats_raw.clear();
                self.news_raw.clear();
                self.coverage_raw.clear();
            }
            self.window = (from, end);
            self.day = Some(day);
            self.floats_raw.retain(|r| r.retrieved_at >= from);
            self.news_raw.retain(|r| r.first_observed_at >= from);
            self.coverage_raw.retain(|r| r.observed_at >= from);
            self.baseline_stamp = None;
            self.delivered.clear();
            self.generations += 1;
            *log.lock().expect("evidence log lock") = EvidenceLog {
                generation: self.generations,
                day: Some(day),
                ..EvidenceLog::default()
            };
        }
        // A replay keeps nothing observed after its day (no decision of the day can read it);
        // live keeps it, because it is the next session date's evidence once the date turns.
        let (from, through) = self.window;
        let live = self.live;
        let inside = |t: i64| t >= from && (live || t <= through);

        // Append-only files: only what was appended since the last call.
        let f = self.tail.float(&self.archive, Some(from));
        let n = self.tail.news(&self.archive, Some(from));
        let c = self.tail.news_coverage(&self.archive, Some(from));
        rep.errors.extend(
            f.errors
                .iter()
                .chain(&n.errors)
                .chain(&c.errors)
                .map(|e| e.to_string()),
        );
        let (mut new_f, mut new_n, mut new_c) = (new_day, new_day, new_day);
        for r in f.records {
            if inside(r.retrieved_at) {
                self.floats_raw.push(r);
                new_f = true;
            }
        }
        for r in n.records {
            if inside(r.first_observed_at) {
                self.news_raw.push(r);
                new_n = true;
            }
        }
        for r in c.records {
            if inside(r.observed_at) {
                self.coverage_raw.push(r);
                new_c = true;
            }
        }
        // Versions and runs depend on the earlier records, so each kind is derived again from
        // the day's whole window and only the records not delivered yet go out.
        let floats = if new_f {
            float_observations(&self.floats_raw)
        } else {
            Vec::new()
        };
        let news = if new_n {
            news_items(&self.news_raw, None)
        } else {
            Vec::new()
        };
        let coverage = if new_c {
            let named: Vec<String> = scoped_symbols(&self.coverage_raw).into_iter().collect();
            let mut v = market_news_coverage(&self.coverage_raw, 0);
            v.extend(news_coverage(&self.coverage_raw, &named, 0));
            v
        } else {
            Vec::new()
        };
        let (closes, baselines) = self.baselines(day, now, &mut rep);

        let d = &mut self.delivered;
        let floats = keep_new("float", floats, d);
        let news = keep_new("news", news, d);
        let coverage = keep_new("coverage", coverage, d);
        let closes = keep_new("close", closes, d);
        let baselines = keep_new("baseline", baselines, d);
        rep.floats = floats.len();
        rep.news = news.len();
        rep.coverage = coverage.len();
        rep.closes = closes.len();
        rep.baselines = baselines.len();
        if rep.new_records() > 0 {
            let mut log = log.lock().expect("evidence log lock");
            for r in floats {
                let s = log.next();
                log.floats.push((s, r));
            }
            for r in news {
                let s = log.next();
                log.news.push((s, r));
            }
            for r in coverage {
                let s = log.next();
                log.coverage.push((s, r));
            }
            for r in closes {
                let s = log.next();
                log.closes.insert(r.id.clone(), (s, r));
            }
            for r in baselines {
                let s = log.next();
                log.baselines.insert(r.id.clone(), (s, r));
            }
        }
        rep
    }

    /// The previous closes and daily baselines of `day` when their inputs changed since the last
    /// build (module docs: as of when, and for which symbols).
    fn baselines(
        &mut self,
        day: NaiveDate,
        now: DateTime<Utc>,
        rep: &mut RefreshReport,
    ) -> (Vec<PrevClose>, Vec<DailyBaseline>) {
        let a = &self.archive;
        let mut stamp = Stamp::new();
        dir_stamp(&a.daily_bars_dir().join("raw"), &mut stamp);
        dir_stamp(&a.split_snapshots_dir(), &mut stamp);
        dir_stamp(&a.corporate_actions_dir(day), &mut stamp);
        stamp.sort();
        if self.baseline_stamp.as_ref() == Some(&stamp) {
            return (Vec::new(), Vec::new());
        }
        rep.rebuilt = true;
        let announcements = match a.read_action_retrievals(day) {
            Ok(v) => v,
            Err(e) => {
                rep.errors.push(format!("corporate actions: {e}"));
                Vec::new()
            }
        };
        let as_of = if self.live {
            now
        } else {
            // A replay: the session date's first announcement check, else its first session.
            let start = DateTime::from_timestamp_nanos(day_bounds(day).0);
            announcements
                .iter()
                .map(|r| r.retrieved_at)
                .filter(|t| *t >= start)
                .min()
                .unwrap_or(start)
                .min(now)
        };
        let p = previous_trading_day(day);
        let mut first = day;
        for _ in 0..baseline::DEFAULT_LOOKBACK {
            first = previous_trading_day(first);
        }
        let raw = match a.read_raw_bars_range(first, p) {
            Ok(r) => r,
            Err(e) => {
                rep.errors.push(format!("raw daily bars: {e}"));
                return (Vec::new(), Vec::new());
            }
        };
        let split = match a.read_split_snapshots_as_of(as_of) {
            Ok(s) => s,
            Err(e) => {
                rep.errors.push(format!("split snapshots: {e}"));
                Vec::new()
            }
        };
        let Some(prev) = raw.get(&p) else {
            // Not imported yet (the nightly import runs after 21:15 ET): try again when it lands.
            rep.errors.push(format!(
                "no raw daily bars for {p} yet: no baselines for {day}"
            ));
            self.baseline_stamp = Some(stamp);
            return (Vec::new(), Vec::new());
        };
        let top = self.band_max.micros();
        let universe: Vec<&String> = prev
            .bars
            .iter()
            .filter(|(_, b)| Price::from_f64_rounded(b.c).is_some_and(|c| c.micros() <= top))
            .map(|(s, _)| s)
            .collect();
        rep.universe = universe.len();
        let policy = CoveragePolicy::v1();
        let (mut closes, mut baselines) = (Vec::new(), Vec::new());
        for sym in universe {
            let inputs = Inputs {
                raw: &raw,
                split: Archive::latest_snapshot_for(&split, sym),
                announcements: &announcements,
                policy: &policy,
                as_of,
            };
            let l = baseline::load(&inputs, sym, day, baseline::DEFAULT_LOOKBACK);
            closes.extend(l.prev_close);
            baselines.extend(l.baseline);
        }
        self.baseline_stamp = Some(stamp);
        (closes, baselines)
    }
}

fn start_minus(start: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_nanos(start) - LOOKBACK
}

/// A market's evidence: the loader (disk state, locked for a whole refresh) and the log (locked
/// only to append or to copy a batch out). Never hold the log while taking the core lock.
pub struct Evidence {
    loader: Mutex<EvidenceLoader>,
    pub(crate) log: Mutex<EvidenceLog>,
}

impl Market {
    /// Sets up this market's evidence (once) and starts loading it: a replay synchronously, here,
    /// before its ingest starts; a live market in the background. Then every `every`.
    pub(crate) fn init_evidence(&self, archive: Archive, band_max: Price, every: StdDuration) {
        let live = self.mode() == ClockMode::Live;
        let ev = Evidence {
            loader: Mutex::new(EvidenceLoader::new(archive, band_max, live)),
            log: Mutex::new(EvidenceLog::default()),
        };
        if self.0.evidence.set(ev).is_err() {
            return;
        }
        if !live {
            self.refresh_evidence();
            if every.is_zero() {
                return;
            }
        }
        let weak: Weak<Inner> = Arc::downgrade(&self.0);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(if every.is_zero() {
                StdDuration::from_secs(3600)
            } else {
                every
            });
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // The first tick is immediate: a live market's first load (with a zero period, its
            // only one); a replay has just loaded and skips it.
            if !live {
                tick.tick().await;
            }
            loop {
                tick.tick().await;
                let Some(inner) = weak.upgrade() else { break };
                let m = Market(inner);
                // Disk and parsing: off the async workers, like the archive metadata check.
                let _ = tokio::task::spawn_blocking(move || m.refresh_evidence()).await;
                if every.is_zero() {
                    break;
                }
            }
        });
    }

    /// Loads what is new for `day` (default: the live trading date now, or the replay's day)
    /// without handing it to the board. Blocking (disk I/O): call from a blocking thread, at
    /// startup, or on a replay day switch. No-op without `STOCK_CONTEXT`.
    pub(crate) fn load_evidence(&self, day: Option<NaiveDate>) {
        let Some(ev) = self.0.evidence.get() else {
            return;
        };
        let now = Utc::now();
        let day = match (day, self.mode()) {
            (Some(d), _) => d,
            (None, ClockMode::Live) => {
                trading_date(UnixNanos::from_utc(now).unwrap_or(UnixNanos(0)))
            }
            (None, ClockMode::Replay) => match self.info().and_then(|i| i.day) {
                Some(d) => d,
                None => return,
            },
        };
        let started = std::time::Instant::now();
        let rep = ev
            .loader
            .lock()
            .expect("evidence loader lock")
            .refresh(day, now, &ev.log);
        for e in &rep.errors {
            tracing::warn!(day = %day, error = %e, "stock context evidence: could not read everything");
        }
        if rep.new_records() > 0 || rep.rebuilt {
            let held = ev.log.lock().expect("evidence log lock").len();
            tracing::info!(
                day = %day,
                floats = rep.floats,
                news = rep.news,
                coverage = rep.coverage,
                closes = rep.closes,
                baselines = rep.baselines,
                rebuilt = rep.rebuilt,
                universe = rep.universe,
                held,
                ms = started.elapsed().as_millis() as u64,
                "stock context evidence loaded"
            );
        }
    }

    /// [`Market::load_evidence`] for today, then hands the board what it has not got.
    pub(crate) fn refresh_evidence(&self) {
        self.load_evidence(None);
        self.deliver_evidence();
    }

    /// Hands the running board the log's records it has not got. The batch is copied out with no
    /// other lock held; the core lock is then taken only to add it (memory only). A board
    /// replaced in between (a seek) is left alone: it was seeded from the log when built.
    pub(crate) fn deliver_evidence(&self) {
        let Some(ev) = self.0.evidence.get() else {
            return;
        };
        let cursor = {
            let core = self.0.core.lock().expect("core lock");
            match core.context.as_ref() {
                Some(cx) => cx.evidence_cursor(),
                None => return,
            }
        };
        let Some(batch) = ev.log.lock().expect("evidence log lock").since(cursor) else {
            return;
        };
        let mut core = self.0.core.lock().expect("core lock");
        if let Some(cx) = core.context.as_mut()
            && cx.evidence_cursor() == cursor
        {
            let n = batch.len();
            cx.add_evidence(batch);
            tracing::debug!(
                records = n,
                held = cx.evidence_len(),
                "stock context evidence delivered"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ContextBoard;
    use crate::quotes::Counted;
    use market_archive::{
        ActionRetrieval, Adjustment, ArchiveConfig, BarFetch, ChunkOutcome, CoverageScope,
        DailyBar, PrevCloses, RawDayBars, SplitSnapshot,
    };
    use market_events::Trade;
    use market_events::session::{et, is_trading_day};
    use std::collections::BTreeMap;
    use stock_context::{MemberStatus, PillarId, Tri};

    const SEC: i64 = 1_000_000_000;
    const SYM: &str = "XYZ";
    /// Traded, no news: its news pillar can only FAIL through the market-wide coverage.
    const OTHER: &str = "QUIET";

    /// D = Friday 2026-10-16, P = 2026-10-15.
    fn dd() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 16).unwrap()
    }
    fn t(h: u32, m: u32, s: u32) -> i64 {
        et(dd(), h, m, s, 0).unwrap().nanos()
    }
    fn utc(ns: i64) -> DateTime<Utc> {
        DateTime::from_timestamp_nanos(ns)
    }

    fn archive(dir: &std::path::Path) -> Archive {
        Archive::new(ArchiveConfig {
            root: dir.join("archive"),
            retention_days: 0,
            min_free_bytes: 0,
        })
    }

    /// 22:00 ET the night of P: when the nightly import has the raw bars and the split set.
    fn night_of_p() -> DateTime<Utc> {
        utc(et(dd() - Duration::days(1), 22, 0, 0, 0).unwrap().nanos())
    }

    fn bar() -> DailyBar {
        // $2.00, 100,000 shares every prior session.
        DailyBar {
            o: Some(2.0),
            h: Some(2.0),
            l: Some(2.0),
            c: 2.0,
            v: Some(100_000),
            n: Some(500),
            vw: Some(2.0),
        }
    }

    fn fetch(adj: Adjustment, from: NaiveDate, to: NaiveDate) -> BarFetch {
        BarFetch {
            retrieved_at: night_of_p(),
            provider: "alpaca".into(),
            feed: "sip".into(),
            adjustment: adj,
            timeframe: "1Day".into(),
            from,
            to,
            symbols: 2,
            symbols_per_request: 200,
            requests: 1,
            asof: None,
            command: "test".into(),
        }
    }

    /// What `swimport maintain` leaves for D: 63 dates of raw bars and a split set fetched the
    /// night of P (adjusted == raw: no split), and the 03:30 ET announcement check that found
    /// nothing for either symbol.
    fn write_daily_world(a: &Archive) {
        let p = previous_trading_day(dd());
        let mut first = p;
        for _ in 0..63 {
            first = previous_trading_day(first);
        }
        let mut adjusted: BTreeMap<String, BTreeMap<NaiveDate, DailyBar>> = BTreeMap::new();
        for x in first.iter_days().take_while(|x| *x <= p) {
            if !is_trading_day(x) {
                continue;
            }
            let mut day = RawDayBars::new(x, fetch(Adjustment::Raw, first, p));
            for s in [SYM, OTHER] {
                day.bars.insert(s.into(), bar());
                adjusted.entry(s.into()).or_default().insert(x, bar());
            }
            a.write_raw_bars(&day).unwrap();
        }
        a.write_split_snapshot(&SplitSnapshot {
            schema: 1,
            fetch: fetch(Adjustment::Split, first, p),
            symbols: vec![OTHER.into(), SYM.into()],
            bars: adjusted,
            set: Some(night_of_p().format("%Y%m%dT%H%M%SZ").to_string()),
        })
        .unwrap();
        let policy = CoveragePolicy::v1();
        let (start, end) = policy.process_window(dd());
        a.write_action_retrieval(&ActionRetrieval {
            schema: 1,
            policy: policy.tag.clone(),
            retrieved_at: utc(t(3, 30, 0)),
            provider: "alpaca".into(),
            endpoint: policy.body.endpoint.clone(),
            types: policy.body.types.clone(),
            data_quality: "all".into(),
            session_date: dd(),
            process_start: start,
            process_end: end,
            symbols_requested: vec![OTHER.into(), SYM.into()],
            symbols_covered: vec![OTHER.into(), SYM.into()],
            chunks: vec![ChunkOutcome {
                symbols: vec![OTHER.into(), SYM.into()],
                pages: 1,
                complete: true,
                error: None,
                followup: false,
            }],
            success: true,
            requests: 1,
            command: "test".into(),
            actions: vec![],
        })
        .unwrap();
    }

    fn float(sym: &str, shares: u64, retrieved_at: i64) -> FloatRecord {
        FloatRecord {
            v: 1,
            symbol: sym.into(),
            provider: "fmp".into(),
            endpoint: format!("/stable/shares-float?symbol={sym}"),
            float_shares: Some(shares),
            outstanding_shares: Some(shares * 2),
            free_float_pct: Some(50.0),
            source_url: Some("https://filings.test/10-q".into()),
            provider_date: None,
            effective_as_of: None,
            retrieved_at,
            response_sha256: "00".into(),
            quality: vec![],
        }
    }

    /// What `swimport evidence` leaves: floats at `float_at`, one FDA story about XYZ published
    /// 07:30 ET and seen a minute later, and a market-wide search covering a day back through
    /// 09:30 ET.
    fn write_collected(a: &Archive, float_at: i64) {
        a.append_float(&[
            float(SYM, 5_000_000, float_at),
            float(OTHER, 4_000_000, float_at),
        ])
        .unwrap();
        a.append_news(&[NewsVersionRecord {
            v: 1,
            provider: "alpaca".into(),
            source: "benzinga".into(),
            article_id: "1".into(),
            created_at: Some("2026-10-16T11:30:00Z".into()),
            updated_at: None,
            created_at_ns: Some(t(7, 30, 0)),
            updated_at_ns: None,
            first_observed_at: t(7, 31, 0),
            headline: "XYZ Announces FDA Approval Of Lead Candidate".into(),
            headline_sha256: "h".into(),
            summary_sha256: "s".into(),
            author: None,
            symbols: vec![SYM.into()],
            url: None,
            version_key: "k1".into(),
        }])
        .unwrap();
        a.append_news_coverage(&[NewsCoverageRecord {
            v: 1,
            id: "cov-1".into(),
            provider: "alpaca".into(),
            sources: vec!["benzinga".into()],
            scope: CoverageScope::All,
            covered_from: t(8, 0, 0) - 86_400 * SEC,
            covered_through: t(9, 30, 0),
            observed_at: t(9, 30, 30),
            requests: 1,
            stories: 1,
        }])
        .unwrap();
    }

    fn board() -> ContextBoard {
        ContextBoard::new(
            false,
            Some(dd()),
            Price::from_cents(10),
            Price::from_cents(2500),
        )
        .expect("v2's band is the default band")
    }

    fn load(a: &Archive, log: &Mutex<EvidenceLog>, cx: &mut ContextBoard) -> RefreshReport {
        let mut loader = EvidenceLoader::new(a.clone(), Price::from_cents(2500), false);
        let rep = loader.refresh(dd(), utc(t(23, 0, 0)), log);
        if let Some(b) = log.lock().unwrap().since(cx.evidence_cursor()) {
            cx.add_evidence(b);
        }
        rep
    }

    /// XYZ: $3.00 against a $2.00 close (+50 %), 120,000 shares a print from 09:30. QUIET: the
    /// same prints at $2.05 (+2.5 %).
    fn trade(cx: &mut ContextBoard, from: i64, to: i64) {
        let closes = PrevCloses::default();
        let counted = Counted {
            ohlc: true,
            volume: true,
        };
        let mut x = from;
        let mut n = 0;
        while x < to {
            for (s, cents) in [(SYM, 300), (OTHER, 205)] {
                n += 1;
                let tr = Trade::new(s, UnixNanos(x), Price::from_cents(cents), 120_000, n);
                cx.apply_trade(&tr, counted, &closes);
            }
            x += 10 * SEC;
        }
        cx.advance(UnixNanos(to));
    }

    fn state(cx: &ContextBoard, sym: &str, p: PillarId) -> (Tri, &'static str) {
        let d = cx.decision(sym).expect("a decision");
        let r = d.pillars.iter().find(|x| x.pillar == p).unwrap();
        (r.state, r.reason)
    }

    #[test]
    fn stored_evidence_admits_a_symbol_at_five_of_five_under_v2() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        write_daily_world(&a);
        write_collected(&a, t(7, 0, 0));
        let log = Mutex::new(EvidenceLog::default());
        let mut cx = board();
        let rep = load(&a, &log, &mut cx);
        assert!(rep.errors.is_empty(), "{:?}", rep.errors);
        assert_eq!(
            (
                rep.floats,
                rep.news,
                rep.coverage,
                rep.closes,
                rep.baselines
            ),
            (2, 1, 1, 2, 2),
            "{rep:?}"
        );
        trade(&mut cx, t(9, 30, 0), t(9, 32, 0));

        let d = cx.decision(SYM).unwrap();
        assert!(d.policy.starts_with("pillars-v2@"), "{}", d.policy);
        for p in [
            PillarId::Price,
            PillarId::Gain,
            PillarId::Rvol,
            PillarId::Float,
            PillarId::News,
        ] {
            assert_eq!(state(&cx, SYM, p).0, Tri::Pass, "{p:?}: {d:#?}");
        }
        assert_eq!(d.counts.pass, 5);
        let gain = d
            .pillars
            .iter()
            .find(|x| x.pillar == PillarId::Gain)
            .unwrap();
        assert_eq!(gain.value, Some(5_000), "+50 % against the stored $2.00");
        // The announcement check backs the close: an assumption, labelled as one.
        assert!(
            gain.evidence[0].starts_with("provider_daily_volume_v1:prev_close:XYZ:2026-10-16"),
            "{:?}",
            gain.evidence
        );

        // The panel: XYZ is a member; QUIET (no news, searched) is not.
        let frame: serde_json::Value =
            serde_json::from_str(&cx.panel_frame("ross.snapshot")).expect("json");
        assert!(
            frame["pillarPolicy"]
                .as_str()
                .unwrap()
                .starts_with("pillars-v2@")
        );
        let panel = &frame["panel"];
        assert!(
            panel["setup_policy"]
                .as_str()
                .unwrap()
                .starts_with("setups-v0@"),
            "{panel}"
        );
        assert!(panel["attention"].is_u64());
        let rows = panel["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 1, "{panel}");
        assert_eq!(rows[0]["symbol"], SYM);
        assert_eq!(rows[0]["counts"]["pass"], 5);
        assert_eq!(rows[0]["float_shares"], 5_000_000);
        assert_eq!(rows[0]["news_first_seen"], t(7, 31, 0));
        assert!(rows[0]["setup"].is_object());
        assert_eq!(
            cx.decision(SYM).unwrap().membership.status,
            Some(MemberStatus::Qualified)
        );
        // QUIET's news FAILs only because the market-wide search covered it.
        assert_eq!(
            state(&cx, OTHER, PillarId::News),
            (Tri::Fail, "no_matching_news_in_covered_sources")
        );
        assert_eq!(state(&cx, OTHER, PillarId::Gain).0, Tri::Fail);
    }

    #[test]
    fn a_record_observed_after_a_decision_is_not_visible_to_it() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        write_daily_world(&a);
        // The float arrives at 09:45 ET, after the first decisions (and loaded before any of
        // them: loading early must not make it visible early).
        write_collected(&a, t(9, 45, 0));
        let log = Mutex::new(EvidenceLog::default());
        let mut cx = board();
        load(&a, &log, &mut cx);

        trade(&mut cx, t(9, 30, 0), t(9, 32, 0));
        let before = cx.decision(SYM).unwrap();
        assert!(before.cutoff < t(9, 45, 0));
        assert_eq!(
            state(&cx, SYM, PillarId::Float),
            (Tri::Unknown, "no_float_evidence")
        );
        assert_eq!(before.counts.pass, 4);
        assert!(
            before.membership.active().is_none(),
            "{:?}",
            before.membership
        );
        let frame: serde_json::Value =
            serde_json::from_str(&cx.panel_frame("ross.snapshot")).unwrap();
        assert!(
            frame["panel"].is_null() || frame["panel"]["rows"].as_array().unwrap().is_empty(),
            "{frame}"
        );

        // From the first decision after 09:45 the float is there, and XYZ is admitted.
        trade(&mut cx, t(9, 44, 0), t(9, 46, 0));
        let after = cx.decision(SYM).unwrap();
        assert!(after.cutoff >= t(9, 45, 0));
        assert_eq!(state(&cx, SYM, PillarId::Float).0, Tri::Pass);
        assert_eq!(after.counts.pass, 5);
        assert_eq!(after.membership.status, Some(MemberStatus::Qualified));
    }

    #[test]
    fn a_refresh_delivers_only_what_is_new_and_a_new_board_gets_everything() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        write_daily_world(&a);
        write_collected(&a, t(7, 0, 0));
        let log = Mutex::new(EvidenceLog::default());
        let mut loader = EvidenceLoader::new(a.clone(), Price::from_cents(2500), false);
        let now = utc(t(23, 0, 0));
        let first = loader.refresh(dd(), now, &log);
        assert!(first.rebuilt);
        let all = log.lock().unwrap().since(None).unwrap();
        assert_eq!(all.len(), first.new_records());
        let generation = all.generation;
        let cursor = Some((generation, all.through));
        assert!(log.lock().unwrap().since(cursor).is_none(), "up to date");

        // Nothing changed on disk: nothing new, no rebuild.
        let again = loader.refresh(dd(), now, &log);
        assert_eq!(
            (again.new_records(), again.rebuilt),
            (0, false),
            "{again:?}"
        );

        // One more float retrieval: exactly that record goes to a board that has the rest.
        a.append_float(&[float(SYM, 6_000_000, t(8, 0, 0))])
            .unwrap();
        let more = loader.refresh(dd(), now, &log);
        assert_eq!(more.new_records(), 1, "{more:?}");
        let b = log.lock().unwrap().since(cursor).unwrap();
        assert_eq!(b.floats.len(), 1);
        assert_eq!((b.floats[0].shares, b.floats[0].version), (6_000_000, 2));
        // A board built after a seek has no cursor: it gets the whole day.
        assert_eq!(
            log.lock().unwrap().since(None).unwrap().len(),
            all.len() + 1
        );

        // A new session date starts a new log generation; the old cursor gets everything.
        let next = dd() + Duration::days(3);
        loader.refresh(next, now, &log);
        {
            let l = log.lock().unwrap();
            assert_eq!(l.day(), Some(next));
            assert!(l.since(cursor).is_some_and(|b| b.generation != generation));
        }

        // A replay switching forward again reads its day's records anew: the first pass read
        // (and dropped) them while they were after its day.
        let back = loader.refresh(dd(), now, &log);
        assert_eq!(back.floats, 3, "{back:?}");
        let mut fresh = EvidenceLoader::new(a.clone(), Price::from_cents(2500), false);
        fresh.refresh(dd() - Duration::days(7), now, &log);
        assert_eq!(fresh.refresh(dd(), now, &log).floats, 3);
    }

    /// The engine's output journals are drained every tick, with no `ross` subscriber anywhere:
    /// the drain is the market's quotes task, not a viewer's side effect. Before this slice the
    /// only panel drain was inside `take_update`, which `Market::publish_context` never reaches
    /// with zero subscribers, and `take_setup_log` had no caller at all.
    #[test]
    fn outputs_are_drained_without_any_viewer() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        write_daily_world(&a);
        write_collected(&a, t(7, 0, 0));
        let log = Mutex::new(EvidenceLog::default());
        let mut cx = board();
        load(&a, &log, &mut cx);
        // A member, so the panel really changes from barrier to barrier.
        trade(&mut cx, t(9, 30, 0), t(9, 40, 0));

        // No `take_update` call: nobody is subscribed. The drain still takes everything, and
        // every snapshot it takes is offered to the observation capture (`crate::observe`) on the
        // way past, so the measurement scaffolding does not depend on a viewer either.
        let mut seen: Vec<(u64, i64)> = Vec::new();
        let took = cx.drain_outputs(&mut |p| seen.push((p.revision, p.cutoff)));
        assert!(took.panels > 0, "panels were published: {took:?}");
        assert_eq!(seen.len(), took.panels, "one observation per drained panel");
        assert!(
            seen.windows(2).all(|w| w[0].1 <= w[1].1),
            "offered oldest first: {seen:?}"
        );
        let tel = cx.output_telemetry();
        assert_eq!(tel.panels_held, 0, "the drain emptied the journal");
        assert_eq!(tel.panels_taken, took.panels as u64);
        assert!(
            tel.complete(),
            "a drain that keeps up drops nothing: {tel:?}"
        );
        assert_eq!(cx.drained(), took);

        // And the latest snapshot is still there for a client that subscribes later: draining
        // the journal is not the same as clearing the panel.
        let frame: serde_json::Value =
            serde_json::from_str(&cx.panel_frame("ross.snapshot")).expect("json");
        assert_eq!(
            frame["panel"]["rows"].as_array().unwrap().len(),
            1,
            "{frame}"
        );
        assert!(
            cx.take_update().is_some(),
            "the first update after a drain still goes out"
        );
        assert!(
            cx.take_update().is_none(),
            "and only once per revision, as before"
        );

        // A second drain with nothing new takes nothing.
        assert!(cx.drain_outputs(&mut |_| {}).is_empty());
    }

    /// With no drain at all the engine holds at most its bound and counts what it dropped: the
    /// unattended run costs bounded memory, and stops claiming the journal is complete.
    #[test]
    fn an_undrained_board_stays_bounded_and_reports_the_loss() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        write_daily_world(&a);
        write_collected(&a, t(7, 0, 0));
        let log = Mutex::new(EvidenceLog::default());
        let mut cx = board().with_output_bounds(4, 8);
        load(&a, &log, &mut cx);
        trade(&mut cx, t(9, 30, 0), t(9, 40, 0));

        let tel = cx.output_telemetry();
        assert!(tel.panels_published > 4, "{tel:?}");
        assert_eq!(tel.panels_held, 4, "held at the bound: {tel:?}");
        assert!(tel.panels_dropped > 0, "{tel:?}");
        assert!(!tel.complete(), "{tel:?}");
        assert_eq!(
            tel.panels_published,
            tel.panels_taken + tel.panels_dropped + tel.panels_held as u64
        );
        // A client subscribing now still gets the current panel, not a dropped one.
        assert!(cx.take_update().is_some());
    }
}
