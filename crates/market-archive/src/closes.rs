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
//! arriving). So the server reads this file at startup (and again whenever its length or mtime
//! changes: the archive maintainer rewrites it after every session) and, for each symbol, takes
//! the close of the latest date **before** D that the file has for it ([`PrevCloses::before`]).
//!
//! It is **optional**, like `assets.json`: a missing or unreadable file only means change % falls
//! back to the open (logged by the server, never fatal). It sits at the archive root, not under a
//! day, and [`crate::Archive::days`] only lists date-named directories, so replay and retention
//! never mistake it for a day. For live mode it must be refreshed after every session, which
//! `swimport maintain` does (market-providers `import::maintain`, the VPS `archiver` service): a
//! file that stops at D-2 makes the server quote change against D-2's close, because "latest date
//! before D present for the symbol" is all it can know.

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

/// The closes indexed for the server's lookup: per symbol, `(date, close)` ascending. Built when
/// the file is (re)read; a lookup is one hash probe and a binary search, done when a symbol's
/// trading date is first seen, never per print.
///
/// **Splits.** The file is raw (unadjusted, on purpose: see `swimport alpaca-daily-closes`), so a
/// close from before a split is on another share basis than today's prints, and a 1-for-40
/// reverse split reads as +3,900 % against it (owner, 2026-10-30: ATYR, OMEX and ENHA topped Top
/// Gainers that way). [`PrevCloses::with_splits`] adds the splits the corporate-actions ledger
/// knows ([`splits_from_ledger`]) and [`PrevCloses::before`] then answers on the asked day's
/// basis. [`PrevCloses::before_raw`] keeps the file's own value for consumers that grade the
/// basis themselves.
#[derive(Clone, Debug, Default)]
pub struct PrevCloses {
    by_symbol: HashMap<String, Vec<(NaiveDate, Price)>>,
    /// Known splits per symbol, `(effective date, price multiplier old/new)`, ascending.
    splits: HashMap<String, Vec<(NaiveDate, f64)>>,
}

/// A previous close as [`PrevCloses::lookup`] finds it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CloseRef {
    /// The session the close belongs to.
    pub date: NaiveDate,
    /// On the asked day's share basis: the raw close times every known split that took effect
    /// after `date` and on or before the asked day.
    pub price: Price,
    /// At least one split was applied.
    pub split_adjusted: bool,
}

/// One split: `symbol`'s prices before `effective` are multiplied by `factor` (old/new shares)
/// to be on its basis from `effective` on. A 1-for-40 reverse split (40 old → 1 new) is 40.
#[derive(Clone, Debug, PartialEq)]
pub struct SplitFact {
    pub symbol: String,
    pub effective: NaiveDate,
    pub factor: f64,
}

