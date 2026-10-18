//! The replay days the app can switch to: every archive day with a manifest that is a trading
//! day, with whether it is fixture data and how many symbols it has.
//!
//! Served as `days` in `GET /api/replay/state` (the clients fetch it when the day menu opens; a
//! list that changes a few times a month has no business on the socket), and checked by the
//! `replay.control {action: "day"}` handler before it asks the driver to switch.
//!
//! The day list itself is one directory listing, but the fixture flag and symbol count come from
//! each day's manifest, and a real day's manifest carries an entry per symbol (thousands, with
//! sources and import provenance): parsing 22 of those on every request would cost far more
//! than the request is worth. So each day's summary is cached and keyed by the manifest file's
//! length and modification time: a day being recorded or re-imported is re-read once it changes,
//! a pruned day drops out because the listing no longer has it, and an unchanged day costs one
//! `stat`.

use chrono::NaiveDate;
use market_archive::Archive;
use market_events::session::sessions_for;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::SystemTime;

/// One selectable day, as served (`{"day": "2026-10-16", "fixture": false, "symbols": 4821}`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DayEntry {
    #[serde(serialize_with = "ymd")]
    pub day: NaiveDate,
    /// Synthetic data (`swarchive fixture`, `SIM_AUTOFIXTURE`), never market evidence.
    pub fixture: bool,
    /// Symbols in the day's manifest.
    pub symbols: usize,
}

fn ymd<S: serde::Serializer>(d: &NaiveDate, s: S) -> Result<S::Ok, S::Error> {
    s.collect_str(&d.format("%Y-%m-%d"))
}

/// Manifest identity for the cache: a rewrite changes the length or the mtime.
type Stamp = (u64, Option<SystemTime>);

pub struct DayCatalog {
    archive: Archive,
    cache: Mutex<HashMap<NaiveDate, (Stamp, DayEntry)>>,
}

impl DayCatalog {
    pub fn new(archive: Archive) -> Self {
        DayCatalog {
            archive,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Every replayable day, ascending (the archive's order; clients sort for display). A day
    /// whose manifest cannot be read is left out and logged rather than failing the whole list:
    /// one bad day must not hide the 21 good ones.
    pub fn list(&self) -> Vec<DayEntry> {
        let days = match self.archive.days() {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %e, "replay day list: archive unreadable");
                return Vec::new();
            }
        };
        let mut cache = self.cache.lock().expect("day cache lock");
        cache.retain(|d, _| days.contains(d));
        let mut out = Vec::with_capacity(days.len());
        for day in days {
            // Weekends and holidays cannot be replayed (the replay core refuses them), so they are
            // never offered.
            if sessions_for(day).is_empty() {
                continue;
            }
            let stamp = std::fs::metadata(self.archive.manifest_path(day))
                .map(|m| (m.len(), m.modified().ok()))
                .unwrap_or((0, None));
            if let Some((s, e)) = cache.get(&day)
                && *s == stamp
            {
                out.push(e.clone());
                continue;
            }
            match self.archive.manifest(day) {
                Ok(Some(m)) => {
                    let e = DayEntry {
                        day,
                        fixture: m.fixture,
                        symbols: m.symbols.len(),
                    };
                    cache.insert(day, (stamp, e.clone()));
                    out.push(e);
                }
                Ok(None) => {}
                Err(e) => tracing::warn!(%day, error = %e, "replay day list: manifest unreadable"),
            }
        }
        out
    }

    /// True when `day` is in [`DayCatalog::list`].
    pub fn contains(&self, day: NaiveDate) -> bool {
        self.list().iter().any(|e| e.day == day)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use market_archive::ArchiveConfig;
    use market_archive::fixture::{DEFAULT_SEED, write_fixture_day};

    fn d(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    #[test]
    fn lists_trading_days_and_follows_manifest_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = Archive::new(ArchiveConfig {
            root: tmp.path().join("archive"),
            retention_days: 0,
            min_free_bytes: 0,
        });
        let syms = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        write_fixture_day(&archive, d("2026-10-14"), &syms(&["SMPL"]), DEFAULT_SEED).unwrap();
        write_fixture_day(
            &archive,
            d("2026-10-15"),
            &syms(&["SMPL", "FXLO"]),
            DEFAULT_SEED,
        )
        .unwrap();
        // A manifest on a Saturday (an import gone wrong): never offered, it cannot be replayed.
        let sat = archive.manifest_path(d("2026-10-17"));
        std::fs::create_dir_all(sat.parent().unwrap()).unwrap();
        std::fs::copy(archive.manifest_path(d("2026-10-15")), &sat).unwrap();
        let cat = DayCatalog::new(archive.clone());
        let entry = |day: &str, symbols: usize| DayEntry {
            day: d(day),
            fixture: true,
            symbols,
        };
        assert_eq!(cat.list(), [entry("2026-10-14", 1), entry("2026-10-15", 2)]);
        assert!(cat.contains(d("2026-10-15")));
        assert!(!cat.contains(d("2026-10-17")));
        // Another symbol committed to a cached day: the rewritten manifest is read again.
        write_fixture_day(&archive, d("2026-10-14"), &syms(&["FXMD"]), DEFAULT_SEED).unwrap();
        assert_eq!(cat.list()[0], entry("2026-10-14", 2));
        // A pruned day drops out.
        std::fs::remove_dir_all(archive.day_dir(d("2026-10-15"))).unwrap();
        assert_eq!(cat.list(), [entry("2026-10-14", 2)]);
        assert_eq!(
            serde_json::to_value(entry("2026-10-14", 2)).unwrap(),
            serde_json::json!({"day": "2026-10-14", "fixture": true, "symbols": 2})
        );
    }
}
