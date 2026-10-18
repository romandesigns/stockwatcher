//! Official daily closes beside the days: `$ARCHIVE_ROOT/daily-closes.json`.
//!
//! ```json
//! {"2026-10-14": {"AAL": 12.34, "SMPL": 4.1}, "2026-10-15": {"AAL": 12.5, ...}, ...}
//! ```
//!
//! Trading date (`YYYY-MM-DD`, ET) → symbol → that day's close in dollars, as the provider's daily
//! bar reports it (`swimport alpaca-daily-closes`: the `c` of Alpaca's SIP `1Day` bar, unadjusted;
//! the reasons are on that command). Dates sort as strings, so the file diffs cleanly between runs
//! and a merge of two ranges is a plain union.
//!
//! What it is for: brokers quote "today's change" against the **previous trading day's close**,
//! not against today's open. A replay of day D (or live mode, D = today) cannot derive that close
//! from its own tape: the archive holds one day at a time, and the prior day's last regular print
//! is not the official close anyway (the closing auction prints after 16:00 and late reports keep
//! arriving). So the server reads this file once at startup and, for each symbol, takes the close
//! of the latest date **before** D that the file has for it ([`PrevCloses::before`]).
//!
//! It is **optional**, like `assets.json`: a missing or unreadable file only means change % falls
//! back to the open (logged by the server, never fatal). It sits at the archive root, not under a
//! day, and [`crate::Archive::days`] only lists date-named directories, so replay and retention
//! never mistake it for a day. For live mode it must be refreshed after every session (a daily
//! job, not built yet): a file that stops at D-2 makes the server quote change against D-2's close,
//! because "latest date before D present for the symbol" is all it can know.

use crate::Archive;
use crate::error::ArchiveError;
use chrono::NaiveDate;
use market_events::Price;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

pub const DAILY_CLOSES_FILE: &str = "daily-closes.json";

/// Trading date → symbol → close (dollars, as delivered). Sorted both ways for stable files.
pub type DailyCloses = BTreeMap<NaiveDate, BTreeMap<String, f64>>;

/// Adds `new` into `into`; a `(date, symbol)` present in both takes `new`'s value (a re-fetch is
/// the fresher answer). Returns how many `(date, symbol)` closes were added or changed.
pub fn merge_daily_closes(into: &mut DailyCloses, new: DailyCloses) -> usize {
    let mut changed = 0;
    for (day, closes) in new {
        let slot = into.entry(day).or_default();
        for (symbol, c) in closes {
            if slot.insert(symbol, c) != Some(c) {
                changed += 1;
            }
        }
    }
    changed
}

impl Archive {
    pub fn daily_closes_path(&self) -> PathBuf {
        self.root().join(DAILY_CLOSES_FILE)
    }

    /// The daily closes, or `None` when the file does not exist (it is optional). A date key that
    /// is not `YYYY-MM-DD` makes the whole file invalid rather than silently dropping a day: a
    /// hand-edited or foreign file should be noticed, not half-used.
    pub fn read_daily_closes(&self) -> Result<Option<DailyCloses>, ArchiveError> {
        let path = self.daily_closes_path();
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(ArchiveError::io(&path, e)),
        };
        let raw: BTreeMap<String, BTreeMap<String, f64>> = serde_json::from_slice(&bytes)
            .map_err(|e| ArchiveError::format(&path, e.to_string()))?;
        let mut out = DailyCloses::new();
        for (k, v) in raw {
            let day = NaiveDate::parse_from_str(&k, "%Y-%m-%d").map_err(|_| {
                ArchiveError::format(&path, format!("date key {k:?} is not YYYY-MM-DD"))
            })?;
            out.insert(day, v);
        }
        Ok(Some(out))
    }

    /// Replaces the file atomically (temp file, fsync, rename), so a server starting mid-write
    /// reads either the old file or the new one, never half of one. Callers merge first
    /// ([`merge_daily_closes`]) when they mean to extend it.
    pub fn write_daily_closes(&self, closes: &DailyCloses) -> Result<PathBuf, ArchiveError> {
        let root = self.root();
        std::fs::create_dir_all(root).map_err(|e| ArchiveError::io(root, e))?;
        let path = self.daily_closes_path();
        let tmp = path.with_extension("json.tmp");
        let raw: BTreeMap<String, &BTreeMap<String, f64>> =
            closes.iter().map(|(d, m)| (d.to_string(), m)).collect();
        // Compact, not pretty: a few months of every US equity is ~10^6 entries.
        let json = serde_json::to_vec(&raw).expect("daily closes serialize");
        std::fs::write(&tmp, json).map_err(|e| ArchiveError::io(&tmp, e))?;
        if let Ok(f) = std::fs::File::open(&tmp) {
            let _ = f.sync_all();
        }
        std::fs::rename(&tmp, &path).map_err(|e| ArchiveError::io(&path, e))?;
        Ok(path)
    }
}

/// The closes indexed for the server's lookup: per symbol, `(date, close)` ascending. Built once
/// at startup; a lookup is one hash probe and a binary search, done when a symbol's trading date
/// is first seen, never per print.
#[derive(Clone, Debug, Default)]
pub struct PrevCloses {
    by_symbol: HashMap<String, Vec<(NaiveDate, Price)>>,
}

