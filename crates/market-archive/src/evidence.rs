//! Collected float and news evidence beside the days: `$ARCHIVE_ROOT/evidence/`.
//!
//! ```text
//! evidence/
//!   float/2026-10.jsonl          # FloatRecord, one per retrieval (month of retrieved_at, UTC)
//!   news/2026-10.jsonl           # NewsVersionRecord, one per new version (month of first_observed_at)
//!   news-coverage/2026-10.jsonl  # NewsCoverageRecord, one per window searched successfully
//! ```
//!
//! **Append-only.** A record is never rewritten or deleted: a newer retrieval or an edited story is
//! a new line. This is what lets a decision at an earlier cutoff be rebuilt exactly later (the
//! context engine joins the latest record observed by the cutoff; `crate::evidence_load`). The
//! rolling-window pruning never touches this directory: [`Archive::days`] lists only date-named
//! directories, and nothing here belongs to one trading day.
//!
//! **Times** are UTC nanoseconds. `retrieved_at` / `first_observed_at` / `observed_at` are our own
//! clock when the response arrived: the time a forward decision may use. A provider's own dates
//! are kept as delivered next to them and never replace them.
//!
//! **Crash safety**: one `write_all` of whole lines then `sync_data` per append. A crash can at
//! worst leave a final line without its newline; the reader skips (and counts) a last line that
//! does not parse, and the next append starts on a new line. Any other bad line is an error: a
//! hand-edited file should be noticed, not half-read.
//!
//! **Copyright**: news versions keep the headline (a title, needed for the classification rules),
//! hashes of headline and summary, symbols, source, author, URL and the provider's times. Never a
//! summary or body text.

use crate::Archive;
use crate::error::ArchiveError;
use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const EVIDENCE_DIR: &str = "evidence";
pub const FLOAT_DIR: &str = "float";
pub const NEWS_DIR: &str = "news";
pub const NEWS_COVERAGE_DIR: &str = "news-coverage";
/// Schema of every record below; a reader refuses another.
pub const EVIDENCE_SCHEMA: u32 = 1;

/// One float retrieval for one symbol.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FloatRecord {
    pub v: u32,
    pub symbol: String,
    /// `fmp`.
    pub provider: String,
    /// Path and query without the key, e.g. `/stable/shares-float?symbol=SGRX`.
    pub endpoint: String,
    /// The provider's public float (`floatShares`). `None`: the response carried no usable row.
    pub float_shares: Option<u64>,
    /// Kept only to flag `float_equals_outstanding`; **never** used as a float.
    pub outstanding_shares: Option<u64>,
    /// `freeFloat` percent as delivered.
    pub free_float_pct: Option<f64>,
    /// The document the provider cites (`source`), when given.
    pub source_url: Option<String>,
    /// The provider's own `date` as delivered. For FMP this is its refresh time (UTC), not the
    /// date the float was true: research report 02, F1.
    pub provider_date: Option<String>,
    /// The date the figure is stated to be true as of, only when the provider supplies one. FMP
    /// does not, so this stays `None` (it is never derived from `provider_date`).
    pub effective_as_of: Option<chrono::NaiveDate>,
    /// When the response arrived (UTC ns): the record's observation time.
    pub retrieved_at: i64,
    /// SHA-256 (hex) of the raw response body this row came from.
    pub response_sha256: String,
    /// Quality flags (`evidence_load` documents each).
    pub quality: Vec<String>,
}

/// One version of one news story as first seen.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NewsVersionRecord {
    pub v: u32,
    /// `alpaca`.
    pub provider: String,
    /// The story's own source field (`benzinga`).
    pub source: String,
    /// The provider's story id (stable across edits).
    pub article_id: String,
    /// `created_at` / `updated_at` exactly as delivered (RFC 3339), and parsed (UTC ns).
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub created_at_ns: Option<i64>,
    pub updated_at_ns: Option<i64>,
    /// When StockWatcher first received this version (UTC ns). Never the publication time.
    pub first_observed_at: i64,
    pub headline: String,
    pub headline_sha256: String,
    pub summary_sha256: String,
    pub author: Option<String>,
    pub symbols: Vec<String>,
    pub url: Option<String>,
    /// What distinguishes versions as the collector wrote it: SHA-256 over the **raw**
    /// `updated_at` string, headline, summary and symbols.
    ///
    /// Kept exactly as written, because it is provenance: it is what that collector hashed, and
    /// rewriting it would change the identity of records already on disk. It is not the right
    /// thing to *compare*, though — see [`NewsVersionRecord::content_key`].
    pub version_key: String,
}

