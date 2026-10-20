//! Provider daily bars kept whole, beside the closes: `$ARCHIVE_ROOT/daily-bars/`.
//!
//! ```text
//! daily-bars/
//!   raw/2026-10-15.json                  # one file per trading date, every symbol (RawDayBars)
//!   split/20261019T231502Z-<hash>.json   # one file per split-adjusted fetch (SplitSnapshot)
//! ```
//!
//! Why this exists: the stock-context engine's RVOL pillar needs 30 prior sessions of full-day
//! volume and its gain pillar a previous close whose corporate-action coverage is known
//! (`docs/stock-context/daily-baseline-plan.md`). The importer already downloads Alpaca's SIP
//! `1Day` bars every night for `daily-closes.json` and used to keep only `c`; the same responses
//! are now kept whole here, so the raw set costs no extra request.
//!
//! # Two shapes, on purpose
//!
//! - **Raw** (`adjustment=raw`) is history as printed: a later fetch of the same day returns the
//!   same numbers (or a provider correction, which should win). So raw bars are **merged** across
//!   fetches, one file per date, like `daily-closes.json` merges dates. Per date rather than one
//!   file: the nightly refresh refetches ~22 dates of ~12k symbols (~1 MB a date), a date's file
//!   is rewritten only when its content changed, and retention deletes whole files. One big file
//!   would be rewritten every night (~60 MB at 63 dates); per-month files would still rewrite
//!   ~20 dates' worth to change one.
//! - **Split-adjusted** (`adjustment=split`) is expressed on the share basis **of the moment it
//!   was fetched**: Alpaca restates every earlier bar each time a new split takes effect, prices
//!   and volumes alike. Two split fetches made on either side of a split are on different bases,
//!   so merging them (the raw rule) would silently mix bases. Each split fetch is therefore an
//!   immutable **snapshot** file, never merged; a reader uses one snapshot at a time.
//!
//! # Coverage
//!
//! A missing bar is not a zero. A raw `(symbol, date)` is *covered* when the date's file lists the
//! symbol under `bars` (it traded) or `no_bar` (it was requested, the provider returned nothing).
//! Anything else is unknown: never requested. A snapshot covers its requested symbols × the
//! trading dates of its `from..=to`. Both record every fetch's parameters and `retrieved_at`
//! ([`BarFetch`]), so a reader can also refuse a bar fetched before its session had finished.
//!
//! Every file has `"schema": 1`; a reader refuses any other version rather than half-read it.
//! Writes are atomic (temp file, fsync, rename), as `daily-closes.json`.

use crate::Archive;
use crate::error::ArchiveError;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const DAILY_BARS_DIR: &str = "daily-bars";
pub const DAILY_BARS_SCHEMA: u32 = 1;
/// A date file keeps the provenance of at most this many fetches (the ones that changed it).
pub const MAX_FETCHES_KEPT: usize = 8;

/// One provider daily bar as delivered. Only the close is required: the existing closes path
/// accepted bars without the other fields, and a bar without `v` is simply not a volume record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DailyBar {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub o: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub h: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub l: Option<f64>,
    pub c: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub v: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vw: Option<f64>,
}

/// Alpaca's `adjustment` query parameter, for the two values this archive stores.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Adjustment {
    Raw,
    Split,
}

impl Adjustment {
    pub fn as_str(self) -> &'static str {
        match self {
            Adjustment::Raw => "raw",
            Adjustment::Split => "split",
        }
    }
}

/// Provenance of one fetch: what was asked, of whom, and when.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BarFetch {
    pub retrieved_at: DateTime<Utc>,
    /// `alpaca`.
    pub provider: String,
    /// `sip`.
    pub feed: String,
    pub adjustment: Adjustment,
    /// `1Day`.
    pub timeframe: String,
    pub from: NaiveDate,
    pub to: NaiveDate,
    /// Symbols requested (the list itself is in a snapshot; a raw date file lists them as
    /// `bars` ∪ `no_bar`).
    pub symbols: usize,
    pub symbols_per_request: usize,
    /// HTTP requests made, pages included (retries not counted).
    pub requests: u64,
    /// Alpaca's `asof` (symbol-rename mapping date). Always `None`: we never send it. Recorded so
    /// a reader never mistakes it for a corporate-action knowledge cutoff, which it is not.
    pub asof: Option<NaiveDate>,
    /// Which command made it (`maintain`, `alpaca-daily-closes`, `alpaca-split-bars`).
    pub command: String,
}

