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
//! | SEC filings, as news items (pillars-v6) | `swimport evidence`'s EDGAR worker (`evidence/filings/`) | `evidence_load::filing_items` |
//! | news coverage, market-wide | `swimport evidence` (`evidence/news-coverage/`) | `evidence_load::market_news_coverage` |
//! | previous close + daily baseline | `swimport maintain` (`daily-bars/`, `corporate-actions/`) | `baseline::load` |
//!
//! # When
//!
//! [`EvidenceLoader::refresh`] runs at startup and then every `CONTEXT_EVIDENCE_CHECK_SECS`
//! (15 s, [`DEFAULT_CHECK_SECS`] says why) on a blocking thread, never on the market path: trade
//! processing never waits for disk.
//! The append-only files are followed with [`EvidenceTail`] (only bytes appended since the last
//! call are parsed); baselines are rebuilt when their inputs changed on disk (a raw date file,
//! the split snapshots, or a new announcement check for the session date: the maintainer makes
//! one every 30 minutes in the session) **or** when the clock alone can change the answer (see
//! "as of" below). The new records go into the [`EvidenceLog`] and
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
//! Baselines are computed **as of** the run's own clock, and nothing retrieved after it is used:
//! live, the wall clock at the refresh; a replay, its simulated clock, so a replayed day sees the
//! announcement checks a live run would have had by the same point of the day and never a later
//! one. (It used to freeze a replay at the day's *first* check, which denied a replayed 10:00 the
//! 09:00 re-check a live 10:00 certainly had.) A build is therefore redone not only when an input
//! file changes but also when time alone can change the answer: when the clock reaches a check
//! the last build could not read, and when a coverage window runs out with no newer check.
//! Raw date files are refreshed every night,
//! so for an older replayed day the records usually carry an `observed_at` after that day and are
//! invisible to it: the rule working, not a gap.
//!
//! # Bounded memory
//!
//! Only the current session date is held: records observed from two days before its first
//! session on (news is stale after 24 h, and a coverage run must reach a full freshness window
//! back), floats from five days before (pillars-v4 keeps a float for 4 days), up to the day's end for a replay. A replay day switch reads the
//! files again for the new window; live only trims as its date moves forward. Baselines are built only for symbols whose previous close
//! was at or below the band's top (above it a stock can only reach the band by falling, which
//! fails the gain pillar), and the log keeps only the latest previous close and baseline per id.
//! News coverage is one market-wide record per search, not one per symbol. The engine drops what
//! no later decision can read ([`stock_context::EvidenceStore::prune`]).

use crate::market::{Inner, Market};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use market_archive::baseline::{self, Inputs};
use market_archive::evidence_load::{
    filing_items, float_observations, market_news_coverage, news_coverage, news_items,
    outstanding_observations, scoped_symbols, source_news_items,
};
use market_archive::{
    Archive, CoveragePolicy, EvidenceTail, FilingRecord, FloatRecord, NewsCoverageRecord,
    NewsVersionRecord, SourceNewsRecord,
};
use market_events::session::{previous_trading_day, sessions_for, trading_date};
use market_events::{Price, UnixNanos};
use market_providers::ClockMode;
use market_providers::warmup::{Enqueue, WarmupQueue};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration as StdDuration, SystemTime};
use stock_context::{
    DailyBaseline, FloatObservation, IntradayBaseline, NewsCoverage, NewsItem,
    OutstandingObservation, PrevClose,
};

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

/// How far back float records are kept: pillars-v4 uses a float for 4 days (owner, 2026-10-31),
/// so a decision at the session's first second can need one from 4 days earlier. One day of
/// margin; `float_lookback_covers_the_policy` keeps it at least the policy's window.
const FLOAT_LOOKBACK: Duration = Duration::days(5);

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
    /// Shares-outstanding figures (never floats): read only by pillars-v5's float fallback.
    outstanding: Vec<(u64, OutstandingObservation)>,
    /// The latest diagnostic intraday baseline per record id, like `closes` and `baselines`.
    ///
    /// It was a Vec appended on every read (2026-10-29), and every decided symbol is re-read
    /// every [`INTRADAY_RETRY_SECS`] (up to [`INTRADAY_SCAN_MAX`] a tick). So each market
    /// appended up to ~400 identical copies of a ~12-32 KiB curve a minute, kept to the end of
    /// the session date. Production grew ~16 MB a minute, all day and all night, until the
    /// kernel killed the process. `len` left this field out, so the log's "held" never showed it.
    /// The engine already ignored the copies (`EvidenceStore` keeps one of each record), so
    /// keeping one per id changes nothing a board decides.
    intraday: HashMap<String, (u64, IntradayBaseline)>,
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
    /// Shares-outstanding figures for pillars-v5's float fallback (never a float).
    pub outstanding: Vec<OutstandingObservation>,
    /// Diagnostic intraday curves (`market_archive::intraday`): the same-time and recent-window
    /// RVOL measures only, never the pillar.
    pub intraday: Vec<IntradayBaseline>,
    pub news: Vec<NewsItem>,
    pub coverage: Vec<NewsCoverage>,
    pub closes: Vec<PrevClose>,
    pub baselines: Vec<DailyBaseline>,
}