impl NewsVersionRecord {
    /// What makes this version's content distinct, canonically.
    ///
    /// [`NewsVersionRecord::version_key`] hashes the raw `updated_at` string, and two transports
    /// write the same instant differently: a REST page sends `2026-10-15T14:00:00Z` where a
    /// stream sends `2026-10-15T14:00:00.000000+00:00`. Those parse to the same nanosecond and
    /// describe the same revision of the same text, but they hash differently — so the same
    /// content read as two contents at one revision time, which is precisely the shape of a
    /// genuine conflict. The story would be reported unanswerable because of a formatting
    /// difference.
    ///
    /// So comparisons use the parsed revision time and the content hashes the provider's text
    /// produced, and the raw key stays on the record untouched.
    pub fn content_key(&self) -> String {
        let mut symbols = self.symbols.clone();
        symbols.sort();
        symbols.dedup();
        let revision = self
            .updated_at_ns
            .or(self.created_at_ns)
            .map(|t| t.to_string())
            .unwrap_or_else(|| "none".to_string());
        sha256_hex(
            format!(
                "{revision}\n{}\n{}\n{}",
                self.headline_sha256,
                self.summary_sha256,
                symbols.join(",")
            )
            .as_bytes(),
        )
    }
}

/// Which symbols a successful search covered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "symbols")]
pub enum CoverageScope {
    /// A query without a symbol filter: every symbol the source tags.
    All,
    Symbols(Vec<String>),
}

impl CoverageScope {
    pub fn covers(&self, symbol: &str) -> bool {
        match self {
            CoverageScope::All => true,
            CoverageScope::Symbols(s) => s.iter().any(|x| x == symbol),
        }
    }
}

/// A news window searched successfully: every page fetched, no error.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewsCoverageRecord {
    pub v: u32,
    pub id: String,
    pub provider: String,
    /// The story sources the provider serves (`benzinga` for Alpaca news).
    pub sources: Vec<String>,
    pub scope: CoverageScope,
    /// The query's `start` and `end` (publication time, UTC ns).
    pub covered_from: i64,
    pub covered_through: i64,
    /// When the last page arrived (UTC ns).
    pub observed_at: i64,
    pub requests: u32,
    pub stories: u32,
}

/// Hex SHA-256.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The `YYYY-MM` (UTC) file a time belongs to.
fn month_of(ns: i64) -> String {
    let t: DateTime<Utc> = DateTime::from_timestamp_nanos(ns);
    format!("{:04}-{:02}", t.year(), t.month())
}

impl Archive {
    pub fn evidence_dir(&self) -> PathBuf {
        self.root().join(EVIDENCE_DIR)
    }

    pub fn append_float(&self, rows: &[FloatRecord]) -> Result<(), ArchiveError> {
        append_grouped(&self.evidence_dir().join(FLOAT_DIR), rows, |r| {
            r.retrieved_at
        })
    }

    pub fn append_news(&self, rows: &[NewsVersionRecord]) -> Result<(), ArchiveError> {
        append_grouped(&self.evidence_dir().join(NEWS_DIR), rows, |r| {
            r.first_observed_at
        })
    }

    pub fn append_news_coverage(&self, rows: &[NewsCoverageRecord]) -> Result<(), ArchiveError> {
        append_grouped(&self.evidence_dir().join(NEWS_COVERAGE_DIR), rows, |r| {
            r.observed_at
        })
    }

    /// Every float record, in file order (append order). Missing directory: none.
    pub fn read_float(&self) -> Result<Vec<FloatRecord>, ArchiveError> {
        read_all(&self.evidence_dir().join(FLOAT_DIR), None)
    }