impl PrevCloses {
    /// Indexes `closes`. A close that is not a finite positive price is dropped: a 0 or negative
    /// reference would turn every change % into ±infinity or nonsense, and the open fallback is
    /// the honest answer for that symbol. Returns the index and how many closes were dropped.
    pub fn new(closes: &DailyCloses) -> (Self, usize) {
        let mut by_symbol: HashMap<String, Vec<(NaiveDate, Price)>> = HashMap::new();
        let mut dropped = 0;
        // Dates ascend in the BTreeMap, so every per-symbol list is pushed in date order.
        for (day, m) in closes {
            for (symbol, &c) in m {
                match Price::from_f64_rounded(c).filter(|p| p.micros() > 0) {
                    Some(p) => by_symbol.entry(symbol.clone()).or_default().push((*day, p)),
                    None => dropped += 1,
                }
            }
        }
        (PrevCloses { by_symbol }, dropped)
    }

    pub fn is_empty(&self) -> bool {
        self.by_symbol.is_empty()
    }

    /// Symbols with at least one close.
    pub fn symbols(&self) -> usize {
        self.by_symbol.len()
    }

    /// The close of the latest date strictly before `day` that has one for `symbol`, with that
    /// date. Per symbol, not "the file's latest day": a stock that had no bar on D-1 (halted all
    /// day) is still quoted against its last official close, as a broker does. `None` when the
    /// file has nothing before `day` for it (then the caller falls back to the open).
    pub fn before(&self, symbol: &str, day: NaiveDate) -> Option<(NaiveDate, Price)> {
        let v = self.by_symbol.get(symbol)?;
        let i = v.partition_point(|(d, _)| *d < day);
        i.checked_sub(1).map(|i| v[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ArchiveConfig;

    fn archive(dir: &std::path::Path) -> Archive {
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

    fn closes(rows: &[(&str, &str, f64)]) -> DailyCloses {
        let mut m = DailyCloses::new();
        for (day, sym, c) in rows {
            m.entry(d(day)).or_default().insert(sym.to_string(), *c);
        }
        m
    }

    #[test]
    fn missing_is_none_and_a_write_round_trips_without_becoming_a_day() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        assert!(a.read_daily_closes().unwrap().is_none());
        let m = closes(&[("2026-10-14", "AAA", 1.25), ("2026-10-15", "AAA", 1.5)]);
        a.write_daily_closes(&m).unwrap();
        assert_eq!(a.read_daily_closes().unwrap(), Some(m));
        assert!(!a.daily_closes_path().with_extension("json.tmp").exists());
        assert!(a.days().unwrap().is_empty(), "never mistaken for a day");
        let text = std::fs::read_to_string(a.daily_closes_path()).unwrap();
        assert_eq!(
            text,
            r#"{"2026-10-14":{"AAA":1.25},"2026-10-15":{"AAA":1.5}}"#
        );
    }

    #[test]
    fn invalid_json_or_a_bad_date_key_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        std::fs::create_dir_all(a.root()).unwrap();
        std::fs::write(a.daily_closes_path(), b"not json").unwrap();
        assert!(a.read_daily_closes().is_err());
        std::fs::write(a.daily_closes_path(), br#"{"14/10/2026":{"AAA":1}}"#).unwrap();
        assert!(a.read_daily_closes().is_err());
    }

    #[test]
    fn a_merge_is_a_union_and_the_new_value_wins() {
        let mut m = closes(&[("2026-10-14", "AAA", 1.0), ("2026-10-14", "BBB", 2.0)]);
        let n = merge_daily_closes(
            &mut m,
            closes(&[
                ("2026-10-14", "BBB", 2.0),
                ("2026-10-14", "AAA", 1.1),
                ("2026-10-15", "AAA", 1.2),
            ]),
        );
        assert_eq!(n, 2, "BBB's identical close is not a change");
        assert_eq!(
            m,
            closes(&[
                ("2026-10-14", "AAA", 1.1),
                ("2026-10-14", "BBB", 2.0),
                ("2026-10-15", "AAA", 1.2),
            ])
        );
    }

    #[test]
    fn the_reference_is_the_latest_close_strictly_before_the_day_per_symbol() {
        let (p, dropped) = PrevCloses::new(&closes(&[
            ("2026-10-13", "AAA", 1.0),
            ("2026-10-14", "AAA", 1.1),
            ("2026-10-15", "AAA", 1.2),
            // BBB had no bar on the 14th (halted all day): its reference is the 13th's close.
            ("2026-10-13", "BBB", 5.0),
            ("2026-10-15", "BBB", 5.5),
            ("2026-10-14", "ZERO", 0.0),
        ]));
        assert_eq!(dropped, 1, "a zero close is no reference");
        assert_eq!(p.symbols(), 2);
        assert_eq!(
            p.before("AAA", d("2026-10-15")),
            Some((d("2026-10-14"), Price::from_cents(110))),
            "the day's own close is never its reference"
        );
        assert_eq!(
            p.before("BBB", d("2026-10-15")),
            Some((d("2026-10-13"), Price::from_cents(500)))
        );
        // Live mode a weekend later: still the latest close the file has.
        assert_eq!(
            p.before("AAA", d("2026-10-19")),
            Some((d("2026-10-15"), Price::from_cents(120)))
        );
        assert_eq!(p.before("AAA", d("2026-10-13")), None, "nothing before");
        assert_eq!(p.before("ZERO", d("2026-10-15")), None);
        assert_eq!(p.before("MISSING", d("2026-10-15")), None);
    }
}