/// The splits a corporate-actions ledger knows (`crate::corporate_actions`, kept by
/// `swimport maintain` from the provider's announcements, fetched pre-session and every 30
/// minutes): each action's latest version, complete forward and reverse splits with an effective
/// date and positive rates. Incomplete or undated records are left out (they cannot say by how
/// much); the server's implausible-gap guard covers those symbols instead.
pub fn splits_from_ledger(ledger: &crate::ActionLedger) -> Vec<SplitFact> {
    ledger
        .values()
        .filter_map(|e| e.versions.last().map(|(_, _, a)| a))
        .filter(|a| a.kind.is_split() && a.complete)
        .filter_map(|a| {
            let (o, n) = (a.old_rate?, a.new_rate?);
            let ok = o.is_finite() && n.is_finite() && o > 0.0 && n > 0.0 && o != n;
            ok.then_some(())?;
            Some(SplitFact {
                symbol: a.symbol.clone()?,
                effective: a.effective_date?,
                factor: o / n,
            })
        })
        .collect()
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
        (
            PrevCloses {
                by_symbol,
                splits: HashMap::new(),
            },
            dropped,
        )
    }

    /// The same closes with `splits` applied by [`PrevCloses::lookup`] / [`PrevCloses::before`]
    /// (replacing any set before). Two records of one split (same symbol and date) count once.
    pub fn with_splits(mut self, splits: impl IntoIterator<Item = SplitFact>) -> Self {
        let mut by: HashMap<String, Vec<(NaiveDate, f64)>> = HashMap::new();
        for s in splits {
            let v = by.entry(s.symbol).or_default();
            if !v.iter().any(|(d, _)| *d == s.effective) {
                v.push((s.effective, s.factor));
            }
        }
        for v in by.values_mut() {
            v.sort_by_key(|(d, _)| *d);
        }
        self.splits = by;
        self
    }

    pub fn is_empty(&self) -> bool {
        self.by_symbol.is_empty()
    }

    /// Symbols with at least one close.
    pub fn symbols(&self) -> usize {
        self.by_symbol.len()
    }

    /// Symbols with at least one known split.
    pub fn split_symbols(&self) -> usize {
        self.splits.len()
    }

    /// The close of the latest date strictly before `day` that has one for `symbol`, with that
    /// date. Per symbol, not "the file's latest day": a stock that had no bar on D-1 (halted all
    /// day) is still quoted against its last official close, as a broker does. `None` when the
    /// file has nothing before `day` for it (then the caller falls back to the open).
    ///
    /// On `day`'s share basis when a known split took effect after that close (type docs).
    pub fn before(&self, symbol: &str, day: NaiveDate) -> Option<(NaiveDate, Price)> {
        self.lookup(symbol, day).map(|c| (c.date, c.price))
    }

    /// The file's close as delivered (unadjusted): for the stock-context engine, which labels it
    /// `Raw` and checks corporate actions through its own evidence.
    pub fn before_raw(&self, symbol: &str, day: NaiveDate) -> Option<(NaiveDate, Price)> {
        let v = self.by_symbol.get(symbol)?;
        let i = v.partition_point(|(d, _)| *d < day);
        i.checked_sub(1).map(|i| v[i])
    }

    /// [`PrevCloses::before`], saying whether a split was applied.
    pub fn lookup(&self, symbol: &str, day: NaiveDate) -> Option<CloseRef> {
        let (date, raw) = self.before_raw(symbol, day)?;
        let factor: f64 = self
            .splits
            .get(symbol)
            .into_iter()
            .flatten()
            .filter(|(eff, _)| *eff > date && *eff <= day)
            .map(|(_, f)| *f)
            .product();
        if factor == 1.0 {
            return Some(CloseRef {
                date,
                price: raw,
                split_adjusted: false,
            });
        }
        let price = Price::from_f64_rounded(raw.to_f64() * factor).filter(|p| p.micros() > 0)?;
        Some(CloseRef {
            date,
            price,
            split_adjusted: true,
        })
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

    fn action(
        id: &str,
        kind: crate::ActionKind,
        sym: &str,
        eff: &str,
        old: f64,
        new: f64,
        complete: bool,
    ) -> crate::CorporateAction {
        crate::CorporateAction {
            id: id.into(),
            kind,
            symbol: Some(sym.into()),
            new_symbol: None,
            alternate_symbol: None,
            effective_date: Some(d(eff)),
            process_date: Some(d(eff)),
            old_rate: Some(old),
            new_rate: Some(new),
            quality: None,
            complete,
            raw: serde_json::Value::Null,
        }
    }

    /// The owner's case (2026-10-30): ATYR closed at $0.128, then a 1-for-40 reverse split took
    /// effect and it trades at $5.17. Raw, that is +3,939 %; on today's basis the close is $5.12
    /// and the move is +1 %.
    #[test]
    fn a_known_split_puts_the_previous_close_on_the_day_basis() {
        use crate::{ActionKind, LedgerEntry};
        let mut ledger = crate::ActionLedger::new();
        let mut put = |a: crate::CorporateAction| {
            let at = chrono::Utc::now();
            ledger.insert(
                a.id.clone(),
                LedgerEntry {
                    first_observed: at,
                    first_retrieval: "r".into(),
                    versions: vec![(at, "r".into(), a)],
                },
            );
        };
        put(action(
            "1",
            ActionKind::ReverseSplit,
            "ATYR",
            "2026-10-30",
            40.0,
            1.0,
            true,
        ));
        put(action(
            "2",
            ActionKind::ForwardSplit,
            "FWD",
            "2026-10-30",
            1.0,
            4.0,
            true,
        ));
        // Incomplete: no amount can be trusted, so nothing is applied (the server's guard covers it).
        put(action(
            "3",
            ActionKind::ReverseSplit,
            "HALF",
            "2026-10-30",
            10.0,
            1.0,
            false,
        ));
        put(action(
            "4",
            ActionKind::NameChange,
            "NAME",
            "2026-10-30",
            1.0,
            1.0,
            true,
        ));
        let splits = splits_from_ledger(&ledger);
        assert_eq!(splits.len(), 2);
        let (p, _) = PrevCloses::new(&closes(&[
            ("2026-10-29", "ATYR", 0.128),
            ("2026-10-29", "FWD", 80.0),
            ("2026-10-29", "HALF", 0.2),
            ("2026-10-28", "OLD", 1.0),
        ]));
        let p = p.with_splits(splits.into_iter().chain([
            // A split before the close is already in the close: never applied again.
            SplitFact {
                symbol: "OLD".into(),
                effective: d("2026-10-27"),
                factor: 10.0,
            },
        ]));
        assert_eq!(p.split_symbols(), 3);
        let atyr = p.lookup("ATYR", d("2026-10-30")).unwrap();
        assert_eq!(
            (atyr.price, atyr.split_adjusted),
            (Price::from_cents(512), true)
        );
        assert_eq!(
            p.before_raw("ATYR", d("2026-10-30")).unwrap().1,
            Price::from_micros(128_000)
        );
        assert_eq!(
            p.before("FWD", d("2026-10-30")).unwrap().1,
            Price::from_cents(2_000)
        );
        // Asked for the day before the split: the raw close is still that day's basis.
        assert!(
            !p.lookup("ATYR", d("2026-10-29"))
                .is_some_and(|c| c.split_adjusted)
        );
        let half = p.lookup("HALF", d("2026-10-30")).unwrap();
        assert_eq!(
            (half.price, half.split_adjusted),
            (Price::from_micros(200_000), false)
        );
        assert!(!p.lookup("OLD", d("2026-10-30")).unwrap().split_adjusted);
    }
}