    /// The newest float record for `symbol` that carries a float and was retrieved at or before
    /// `at` (UTC ns): what the server held for it then. Reads month files newest first, from
    /// `at`'s month back, and stops at the first month that has one, so a past day costs one or
    /// two month files rather than the whole history; lines not naming the symbol are skipped
    /// before they are parsed. Unreadable lines are skipped (a lookup, not an audit).
    pub fn float_as_of(&self, symbol: &str, at: i64) -> Option<FloatRecord> {
        let dir = self.evidence_dir().join(FLOAT_DIR);
        let last = month_of(at);
        let mut files = month_files(&dir, None).ok()?;
        files.retain(|p| {
            p.file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|m| m <= last.as_str())
        });
        let needle = format!("\"symbol\":{}", serde_json::to_string(symbol).ok()?);
        for path in files.iter().rev() {
            let Ok(text) = std::fs::read_to_string(path) else {
                continue;
            };
            let best = text
                .lines()
                .filter(|l| l.contains(&needle))
                .filter_map(|l| parse_line::<FloatRecord>(l).ok())
                .filter(|r| r.symbol == symbol && r.retrieved_at <= at && r.float_shares.is_some())
                .max_by_key(|r| r.retrieved_at);
            if best.is_some() {
                return best;
            }
        }
        None
    }

    /// News versions, optionally only from files for months at or after `since` (UTC ns).
    pub fn read_news(&self, since: Option<i64>) -> Result<Vec<NewsVersionRecord>, ArchiveError> {
        read_all(&self.evidence_dir().join(NEWS_DIR), since)
    }

    pub fn read_news_coverage(
        &self,
        since: Option<i64>,
    ) -> Result<Vec<NewsCoverageRecord>, ArchiveError> {
        read_all(&self.evidence_dir().join(NEWS_COVERAGE_DIR), since)
    }
}

fn append_grouped<T: Serialize>(
    dir: &Path,
    rows: &[T],
    at: impl Fn(&T) -> i64,
) -> Result<(), ArchiveError> {
    if rows.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(dir).map_err(|e| ArchiveError::io(dir, e))?;
    let mut by_month: std::collections::BTreeMap<String, Vec<u8>> = Default::default();
    for r in rows {
        let buf = by_month.entry(month_of(at(r))).or_default();
        serde_json::to_writer(&mut *buf, r)
            .map_err(|e| ArchiveError::format(dir, e.to_string()))?;
        buf.push(b'\n');
    }
    for (month, buf) in by_month {
        let path = dir.join(format!("{month}.jsonl"));
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| ArchiveError::io(&path, e))?;
        // A previous crash may have left a line without its newline: start on a fresh line so the
        // torn line stays one bad line instead of corrupting this one too.
        let len = f.metadata().map_err(|e| ArchiveError::io(&path, e))?.len();
        if len > 0 && !ends_with_newline(&path, len)? {
            f.write_all(b"\n").map_err(|e| ArchiveError::io(&path, e))?;
        }
        f.write_all(&buf).map_err(|e| ArchiveError::io(&path, e))?;
        f.sync_data().map_err(|e| ArchiveError::io(&path, e))?;
    }
    Ok(())
}

fn ends_with_newline(path: &Path, len: u64) -> Result<bool, ArchiveError> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).map_err(|e| ArchiveError::io(path, e))?;
    f.seek(SeekFrom::Start(len - 1))
        .map_err(|e| ArchiveError::io(path, e))?;
    let mut b = [0u8; 1];
    f.read_exact(&mut b)
        .map_err(|e| ArchiveError::io(path, e))?;
    Ok(b[0] == b'\n')
}

#[derive(Deserialize)]
struct Versioned {
    v: u32,
}

/// The month files of `dir` from `since`'s month on, oldest first. Missing directory: none.
fn month_files(dir: &Path, since: Option<i64>) -> Result<Vec<PathBuf>, ArchiveError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(ArchiveError::io(dir, e)),
    };
    let first_month = since.map(month_of);
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    for e in entries {
        let e = e.map_err(|err| ArchiveError::io(dir, err))?;
        let name = e.file_name().to_string_lossy().to_string();
        let Some(month) = name.strip_suffix(".jsonl") else {
            continue;
        };
        if first_month.as_deref().is_some_and(|m| month < m) {
            continue;
        }
        files.push((month.to_string(), e.path()));
    }
    files.sort();
    Ok(files.into_iter().map(|(_, p)| p).collect())
}

fn parse_line<T: for<'de> Deserialize<'de>>(line: &str) -> Result<T, String> {
    serde_json::from_str::<Versioned>(line)
        .map_err(|e| e.to_string())
        .and_then(|v| {
            if v.v != EVIDENCE_SCHEMA {
                Err(format!("unsupported evidence schema {}", v.v))
            } else {
                serde_json::from_str::<T>(line).map_err(|e| e.to_string())
            }
        })
}

/// What one [`EvidenceTail`] call found: the new records, and the files it could not read this
/// time (their offsets did not move, so the next call tries them again).
#[derive(Debug)]
pub struct TailRead<T> {
    pub records: Vec<T>,
    pub errors: Vec<ArchiveError>,
}