impl EvidenceBatch {
    pub fn len(&self) -> usize {
        self.floats.len()
            + self.outstanding.len()
            + self.intraday.len()
            + self.news.len()
            + self.coverage.len()
            + self.closes.len()
            + self.baselines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Puts a batch of evidence into a bare engine, each record keeping its own `observed_at`.
///
/// For the restart restore's own replay engine (`crate::history`): it decides the day's barriers
/// on the recorded clock, so it needs the same records the live engine has, joined as of each
/// historical cutoff. Nothing here changes an `observed_at`, so a record observed at 11:00 is as
/// invisible to a 09:30 barrier in the replay as it was in the live run.
pub fn seed_engine(engine: &mut stock_context::Engine, batch: EvidenceBatch) {
    for r in batch.closes {
        engine.add_prev_close(r);
    }
    for r in batch.baselines {
        engine.add_daily(r);
    }
    for r in batch.floats {
        engine.add_float(r);
    }
    for r in batch.outstanding {
        engine.add_outstanding(r);
    }
    for r in batch.intraday {
        engine.add_intraday(r);
    }
    for r in batch.news {
        engine.add_news(r);
    }
    for r in batch.coverage {
        engine.add_coverage(r);
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
            + self.outstanding.len()
            + self.intraday.len()
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
            outstanding: pick(self.outstanding.iter(), after),
            intraday: pick(self.intraday.values(), after),
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

    /// Keeps each record id's latest intraday baseline. A re-read that found the same record
    /// is not news: it takes no sequence, so no board is sent it again.
    fn add_intraday(&mut self, records: Vec<IntradayBaseline>) {
        for r in records {
            if self.intraday.get(&r.id).is_some_and(|(_, held)| *held == r) {
                continue;
            }
            let s = self.next();
            self.intraday.insert(r.id.clone(), (s, r));
        }
    }

    /// Everything one symbol's decisions can read: its own records, and the market-wide news
    /// coverage, which speaks for every instrument.
    ///
    /// For a restart's per-symbol restore ([`crate::history`]): the symbol's recorded day is
    /// replayed on its own engine, and that engine needs the same evidence the live one had —
    /// each record still carrying its own `observed_at`, so a historical cutoff joins exactly
    /// what was known then and nothing later. Copying the whole log per symbol would be
    /// thousands of records times thousands of symbols; this is one pass and only the matches.
    pub fn for_symbol(&self, symbol: &str) -> EvidenceBatch {
        let own = stock_context::InstrumentId::symbol(symbol);
        let mine = |i: &stock_context::InstrumentId| *i == own;
        EvidenceBatch {
            generation: self.generation,
            through: self.seq,
            floats: self
                .floats
                .iter()
                .filter(|(_, r)| mine(&r.instrument))
                .map(|(_, r)| r.clone())
                .collect(),
            outstanding: self
                .outstanding
                .iter()
                .filter(|(_, r)| mine(&r.instrument))
                .map(|(_, r)| r.clone())
                .collect(),
            intraday: self
                .intraday
                .values()
                .filter(|(_, r)| mine(&r.instrument))
                .map(|(_, r)| r.clone())
                .collect(),
            news: self
                .news
                .iter()
                .filter(|(_, r)| mine(&r.instrument))
                .map(|(_, r)| r.clone())
                .collect(),
            coverage: self
                .coverage
                .iter()
                .filter(|(_, r)| mine(&r.instrument) || r.instrument.is_market())
                .map(|(_, r)| r.clone())
                .collect(),
            closes: self
                .closes
                .values()
                .filter(|(_, r)| mine(&r.instrument))
                .map(|(_, r)| r.clone())
                .collect(),
            baselines: self
                .baselines
                .values()
                .filter(|(_, r)| mine(&r.instrument))
                .map(|(_, r)| r.clone())
                .collect(),
        }
    }
}

/// What one refresh found (one log line).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RefreshReport {
    pub day: Option<NaiveDate>,
    pub floats: usize,
    /// Shares-outstanding figures delivered (pillars-v5's float fallback).
    pub outstanding: usize,
    /// Diagnostic intraday baselines delivered (`market_archive::intraday`).
    pub intraday: usize,
    /// Prior sessions a curve existed for but which were not comparable: an unverifiable share
    /// basis, a provisional recording, an incomplete collection, or an unapplied revision.
    pub intraday_suppressed: usize,
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
        self.floats
            + self.outstanding
            + self.intraday
            + self.news
            + self.coverage
            + self.closes
            + self.baselines
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
    /// The first `retrieved_at` a float record is kept from ([`FLOAT_LOOKBACK`]).
    float_from: i64,
    floats_raw: Vec<FloatRecord>,
    news_raw: Vec<NewsVersionRecord>,
    /// SEC filings (`evidence/filings/`), delivered as news items beside the Benzinga stories.
    /// Their coverage records are the EDGAR collector's own and are not read: a news FAIL rests
    /// on the market-wide Benzinga searches (pillars-v6).
    filings_raw: Vec<FilingRecord>,
    /// Any other source's items (`evidence/source-news/`: the press wires), through the same
    /// seam (`stock_context::SourceNews`).
    sources_raw: Vec<SourceNewsRecord>,
    coverage_raw: Vec<NewsCoverageRecord>,
    baseline_stamp: Option<Stamp>,
    /// The run-clock `as_of` the last baseline build used.
    baseline_as_of: Option<DateTime<Utc>>,
    /// Per symbol, the input generation its diagnostic intraday baseline was last read at.
    ///
    /// Not "read once a day": a curve the maintainer derives later today, and a share basis a
    /// later announcement check verifies, both change the answer for a symbol already looked at.
    /// Caching the look for the whole date would have left those symbols without a diagnostic
    /// until tomorrow. The generation moves when either input could have (`intraday_generation`),
    /// and a bounded retry covers a filesystem whose directory stamps do not move.
    intraday_read: HashMap<String, (u64, DateTime<Utc>)>,
    /// Bumped when the stored curves or the share-basis factors could have changed.
    intraday_generation: u64,
    /// Modified times of the lookback dates' curve directories at the last check: a new curve
    /// file moves its directory's stamp.
    intraday_stamp: Vec<Option<SystemTime>>,
    /// Per symbol, the share-basis restatement the daily denominator used, with its provenance
    /// and expiry ([`market_archive::intraday::Basis`]).
    ///
    /// The intraday diagnostic has to restate its curves by exactly these: a prior session's raw
    /// share count is not comparable with today's across a split, and the evidence behind the
    /// restatement has its own observation time and its own expiry. Kept from the last baseline
    /// build rather than recomputed, so the two measures can never drift apart, and bounded by
    /// the universe.
    basis: HashMap<String, market_archive::intraday::Basis>,
    /// The earliest announcement retrieval of the day later than `baseline_as_of`, when the last
    /// build saw one: at that time a rebuild can read a check the last one could not, with no
    /// file having changed. `None`: none known.
    next_check_at: Option<DateTime<Utc>>,
    /// When the coverage the last build rested on runs out of freshness (the latest readable
    /// check plus the policy's limit). Past it the same files give a different answer — the
    /// outage case — so the build is redone. `None`: the build read no check, so nothing expires.
    coverage_expires_at: Option<DateTime<Utc>>,
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
            float_from: 0,
            floats_raw: Vec::new(),
            news_raw: Vec::new(),
            filings_raw: Vec::new(),
            sources_raw: Vec::new(),
            coverage_raw: Vec::new(),
            baseline_stamp: None,
            baseline_as_of: None,
            next_check_at: None,
            coverage_expires_at: None,
            intraday_read: HashMap::new(),
            intraday_generation: 0,
            intraday_stamp: Vec::new(),
            basis: HashMap::new(),
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
            let float_from = ns(DateTime::from_timestamp_nanos(start) - FLOAT_LOOKBACK);
            if self.day.is_some() && (!self.live || from < self.window.0) {
                // A replay day switch: the records the new day needs were read under the old
                // window and dropped (the tail never hands a line out twice), so read the files
                // again from the new window's start. Live only ever moves forward and keeps
                // everything newer than its window's start, so it just trims.
                self.tail = EvidenceTail::default();
                self.floats_raw.clear();
                self.news_raw.clear();
                self.filings_raw.clear();
                self.sources_raw.clear();
                self.coverage_raw.clear();
            }
            self.window = (from, end);
            self.float_from = float_from;
            self.day = Some(day);
            self.floats_raw.retain(|r| r.retrieved_at >= float_from);
            self.news_raw.retain(|r| r.first_observed_at >= from);
            self.filings_raw.retain(|r| r.first_observed_at >= from);
            self.sources_raw.retain(|r| r.first_observed_at >= from);
            self.coverage_raw.retain(|r| r.observed_at >= from);
            self.baseline_stamp = None;
            self.baseline_as_of = None;
            self.next_check_at = None;
            self.coverage_expires_at = None;
            self.intraday_read.clear();
            self.intraday_stamp.clear();
            self.intraday_generation += 1;
            self.basis.clear();
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
        let float_from = self.float_from;
        let float_inside = |t: i64| t >= float_from && (live || t <= through);

        // Append-only files: only what was appended since the last call.
        let f = self.tail.float(&self.archive, Some(float_from));
        let n = self.tail.news(&self.archive, Some(from));
        let c = self.tail.news_coverage(&self.archive, Some(from));
        let g = self.tail.filings(&self.archive, Some(from));
        let w = self.tail.source_news(&self.archive, Some(from));
        rep.errors.extend(
            f.errors
                .iter()
                .chain(&n.errors)
                .chain(&c.errors)
                .chain(&g.errors)
                .chain(&w.errors)
                .map(|e| e.to_string()),
        );
        let (mut new_f, mut new_n, mut new_c) = (new_day, new_day, new_day);
        let mut new_g = new_day;
        let mut new_w = new_day;
        for r in w.records {
            // Record-only FMP news (`shadow_*`, FMP_NEWS_COUNT unset) is archived for the
            // comparison audit and never enters the engine: not even as a never-passing item,
            // whose arrival would still schedule a re-evaluation of its symbol and could so move
            // when a decision is logged (market_providers::import::fmp_news).
            if market_providers::import::fmp_news::is_shadow(&r) {
                continue;
            }
            if inside(r.first_observed_at) {
                self.sources_raw.push(r);
                new_w = true;
            }
        }
        for r in g.records {
            if inside(r.first_observed_at) {
                self.filings_raw.push(r);
                new_g = true;
            }
        }
        for r in f.records {
            if float_inside(r.retrieved_at) {
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
        let (floats, outstanding) = if new_f {
            (
                float_observations(&self.floats_raw),
                outstanding_observations(&self.floats_raw),
            )
        } else {
            (Vec::new(), Vec::new())
        };
        let mut news = if new_n {
            news_items(&self.news_raw, None)
        } else {
            Vec::new()
        };
        if new_g {
            news.extend(filing_items(&self.filings_raw, None));
        }
        if new_w {
            news.extend(source_news_items(&self.sources_raw, None));
        }
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
        let outstanding = keep_new("outstanding", outstanding, d);
        let news = keep_new("news", news, d);
        let coverage = keep_new("coverage", coverage, d);
        let closes = keep_new("close", closes, d);
        let baselines = keep_new("baseline", baselines, d);
        rep.floats = floats.len();
        rep.outstanding = outstanding.len();
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
            for r in outstanding {
                let s = log.next();
                log.outstanding.push((s, r));
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

    /// Notes whether the stored curves could have changed since the last check, and returns the
    /// current input generation.
    ///
    /// One `stat` per lookback date, not a directory listing: a new curve file moves its date
    /// directory's modified time. Cheap enough for the 15 s tick, and the bounded retry in
    /// [`EvidenceLoader::intraday_pending`] covers a filesystem whose directory stamps do not
    /// move at all.
    pub fn intraday_generation(&mut self, day: NaiveDate) -> u64 {
        let stamp: Vec<Option<SystemTime>> = self
            .intraday_lookback(day)
            .iter()
            .map(|d| {
                std::fs::metadata(
                    self.archive
                        .root()
                        .join(market_archive::INTRADAY_DIR)
                        .join(d.format("%Y-%m-%d").to_string()),
                )
                .ok()
                .and_then(|m| m.modified().ok())
            })
            .collect();
        if self.intraday_stamp != stamp {
            self.intraday_stamp = stamp;
            self.intraday_generation += 1;
        }
        self.intraday_generation
    }

    /// Symbols of `symbols` whose diagnostic intraday baseline needs reading: never read, read at
    /// an older input generation, or last read longer ago than [`INTRADAY_RETRY_SECS`].
    ///
    /// The retry is the honest part. A symbol whose curves were missing at 09:30 may have them by
    /// 10:00, and a symbol whose share basis was unverifiable may have a fresh check by then;
    /// caching the look for the whole session date would have left both without a diagnostic
    /// until the next day.
    pub fn intraday_pending(
        &self,
        symbols: &[String],
        max: usize,
        generation: u64,
        now: DateTime<Utc>,
    ) -> Vec<String> {
        let retry = Duration::seconds(INTRADAY_RETRY_SECS);
        symbols
            .iter()
            .filter(|s| match self.intraday_read.get(*s) {
                None => true,
                Some(&(g, at)) => g != generation || now - at >= retry,
            })
            .take(max)
            .cloned()
            .collect()
    }

    /// The trading dates before `day` the diagnostic curves are looked for over.
    fn intraday_lookback(&self, day: NaiveDate) -> Vec<NaiveDate> {
        let mut dates = Vec::with_capacity(INTRADAY_LOOKBACK_SESSIONS);
        let mut x = day;
        for _ in 0..INTRADAY_LOOKBACK_SESSIONS {
            x = previous_trading_day(x);
            dates.push(x);
        }
        dates.reverse();
        dates
    }

    /// Reads the stored intraday curves of the sessions before `day` for `symbols` and returns
    /// one diagnostic baseline each (`market_archive::intraday`).
    ///
    /// Diagnostic-only: these feed `RvolDiagnostics::same_time_milli` and `recent_milli`, never
    /// the RVOL pillar, whose 30-session daily denominator is unchanged. Each symbol is read once
    /// per session date, because prior sessions do not change during a day, and a symbol with no
    /// stored curves gets no record at all — the engine then says `insufficient_sessions` rather
    /// than a number.
    ///
    /// How many prior sessions can ever exist is capped by the archive's own retention window, so
    /// on a short archive the diagnostic stays `insufficient_sessions`. That is the truth about
    /// the data, not a failure of this path.
    pub fn intraday(
        &mut self,
        day: NaiveDate,
        symbols: &[String],
        generation: u64,
        now: DateTime<Utc>,
        rep: &mut RefreshReport,
    ) -> Vec<IntradayBaseline> {
        let dates = self.intraday_lookback(day);
        let fallback = market_archive::intraday::Basis::default();
        let mut out = Vec::new();
        for sym in symbols {
            // Note the look at this generation whatever came back, so an unchanged input is not
            // re-read every tick; a later generation or the retry interval brings it back.
            self.intraday_read.insert(sym.clone(), (generation, now));
            let curves = match self.archive.read_intraday_curves(sym, &dates) {
                Ok(c) => c,
                Err(e) => {
                    rep.errors.push(format!("intraday curves {sym}: {e}"));
                    continue;
                }
            };
            if curves.is_empty() {
                continue;
            }
            // Restated by exactly the factors the daily denominator used for the same sessions,
            // and stamped with the latest of every input's observation time, so a basis verified
            // later today cannot reach an earlier decision. No basis at all means every session
            // is suppressed, which is the honest answer.
            let basis = self.basis.get(sym).unwrap_or(&fallback);
            let a = market_archive::intraday::intraday_baseline(&curves, sym, basis);
            if !a.suppressed.is_empty() {
                rep.intraday_suppressed += a.suppressed.values().sum::<usize>();
                tracing::debug!(
                    symbol = %sym,
                    usable = a.usable,
                    suppressed = ?a.suppressed,
                    basis = %basis.source,
                    "intraday diagnostic sessions left out"
                );
            }
            if let Some(b) = a.baseline {
                out.push(b);
            }
        }
        out
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
        // Changed inputs are not the only reason to rebuild. A corporate-action record is only
        // as good as the check behind it is fresh (`corporate-actions-coverage-v1`), so time
        // alone changes the answer: a check that becomes visible as the run clock reaches it
        // (recovery), and a coverage window that runs out with no newer check (the outage the
        // audit found keeping a stale PASS alive). Neither touches a file, so an mtime stamp
        // cannot see either.
        let timed_out = self.next_check_at.is_some_and(|t| now >= t)
            || self.coverage_expires_at.is_some_and(|t| now >= t);
        let changed = self.baseline_stamp.as_ref() != Some(&stamp);
        if !changed && self.baseline_as_of.is_some() && !timed_out {
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
        // Evidence as of the **run's own clock**, live or replay (the caller passes it): a
        // replayed day sees the checks a live run would have had by the same point of the day and
        // never a later one, and each record still joins only from its own `observed_at`.
        //
        // This replaces freezing a replay at the day's first check. Freezing was not the safer
        // reading: it denied a replayed 10:00 the 09:00 re-check a live 10:00 certainly had, and
        // with the assumption horizon now enforced that left a replayed day permanently UNKNOWN
        // from 90 minutes after its pre-session check.
        let as_of = now;
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
            self.remember_build(stamp, as_of, &announcements);
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
            // The restatement the daily denominator used, with the evidence behind it, kept for
            // the intraday diagnostic to apply to the same sessions. A symbol whose basis could
            // not be verified has no factors, and its curves are then suppressed rather than
            // compared across two share bases.
            match (&l.baseline, l.volume_factors.is_empty()) {
                (Some(b), false) => {
                    self.basis.insert(
                        sym.clone(),
                        market_archive::intraday::Basis {
                            factors: l.volume_factors.clone(),
                            evidence: b.basis.clone(),
                            observed_at: b.observed_at,
                            source: format!("{}@{}", b.id, b.version),
                        },
                    );
                }
                _ => {
                    self.basis.remove(sym);
                }
            }
            closes.extend(l.prev_close);
            baselines.extend(l.baseline);
        }
        self.remember_build(stamp, as_of, &announcements);
        // New factors are new input for the intraday diagnostic: a symbol already looked at has
        // to be looked at again, or a basis verified by this build would never reach the curves
        // read before it.
        self.intraday_generation += 1;
        (closes, baselines)
    }

    /// Records what the finished build rested on, so the next tick knows when time alone can
    /// change the answer.
    fn remember_build(
        &mut self,
        stamp: Stamp,
        as_of: DateTime<Utc>,
        announcements: &[market_archive::ActionRetrieval],
    ) {
        self.baseline_stamp = Some(stamp);
        self.baseline_as_of = Some(as_of);
        self.next_check_at = announcements
            .iter()
            .map(|r| r.retrieved_at)
            .filter(|t| *t > as_of)
            .min();
        let freshness = Duration::seconds(CoveragePolicy::v1().body.freshness_secs);
        self.coverage_expires_at = announcements
            .iter()
            .map(|r| r.retrieved_at)
            .filter(|t| *t <= as_of)
            .max()
            .map(|t| t + freshness)
            .filter(|t| *t > as_of);
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
    /// The volume the records come from. The warmup inbox is written to it, never from the
    /// market-event path.
    archive: Archive,
    /// Bounded, non-blocking warmup requests for symbols whose evidence a warmup could supply
    /// ([`Market::request_warmups`]).
    warmups: WarmupQueue,
    stats: Mutex<WarmupStats>,
}

/// What the warmup requester has done, all of it counted rather than assumed
/// ([`Market::request_warmups`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct WarmupStats {
    /// Symbols seen missing evidence a warmup could supply.
    pub candidates: u64,
    /// New requests buffered in memory.
    pub queued: u64,
    /// Already asked for that date (buffered, filed or finished).
    pub known: u64,
    /// Dropped because the in-memory queue was full. The honest part.
    pub full: u64,
    /// The queue was locked that instant; the next tick asks again.
    pub busy: u64,
    /// Requests written to the archive inbox.
    pub filed: u64,
    /// Flushes that failed; the requests stay buffered for the next tick.
    pub flush_errors: u64,
    /// Requests buffered in memory after the last flush.
    pub pending: usize,
    /// Dedup entries held, all dates: bounded by the symbols asked for on the current date.
    pub tracked: usize,
    /// Dates whose dedup state was dropped as the session date moved on.
    pub rolled: u64,
}

/// Symbols the requester collects per tick.
///
/// A bound, not a target: the scan skips what has already been asked for today before it takes
/// this many, so a long tail really is covered over several ticks rather than blocked behind
/// whatever happens to sort first.
pub const WARMUP_SCAN_MAX: usize = 200;

/// In-memory warmup requests held before `capture` starts dropping them (and counting it).
pub const WARMUP_QUEUE_CAPACITY: usize = 2_000;

/// Prior trading dates the diagnostic intraday curves are looked for over. More than the
/// diagnostic's minimum so a few missing days still leave enough, and no more than the archive's
/// own retention window can hold anyway.
pub const INTRADAY_LOOKBACK_SESSIONS: usize = 25;

/// Symbols whose intraday curves are read per tick. Bounded like the warmup scan, and for the
/// same reason: the rest are read on later ticks.
pub const INTRADAY_SCAN_MAX: usize = 100;

/// How long a symbol's intraday read stands before it is tried again on an unchanged input
/// generation. A backstop for a filesystem whose directory stamps do not move; the generation
/// normally brings a symbol back as soon as its input really changed.
pub const INTRADAY_RETRY_SECS: i64 = 300;

impl Market {
    /// Sets up this market's evidence (once) and starts loading it: a replay synchronously, here,
    /// before its ingest starts; a live market in the background. Then every `every`.
    pub(crate) fn init_evidence(&self, archive: Archive, band_max: Price, every: StdDuration) {
        let live = self.mode() == ClockMode::Live;
        let ev = Evidence {
            loader: Mutex::new(EvidenceLoader::new(archive.clone(), band_max, live)),
            log: Mutex::new(EvidenceLog::default()),
            archive,
            warmups: WarmupQueue::new(WARMUP_QUEUE_CAPACITY),
            stats: Mutex::new(WarmupStats::default()),
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
        let wall = Utc::now();
        let day = match (day, self.mode()) {
            (Some(d), _) => d,
            (None, ClockMode::Live) => {
                trading_date(UnixNanos::from_utc(wall).unwrap_or(UnixNanos(0)))
            }
            (None, ClockMode::Replay) => match self.info().and_then(|i| i.day) {
                Some(d) => d,
                None => return,
            },
        };
        // The run's own clock: the wall clock live, the simulated clock in a replay, so a
        // replayed day reads evidence as of the point of the day it is at (`EvidenceLoader`
        // module docs, "as of"). Before the replay clock exists, the day's own start — never the
        // wall clock, which would make every one of that day's later re-checks visible at once.
        let now = match self.mode() {
            ClockMode::Live => wall,
            ClockMode::Replay => self
                .clock()
                .map(|c| DateTime::from_timestamp_nanos(c.now.nanos()))
                .unwrap_or_else(|| DateTime::from_timestamp_nanos(day_bounds(day).0)),
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
        self.load_intraday();
        self.deliver_evidence();
        self.request_warmups();
        self.refresh_readiness();
    }

    /// Reads the diagnostic intraday curves for the symbols that have traded today and puts them
    /// in the log for the board to take (`market_archive::intraday`).
    ///
    /// Bounded and resumable like the warmup scan: `INTRADAY_SCAN_MAX` symbols a tick, the
    /// already-read ones skipped **before** the budget is taken, and the disk read on this
    /// blocking thread rather than under the core lock. Diagnostic-only — the RVOL pillar's
    /// 30-session daily denominator is untouched.
    pub(crate) fn load_intraday(&self) {
        let Some(ev) = self.0.evidence.get() else {
            return;
        };
        let Some(day) = ev.log.lock().expect("evidence log lock").day() else {
            return;
        };
        // Every symbol with a decision today, then the loader decides which of them need
        // reading: off the core lock, so a disk read never happens under it.
        let all = {
            let core = self.0.core.lock().expect("core lock");
            match core.context.as_ref() {
                Some(cx) => cx.decided_symbols(usize::MAX, &std::collections::BTreeSet::new()),
                None => return,
            }
        };
        if all.is_empty() {
            return;
        }
        let now = Utc::now();
        let mut rep = RefreshReport::default();
        let records = {
            let mut l = ev.loader.lock().expect("evidence loader lock");
            let generation = l.intraday_generation(day);
            let symbols = l.intraday_pending(&all, INTRADAY_SCAN_MAX, generation, now);
            if symbols.is_empty() {
                return;
            }
            l.intraday(day, &symbols, generation, now, &mut rep)
        };
        for e in &rep.errors {
            tracing::warn!(day = %day, error = %e, "intraday diagnostic curves");
        }
        if records.is_empty() {
            return;
        }
        let n = records.len();
        ev.log
            .lock()
            .expect("evidence log lock")
            .add_intraday(records);
        tracing::debug!(
            day = %day,
            baselines = n,
            symbols = all.len(),
            "intraday diagnostic curves loaded"
        );
    }

    /// Takes the per-pillar readiness summary and caches it for the `ross` frames.
    ///
    /// On this task, not the publish path: it is one pass over every symbol's latest decision,
    /// which is cheap per symbol and not something to run under the core lock once a second.
    /// The summary carries the time it was taken, so a client shows it as a snapshot rather than
    /// as live state.
    pub(crate) fn refresh_readiness(&self) {
        let at = crate::observe::wall_now();
        let warmups = self.warmup_stats();
        let mut r = {
            let core = self.0.core.lock().expect("core lock");
            match core.context.as_ref() {
                Some(cx) => cx.readiness(at),
                None => return,
            }
        };
        r.warmups = warmups;
        *self.0.readiness.lock().expect("readiness lock") = Some(r);
    }

    /// The last readiness summary taken (logs and tests).
    pub(crate) fn readiness(&self) -> Option<crate::context::Readiness> {
        self.0.readiness.lock().expect("readiness lock").clone()
    }

    /// Asks the archive maintainer to warm up the symbols whose evidence a warmup could supply.
    ///
    /// This is the half of the warmup path that was missing: `market_archive::warmup` and
    /// `market_providers::warmup::WarmupQueue` existed and were tested, the maintainer consumed
    /// the inbox, and nothing ever filed a request — so a stock that entered the band intraday
    /// stayed gain/RVOL UNKNOWN all day however strongly it traded.
    ///
    /// Bounded and off both hot paths: the candidates are read from the latest decisions under
    /// the core lock (in memory, `WARMUP_SCAN_MAX` at a time), the requests are buffered without
    /// blocking, and the inbox is written here, on the evidence task's blocking thread. Drops,
    /// contention and flush failures are counted, not assumed away; a failed flush leaves the
    /// requests buffered for the next tick.
    pub(crate) fn request_warmups(&self) {
        let Some(ev) = self.0.evidence.get() else {
            return;
        };
        let Some(day) = ev.log.lock().expect("evidence log lock").day() else {
            return;
        };
        // What has already been asked for today, read off the core lock: the candidate scan
        // skips these **before** it takes its budget, so an unresolved first batch cannot block
        // the symbols behind it (and the queue's dedup is never consulted under the core lock).
        let asked = ev.warmups.asked_for(day);
        let missing = {
            let core = self.0.core.lock().expect("core lock");
            match core.context.as_ref() {
                Some(cx) => cx.missing_evidence(WARMUP_SCAN_MAX, &asked),
                None => return,
            }
        };
        let now = Utc::now();
        let mut s = WarmupStats {
            candidates: missing.len() as u64,
            ..WarmupStats::default()
        };
        for (symbol, reason) in &missing {
            match ev
                .warmups
                .request(symbol, day, &format!("server:{reason}"), now)
            {
                Enqueue::Queued => s.queued += 1,
                Enqueue::Known => s.known += 1,
                Enqueue::Full => s.full += 1,
                Enqueue::Busy => s.busy += 1,
            }
        }
        match ev.warmups.flush(&ev.archive) {
            Ok(n) => s.filed += n as u64,
            Err(e) => {
                s.flush_errors += 1;
                tracing::warn!(day = %day, error = %e, "warmup requests: the inbox write failed; they stay buffered");
            }
        }
        let total = {
            let mut t = ev.stats.lock().expect("warmup stats lock");
            t.candidates += s.candidates;
            t.queued += s.queued;
            t.known += s.known;
            t.full += s.full;
            t.busy += s.busy;
            t.filed += s.filed;
            t.flush_errors += s.flush_errors;
            t.pending = ev.warmups.pending();
            t.tracked = ev.warmups.tracked();
            t.rolled = ev.warmups.rolled();
            *t
        };
        if s.queued > 0 || s.filed > 0 || s.full > 0 || s.flush_errors > 0 {
            tracing::info!(
                day = %day,
                candidates = s.candidates,
                queued = s.queued,
                filed = s.filed,
                known = s.known,
                dropped_queue_full = s.full,
                busy = s.busy,
                dropped_total = total.full,
                filed_total = total.filed,
                pending = total.pending,
                tracked = total.tracked,
                dedup_dates_rolled = total.rolled,
                "warmup requests for missing evidence"
            );
        }
    }

    /// Everything the warmup requester has done since start (logs and tests).
    pub(crate) fn warmup_stats(&self) -> WarmupStats {
        self.0
            .evidence
            .get()
            .map(|ev| *ev.stats.lock().expect("warmup stats lock"))
            .unwrap_or_default()
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
        // The News pending cards (crate::pending) read the same stories as they land: each id
        // once, so a new log generation re-delivering the day costs a set lookup per item.
        if core.pending.enabled() && !batch.news.is_empty() {
            let now = crate::observe::wall_now() / 1_000_000;
            core.pending.on_news(
                batch.news.iter().filter_map(crate::pending::hit_from_item),
                now,
            );
        }
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
    use market_events::session::{et, is_trading_day};
    use market_events::{MarketEvent, Trade};
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
        // The 03:30 ET pre-session check and the 09:00 re-check: the maintainer makes one every
        // 30 minutes through the session (`refresh_every_secs`), and the coverage policy only
        // lets a "no known action" finding stand for 90 minutes, so a day with the pre-session
        // check alone is an outage, not the normal case.
        write_action_check(a, t(3, 30, 0));
        write_action_check(a, t(9, 0, 0));
    }

    /// One fully paginated announcement check at `at` (ET ns) that found nothing for either
    /// symbol.
    fn write_action_check(a: &Archive, at: i64) {
        let policy = CoveragePolicy::v1();
        let (start, end) = policy.process_window(dd());
        a.write_action_retrieval(&ActionRetrieval {
            schema: 1,
            policy: policy.tag.clone(),
            retrieved_at: utc(at),
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

    fn loader(a: &Archive) -> EvidenceLoader {
        EvidenceLoader::new(a.clone(), Price::from_cents(2500), false)
    }

    /// One refresh of `l` at the run clock `now`, with whatever it produced handed to the board.
    /// The replay's `as_of` is its own clock, so the time passed here is the point of the
    /// simulated day the board is at (module docs, "as of"). A run keeps **one** loader: a new
    /// one restarts the log's sequence, which a board's cursor would then skip past.
    fn deliver(
        l: &mut EvidenceLoader,
        log: &Mutex<EvidenceLog>,
        cx: &mut ContextBoard,
        now: DateTime<Utc>,
    ) -> RefreshReport {
        let rep = l.refresh(dd(), now, log);
        if let Some(b) = log.lock().unwrap().since(cx.evidence_cursor()) {
            cx.add_evidence(b);
        }
        rep
    }

    /// One refresh just before the open, where the 09:00 re-check is the latest one.
    fn load(a: &Archive, log: &Mutex<EvidenceLog>, cx: &mut ContextBoard) -> RefreshReport {
        deliver(&mut loader(a), log, cx, utc(t(9, 29, 0)))
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

    /// A cancel of `(symbol, seq)` as the provider publishes it: a reference, the original price
    /// and size, and no execution time.
    fn cancel_event(symbol: &str, seq: u64, cents: i64, size: u64, at: i64) -> MarketEvent {
        MarketEvent::Cancel(market_events::Cancel {
            symbol: symbol.into(),
            event_ts: UnixNanos(at),
            receive_ts: None,
            source: Default::default(),
            exchange: String::new(),
            tape: Default::default(),
            trade_id: seq,
            price: Price::from_cents(cents),
            size,
            action: market_events::CancelAction::Cancel,
        })
    }

    fn state(cx: &ContextBoard, sym: &str, p: PillarId) -> (Tri, &'static str) {
        let d = cx.decision(sym).expect("a decision");
        let r = d.pillars.iter().find(|x| x.pillar == p).unwrap();
        (r.state, r.reason)
    }

    /// The Ross row chart (owner, 2026-10-21; `crate::day_line`): a member's row carries its day
    /// line of 30 s closes from the day's first price to the last completed interval, the prior
    /// close as the baseline, and an update after more trading carries only the new points.
    #[test]
    fn a_member_row_carries_its_day_line_to_the_last_completed_30s_close() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        write_daily_world(&a);
        write_collected(&a, t(7, 0, 0));
        let log = Mutex::new(EvidenceLog::default());
        let mut cx = board();
        load(&a, &log, &mut cx);
        // The chart engine sees the same prints the board does (the market feeds both).
        let mut charts = chart_engine::ChartEngine::default();
        let feed = |charts: &mut chart_engine::ChartEngine, from: i64, to: i64, cents: i64| {
            let mut x = from;
            let mut n = from as u64;
            while x < to {
                n += 1;
                charts.apply(&MarketEvent::Trade(Trade::new(
                    SYM,
                    UnixNanos(x),
                    Price::from_cents(cents),
                    120_000,
                    n,
                )));
                x += 10 * SEC;
            }
            charts.advance_clock(UnixNanos(to));
        };
        trade(&mut cx, t(9, 30, 0), t(9, 32, 0));
        feed(&mut charts, t(9, 30, 0), t(9, 31, 0), 300);
        // The last completed minute closes higher; a print in the forming 09:32:00 interval is
        // not a close yet.
        feed(&mut charts, t(9, 31, 0), t(9, 32, 0), 310);
        charts.apply(&MarketEvent::Trade(Trade::new(
            SYM,
            UnixNanos(t(9, 32, 5)),
            Price::from_cents(320),
            100,
            9_999,
        )));
        charts.advance_clock(UnixNanos(t(9, 32, 10)));
        let mut closes_file = BTreeMap::new();
        closes_file.insert(
            dd().pred_opt().unwrap(),
            BTreeMap::from([(SYM.to_string(), 2.0)]),
        );
        let (closes, _) = PrevCloses::new(&closes_file);
        let src = crate::day_line::Source {
            engine: &charts,
            closes: &closes,
        };

        let frame: serde_json::Value =
            serde_json::from_str(&cx.panel_frame_with("ross.snapshot", None, Some(src))).unwrap();
        let row = &frame["panel"]["rows"][0];
        assert_eq!(row["symbol"], SYM, "{frame}");
        assert_eq!(row["day_line_start"], t(9, 30, 0));
        assert_eq!(row["day_line_step_s"], 30);
        assert_eq!(row["day_line_from"], 0);
        assert_eq!(
            row["day_line"],
            serde_json::json!([3_000_000, 3_000_000, 3_100_000, 3_100_000]),
            "ends at the last completed 30 s close, 09:31:30–09:32:00"
        );
        assert_eq!(row["prev_close"], 2_000_000);
        // The engine's 20-close spark is still there for older clients.
        assert!(row["spark"].is_array());

        // An update: the first one carries the whole line (nothing published yet)…
        let (first, _) = cx
            .take_update_with(None, Some(src))
            .expect("a first update");
        let first: serde_json::Value = serde_json::from_str(&first).unwrap();
        assert_eq!(first["panel"]["rows"][0]["day_line_from"], 0);
        // …and after more trading only the points since.
        trade(&mut cx, t(9, 32, 0), t(9, 33, 0));
        feed(&mut charts, t(9, 32, 10), t(9, 33, 0), 330);
        let src = crate::day_line::Source {
            engine: &charts,
            closes: &closes,
        };
        let (next, _) = cx.take_update_with(None, Some(src)).expect("a new barrier");
        let next: serde_json::Value = serde_json::from_str(&next).unwrap();
        let row = &next["panel"]["rows"][0];
        assert_eq!(row["day_line_from"], 4);
        assert_eq!(row["day_line"], serde_json::json!([3_300_000, 3_300_000]));
    }

    /// pillars-v5 (owner, 2026-11-22): FMP returned no float for XYZ but did state its shares
    /// outstanding, under 20M, so the float cannot be 20M or more and XYZ is admitted. The row
    /// carries the outstanding figure in its own field, never as the float.
    #[test]
    fn a_null_float_with_outstanding_below_the_max_is_admitted_under_v5() {
        // A later retrieval of XYZ with no float row but 8M outstanding, after a usable one.
        let dir2 = tempfile::tempdir().unwrap();
        let b = archive(dir2.path());
        write_daily_world(&b);
        write_collected(&b, t(7, 0, 0));
        let mut null = float(SYM, 1, t(7, 30, 0));
        null.float_shares = None;
        null.outstanding_shares = Some(8_000_000);
        null.free_float_pct = None;
        b.append_float(&[null]).unwrap();
        let log = Mutex::new(EvidenceLog::default());
        let mut cx = board();
        let rep = load(&b, &log, &mut cx);
        assert!(rep.outstanding >= 1, "{rep:?}");
        trade(&mut cx, t(9, 30, 0), t(9, 32, 0));
        // The usable 07:00 float (5M) still decides: a fallback is only for no usable float.
        assert_eq!(state(&cx, SYM, PillarId::Float), (Tri::Pass, "below_max"));

        // Now an archive where XYZ never had a usable float.
        let dir3 = tempfile::tempdir().unwrap();
        let c = archive(dir3.path());
        write_daily_world(&c);
        write_collected(&c, t(7, 0, 0));
        // Re-write the float file with XYZ null (the collector's `no_row` record).
        let float_dir = c.evidence_dir().join("float");
        std::fs::remove_dir_all(&float_dir).unwrap();
        let mut null = float(SYM, 1, t(7, 0, 0));
        null.float_shares = None;
        null.outstanding_shares = Some(8_000_000);
        null.free_float_pct = None;
        c.append_float(&[null, float(OTHER, 4_000_000, t(7, 0, 0))])
            .unwrap();
        let log = Mutex::new(EvidenceLog::default());
        let mut cx = board();
        load(&c, &log, &mut cx);
        trade(&mut cx, t(9, 30, 0), t(9, 32, 0));
        assert_eq!(
            state(&cx, SYM, PillarId::Float),
            (Tri::Pass, "below_max_via_outstanding")
        );
        let frame: serde_json::Value =
            serde_json::from_str(&cx.panel_frame("ross.snapshot", None)).expect("json");
        let rows = frame["panel"]["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 1, "{frame}");
        assert_eq!(rows[0]["symbol"], SYM);
        assert!(rows[0]["float_shares"].is_null());
        assert_eq!(rows[0]["outstanding_shares"], 8_000_000);
    }

    /// A symbol passing price, gain and RVOL whose float is unknown is not a member, and is
    /// listed as near-qualifying with the pillar named; one that FAILS a pillar is not.
    #[test]
    fn readiness_lists_near_qualifying_symbols_with_the_unknown_pillar() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        write_daily_world(&a);
        write_collected(&a, t(7, 0, 0));
        let float_dir = a.evidence_dir().join("float");
        std::fs::remove_dir_all(&float_dir).unwrap();
        // XYZ: no float and no outstanding figure; QUIET keeps its float.
        a.append_float(&[float(OTHER, 4_000_000, t(7, 0, 0))])
            .unwrap();
        let log = Mutex::new(EvidenceLog::default());
        let mut cx = board();
        load(&a, &log, &mut cx);
        trade(&mut cx, t(9, 30, 0), t(9, 32, 0));
        assert_eq!(state(&cx, SYM, PillarId::Float).0, Tri::Unknown);
        let r = cx.readiness(t(9, 32, 5));
        assert_eq!(r.members, 0, "{r:?}");
        assert_eq!(r.near_total, 1, "{:?}", r.near);
        let n = &r.near[0];
        assert_eq!(n.symbol, SYM);
        assert_eq!(n.gain_bp, Some(5_000));
        assert_eq!(n.unknown.len(), 1);
        assert_eq!(n.unknown[0].pillar, "float");
        assert_eq!(n.unknown[0].reason, "no_float_evidence");
        // QUIET fails gain: never near.
        assert!(r.near.iter().all(|x| x.symbol != OTHER));
        let frame: serde_json::Value =
            serde_json::from_str(&cx.panel_frame("ross.snapshot", Some(&r))).expect("json");
        assert_eq!(
            frame["readiness"]["near"][0]["unknown"][0]["pillar"],
            "float"
        );
        assert_eq!(frame["readiness"]["near_total"], 1);
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
        assert!(d.policy.starts_with("pillars-v6@"), "{}", d.policy);
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
            serde_json::from_str(&cx.panel_frame("ross.snapshot", None)).expect("json");
        assert!(
            frame["pillarPolicy"]
                .as_str()
                .unwrap()
                .starts_with("pillars-v6@")
        );
        let panel = &frame["panel"];
        assert!(
            panel["setup_policy"]
                .as_str()
                .unwrap()
                .starts_with("detection-v2@"),
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
        assert_eq!(rows[0]["setup"]["family"], "micro_pullback");
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
            serde_json::from_str(&cx.panel_frame("ross.snapshot", None)).unwrap();
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
    fn float_lookback_covers_the_policy() {
        let window = stock_context::Policy::current().body.float.stale_after_secs;
        assert!(FLOAT_LOOKBACK.num_seconds() >= window);
    }

    /// pillars-v4 keeps a float for 4 days, so the loader must hand the board one retrieved 3
    /// days before the session (the 2-day [`LOOKBACK`] used to drop it), but not one from 6.
    #[test]
    fn a_three_day_old_float_reaches_the_board() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        const DAY: i64 = 86_400 * SEC;
        a.append_float(&[
            float("OLD", 4_000_000, t(8, 0, 0) - 6 * DAY),
            float(SYM, 5_000_000, t(8, 0, 0) - 3 * DAY),
        ])
        .unwrap();
        let log = Mutex::new(EvidenceLog::default());
        let mut loader = EvidenceLoader::new(a.clone(), Price::from_cents(2500), false);
        loader.refresh(dd(), utc(t(23, 0, 0)), &log);
        let all = log.lock().unwrap().since(None).unwrap();
        let got: Vec<_> = all.floats.iter().map(|f| &f.instrument).collect();
        assert_eq!(got, [&stock_context::InstrumentId::symbol(SYM)]);
    }

    /// Record-only FMP news stays in the archive: the loader never hands it to the engine, while a
    /// counting FMP item (FMP_NEWS_COUNT=1) goes through the seam like any wire release.
    #[test]
    fn record_only_fmp_news_never_reaches_the_engine() {
        use market_providers::import::fmp_news::RULES;
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        write_daily_world(&a);
        write_collected(&a, t(7, 0, 0));
        let log = Mutex::new(EvidenceLog::default());
        let mut loader = EvidenceLoader::new(a.clone(), Price::from_cents(2500), false);
        let now = utc(t(23, 0, 0));
        loader.refresh(dd(), now, &log);
        let cursor = log
            .lock()
            .unwrap()
            .since(None)
            .map(|b| (b.generation, b.through));
        let rec = |id: &str, kind: &str, et: Option<&str>| SourceNewsRecord {
            v: 1,
            provider: "press_wire".into(),
            source: "fmp/prnewswire".into(),
            id: id.into(),
            version: 1,
            symbols: vec![SYM.into()],
            published_at: t(8, 0, 0),
            first_observed_at: t(8, 1, 0),
            headline: Some("XYZ Announces Positive Topline Results".into()),
            url: None,
            kind: kind.into(),
            event_type: et.map(str::to_string),
            rules: RULES.into(),
        };
        a.append_source_news(&[
            rec("fmp:shadow", "recorded", Some("shadow_catalyst")),
            rec("fmp:counted", "catalyst", None),
        ])
        .unwrap();
        loader.refresh(dd(), now, &log);
        let batch = log.lock().unwrap().since(cursor).expect("the counted item");
        let ids: Vec<&str> = batch.news.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, ["news:press_wire:fmp:counted"]);
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
        // The float and its shares-outstanding figure (pillars-v5's fallback reads the latter).
        assert_eq!((more.floats, more.outstanding), (1, 1), "{more:?}");
        let b = log.lock().unwrap().since(cursor).unwrap();
        assert_eq!(b.floats.len(), 1);
        assert_eq!((b.floats[0].shares, b.floats[0].version), (6_000_000, 2));
        // A board built after a seek has no cursor: it gets the whole day.
        assert_eq!(
            log.lock().unwrap().since(None).unwrap().len(),
            all.len() + 2
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
            serde_json::from_str(&cx.panel_frame("ross.snapshot", None)).expect("json");
        assert_eq!(
            frame["panel"]["rows"].as_array().unwrap().len(),
            1,
            "{frame}"
        );
        assert!(
            cx.take_update(None).is_some(),
            "the first update after a drain still goes out"
        );
        assert!(
            cx.take_update(None).is_none(),
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
        assert!(cx.take_update(None).is_some());
    }

    /// The drain's stage timestamp must be read when each snapshot is captured, inside the core
    /// lock, not hoisted before `core.lock()`.
    ///
    /// Codex's review of the first slice caught the hoisted sample: it leaves the time spent
    /// waiting for the core lock out of barrier-to-drain, so the segment under-reports exactly
    /// when contention makes it worth measuring. The capture callback is the seam, and it runs
    /// under the lock, so reading the clock inside it includes the wait by construction. A test
    /// clock that advances on every read proves the read happens per capture rather than once
    /// for the whole drain.
    #[test]
    fn the_drain_stage_time_is_read_per_capture_not_hoisted() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        write_daily_world(&a);
        write_collected(&a, t(7, 0, 0));
        let log = Mutex::new(EvidenceLog::default());
        let mut cx = board();
        load(&a, &log, &mut cx);
        trade(&mut cx, t(9, 30, 0), t(9, 40, 0));

        let reads = std::cell::Cell::new(0i64);
        let clock = || {
            reads.set(reads.get() + 1);
            reads.get()
        };
        let mut stamped: Vec<i64> = Vec::new();
        let took = cx.drain_outputs(&mut |_p| stamped.push(clock()));
        assert!(took.panels > 1, "several snapshots to stamp: {took:?}");
        assert_eq!(stamped.len(), took.panels);
        assert_eq!(
            stamped,
            (1..=took.panels as i64).collect::<Vec<_>>(),
            "one clock read per captured snapshot, in order"
        );
    }

    /// The outage the audit found, end to end on the server: the maintainer stops checking, the
    /// assumption behind the gain and RVOL pillars runs out, and the verdict becomes UNKNOWN
    /// **with no file changing on disk**. A fresh check then restores it.
    ///
    /// `coverage_for` already turned a check older than the policy's freshness into
    /// `StaleCoverage`, but only at build time, and the build was skipped while the input mtimes
    /// were unchanged (`ross-panel-v1.md` acknowledges the last provider check staying in use if
    /// the maintainer stops). Two things fix it: the record carries the horizon its assumption is
    /// good to, and the build is redone when the clock alone can change the answer.
    #[test]
    fn an_action_assumption_expires_on_the_clock_and_a_fresh_check_restores_it() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        write_daily_world(&a);
        write_collected(&a, t(7, 0, 0));
        let log = Mutex::new(EvidenceLog::default());
        let mut cx = board();

        let mut l = loader(&a);
        // 09:29: the 09:00 check is 29 minutes old, so the assumption holds and XYZ is 5/5.
        deliver(&mut l, &log, &mut cx, utc(t(9, 29, 0)));
        trade(&mut cx, t(9, 30, 0), t(9, 32, 0));
        assert_eq!(state(&cx, SYM, PillarId::Gain).0, Tri::Pass);
        assert_eq!(state(&cx, SYM, PillarId::Rvol).0, Tri::Pass);
        assert_eq!(cx.decision(SYM).unwrap().counts.pass, 5);
        let expired_at =
            utc(t(9, 0, 0)) + Duration::seconds(CoveragePolicy::v1().body.freshness_secs);

        // Nobody checks again. The decisions run past 10:30 on the engine's own clock, with no
        // new record of any kind delivered.
        trade(&mut cx, t(10, 25, 0), t(10, 40, 0));
        assert_eq!(
            state(&cx, SYM, PillarId::Gain),
            (Tri::Unknown, "actions_coverage_expired"),
            "the assumption ran out at {expired_at}"
        );
        assert_eq!(
            state(&cx, SYM, PillarId::Rvol),
            (Tri::Unknown, "actions_coverage_expired")
        );
        let d = cx.decision(SYM).unwrap();
        assert_ne!(
            d.qualification,
            stock_context::Qualification::Qualified,
            "a stale assumption is not a qualification"
        );
        assert_eq!(
            d.counts.fail, 0,
            "UNKNOWN, never a FAIL: nothing was ruled out"
        );

        // The maintainer comes back at 10:35 and the loader, now past the horizon, rebuilds.
        write_action_check(&a, t(10, 35, 0));
        let rep = deliver(&mut l, &log, &mut cx, utc(t(10, 36, 0)));
        assert!(rep.rebuilt, "{rep:?}");
        assert!(rep.closes > 0 && rep.baselines > 0, "{rep:?}");
        trade(&mut cx, t(10, 41, 0), t(10, 43, 0));
        let d = cx.decision(SYM).unwrap().clone();
        assert_eq!(
            state(&cx, SYM, PillarId::Gain),
            (Tri::Pass, "at_or_above_min"),
            "{d:#?}"
        );
        assert_eq!(state(&cx, SYM, PillarId::Rvol).0, Tri::Pass, "{d:#?}");
        assert_eq!(d.counts.pass, 5);
    }

    /// The build is redone when time alone can change the answer, and skipped when it cannot:
    /// the clock reaching a check the last build could not read, and a coverage window running
    /// out with no newer check. Neither touches a file, so the mtime stamp cannot see either.
    #[test]
    fn baselines_are_rebuilt_when_the_clock_alone_changes_the_answer() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        write_daily_world(&a);
        let log = Mutex::new(EvidenceLog::default());
        let mut loader = EvidenceLoader::new(a.clone(), Price::from_cents(2500), false);

        // 04:00: only the 03:30 check is readable, and the 09:00 one is known to be ahead.
        let rep = loader.refresh(dd(), utc(t(4, 0, 0)), &log);
        assert!(rep.rebuilt, "the first build: {rep:?}");
        assert!(rep.closes > 0, "{rep:?}");

        // 04:05: same files, the same readable 03:30 check, still inside its 90 minutes, and the
        // 09:00 check still ahead: nothing can have changed, so nothing is redone.
        let rep = loader.refresh(dd(), utc(t(4, 5, 0)), &log);
        assert!(!rep.rebuilt, "no reason to rebuild: {rep:?}");

        // 05:01: the 03:30 check has run out of freshness and no newer one is readable yet, so
        // the same files now give a different answer.
        let rep = loader.refresh(dd(), utc(t(5, 1, 0)), &log);
        assert!(rep.rebuilt, "the window ran out: {rep:?}");
        // The record it issues is honest about it: a raw close whose corporate actions are not
        // checked through the evaluated date, which the gain pillar refuses.
        let through = log
            .lock()
            .unwrap()
            .closes
            .values()
            .filter_map(|(_, c)| c.actions_checked_through)
            .max();
        assert!(
            through.is_none_or(|x| x < dd()),
            "a stale check cannot reach the evaluated date, got {through:?}"
        );

        // 09:01: the clock has reached the 09:00 check the earlier builds could not read.
        let rep = loader.refresh(dd(), utc(t(9, 1, 0)), &log);
        assert!(rep.rebuilt, "a check became readable: {rep:?}");
        assert!(rep.closes > 0, "and it produced a usable close: {rep:?}");
    }

    /// A replayed day reads evidence as of the point of the day it is at, never later: the 09:00
    /// check is invisible to a board replaying 04:00 and visible to one replaying 09:30.
    #[test]
    fn a_replay_sees_the_checks_a_live_run_would_have_had_by_then() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        write_daily_world(&a);
        let log = Mutex::new(EvidenceLog::default());
        let mut early = EvidenceLoader::new(a.clone(), Price::from_cents(2500), false);
        early.refresh(dd(), utc(t(4, 0, 0)), &log);
        let at_four = log
            .lock()
            .unwrap()
            .closes
            .values()
            .map(|(_, c)| c.actions.coverage_at)
            .max()
            .flatten();
        assert_eq!(
            at_four,
            Some(t(3, 30, 0)),
            "at 04:00 the latest readable check is the pre-session one"
        );

        let log = Mutex::new(EvidenceLog::default());
        let mut later = EvidenceLoader::new(a.clone(), Price::from_cents(2500), false);
        later.refresh(dd(), utc(t(9, 30, 0)), &log);
        let at_open = log
            .lock()
            .unwrap()
            .closes
            .values()
            .map(|(_, c)| c.actions.coverage_at)
            .max()
            .flatten();
        assert_eq!(
            at_open,
            Some(t(9, 0, 0)),
            "at 09:30 the 09:00 re-check is readable, as it was for a live 09:30"
        );
    }

    /// `XYZ` is on the watch list, so it has evidence. A symbol that entered the band intraday
    /// has none, and nothing used to ask for any: `market_archive::warmup` and `WarmupQueue`
    /// existed and were tested, the maintainer consumed the inbox, and no request was ever filed
    /// (the audit's F2). The requester now files one, bounded, with the reason on it.
    #[test]
    fn a_symbol_missing_evidence_is_asked_for_by_name_and_only_once() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        write_daily_world(&a);
        write_collected(&a, t(7, 0, 0));
        let log = Mutex::new(EvidenceLog::default());
        let mut cx = board();
        load(&a, &log, &mut cx);
        trade(&mut cx, t(9, 30, 0), t(9, 32, 0));

        // NEWW trades inside the band with no stored evidence of any kind.
        let closes = PrevCloses::default();
        let counted = Counted {
            ohlc: true,
            volume: true,
        };
        let mut x = t(9, 32, 0);
        for n in 0..6 {
            let tr = Trade::new("NEWW", UnixNanos(x), Price::from_cents(300), 50_000, n);
            cx.apply_trade(&tr, counted, &closes);
            x += 10 * SEC;
        }
        cx.advance(UnixNanos(t(9, 34, 0)));

        let missing = cx.missing_evidence(WARMUP_SCAN_MAX, &std::collections::BTreeSet::new());
        assert!(
            missing
                .iter()
                .any(|(s, r)| s == "NEWW" && crate::context::WARMUP_REPAIRS.contains(r)),
            "the entrant is asked for: {missing:?}"
        );
        assert!(
            !missing.iter().any(|(s, _)| s == SYM),
            "a symbol with its evidence is not: {missing:?}"
        );

        // The queue files it once per symbol and date, with the reason, and the archive refuses
        // a second request for the same day even from a fresh queue (a restart).
        let q = WarmupQueue::new(WARMUP_QUEUE_CAPACITY);
        let now = utc(t(9, 34, 1));
        for (symbol, reason) in &missing {
            assert_eq!(
                q.request(symbol, dd(), &format!("server:{reason}"), now),
                Enqueue::Queued
            );
            assert_eq!(
                q.request(symbol, dd(), &format!("server:{reason}"), now),
                Enqueue::Known
            );
        }
        assert_eq!(q.flush(&a).unwrap(), missing.len());
        let pending = a.pending_warmups(dd(), 100).unwrap();
        assert!(pending.iter().any(|r| r.symbol == "NEWW"));
        let neww = pending.iter().find(|r| r.symbol == "NEWW").unwrap();
        assert!(
            neww.reason.starts_with("server:"),
            "the reason travels with it: {}",
            neww.reason
        );
        assert_eq!((neww.attempts, neww.last_error.clone()), (0, None));
        let fresh = WarmupQueue::new(WARMUP_QUEUE_CAPACITY);
        assert_eq!(
            fresh.request("NEWW", dd(), "server:again", now),
            Enqueue::Queued,
            "a restart buffers it again"
        );
        assert_eq!(
            fresh.flush(&a).unwrap(),
            0,
            "but the inbox already has it: nothing new is filed"
        );
    }

    /// A short history is not a missing one. A recent listing cannot be given 30 prior sessions
    /// by asking again, so `insufficient_baseline` is never a warmup candidate — the limit is
    /// reported, not papered over.
    #[test]
    fn a_short_history_is_not_a_warmup_candidate() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        write_daily_world(&a);
        write_collected(&a, t(7, 0, 0));
        let log = Mutex::new(EvidenceLog::default());
        let mut cx = board();
        load(&a, &log, &mut cx);
        trade(&mut cx, t(9, 30, 0), t(9, 32, 0));
        let d = cx.decision(SYM).unwrap();
        assert!(
            crate::context::warmup_reason(d).is_none(),
            "{:?}",
            d.pillars
        );
        assert!(
            !crate::context::WARMUP_REPAIRS.contains(&"insufficient_baseline"),
            "a genuinely short history is not a missing baseline"
        );
        assert!(
            !crate::context::WARMUP_REPAIRS.contains(&"actions_coverage_expired"),
            "an expired check is repaired by the maintainer's re-check, not a warmup"
        );
    }

    /// The scan is bounded: a long tail of symbols is asked for over several ticks instead of
    /// all at once, and the in-memory queue counts what it had to drop.
    #[test]
    fn the_warmup_scan_and_queue_are_bounded_with_counted_drops() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        let log = Mutex::new(EvidenceLog::default());
        let mut cx = board();
        load(&a, &log, &mut cx);
        // Twelve band entrants, none with any evidence.
        let closes = PrevCloses::default();
        let counted = Counted {
            ohlc: true,
            volume: true,
        };
        let mut x = t(9, 30, 0);
        for k in 0..12u32 {
            let sym = format!("NW{k:02}");
            for n in 0..4 {
                let tr = Trade::new(&sym, UnixNanos(x), Price::from_cents(300), 50_000, n);
                cx.apply_trade(&tr, counted, &closes);
                x += SEC;
            }
        }
        cx.advance(UnixNanos(t(9, 33, 0)));
        assert_eq!(
            cx.missing_evidence(5, &std::collections::BTreeSet::new())
                .len(),
            5,
            "the scan stops at the bound"
        );
        assert!(
            cx.missing_evidence(WARMUP_SCAN_MAX, &std::collections::BTreeSet::new())
                .len()
                >= 12
        );

        let q = WarmupQueue::new(3);
        let now = utc(t(9, 33, 1));
        for (symbol, reason) in
            cx.missing_evidence(WARMUP_SCAN_MAX, &std::collections::BTreeSet::new())
        {
            q.request(&symbol, dd(), &format!("server:{reason}"), now);
        }
        assert!(q.dropped() > 0, "a full queue drops and counts it");
        assert_eq!(q.flush(&a).unwrap(), 3, "what it held is filed");
    }

    /// An empty panel says *why*: the per-pillar counts and the commonest blocking reasons ride
    /// with every `ross` frame, so a client stops having to render "No stock at 5/5 yet" over a
    /// missing-evidence state (the audit's F10).
    #[test]
    fn the_frame_says_why_the_panel_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        // Raw bars and a split snapshot, but no corporate-action check and nothing collected:
        // gain and RVOL cannot be decided, and news has not been searched.
        let log = Mutex::new(EvidenceLog::default());
        let mut cx = board();
        load(&a, &log, &mut cx);
        trade(&mut cx, t(9, 30, 0), t(9, 32, 0));

        let r = cx.readiness(t(9, 32, 5));
        assert_eq!(r.symbols, 2, "{r:?}");
        assert_eq!((r.qualified, r.members), (0, 0), "{r:?}");
        assert_eq!(r.observed_at, t(9, 32, 5));
        let by = |name: &str| {
            r.pillars
                .iter()
                .find(|p| p.pillar == name)
                .copied()
                .expect("every pillar is reported")
        };
        assert_eq!(by("price").pass, 2, "both prices are in the band");
        assert_eq!(by("gain").unknown, 2, "{:?}", r.pillars);
        assert_eq!(by("rvol").unknown, 2, "{:?}", r.pillars);
        assert!(
            r.blocking
                .iter()
                .any(|b| b.pillar == "rvol" && b.symbols == 2),
            "the reason is named with its count: {:?}",
            r.blocking
        );
        assert!(
            r.blocking.len() <= crate::context::TOP_REASONS,
            "the list is bounded: {:?}",
            r.blocking
        );
        assert!(
            r.summary_line().contains("0 qualified"),
            "{}",
            r.summary_line()
        );

        // The frame carries it; without one it is explicitly null, never a silent omission.
        let frame: serde_json::Value =
            serde_json::from_str(&cx.panel_frame("ross.snapshot", Some(&r))).expect("json");
        assert_eq!(frame["readiness"]["symbols"], 2);
        assert_eq!(frame["readiness"]["blocking"][0]["symbols"], 2);
        assert!(
            frame["panel"].is_null() || frame["panel"]["rows"].as_array().unwrap().is_empty(),
            "no member, so no panel yet: exactly the state that needed a reason"
        );
        let bare: serde_json::Value =
            serde_json::from_str(&cx.panel_frame("ross.snapshot", None)).expect("json");
        assert!(bare["readiness"].is_null());
    }

    /// The 2026-10-30 misreading: the readiness line led with `float:no_float_evidence x7795`
    /// and was taken for a broken float feed, when almost all of those symbols were outside the
    /// band — already FAILING price, so no float figure could ever have put them on the panel.
    /// Only a symbol nothing has ruled out yet is blocked by missing evidence.
    #[test]
    fn a_symbol_another_pillar_fails_is_not_counted_as_blocked() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        let log = Mutex::new(EvidenceLog::default());
        let mut cx = board();
        load(&a, &log, &mut cx);
        // Out of the band ($0.10–$25) in numbers: price FAILS, and the evidence pillars are as
        // unknown for them as for the in-band pair.
        let closes = PrevCloses::default();
        let counted = Counted {
            ohlc: true,
            volume: true,
        };
        for (i, s) in ["BIG1", "BIG2", "BIG3"].into_iter().enumerate() {
            let tr = Trade::new(
                s,
                UnixNanos(t(9, 30, 1)),
                Price::from_cents(4000),
                1_000,
                900 + i as u64,
            );
            cx.apply_trade(&tr, counted, &closes);
        }
        trade(&mut cx, t(9, 30, 0), t(9, 32, 0));

        let r = cx.readiness(t(9, 32, 5));
        assert_eq!(r.symbols, 5, "{r:?}");
        let price = r.pillars.iter().find(|p| p.pillar == "price").unwrap();
        assert_eq!(price.fail, 3, "the three are outside the band: {r:?}");
        assert_eq!(
            r.contenders, 2,
            "only the in-band pair can still qualify: {r:?}"
        );
        for b in &r.blocking {
            assert!(
                b.symbols <= 2,
                "{}:{} counts a symbol price already rules out: {:?}",
                b.pillar,
                b.reason,
                r.blocking
            );
        }
        assert!(
            r.blocking
                .iter()
                .any(|b| b.pillar == "float" && b.reason == "no_float_evidence" && b.symbols == 2),
            "the in-band pair's missing float is still named: {:?}",
            r.blocking
        );
        assert!(
            r.summary_line().contains("2 awaiting evidence"),
            "{}",
            r.summary_line()
        );
    }

    /// With the evidence in place the same summary says so, so a reader can tell "nothing
    /// qualifies" from "nothing could be decided".
    #[test]
    fn readiness_distinguishes_nothing_qualifies_from_nothing_decided() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        write_daily_world(&a);
        write_collected(&a, t(7, 0, 0));
        let log = Mutex::new(EvidenceLog::default());
        let mut cx = board();
        load(&a, &log, &mut cx);
        trade(&mut cx, t(9, 30, 0), t(9, 32, 0));
        let r = cx.readiness(t(9, 32, 5));
        assert_eq!((r.symbols, r.qualified, r.members), (2, 1, 1), "{r:?}");
        let gain = r
            .pillars
            .iter()
            .find(|p| p.pillar == "gain")
            .copied()
            .unwrap();
        assert_eq!(
            (gain.pass, gain.fail, gain.unknown),
            (1, 1, 0),
            "QUIET fails the gain pillar on the facts, it is not undecided"
        );
    }

    /// Readiness is published on its own, over an empty panel that never changes.
    ///
    /// Codex's review of the readiness slice caught the gap: the engine publishes no panel at all
    /// while nothing qualifies, and `take_update` used to need a new panel revision, so a
    /// subscriber that connected before the first summary existed would have kept
    /// `readiness: null` for ever — exactly the state the summary was added to explain. A frame
    /// sent only because the summary moved carries the panel already published, so it is not a
    /// new detection and must not become a latency sample.
    #[test]
    fn readiness_is_published_even_when_the_panel_never_changes() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        let log = Mutex::new(EvidenceLog::default());
        let mut cx = board();
        load(&a, &log, &mut cx);
        // No evidence at all: nothing can qualify, so the engine publishes no panel ever.
        trade(&mut cx, t(9, 30, 0), t(9, 32, 0));
        assert!(
            cx.take_update(None).is_none(),
            "no panel and no readiness: nothing to send"
        );

        // The first summary goes out on its own, with no panel in it.
        let first = cx.readiness(t(9, 32, 5));
        let (frame, snap) = cx
            .take_update(Some(&first))
            .expect("a readiness-only frame");
        assert!(
            snap.is_none(),
            "no new panel, so no publish observation and no latency sample"
        );
        let json: serde_json::Value = serde_json::from_str(&frame).expect("json");
        assert_eq!(json["type"], "ross.update");
        assert_eq!(json["readiness"]["symbols"], 2);
        assert!(json["panel"].is_null(), "{json}");

        // The same summary again is not news.
        assert!(cx.take_update(Some(&first)).is_none());

        // A later summary is: the counts a client shows must keep up with the market even while
        // the panel stays empty.
        let mut later = cx.readiness(t(9, 33, 5));
        later.symbols += 1;
        let (frame, snap) = cx.take_update(Some(&later)).expect("the next summary");
        assert!(snap.is_none());
        let json: serde_json::Value = serde_json::from_str(&frame).expect("json");
        assert_eq!(json["readiness"]["observed_at"], t(9, 33, 5));
    }

    /// And when a panel does appear, the frame carries it and counts as a publication once.
    #[test]
    fn a_new_panel_is_published_once_and_a_resend_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        write_daily_world(&a);
        write_collected(&a, t(7, 0, 0));
        let log = Mutex::new(EvidenceLog::default());
        let mut cx = board();
        load(&a, &log, &mut cx);
        trade(&mut cx, t(9, 30, 0), t(9, 32, 0));

        let r = cx.readiness(t(9, 32, 5));
        let (_, snap) = cx.take_update(Some(&r)).expect("a panel frame");
        let snap = snap.expect("the panel is new, so it is observed");
        assert_eq!(snap.rows.len(), 1);
        // Nothing moved: no frame at all.
        assert!(cx.take_update(Some(&r)).is_none());
        // Only the summary moved: a frame, but the same panel, and no second observation.
        let mut again = r.clone();
        again.observed_at = t(9, 32, 6);
        let (_, resend) = cx
            .take_update(Some(&again))
            .expect("a readiness-only frame");
        assert!(
            resend.is_none(),
            "the panel in it is the one already published"
        );
    }