/// Every symbol's raw bar on one trading date (`daily-bars/raw/<date>.json`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RawDayBars {
    pub schema: u32,
    pub date: NaiveDate,
    pub adjustment: Adjustment,
    pub bars: BTreeMap<String, DailyBar>,
    /// Requested, nothing returned (no trades, halted all day, not yet listed: the bar cannot
    /// say which). Never a zero-volume session.
    pub no_bar: BTreeSet<String>,
    /// The fetches that changed this file, oldest first (at most [`MAX_FETCHES_KEPT`]).
    pub fetches: Vec<BarFetch>,
}

impl RawDayBars {
    pub fn new(date: NaiveDate, fetch: BarFetch) -> Self {
        RawDayBars {
            schema: DAILY_BARS_SCHEMA,
            date,
            adjustment: Adjustment::Raw,
            bars: BTreeMap::new(),
            no_bar: BTreeSet::new(),
            fetches: vec![fetch],
        }
    }

    /// `Some(Some(bar))` traded, `Some(None)` covered without a bar, `None` never requested.
    pub fn coverage(&self, symbol: &str) -> Option<Option<&DailyBar>> {
        match self.bars.get(symbol) {
            Some(b) => Some(Some(b)),
            None if self.no_bar.contains(symbol) => Some(None),
            None => None,
        }
    }

    /// The earliest retrieval that contributed to this file.
    pub fn first_retrieved(&self) -> Option<DateTime<Utc>> {
        self.fetches.iter().map(|f| f.retrieved_at).min()
    }

    /// The latest retrieval that changed this file.
    pub fn last_retrieved(&self) -> Option<DateTime<Utc>> {
        self.fetches.iter().map(|f| f.retrieved_at).max()
    }
}

/// Adds a newer fetch of the same date. A bar present in `new` replaces ours (a re-fetch is the
/// fresher answer, as for closes). A symbol `new` returned nothing for keeps an older bar: a
/// provider that drops a bar it once served is more likely a partial answer than an un-trade, and
/// deleting volume history on that evidence is irreversible. Returns how many symbols changed;
/// when any did, `new`'s fetches are appended to the provenance.
pub fn merge_raw_day(into: &mut RawDayBars, new: RawDayBars) -> usize {
    debug_assert_eq!(into.date, new.date);
    let mut changed = 0;
    for (symbol, bar) in new.bars {
        into.no_bar.remove(&symbol);
        if into.bars.get(&symbol) != Some(&bar) {
            into.bars.insert(symbol, bar);
            changed += 1;
        }
    }
    for symbol in new.no_bar {
        if !into.bars.contains_key(&symbol) && into.no_bar.insert(symbol) {
            changed += 1;
        }
    }
    if changed > 0 {
        into.fetches.extend(new.fetches);
        let excess = into.fetches.len().saturating_sub(MAX_FETCHES_KEPT);
        into.fetches.drain(..excess);
    }
    changed
}

/// One split-adjusted fetch, kept as fetched (`daily-bars/split/<retrieved>-<hash>.json`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SplitSnapshot {
    pub schema: u32,
    pub fetch: BarFetch,
    /// Exactly the symbols requested, sorted.
    pub symbols: Vec<String>,
    /// symbol → date → bar, as returned (dates outside `from..=to` dropped).
    pub bars: BTreeMap<String, BTreeMap<NaiveDate, DailyBar>>,
    /// The group this snapshot belongs to (one `maintain` run's watch list, a UTC timestamp);
    /// `None` for a manual `alpaca-split-bars` run, which is a set of its own.
    #[serde(default)]
    pub set: Option<String>,
}

impl SplitSnapshot {
    /// `Some(Some(bar))` traded, `Some(None)` covered without a bar, `None` not covered (symbol not
    /// requested or date outside the range). Calendar checks are the reader's.
    pub fn coverage(&self, symbol: &str, date: NaiveDate) -> Option<Option<&DailyBar>> {
        if date < self.fetch.from
            || date > self.fetch.to
            || self
                .symbols
                .binary_search_by(|s| s.as_str().cmp(symbol))
                .is_err()
        {
            return None;
        }
        Some(self.bars.get(symbol).and_then(|m| m.get(&date)))
    }