/// Reads only what was appended to the evidence files since the previous call: the reader for a
/// process that follows them while the collector (`swimport evidence`) writes, without re-parsing
/// a month of floats (~165k lines) every few seconds.
///
/// Per file it remembers the byte offset just past the last newline it consumed. A line is only
/// consumed once its newline is on disk (the collector writes whole lines then syncs, so a
/// half-written append is simply not there yet). The torn-line rules are [`Archive::read_float`]'s:
/// a line that does not end in `}` with a newline after it is a crash's leftover and is skipped
/// (counted); any other bad line stops that file with an error and is retried next call. A file
/// shorter than its offset was replaced, never appended (the collector never does that): it is
/// read again from the start, so the caller must tolerate seeing a record twice.
#[derive(Debug, Default)]
pub struct EvidenceTail {
    offsets: std::collections::HashMap<PathBuf, u64>,
    /// Torn lines skipped so far.
    pub skipped: usize,
}

impl EvidenceTail {
    pub fn float(&mut self, a: &Archive, since: Option<i64>) -> TailRead<FloatRecord> {
        self.read(&a.evidence_dir().join(FLOAT_DIR), since)
    }

    pub fn news(&mut self, a: &Archive, since: Option<i64>) -> TailRead<NewsVersionRecord> {
        self.read(&a.evidence_dir().join(NEWS_DIR), since)
    }

    pub fn news_coverage(
        &mut self,
        a: &Archive,
        since: Option<i64>,
    ) -> TailRead<NewsCoverageRecord> {
        self.read(&a.evidence_dir().join(NEWS_COVERAGE_DIR), since)
    }

    fn read<T: for<'de> Deserialize<'de>>(
        &mut self,
        dir: &Path,
        since: Option<i64>,
    ) -> TailRead<T> {
        use std::io::{Read, Seek, SeekFrom};
        let mut out = TailRead {
            records: Vec::new(),
            errors: Vec::new(),
        };
        let files = match month_files(dir, since) {
            Ok(f) => f,
            Err(e) => {
                out.errors.push(e);
                return out;
            }
        };
        for path in files {
            let len = match std::fs::metadata(&path) {
                Ok(m) => m.len(),
                Err(e) => {
                    out.errors.push(ArchiveError::io(&path, e));
                    continue;
                }
            };
            let mut off = self.offsets.get(&path).copied().unwrap_or(0);
            if len < off {
                off = 0;
            }
            if len == off {
                continue;
            }
            let mut buf = Vec::with_capacity((len - off) as usize);
            let read = std::fs::File::open(&path).and_then(|mut f| {
                f.seek(SeekFrom::Start(off))?;
                f.read_to_end(&mut buf)
            });
            if let Err(e) = read {
                out.errors.push(ArchiveError::io(&path, e));
                continue;
            }
            // Only whole lines; the rest waits for its newline.
            let Some(end) = buf.iter().rposition(|b| *b == b'\n') else {
                continue;
            };
            let text = String::from_utf8_lossy(&buf[..end]);
            let mut rows = Vec::new();
            let mut failed = None;
            for (i, line) in text.split('\n').enumerate() {
                if line.trim().is_empty() {
                    continue;
                }
                match parse_line::<T>(line) {
                    Ok(r) => rows.push(r),
                    // A torn line that a later append already moved past.
                    Err(_) if !line.trim_end().ends_with('}') => self.skipped += 1,
                    Err(e) => {
                        failed = Some(ArchiveError::format(
                            &path,
                            format!("line {} after byte {off}: {e}", i + 1),
                        ));
                        break;
                    }
                }
            }
            match failed {
                Some(e) => out.errors.push(e),
                None => {
                    out.records.extend(rows);
                    self.offsets.insert(path, off + end as u64 + 1);
                }
            }
        }
        out
    }
}