    /// Writes a derived intraday curve for `date` holding `shares` eligible shares from 09:30,
    /// marked as a final day collected whole so it counts as prior-session history.
    fn write_curve(a: &Archive, date: NaiveDate, symbol: &str, shares: u64, observed_at: i64) {
        let open = et(date, 9, 30, 0, 0).expect("valid").nanos();
        let four = et(date, 4, 0, 0, 0).expect("valid").nanos();
        let at = (open - four) / SEC;
        a.write_intraday_curve(&market_archive::IntradayCurveRecord {
            v: 1,
            symbol: symbol.into(),
            date,
            step_secs: 15,
            cum: vec![(0, 0), (at + 15, shares)],
            collection_complete: true,
            unusable: None,
            source: market_archive::CurveSource {
                fidelity: market_archive::CurveFidelity::ImportedHistory,
                file: format!("{symbol}.swa"),
                sha256: "00".into(),
                events: 1,
                sources: vec![],
            },
            coverage: Default::default(),
            rules: market_archive::CURVE_RULES_VERSION.into(),
            observed_at,
        })
        .expect("write curve");
    }

    /// Stored curves reach the engine as the same-time and recent-window **diagnostics**, and the
    /// RVOL pillar is untouched by them.
    ///
    /// `IntradayBaseline` had been in the engine since the pillars went in with nothing producing
    /// one, so both measures were permanently `None` (the audit's F5/F10). The loader reads the
    /// curves the maintainer derives, once per symbol per session date, bounded per tick.
    #[test]
    fn stored_intraday_curves_become_diagnostics_and_not_the_pillar() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        write_daily_world(&a);
        write_collected(&a, t(7, 0, 0));
        // Ten prior sessions, 10,000 eligible shares each by 09:30: the diagnostic's minimum.
        let mut d = dd();
        for _ in 0..10 {
            d = previous_trading_day(d);
            write_curve(&a, d, SYM, 10_000, t(4, 0, 0));
        }
        let log = Mutex::new(EvidenceLog::default());
        let mut cx = board();
        let mut l = loader(&a);
        deliver(&mut l, &log, &mut cx, utc(t(9, 29, 0)));