    /// File name: retrieval time (sorts chronologically) plus a short hash of the request, so two
    /// fetches in the same second for different symbol lists never collide.
    pub fn file_name(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(self.fetch.from.to_string());
        h.update(self.fetch.to.to_string());
        for s in &self.symbols {
            h.update(s.as_bytes());
            h.update(b",");
        }
        let hash = hex::encode(&h.finalize()[..4]);
        // Milliseconds: two snapshots of one set may be fetched within the same second.
        let at = self.fetch.retrieved_at.format("%Y%m%dT%H%M%S%.3fZ");
        match &self.set {
            // Grouped: `maintain` writes the watch list as several snapshots (≤ 2,000 symbols
            // each); retention keeps or drops a set whole.
            Some(set) => format!("set-{set}-{at}-{hash}.json"),
            None => format!("{at}-{hash}.json"),
        }
    }
}

/// The retention group of a snapshot file name: its `set-<id>` prefix, or the file itself.
fn set_key(name: &str) -> &str {
    match name.strip_prefix("set-") {
        Some(rest) => rest.split('-').next().unwrap_or(rest),
        None => name.split('-').next().unwrap_or(name),
    }
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), ArchiveError> {
    let dir = path.parent().expect("has a parent");
    std::fs::create_dir_all(dir).map_err(|e| ArchiveError::io(dir, e))?;
    let tmp = path.with_extension("json.tmp");
    // Compact: a date file holds ~12k bars.
    let json = serde_json::to_vec(value).expect("daily bars serialize");
    std::fs::write(&tmp, json).map_err(|e| ArchiveError::io(&tmp, e))?;
    if let Ok(f) = std::fs::File::open(&tmp) {
        let _ = f.sync_all();
    }
    std::fs::rename(&tmp, path).map_err(|e| ArchiveError::io(path, e))
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>, ArchiveError> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(ArchiveError::io(path, e)),
    };
    // Schema first, so a future version is refused by name rather than by a field error.
    let probe: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| ArchiveError::format(path, e.to_string()))?;
    let schema = probe.get("schema").and_then(|v| v.as_u64());
    if schema != Some(u64::from(DAILY_BARS_SCHEMA)) {
        return Err(ArchiveError::format(
            path,
            format!("daily bars schema {schema:?}, expected {DAILY_BARS_SCHEMA}"),
        ));
    }
    serde_json::from_value(probe)
        .map(Some)
        .map_err(|e| ArchiveError::format(path, e.to_string()))
}

/// Report of [`Archive::merge_raw_bars`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawMergeReport {
    /// Date files written (something changed).
    pub dates_written: Vec<NaiveDate>,
    /// `(date, symbol)` bars or no-bar entries added or changed.
    pub changed: usize,
}

impl Archive {
    pub fn daily_bars_dir(&self) -> PathBuf {
        self.root().join(DAILY_BARS_DIR)
    }

    pub fn raw_bars_path(&self, date: NaiveDate) -> PathBuf {
        self.daily_bars_dir()
            .join("raw")
            .join(format!("{}.json", date.format("%Y-%m-%d")))
    }

    pub fn split_snapshots_dir(&self) -> PathBuf {
        self.daily_bars_dir().join("split")
    }

    /// One date's raw bars, `None` when the file does not exist. A file for another date or
    /// another adjustment is an error, never silently used.
    pub fn read_raw_bars(&self, date: NaiveDate) -> Result<Option<RawDayBars>, ArchiveError> {
        let path = self.raw_bars_path(date);
        let Some(day) = read_json::<RawDayBars>(&path)? else {
            return Ok(None);
        };
        if day.date != date || day.adjustment != Adjustment::Raw {
            return Err(ArchiveError::format(
                &path,
                format!("holds {} {:?}", day.date, day.adjustment),
            ));
        }
        Ok(Some(day))
    }

    pub fn write_raw_bars(&self, day: &RawDayBars) -> Result<PathBuf, ArchiveError> {
        let path = self.raw_bars_path(day.date);
        write_json_atomic(&path, day)?;
        Ok(path)
    }