fn read_all<T: for<'de> Deserialize<'de>>(
    dir: &Path,
    since: Option<i64>,
) -> Result<Vec<T>, ArchiveError> {
    let mut out = Vec::new();
    for path in month_files(dir, since)? {
        let text = std::fs::read_to_string(&path).map_err(|e| ArchiveError::io(&path, e))?;
        let lines: Vec<&str> = text.split('\n').collect();
        let last = lines.len().saturating_sub(1);
        for (i, line) in lines.iter().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let parsed = parse_line::<T>(line);
            match parsed {
                Ok(r) => out.push(r),
                // A torn final line (no newline after it) from a crash mid-append.
                Err(_) if i == last => {}
                // A torn line that a later append already moved past.
                Err(_) if i + 1 < lines.len() && !line.ends_with('}') => {}
                Err(e) => {
                    return Err(ArchiveError::format(&path, format!("line {}: {e}", i + 1)));
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ArchiveConfig;

    fn archive() -> (tempfile::TempDir, Archive) {
        let dir = tempfile::tempdir().unwrap();
        let cfg = ArchiveConfig {
            root: dir.path().join("archive"),
            retention_days: 0,
            min_free_bytes: 0,
        };
        (dir, Archive::new(cfg))
    }

    fn cov(id: &str, observed_at: i64) -> NewsCoverageRecord {
        NewsCoverageRecord {
            v: EVIDENCE_SCHEMA,
            id: id.into(),
            provider: "alpaca".into(),
            sources: vec!["benzinga".into()],
            scope: CoverageScope::All,
            covered_from: 0,
            covered_through: 1,
            observed_at,
            requests: 1,
            stories: 0,
        }
    }

    #[test]
    fn appends_by_month_and_reads_back_in_order() {
        let (_d, a) = archive();
        let oct = 1_792_000_000_000_000_000; // 2026-10
        let nov = oct + 40 * 86_400 * 1_000_000_000;
        a.append_news_coverage(&[cov("a", oct), cov("b", nov)])
            .unwrap();
        a.append_news_coverage(&[cov("c", oct)]).unwrap();
        let ids: Vec<_> = a
            .read_news_coverage(None)
            .unwrap()
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(ids, ["a", "c", "b"]);
        let ids: Vec<_> = a
            .read_news_coverage(Some(nov))
            .unwrap()
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(ids, ["b"]);
        assert!(a.days().unwrap().is_empty(), "never mistaken for a day");
    }

    #[test]
    fn a_torn_last_line_is_skipped_and_the_next_append_starts_a_new_line() {
        let (_d, a) = archive();
        let t = 1_792_000_000_000_000_000;
        a.append_news_coverage(&[cov("a", t)]).unwrap();
        let path = a
            .evidence_dir()
            .join(NEWS_COVERAGE_DIR)
            .join(format!("{}.jsonl", month_of(t)));
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        f.write_all(b"{\"v\":1,\"id\":\"tor").unwrap();
        drop(f);
        assert_eq!(a.read_news_coverage(None).unwrap().len(), 1);
        a.append_news_coverage(&[cov("b", t)]).unwrap();
        let ids: Vec<_> = a
            .read_news_coverage(None)
            .unwrap()
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(ids, ["a", "b"]);
    }

    #[test]
    fn the_tail_reads_each_whole_line_once_and_waits_for_a_newline() {
        let (_d, a) = archive();
        let t = 1_792_000_000_000_000_000;
        let mut tail = EvidenceTail::default();
        assert!(
            tail.news_coverage(&a, None).records.is_empty(),
            "no dir yet"
        );
        a.append_news_coverage(&[cov("a", t), cov("b", t)]).unwrap();
        let ids = |r: TailRead<NewsCoverageRecord>| -> Vec<String> {
            assert!(r.errors.is_empty(), "{:?}", r.errors);
            r.records.into_iter().map(|c| c.id).collect()
        };
        assert_eq!(ids(tail.news_coverage(&a, None)), ["a", "b"]);
        assert!(ids(tail.news_coverage(&a, None)).is_empty(), "nothing new");
        // Half an append (no newline yet): not consumed.
        let path = a
            .evidence_dir()
            .join(NEWS_COVERAGE_DIR)
            .join(format!("{}.jsonl", month_of(t)));
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        f.write_all(b"{\"v\":1,\"id\":\"tor").unwrap();
        drop(f);
        assert!(ids(tail.news_coverage(&a, None)).is_empty());
        // The next append starts a fresh line: the torn one is skipped, the new one read.
        a.append_news_coverage(&[cov("c", t)]).unwrap();
        assert_eq!(ids(tail.news_coverage(&a, None)), ["c"]);
        assert_eq!(tail.skipped, 1);
        // A later month is a new file, picked up on its own.
        let nov = t + 40 * 86_400 * 1_000_000_000;
        a.append_news_coverage(&[cov("d", nov)]).unwrap();
        assert_eq!(ids(tail.news_coverage(&a, None)), ["d"]);
        // A full read agrees with everything the tail handed out.
        let all: Vec<_> = a
            .read_news_coverage(None)
            .unwrap()
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(all, ["a", "b", "c", "d"]);
    }

    #[test]
    fn a_foreign_schema_is_refused() {
        let (_d, a) = archive();
        let t = 1_792_000_000_000_000_000;
        let mut c = cov("a", t);
        c.v = 2;
        a.append_news_coverage(&[c, cov("b", t)]).unwrap();
        assert!(a.read_news_coverage(None).is_err());
    }
}