        // Before any curve is loaded the diagnostic is honestly unavailable.
        trade(&mut cx, t(9, 30, 0), t(9, 32, 0));
        let before = cx.decision(SYM).unwrap();
        assert_eq!(before.rvol.same_time_reason, "no_intraday_baseline");
        assert_eq!(before.rvol.same_time_milli, None);
        let pillar_before = before
            .pillars
            .iter()
            .find(|p| p.pillar == PillarId::Rvol)
            .cloned()
            .unwrap();

        // The loader reads them and the board takes them.
        let mut rep = RefreshReport::default();
        let records = l.intraday(dd(), &[SYM.to_string()], 1, utc(t(9, 29, 0)), &mut rep);
        assert_eq!(records.len(), 1, "{rep:?}");
        assert_eq!(records[0].sessions.len(), 10);
        assert!(records[0].sessions.iter().all(|s| s.complete));
        log.lock().unwrap().add_intraday(records);
        if let Some(b) = log.lock().unwrap().since(cx.evidence_cursor()) {
            cx.add_evidence(b);
        }
        trade(&mut cx, t(9, 32, 0), t(9, 34, 0));

        let after = cx.decision(SYM).unwrap();
        assert_eq!(after.rvol.same_time_sessions, 10);
        assert_eq!(after.rvol.same_time_reason, "ok", "{:?}", after.rvol);
        let same_time = after.rvol.same_time_milli.expect("a same-time measure");
        assert!(same_time > 0, "{:?}", after.rvol);