    /// Dates that have a raw file, ascending.
    pub fn raw_bar_dates(&self) -> Result<Vec<NaiveDate>, ArchiveError> {
        let dir = self.daily_bars_dir().join("raw");
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(ArchiveError::io(&dir, e)),
        };
        let mut out: Vec<NaiveDate> = rd
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                NaiveDate::parse_from_str(name.strip_suffix(".json")?, "%Y-%m-%d").ok()
            })
            .collect();
        out.sort();
        Ok(out)
    }

    /// Merges one fetch (a [`RawDayBars`] per date it returned) into the date files; a file is
    /// rewritten only when something in it changed, so an identical nightly refetch writes
    /// nothing. An unreadable existing file stops the merge for that date with an error rather
    /// than being replaced by just this fetch.
    pub fn merge_raw_bars(
        &self,
        fetched: impl IntoIterator<Item = RawDayBars>,
    ) -> Result<RawMergeReport, ArchiveError> {
        let mut rep = RawMergeReport::default();
        for new in fetched {
            let date = new.date;
            let (day, changed) = match self.read_raw_bars(date)? {
                Some(mut old) => {
                    let n = merge_raw_day(&mut old, new);
                    (old, n)
                }
                None => {
                    let n = new.bars.len() + new.no_bar.len();
                    (new, n)
                }
            };
            if changed > 0 {
                self.write_raw_bars(&day)?;
                rep.dates_written.push(date);
                rep.changed += changed;
            }
        }
        Ok(rep)
    }

    /// Deletes raw date files before `keep_from`. Returns the dates removed.
    pub fn prune_raw_bars(&self, keep_from: NaiveDate) -> Result<Vec<NaiveDate>, ArchiveError> {
        let mut gone = Vec::new();
        for d in self.raw_bar_dates()? {
            if d < keep_from {
                let p = self.raw_bars_path(d);
                std::fs::remove_file(&p).map_err(|e| ArchiveError::io(&p, e))?;
                gone.push(d);
            }
        }
        Ok(gone)
    }

    /// Every raw file from `from` to `to` inclusive that exists, keyed by date.
    pub fn read_raw_bars_range(
        &self,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<BTreeMap<NaiveDate, RawDayBars>, ArchiveError> {
        let mut out = BTreeMap::new();
        for d in self.raw_bar_dates()? {
            if d >= from
                && d <= to
                && let Some(day) = self.read_raw_bars(d)?
            {
                out.insert(d, day);
            }
        }
        Ok(out)
    }

    /// Writes a split snapshot under its own name; never overwrites another fetch's file.
    pub fn write_split_snapshot(&self, snap: &SplitSnapshot) -> Result<PathBuf, ArchiveError> {
        let path = self.split_snapshots_dir().join(snap.file_name());
        if path.exists() {
            return Err(ArchiveError::format(&path, "snapshot already exists"));
        }
        write_json_atomic(&path, snap)?;
        Ok(path)
    }

    /// Every split snapshot, oldest retrieval first.
    pub fn read_split_snapshots(&self) -> Result<Vec<SplitSnapshot>, ArchiveError> {
        let dir = self.split_snapshots_dir();
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(ArchiveError::io(&dir, e)),
        };
        let mut paths: Vec<PathBuf> = rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        paths.sort();
        let mut out = Vec::with_capacity(paths.len());
        for p in paths {
            if let Some(s) = read_json::<SplitSnapshot>(&p)? {
                if s.fetch.adjustment != Adjustment::Split {
                    return Err(ArchiveError::format(&p, "not a split-adjusted fetch"));
                }
                out.push(s);
            }
        }
        out.sort_by_key(|s| s.fetch.retrieved_at);
        Ok(out)
    }

    /// The split snapshots a decision at `as_of` may use, oldest retrieval first: the newest
    /// nightly set (`set-…` files) retrieved by then, plus every one-off snapshot (manual or
    /// warmup) retrieved after that set's first file and by then. Older sets are not read: a
    /// nightly set is the whole watch list on one basis (~35 MB of JSON), and parsing all five
    /// kept sets on every reload would cost ~175 MB for symbols the newest set already has. A
    /// symbol that only an older set requested gets no snapshot here, so its baseline is
    /// unverified (UNKNOWN), never put on a wrong basis. While a set is still being written its
    /// later chunks are simply not there yet; a reload after they land picks them up.
    ///
    /// Files are chosen by the time in their name, then checked against `fetch.retrieved_at`:
    /// nothing retrieved after `as_of` is returned.
    pub fn read_split_snapshots_as_of(
        &self,
        as_of: DateTime<Utc>,
    ) -> Result<Vec<SplitSnapshot>, ArchiveError> {
        let dir = self.split_snapshots_dir();
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(ArchiveError::io(&dir, e)),
        };
        let limit = as_of.naive_utc();
        // (set id, retrieval time from the name, path) of every file retrieved by `as_of`.
        let files: Vec<(Option<String>, chrono::NaiveDateTime, PathBuf)> = rd
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let stem = name.strip_suffix(".json")?;
                let (set, rest) = match stem.strip_prefix("set-") {
                    Some(r) => {
                        let (set, rest) = r.split_once('-')?;
                        (Some(set.to_string()), rest)
                    }
                    None => (None, stem),
                };
                let at =
                    chrono::NaiveDateTime::parse_from_str(rest.get(..15)?, "%Y%m%dT%H%M%S").ok()?;
                (at <= limit).then(|| (set, at, e.path()))
            })
            .collect();
        let newest = files.iter().filter_map(|(s, _, _)| s.as_deref()).max();
        let set_start = newest.and_then(|n| {
            files
                .iter()
                .filter(|(s, _, _)| s.as_deref() == Some(n))
                .map(|(_, at, _)| *at)
                .min()
        });
        let mut out = Vec::new();
        for (set, at, path) in &files {
            let wanted = match (set, newest) {
                (Some(s), Some(n)) => s == n,
                (None, _) => set_start.is_none_or(|st| *at >= st),
                (Some(_), None) => false,
            };
            if !wanted {
                continue;
            }
            if let Some(s) = read_json::<SplitSnapshot>(path)? {
                if s.fetch.adjustment != Adjustment::Split {
                    return Err(ArchiveError::format(path, "not a split-adjusted fetch"));
                }
                if s.fetch.retrieved_at <= as_of {
                    out.push(s);
                }
            }
        }
        out.sort_by_key(|s| s.fetch.retrieved_at);
        Ok(out)
    }

    /// Deletes snapshots outside the `keep` most recent nightly sets (by timestamp, read from the
    /// file names: no snapshot is parsed). Nightly sets (`set-` files) are kept by count; a manual
    /// or warmup snapshot (a set of its own) is kept while it is not older than the oldest nightly
    /// set kept, so warmups never push a nightly set out and old one-offs do not pile up. Returns
    /// the files removed.
    pub fn prune_split_sets(&self, keep: usize) -> Result<Vec<PathBuf>, ArchiveError> {
        let dir = self.split_snapshots_dir();
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(ArchiveError::io(&dir, e)),
        };
        let files: Vec<(String, PathBuf)> = rd
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                name.ends_with(".json").then(|| (name, e.path()))
            })
            .collect();
        let mut sets: Vec<&str> = files
            .iter()
            .filter(|(n, _)| n.starts_with("set-"))
            .map(|(n, _)| set_key(n))
            .collect();
        sets.sort();
        sets.dedup();
        let cut = sets.len().saturating_sub(keep);
        let oldest_kept = sets.get(cut).copied();
        let mut gone = Vec::new();
        for (name, path) in &files {
            let key = set_key(name);
            let drop = if name.starts_with("set-") {
                sets[..cut].contains(&key)
            } else {
                oldest_kept.is_some_and(|o| key < o)
            };
            if drop {
                std::fs::remove_file(path).map_err(|e| ArchiveError::io(path, e))?;
                gone.push(path.clone());
            }
        }
        Ok(gone)
    }

    /// The most recent snapshot that requested `symbol`, if any (one snapshot is one basis).
    pub fn latest_snapshot_for<'a>(
        snapshots: &'a [SplitSnapshot],
        symbol: &str,
    ) -> Option<&'a SplitSnapshot> {
        snapshots.iter().rev().find(|s| {
            s.symbols
                .binary_search_by(|x| x.as_str().cmp(symbol))
                .is_ok()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ArchiveConfig;

    fn archive(dir: &Path) -> Archive {
        let root = dir.join("archive").to_string_lossy().into_owned();
        Archive::new(
            ArchiveConfig::from_lookup(|k| match k {
                "ARCHIVE_ROOT" => Some(root.clone()),
                "ARCHIVE_MIN_FREE_MB" => Some("0".into()),
                _ => None,
            })
            .unwrap(),
        )
    }

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn fetch(at: &str) -> BarFetch {
        BarFetch {
            retrieved_at: DateTime::parse_from_rfc3339(at).unwrap().to_utc(),
            provider: "alpaca".into(),
            feed: "sip".into(),
            adjustment: Adjustment::Raw,
            timeframe: "1Day".into(),
            from: d("2026-10-14"),
            to: d("2026-10-15"),
            symbols: 2,
            symbols_per_request: 200,
            requests: 1,
            asof: None,
            command: "test".into(),
        }
    }

    fn bar(c: f64, v: u64) -> DailyBar {
        DailyBar {
            o: Some(c),
            h: Some(c),
            l: Some(c),
            c,
            v: Some(v),
            n: Some(1),
            vw: None,
        }
    }

    #[test]
    fn raw_dates_merge_round_trip_and_never_become_days() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        let mut day = RawDayBars::new(d("2026-10-14"), fetch("2026-10-14T23:00:00Z"));
        day.bars.insert("AAA".into(), bar(1.0, 100));
        day.no_bar.insert("BBB".into());
        let rep = a.merge_raw_bars([day.clone()]).unwrap();
        assert_eq!(rep.dates_written, [d("2026-10-14")]);
        assert_eq!(a.read_raw_bars(d("2026-10-14")).unwrap(), Some(day.clone()));
        assert!(a.days().unwrap().is_empty(), "never mistaken for a day");

        // The identical refetch writes nothing.
        let mut again = day.clone();
        again.fetches = vec![fetch("2026-10-15T23:00:00Z")];
        assert_eq!(
            a.merge_raw_bars([again]).unwrap(),
            RawMergeReport::default()
        );

        // BBB now has a bar (a late report), AAA is absent from the answer: AAA is kept.
        let mut newer = RawDayBars::new(d("2026-10-14"), fetch("2026-10-16T23:00:00Z"));
        newer.bars.insert("BBB".into(), bar(2.0, 5));
        newer.no_bar.insert("AAA".into());
        let rep = a.merge_raw_bars([newer]).unwrap();
        assert_eq!(rep.changed, 1);
        let back = a.read_raw_bars(d("2026-10-14")).unwrap().unwrap();
        assert_eq!(back.coverage("AAA"), Some(Some(&bar(1.0, 100))));
        assert_eq!(back.coverage("BBB"), Some(Some(&bar(2.0, 5))));
        assert_eq!(
            back.coverage("CCC"),
            None,
            "never requested is not 'no bar'"
        );
        assert_eq!(back.fetches.len(), 2);
        assert_eq!(a.raw_bar_dates().unwrap(), [d("2026-10-14")]);
        assert_eq!(
            a.prune_raw_bars(d("2026-10-15")).unwrap(),
            [d("2026-10-14")]
        );
        assert!(a.raw_bar_dates().unwrap().is_empty());
    }

    #[test]
    fn another_schema_or_a_misfiled_date_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        let p = a.raw_bars_path(d("2026-10-14"));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, br#"{"schema":2}"#).unwrap();
        assert!(a.read_raw_bars(d("2026-10-14")).is_err());
        let day = RawDayBars::new(d("2026-10-15"), fetch("2026-10-15T23:00:00Z"));
        std::fs::write(&p, serde_json::to_vec(&day).unwrap()).unwrap();
        assert!(a.read_raw_bars(d("2026-10-14")).is_err());
    }

    #[test]
    fn split_snapshots_are_separate_files_never_merged() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        let mut f = fetch("2026-10-16T01:00:00Z");
        f.adjustment = Adjustment::Split;
        let mut s = SplitSnapshot {
            schema: DAILY_BARS_SCHEMA,
            fetch: f,
            symbols: vec!["AAA".into(), "BBB".into()],
            bars: BTreeMap::new(),
            set: None,
        };
        s.bars
            .entry("AAA".into())
            .or_default()
            .insert(d("2026-10-14"), bar(1.0, 100));
        a.write_split_snapshot(&s).unwrap();
        assert!(a.write_split_snapshot(&s).is_err(), "never overwritten");
        let mut later = s.clone();
        later.fetch.retrieved_at = DateTime::parse_from_rfc3339("2026-10-17T01:00:00Z")
            .unwrap()
            .to_utc();
        a.write_split_snapshot(&later).unwrap();
        let all = a.read_split_snapshots().unwrap();
        assert_eq!(all, [s.clone(), later]);
        assert_eq!(
            s.coverage("AAA", d("2026-10-14")),
            Some(Some(&bar(1.0, 100)))
        );
        assert_eq!(s.coverage("BBB", d("2026-10-15")), Some(None));
        assert_eq!(s.coverage("CCC", d("2026-10-15")), None);
        assert_eq!(
            s.coverage("AAA", d("2026-10-16")),
            None,
            "outside the range"
        );

        // Retention by set: two snapshots of one maintain set count once.
        for (set, at, sym) in [
            ("20261018T030000Z", "2026-10-18T03:00:00Z", "AAA"),
            ("20261018T030000Z", "2026-10-18T03:01:00Z", "BBB"),
            ("20261019T030000Z", "2026-10-19T03:00:00Z", "AAA"),
        ] {
            let mut x = s.clone();
            x.set = Some(set.into());
            x.symbols = vec![sym.into()];
            x.fetch.retrieved_at = DateTime::parse_from_rfc3339(at).unwrap().to_utc();
            a.write_split_snapshot(&x).unwrap();
        }
        // Sets now: 10-16 (manual), 10-17 (manual), 10-18 (2 files), 10-19. Keep 2.
        assert_eq!(a.prune_split_sets(2).unwrap().len(), 2);
        let left = a.read_split_snapshots().unwrap();
        assert_eq!(left.len(), 3);
        assert!(left.iter().all(|x| x.set.is_some()));
        assert_eq!(
            Archive::latest_snapshot_for(&left, "AAA")
                .unwrap()
                .set
                .as_deref(),
            Some("20261019T030000Z")
        );
        assert_eq!(
            Archive::latest_snapshot_for(&left, "BBB")
                .unwrap()
                .set
                .as_deref(),
            Some("20261018T030000Z")
        );
    }

    #[test]
    fn as_of_reads_the_newest_set_by_then_and_later_one_offs_only() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        let at = |s: &str| DateTime::parse_from_rfc3339(s).unwrap().to_utc();
        let snap = |set: Option<&str>, when: &str, sym: &str| {
            let mut f = fetch(when);
            f.adjustment = Adjustment::Split;
            let s = SplitSnapshot {
                schema: DAILY_BARS_SCHEMA,
                fetch: f,
                symbols: vec![sym.into()],
                bars: BTreeMap::new(),
                set: set.map(String::from),
            };
            a.write_split_snapshot(&s).unwrap();
            s
        };
        let old = snap(Some("20261018T030000Z"), "2026-10-18T03:00:00Z", "AAA");
        let manual_before = snap(None, "2026-10-18T12:00:00Z", "CCC");
        let s1 = snap(Some("20261019T030000Z"), "2026-10-19T03:00:00Z", "AAA");
        let s2 = snap(Some("20261019T030000Z"), "2026-10-19T03:02:00Z", "BBB");
        let warm = snap(None, "2026-10-19T14:00:00Z", "DDD");
        let next = snap(Some("20261020T030000Z"), "2026-10-20T03:00:00Z", "AAA");
        let names = |v: Vec<SplitSnapshot>| -> Vec<(Option<String>, Vec<String>)> {
            v.into_iter().map(|s| (s.set, s.symbols)).collect()
        };
        // Mid-set: only the chunk already retrieved.
        assert_eq!(
            names(
                a.read_split_snapshots_as_of(at("2026-10-19T03:01:00Z"))
                    .unwrap()
            ),
            names(vec![s1.clone()])
        );
        assert_eq!(
            names(
                a.read_split_snapshots_as_of(at("2026-10-19T20:00:00Z"))
                    .unwrap()
            ),
            names(vec![s1.clone(), s2.clone(), warm.clone()])
        );
        assert_eq!(
            names(
                a.read_split_snapshots_as_of(at("2026-10-20T04:00:00Z"))
                    .unwrap()
            ),
            names(vec![next])
        );
        // Before any later set: the old one and the manual run after it.
        assert_eq!(
            names(
                a.read_split_snapshots_as_of(at("2026-10-18T23:00:00Z"))
                    .unwrap()
            ),
            names(vec![old, manual_before])
        );
        assert!(
            a.read_split_snapshots_as_of(at("2026-10-01T00:00:00Z"))
                .unwrap()
                .is_empty()
        );
    }
}