        // And the pillar is exactly what it was: same rule, same state.
        let pillar_after = after
            .pillars
            .iter()
            .find(|p| p.pillar == PillarId::Rvol)
            .cloned()
            .unwrap();
        assert_eq!(pillar_after.rule, pillar_before.rule);
        assert_eq!(pillar_after.state, pillar_before.state);
        assert!(
            pillar_after.rule.contains("daily denominator"),
            "the pillar keeps the owner's daily denominator: {}",
            pillar_after.rule
        );
    }

    /// A symbol with no stored curves gets no record, is not re-read every tick, and the engine
    /// says `insufficient_sessions` rather than inventing a number.
    #[test]
    fn a_symbol_without_curves_is_read_once_and_stays_insufficient() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        let mut l = loader(&a);
        let mut rep = RefreshReport::default();
        let syms = vec![SYM.to_string(), OTHER.to_string()];
        assert_eq!(
            l.intraday_pending(&syms, INTRADAY_SCAN_MAX, 1, utc(t(9, 29, 0))),
            syms
        );
        assert!(
            l.intraday(dd(), &syms, 1, utc(t(9, 29, 0)), &mut rep)
                .is_empty()
        );
        assert!(rep.errors.is_empty(), "{rep:?}");
        assert!(
            l.intraday_pending(&syms, INTRADAY_SCAN_MAX, 1, utc(t(9, 29, 0)))
                .is_empty(),
            "read once per session date, whatever came back"
        );

        // Curves exist but the loader has no share-basis factors for the symbol (no baseline
        // build has verified one), so every session is suppressed as incomparable rather than
        // compared across two share bases.
        let mut d = dd();
        for _ in 0..9 {
            d = previous_trading_day(d);
            write_curve(&a, d, SYM, 10_000, t(4, 0, 0));
        }
        let mut rep2 = RefreshReport::default();
        let records = loader(&a).intraday(dd(), &[SYM.to_string()], 1, utc(t(9, 29, 0)), &mut rep2);
        assert_eq!(records.len(), 1, "carried for the record");
        assert_eq!(rep2.intraday_suppressed, 9, "and every session left out");
        assert!(records[0].sessions.iter().all(|s| !s.complete));

        // Nine comparable sessions is still under the diagnostic's minimum of ten: the policy's
        // minimum is unchanged, and the engine says so instead of publishing a number.
        let basis = market_archive::intraday::Basis {
            factors: records[0].sessions.iter().map(|s| (s.date, 1.0)).collect(),
            evidence: stock_context::ActionsEvidence {
                source: stock_context::ActionsSource::BarVerified,
                ..Default::default()
            },
            observed_at: t(4, 0, 0),
            source: "fixture:baseline".into(),
        };
        let curves = {
            let mut dates = Vec::new();
            let mut x = dd();
            for _ in 0..INTRADAY_LOOKBACK_SESSIONS {
                x = previous_trading_day(x);
                dates.push(x);
            }
            a.read_intraday_curves(SYM, &dates).expect("read")
        };
        let asm = market_archive::intraday::intraday_baseline(&curves, SYM, &basis);
        assert_eq!(asm.usable, 9);
        let log = Mutex::new(EvidenceLog::default());
        let mut cx = board();
        deliver(&mut loader(&a), &log, &mut cx, utc(t(9, 29, 0)));
        log.lock()
            .unwrap()
            .add_intraday(vec![asm.baseline.expect("a baseline")]);
        if let Some(b) = log.lock().unwrap().since(cx.evidence_cursor()) {
            cx.add_evidence(b);
        }
        trade(&mut cx, t(9, 30, 0), t(9, 32, 0));
        let got = cx.decision(SYM).unwrap();
        assert_eq!(got.rvol.same_time_sessions, 9);
        assert_eq!(got.rvol.same_time_reason, "insufficient_sessions");
        assert_eq!(got.rvol.same_time_milli, None);
    }

    /// The scan is bounded and skips what it has read **before** taking its budget, so a long
    /// tail is read over several ticks rather than blocked behind whatever sorts first.
    #[test]
    fn the_intraday_scan_is_bounded_and_fair() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        let mut l = loader(&a);
        let mut rep = RefreshReport::default();
        let syms: Vec<String> = (0..250).map(|k| format!("S{k:04}")).collect();
        let first = l.intraday_pending(&syms, INTRADAY_SCAN_MAX, 1, utc(t(9, 29, 0)));
        assert_eq!(first.len(), INTRADAY_SCAN_MAX);
        l.intraday(dd(), &first, 1, utc(t(9, 29, 0)), &mut rep);
        let second = l.intraday_pending(&syms, INTRADAY_SCAN_MAX, 1, utc(t(9, 29, 0)));
        assert_eq!(second.len(), INTRADAY_SCAN_MAX);
        assert!(
            second.iter().all(|s| !first.contains(s)),
            "the next batch, not the same one again"
        );
        l.intraday(dd(), &second, 1, utc(t(9, 29, 0)), &mut rep);
        assert_eq!(
            l.intraday_pending(&syms, INTRADAY_SCAN_MAX, 1, utc(t(9, 29, 0)))
                .len(),
            50
        );
    }

    /// A curve derived later today reaches the engine on the same date.
    ///
    /// The loader used to note "read" for the whole session date, so a symbol whose curves the
    /// maintainer had not derived yet at 09:30 would have had no diagnostic until the next day
    /// however many curves appeared at 10:00. The read is noted against an input generation
    /// instead, and the generation moves when the stored curves could have changed.
    #[test]
    fn a_curve_derived_later_today_is_picked_up_on_the_same_date() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        let mut l = loader(&a);
        let mut rep = RefreshReport::default();
        let syms = vec![SYM.to_string()];
        let at = utc(t(9, 30, 0));

        // Nothing derived yet: looked at, nothing to read.
        let g0 = l.intraday_generation(dd());
        assert_eq!(l.intraday_pending(&syms, INTRADAY_SCAN_MAX, g0, at), syms);
        assert!(l.intraday(dd(), &syms, g0, at, &mut rep).is_empty());
        assert!(
            l.intraday_pending(&syms, INTRADAY_SCAN_MAX, g0, at)
                .is_empty(),
            "not re-read while nothing has changed"
        );

        // The maintainer derives the day's curves at 10:00. The input generation moves, so the
        // symbol is read again on the same date.
        let mut d = dd();
        for _ in 0..10 {
            d = previous_trading_day(d);
            write_curve(&a, d, SYM, 10_000, t(4, 0, 0));
        }
        let g1 = l.intraday_generation(dd());
        assert_ne!(g1, g0, "new curve input is a new generation");
        let later = utc(t(10, 0, 0));
        assert_eq!(
            l.intraday_pending(&syms, INTRADAY_SCAN_MAX, g1, later),
            syms,
            "the symbol comes back"
        );
        let records = l.intraday(dd(), &syms, g1, later, &mut rep);
        assert_eq!(records.len(), 1, "{rep:?}");
        assert_eq!(records[0].sessions.len(), 10);
    }

    /// A share basis verified later today re-assembles the curves that were read before it.
    ///
    /// Without factors every session is suppressed as incomparable. A baseline build that then
    /// verifies the basis has to bring the symbol back, or the curves read earlier would stay
    /// suppressed for the rest of the day although the evidence to compare them arrived.
    #[test]
    fn a_basis_verified_later_today_reassembles_the_curves_read_before_it() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        write_daily_world(&a);
        write_collected(&a, t(7, 0, 0));
        let mut d = dd();
        for _ in 0..10 {
            d = previous_trading_day(d);
            write_curve(&a, d, SYM, 10_000, t(4, 0, 0));
        }
        let mut l = loader(&a);
        let mut rep = RefreshReport::default();
        let syms = vec![SYM.to_string()];
        let at = utc(t(9, 30, 0));

        // Read before any baseline build: no factors, so every session is suppressed.
        let g0 = l.intraday_generation(dd());
        let first = l.intraday(dd(), &syms, g0, at, &mut rep);
        assert_eq!(first.len(), 1, "carried for the record");
        assert!(
            first[0].sessions.iter().all(|s| !s.complete),
            "incomparable until a basis is verified"
        );
        assert_eq!(rep.intraday_suppressed, 10);
        assert!(
            l.intraday_pending(&syms, INTRADAY_SCAN_MAX, g0, at)
                .is_empty(),
            "not re-read while nothing has changed"
        );

        // A refresh builds the daily baselines, which verifies the share basis. That is new
        // input for the diagnostic, so the generation moves and the symbol comes back.
        let log = Mutex::new(EvidenceLog::default());
        l.refresh(dd(), utc(t(9, 31, 0)), &log);
        let g1 = l.intraday_generation(dd());
        assert_ne!(g1, g0, "a new basis is a new generation");
        assert_eq!(l.intraday_pending(&syms, INTRADAY_SCAN_MAX, g1, at), syms);
        let mut rep2 = RefreshReport::default();
        let again = l.intraday(dd(), &syms, g1, utc(t(9, 31, 0)), &mut rep2);
        assert_eq!(again.len(), 1);
        assert_eq!(
            again[0].sessions.iter().filter(|s| s.complete).count(),
            10,
            "restated and comparable: {rep2:?}"
        );
        assert_eq!(rep2.intraday_suppressed, 0);
        // And the record is not visible before the basis it rests on was observed.
        assert!(
            again[0].observed_at >= t(4, 0, 0),
            "stamped with the latest of every input"
        );
    }

    /// On an unchanged generation a symbol is still retried after the backstop interval, for a
    /// filesystem whose directory stamps do not move.
    #[test]
    fn an_unchanged_generation_is_still_retried_after_the_backstop() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        let mut l = loader(&a);
        let mut rep = RefreshReport::default();
        let syms = vec![SYM.to_string()];
        let at = utc(t(9, 30, 0));
        let g = l.intraday_generation(dd());
        l.intraday(dd(), &syms, g, at, &mut rep);
        assert!(
            l.intraday_pending(&syms, INTRADAY_SCAN_MAX, g, at)
                .is_empty()
        );
        let soon = at + Duration::seconds(INTRADAY_RETRY_SECS - 1);
        assert!(
            l.intraday_pending(&syms, INTRADAY_SCAN_MAX, g, soon)
                .is_empty(),
            "not yet"
        );
        let later = at + Duration::seconds(INTRADAY_RETRY_SECS);
        assert_eq!(
            l.intraday_pending(&syms, INTRADAY_SCAN_MAX, g, later),
            syms,
            "the backstop brings it back"
        );
    }

    /// Re-reading a symbol's unchanged curves every backstop interval leaves the log the size it
    /// was, and no board is sent the same record again.
    ///
    /// The log appended every read (2026-10-29): production re-read every decided symbol every
    /// five minutes and kept every ~12-32 KiB copy until the session date ended. The live and
    /// replay markets together grew ~16 MB a minute, around the clock, until the kernel killed
    /// the process.
    #[test]
    fn rereading_unchanged_intraday_curves_does_not_grow_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        let mut d = dd();
        for _ in 0..10 {
            d = previous_trading_day(d);
            write_curve(&a, d, SYM, 10_000, t(4, 0, 0));
        }
        let mut l = loader(&a);
        let mut log = EvidenceLog::default();
        let syms = vec![SYM.to_string()];
        let g = l.intraday_generation(dd());
        let mut at = utc(t(9, 30, 0));
        let mut rep = RefreshReport::default();
        log.add_intraday(l.intraday(dd(), &syms, g, at, &mut rep));
        assert_eq!(log.intraday.len(), 1, "{rep:?}");
        let held = log.len();
        let cursor = log.since(None).map(|b| (b.generation, b.through));
        for _ in 0..100 {
            at += Duration::seconds(INTRADAY_RETRY_SECS);
            assert_eq!(l.intraday_pending(&syms, INTRADAY_SCAN_MAX, g, at), syms);
            log.add_intraday(l.intraday(dd(), &syms, g, at, &mut rep));
        }
        assert_eq!(
            log.intraday.len(),
            1,
            "100 re-reads of the same curves hold one record"
        );
        assert_eq!(log.len(), held);
        assert!(
            log.since(cursor).is_none(),
            "a board that has the record is not sent it again"
        );

        // A changed record replaces it, and is news.
        let mut changed = log.intraday[&log.intraday.keys().next().unwrap().clone()]
            .1
            .clone();
        changed.observed_at += 1;
        log.add_intraday(vec![changed.clone()]);
        assert_eq!(log.len(), held);
        let b = log.since(cursor).expect("the changed record is delivered");
        assert_eq!(b.intraday, vec![changed]);
    }

    /// A cancel reaches the scanner on **both** paths: with a chart loaded and without one.
    ///
    /// Routing it only on the loaded branch left the hole open for every symbol whose history is
    /// still loading, which is exactly the window a restart spends in. The scanner resolves the
    /// provider reference against the prints it admitted itself, so it does not need a chart.
    #[test]
    fn a_cancel_reaches_the_scanner_loaded_or_not() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        write_daily_world(&a);
        write_collected(&a, t(7, 0, 0));
        let log = Mutex::new(EvidenceLog::default());
        let mut cx = board();
        load(&a, &log, &mut cx);
        trade(&mut cx, t(9, 30, 0), t(9, 32, 0));
        // Past the last print's own barrier, so nothing is still queued.
        cx.advance(UnixNanos(t(9, 32, 30)));
        let before = cx.decision(SYM).unwrap().volume.clone();
        assert!(before.shares > 0 && before.trades > 0);

        // The cancel as the provider publishes it: a reference, a price and a size, and no
        // execution time. `trade()` printed 120,000 shares at $3.00 with `source_seq` counting
        // up; the first of SYM's prints carries seq 1.
        let raw = cancel_event(SYM, 1, 300, 120_000, t(9, 32, 31));

        // Without a chart engine answer at all (the unloaded path).
        cx.apply_revision(&raw, None, UnixNanos(t(9, 32, 31)));
        cx.advance(UnixNanos(t(9, 33, 0)));
        let after = cx.decision(SYM).unwrap().volume.clone();
        assert_eq!(
            after.shares,
            before.shares - 120_000,
            "resolved against the scanner's own admissions; telemetry {:?}",
            cx.revision_telemetry()
        );
        assert_eq!(after.trades, before.trades - 1);
        let tel = cx.revision_telemetry();
        assert_eq!(tel.applied, 1);
        assert_eq!(
            tel.day_high_unproven, 1,
            "no resolver could prove the date's high, and it is counted: {tel:?}"
        );

        // And with the chart engine's answer, which supplies the date's last price and high.
        let resolved = chart_engine::Revision {
            symbol: SYM.into(),
            ts: UnixNanos(t(9, 30, 0)),
            session: market_events::SessionId {
                date: dd(),
                kind: market_events::session::SessionKind::Regular,
            },
            removed: chart_engine::RevisedPrint {
                price: Price::from_cents(300),
                size: 120_000,
                ohlc: true,
                volume: true,
            },
            added: None,
            last: Some((Price::from_cents(300), UnixNanos(t(9, 31, 50)))),
            day_high: Some(chart_engine::DayHigh {
                price: Price::from_cents(300),
                complete: true,
            }),
        };
        let raw2 = cancel_event(SYM, 3, 300, 120_000, t(9, 33, 1));
        let before2 = cx.decision(SYM).unwrap().volume.clone();
        cx.apply_revision(&raw2, Some(&resolved), UnixNanos(t(9, 33, 1)));
        cx.advance(UnixNanos(t(9, 33, 30)));
        let after2 = cx.decision(SYM).unwrap().volume.clone();
        assert_eq!(after2.shares, before2.shares - 120_000);
        let tel = cx.revision_telemetry();
        assert_eq!(tel.applied, 2);
        assert_eq!(
            tel.day_high_unproven, 1,
            "the second one had a proven high, so nothing new was counted: {tel:?}"
        );

        // The readiness summary carries the counters, so an unrepaired revision would show.
        let r = cx.readiness(t(9, 33, 31));
        assert_eq!(r.revisions.applied, 2);
        assert!(
            r.summary_line().contains("2 applied"),
            "{}",
            r.summary_line()
        );
    }

    /// A cancel naming a print the scanner never admitted subtracts nothing at all.
    #[test]
    fn a_cancel_for_a_print_the_scanner_never_admitted_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        write_daily_world(&a);
        write_collected(&a, t(7, 0, 0));
        let log = Mutex::new(EvidenceLog::default());
        let mut cx = board();
        load(&a, &log, &mut cx);
        trade(&mut cx, t(9, 30, 0), t(9, 32, 0));
        cx.advance(UnixNanos(t(9, 32, 30)));
        let before = cx.decision(SYM).unwrap().volume.clone();

        // A print from before this process existed: the chart engine may well resolve it, but
        // the scanner never counted it, so subtracting its size would take volume off an
        // unrelated print.
        let raw = cancel_event(SYM, 9_999, 300, 120_000, t(9, 32, 31));
        cx.apply_revision(&raw, None, UnixNanos(t(9, 32, 31)));
        cx.advance(UnixNanos(t(9, 33, 0)));
        let after = cx.decision(SYM).unwrap().volume.clone();
        assert_eq!(
            (after.shares, after.trades, after.dollars),
            (before.shares, before.trades, before.dollars),
            "nothing was subtracted from an unrelated print"
        );
        // But something the provider says is not there may still be in the count — up to the
        // cancel's 120,000 shares — so the count alone is no longer a lower bound. Net of that
        // bound it still clears the threshold here, so the pillar passes on the net figure and
        // says so; the doubt itself is still reported.
        assert!(after.integrity.volume_untrusted);
        assert_eq!(
            state(&cx, SYM, PillarId::Rvol),
            (Tri::Pass, "at_or_above_min_net_of_unplaced")
        );
        let tel = cx.revision_telemetry();
        assert_eq!(tel.unknown_target, 1);
        assert_eq!(tel.applied, 0);
        assert!(!tel.complete(), "{tel:?}");
    }

    /// A print redelivered on the unloaded path is counted once, which is the path that had no
    /// duplicate exclusion at all (`Counted::from_effect`).
    #[test]
    fn a_redelivered_print_on_the_unloaded_path_is_counted_once() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        let log = Mutex::new(EvidenceLog::default());
        let mut cx = board();
        load(&a, &log, &mut cx);
        let closes = PrevCloses::default();
        let counted = Counted {
            ohlc: true,
            volume: true,
        };
        // The same print twice, with the verdict the market uses for a symbol whose history is
        // not loaded yet: its own conditions, which exclude nothing.
        let tr = Trade::new(
            "NEWW",
            UnixNanos(t(9, 30, 1)),
            Price::from_cents(300),
            5_000,
            1,
        );
        cx.apply_trade(&tr, counted, &closes);
        cx.apply_trade(&tr, counted, &closes);
        cx.advance(UnixNanos(t(9, 30, 30)));
        let d = cx.decision("NEWW").unwrap();
        assert_eq!(d.volume.shares, 5_000, "counted once, not twice");
        assert_eq!(d.volume.trades, 1);
        assert_eq!(cx.revision_telemetry().duplicates_dropped, 1);
    }

    /// SYM's floats and the market-wide Benzinga search, but no Benzinga story: only what the test
    /// adds can pass its news pillar.
    fn write_without_news(a: &Archive) {
        a.append_float(&[
            float(SYM, 5_000_000, t(7, 0, 0)),
            float(OTHER, 4_000_000, t(7, 0, 0)),
        ])
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
            stories: 0,
        }])
        .unwrap();
    }

    fn filing(acc: &str, form: &str, accepted: i64, observed: i64) -> market_archive::FilingRecord {
        market_archive::FilingRecord {
            v: 1,
            provider: "sec_edgar".into(),
            source: "current_feed".into(),
            accession: acc.into(),
            cik: 1234567,
            company: "XYZ CORP".into(),
            form: form.into(),
            accepted_at: None,
            accepted_at_ns: accepted,
            dated_by: market_archive::DATED_BY_ACCEPTANCE.into(),
            filed: Some(dd()),
            items: vec![],
            symbols: vec![SYM.into()],
            url: format!("https://www.sec.gov/Archives/edgar/data/1234567/{acc}-index.htm"),
            first_observed_at: observed,
        }
    }

    #[test]
    fn an_sec_8k_in_the_archive_passes_the_news_pillar_and_the_row_names_it() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        write_daily_world(&a);
        write_without_news(&a);
        a.append_filings(&[
            filing("0001234567-26-000001", "8-K", t(7, 12, 0), t(7, 12, 40)),
            filing("0001234567-26-000002", "424B5", t(7, 40, 0), t(7, 40, 30)),
        ])
        .unwrap();
        let log = Mutex::new(EvidenceLog::default());
        let mut cx = board();
        let rep = load(&a, &log, &mut cx);
        assert!(rep.errors.is_empty(), "{:?}", rep.errors);
        assert_eq!(rep.news, 2, "both filings, as news items");
        trade(&mut cx, t(9, 30, 0), t(9, 32, 0));
        assert_eq!(state(&cx, SYM, PillarId::News).0, Tri::Pass);
        let frame: serde_json::Value =
            serde_json::from_str(&cx.panel_frame("ross.snapshot", None)).expect("json");
        let row = &frame["panel"]["rows"][0];
        assert_eq!(row["symbol"], SYM);
        assert_eq!(row["news_evidence"]["source"], "sec_edgar");
        assert_eq!(row["news_evidence"]["form"], "8-K");
        assert_eq!(row["news_evidence"]["published_at"], t(7, 12, 0));
        assert_eq!(row["offering"]["form"], "424B5");
        assert!(row.get("news_unconfirmed").is_none());
        assert_eq!(frame["panel"]["news_optional"], false);
    }

    #[test]
    fn with_news_optional_a_symbol_without_news_is_a_member_and_leaves_the_near_list() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        write_daily_world(&a);
        write_without_news(&a);
        let log = Mutex::new(EvidenceLog::default());
        let mut cx = board();
        load(&a, &log, &mut cx);
        trade(&mut cx, t(9, 30, 0), t(9, 32, 0));
        // News required: four pass, the Benzinga search found nothing (FAIL): not a member, and
        // not near either (a FAIL keeps a symbol out of that list).
        assert_eq!(state(&cx, SYM, PillarId::News).0, Tri::Fail);
        assert!(cx.decision(SYM).unwrap().membership.active().is_none());
        assert!(
            !cx.readiness(t(9, 32, 5))
                .summary_line()
                .contains("news optional")
        );

        // The owner's switch off: the next barrier admits it on the other four.
        assert!(cx.set_news_required(false));
        trade(&mut cx, t(9, 32, 0), t(9, 32, 30));
        let d = cx.decision(SYM).unwrap();
        assert!(d.membership.active().is_some(), "{:?}", d.membership);
        let r = cx.readiness(t(9, 32, 35));
        assert!(r.news_optional && r.summary_line().contains("(news optional)"));
        assert!(r.near.iter().all(|n| n.symbol != SYM));
        let frame: serde_json::Value =
            serde_json::from_str(&cx.panel_frame("ross.snapshot", Some(&r))).expect("json");
        assert_eq!(frame["panel"]["news_optional"], true);
        assert!(
            frame["panelPolicy"]
                .as_str()
                .unwrap()
                .starts_with("ross-panel-v2@")
        );
        let row = &frame["panel"]["rows"][0];
        assert_eq!(row["news_unconfirmed"], true);
        assert!(row.get("news_evidence").is_none());
    }

    #[test]
    fn the_news_switch_reads_the_detectors_document_like_the_clients() {
        use crate::context::news_required;
        use serde_json::json;
        assert!(news_required(None), "absent: required (today's rule)");
        assert!(news_required(Some(&json!({}))));
        assert!(!news_required(Some(
            &json!({"ross-require-news": {"on": false}})
        )));
        assert!(!news_required(Some(&json!({"ross-require-news": false}))));
        assert!(news_required(Some(
            &json!({"ross-require-news": {"on": true}})
        )));
        assert!(
            news_required(Some(&json!({"ross-require-news": "off"}))),
            "a bad shape is the default"
        );
    }

    #[test]
    fn near_rows_ignore_the_news_pillar_when_it_is_optional() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        write_daily_world(&a);
        // No float and no news searched: float and news UNKNOWN.
        let log = Mutex::new(EvidenceLog::default());
        let mut cx = board();
        load(&a, &log, &mut cx);
        trade(&mut cx, t(9, 30, 0), t(9, 32, 0));
        let d = cx.decision(SYM).unwrap().clone();
        let req = crate::context::near_row(&d, false).expect("near with news required");
        assert_eq!(
            req.unknown.iter().map(|u| u.pillar).collect::<Vec<_>>(),
            vec!["float", "news"]
        );
        let opt = crate::context::near_row(&d, true).expect("still near on its float");
        assert_eq!(
            opt.unknown.iter().map(|u| u.pillar).collect::<Vec<_>>(),
            vec!["float"]
        );
    }

    fn halt(start_ns: i64, end_ns: Option<i64>, reason: &str) -> chart_engine::wire::HaltUpdate {
        chart_engine::wire::HaltUpdate {
            symbol: SYM.into(),
            halt: chart_engine::wire::HaltWire {
                start: start_ns / 1_000_000,
                start_ns,
                end: end_ns.map(|e| e / 1_000_000),
                end_ns,
                reason: reason.into(),
                code: "H".into(),
            },
        }
    }

    #[test]
    fn a_t1_halt_passes_the_news_pillar_as_news_pending_from_the_halt_on() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        write_daily_world(&a);
        write_without_news(&a);
        let log = Mutex::new(EvidenceLog::default());
        let mut cx = board();
        load(&a, &log, &mut cx);
        // Any other halt reason is not news.
        assert!(crate::context::t1_news_item(&halt(t(9, 31, 0), None, "LUDP")).is_none());
        // Nor is the scheduled 19:50:00 ET T1 the evening before a reverse split.
        assert!(crate::context::t1_news_item(&halt(t(19, 50, 0), None, "T1")).is_none());
        assert!(crate::context::t1_news_item(&halt(t(19, 50, 1), None, "T1")).is_some());
        trade(&mut cx, t(9, 30, 0), t(9, 31, 0));
        assert_ne!(state(&cx, SYM, PillarId::News).0, Tri::Pass);
        cx.apply_halt_news(&halt(t(9, 31, 5), None, "T1"));
        trade(&mut cx, t(9, 31, 0), t(9, 32, 0));
        assert_eq!(state(&cx, SYM, PillarId::News).0, Tri::Pass);
        let frame: serde_json::Value =
            serde_json::from_str(&cx.panel_frame("ross.snapshot", None)).expect("json");
        let ev = &frame["panel"]["rows"][0]["news_evidence"];
        assert_eq!(ev["source"], "halt_t1");
        assert_eq!(ev["channel"], "T1");
        assert_eq!(ev["pending"], true);
        assert_eq!(ev["published_at"], t(9, 31, 5));
        assert_eq!(ev["halt"]["halted_at"], t(9, 31, 5));
        assert!(ev["halt"]["resumed_at"].is_null());
        // The resume restates it (version 2, observed at the resume) with the resume time.
        let resumed =
            crate::context::t1_news_item(&halt(t(9, 31, 5), Some(t(9, 36, 5)), "T1")).unwrap();
        assert_eq!((resumed.version, resumed.observed_at), (2, t(9, 36, 5)));
        assert_eq!(resumed.halt.unwrap().resumed_at, Some(t(9, 36, 5)));
    }
}
