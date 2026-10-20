//! The preserved original: the live recording, exactly as received, kept after the nightly import
//! has replaced it.
//!
//! # Why keep it
//!
//! The import is the complete, corrected day in time order, which is what replay wants. Forward
//! research wants the opposite: what the live market actually received, in the order and with
//! the delivery times it received it (late prints where they arrived, the gaps a disconnect left).
//! Only the recording has that, and the import used to delete it. So [`Archive::finish_import`]
//! now **moves** the recording here instead, unchanged.
//!
//! # Layout
//!
//! ```text
//! $ARCHIVE_ROOT/live-original/2026-10-15/   # was 2026-10-15/live/, moved by one rename
//!   manifest.json                           # the recording's manifest: "live": {"sealed": true},
//!                                           #   counts, bytes and sha256 per file
//!   quotes.json, SMPL.swa, …                # untouched
//! ```
//!
//! At the archive root, not inside the day: the rolling window (`swimport maintain`) and
//! `ARCHIVE_RETENTION_DAYS` remove whole day directories, and the original has its own retention
//! (`LIVE_ORIGINAL_DAYS`, applied by the recorder with [`Archive::prune_originals`]). A sibling
//! directory is what makes it *pinned*: nothing that manages imported days can delete it.
//!
//! # Immutable after sealing
//!
//! Sealing (truncating torn tails, recounting, compacting each file to one frame, hashing) is the
//! last write a recording gets, and the move does not change a byte. A recording moved before it
//! was sealed (the recorder was down when the import finished) is sealed here once, by the
//! recorder's housekeeping ([`Archive::seal_original`]); a sealed original is never written again.
//! Nothing else writes here: the live writer only writes `<day>/live/`, and refuses an imported
//! day anyway. [`Archive::verify_original`] rechecks the hashes.

use crate::archive::read_manifest_file;
use crate::error::ArchiveError;
use crate::lifecycle::REMOVING_PREFIX;
use crate::manifest::DayManifest;
use crate::reader::SymbolReader;
use crate::{Archive, VerifyReport, valid_symbol};
use chrono::NaiveDate;
use std::path::PathBuf;

/// The originals' directory at the archive root.
pub const ORIGINALS_DIR: &str = "live-original";

impl Archive {
    pub fn originals_dir(&self) -> PathBuf {
        self.root().join(ORIGINALS_DIR)
    }

    pub fn original_dir(&self, day: NaiveDate) -> PathBuf {
        self.originals_dir()
            .join(day.format("%Y-%m-%d").to_string())
    }

    pub fn original_manifest_path(&self, day: NaiveDate) -> PathBuf {
        self.original_dir(day).join(crate::format::MANIFEST)
    }

    pub fn original_manifest(&self, day: NaiveDate) -> Result<Option<DayManifest>, ArchiveError> {
        read_manifest_file(&self.original_manifest_path(day))
    }

    /// Days with a preserved original (a manifest), ascending.
    pub fn original_days(&self) -> Result<Vec<NaiveDate>, ArchiveError> {
        let dir = self.originals_dir();
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(ArchiveError::io(&dir, e)),
        };
        let mut days: Vec<NaiveDate> = rd
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let d = NaiveDate::parse_from_str(&name, "%Y-%m-%d").ok()?;
                self.original_manifest_path(d).is_file().then_some(d)
            })
            .collect();
        days.sort();
        Ok(days)
    }

    /// A preserved original's events for `symbol`, in the order they were received.
    pub fn original_reader(
        &self,
        day: NaiveDate,
        symbol: &str,
    ) -> Result<SymbolReader, ArchiveError> {
        if !valid_symbol(symbol) {
            return Err(ArchiveError::InvalidSymbol(symbol.to_string()));
        }
        SymbolReader::open(
            &self
                .original_dir(day)
                .join(format!("{symbol}.{}", crate::format::EXT)),
        )
    }

    /// Moves `day`'s live recording to its preserved original (one rename on the same volume, so
    /// no byte changes and no reader sees half of it). Returns whether there was one. Refuses
    /// (`Format` error, nothing moved) when the day already has an original: an original is
    /// never overwritten.
    pub fn preserve_live(&self, day: NaiveDate) -> Result<bool, ArchiveError> {
        let from = self.live_dir(day);
        if !from.is_dir() {
            return Ok(false);
        }
        let to = self.original_dir(day);
        if to.exists() {
            return Err(ArchiveError::format(
                &to,
                "the day already has a preserved original; the recording stays in live/",
            ));
        }
        let parent = self.originals_dir();
        std::fs::create_dir_all(&parent).map_err(|e| ArchiveError::io(&parent, e))?;
        std::fs::rename(&from, &to).map_err(|e| ArchiveError::io(&from, e))?;
        Ok(true)
    }

    /// Seals a preserved original that was moved before it was sealed (module docs); a sealed
    /// one is left alone. Returns whether it sealed.
    pub fn seal_original(&self, day: NaiveDate) -> Result<bool, ArchiveError> {
        let sealed = self
            .original_manifest(day)?
            .and_then(|m| m.live)
            .is_some_and(|l| l.sealed);
        if sealed {
            return Ok(false);
        }
        self.seal_dir(day, &self.original_dir(day))?;
        Ok(true)
    }

    /// Recomputes every file's SHA-256 and record count against the original's manifest.
    pub fn verify_original(&self, day: NaiveDate) -> Result<VerifyReport, ArchiveError> {
        let m = self
            .original_manifest(day)?
            .ok_or_else(|| ArchiveError::NotFound(self.original_manifest_path(day)))?;
        self.verify_in(day, &self.original_dir(day), &m)
    }

    /// Keeps the newest `keep` originals and removes the rest (renamed out of the listing first,
    /// like [`Archive::remove_day`]). `keep` 0 removes all. Returns the days removed.
    pub fn prune_originals(&self, keep: usize) -> Result<Vec<NaiveDate>, ArchiveError> {
        let days = self.original_days()?;
        let cut = days.len().saturating_sub(keep);
        let mut out = Vec::new();
        for d in &days[..cut] {
            let dir = self.original_dir(*d);
            let doomed = self.root().join(format!(
                "{REMOVING_PREFIX}{}-{ORIGINALS_DIR}",
                d.format("%Y-%m-%d")
            ));
            match std::fs::rename(&dir, &doomed) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(ArchiveError::io(&dir, e)),
            }
            std::fs::remove_dir_all(&doomed).map_err(|e| ArchiveError::io(&doomed, e))?;
            out.push(*d);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coverage::{
        Collection, CoverageBasis, CoverageEvent, CoverageRecord, GapReason, LinkStatus,
        Subscription,
    };
    use crate::writer::FsyncPolicy;
    use crate::{ArchiveConfig, LiveWriter};
    use market_events::{MarketEvent, Price, Trade, UnixNanos};
    use std::collections::BTreeMap;
    use std::path::Path;

    fn d(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    fn archive(dir: &Path) -> Archive {
        Archive::new(ArchiveConfig {
            root: dir.join("archive"),
            retention_days: 0,
            min_free_bytes: 0,
        })
    }

    fn trade(sym: &str, i: u64) -> MarketEvent {
        let t = UnixNanos(1_792_075_200_000_000_000 + i as i64 * 1_000_000);
        let mut tr = Trade::new(sym, t, Price::from_cents(1000 + i as i64 % 50), 100, i);
        tr.receive_ts = Some(UnixNanos(t.0 + 5_000_000));
        MarketEvent::Trade(tr)
    }

    fn record(a: &Archive, day: NaiveDate, n: u64) {
        let mut w = LiveWriter::open(a, day, 0, "test").unwrap();
        for i in 0..n {
            w.append(&trade(if i % 2 == 0 { "AAA" } else { "BBB" }, i))
                .unwrap();
        }
        w.finish().unwrap();
    }

    fn import(a: &Archive, day: NaiveDate) {
        a.mark_importing(day).unwrap();
        let mut w = a.writer(day, "AAA", FsyncPolicy::Never).unwrap();
        w.append(&trade("AAA", 1)).unwrap();
        a.commit(day, w.finalize().unwrap()).unwrap();
        a.finish_import(day).unwrap();
    }

    /// Every file of a directory, by name, with its bytes.
    fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                (
                    e.file_name().into_string().unwrap(),
                    std::fs::read(e.path()).unwrap(),
                )
            })
            .collect()
    }

    #[test]
    fn the_import_preserves_the_sealed_recording_byte_for_byte() {
        let tmp = tempfile::tempdir().unwrap();
        let a = archive(tmp.path());
        let day = d("2026-10-15");
        record(&a, day, 2_000);
        a.write_live_quotes(day, &[]).unwrap();
        a.seal_live(day).unwrap();
        let before = snapshot(&a.live_dir(day));
        assert!(before.contains_key("AAA.swa") && before.contains_key("manifest.json"));

        import(&a, day);
        // Readers take the import; the recording left the day, unchanged, for its original.
        assert_eq!(a.day_data(day), Some(crate::DayData::Final));
        assert!(!a.live_dir(day).exists());
        assert_eq!(a.original_days().unwrap(), [day]);
        assert_eq!(snapshot(&a.original_dir(day)), before, "byte-identical");
        assert!(a.verify_original(day).unwrap().ok());
        let n = a.original_reader(day, "BBB").unwrap().count();
        assert_eq!(n, 1_000, "receipt-order events still readable");
        // Housekeeping never rewrites a sealed original.
        assert!(!a.seal_original(day).unwrap());
        assert_eq!(snapshot(&a.original_dir(day)), before);
        // The rolling window removes the imported day; the original is pinned beside it.
        a.remove_day(day).unwrap();
        assert_eq!(a.original_days().unwrap(), [day]);
        assert_eq!(snapshot(&a.original_dir(day)), before);
    }

    #[test]
    fn a_recording_moved_unsealed_is_sealed_once_and_never_overwritten() {
        let tmp = tempfile::tempdir().unwrap();
        let a = archive(tmp.path());
        let day = d("2026-10-15");
        record(&a, day, 100);
        // A killed recorder: a torn last frame, never sealed. Then the import finishes.
        let mut w = LiveWriter::open(&a, day, 0, "test").unwrap();
        for i in 100..110 {
            w.append(&trade("AAA", i * 2)).unwrap();
        }
        w.flush().unwrap();
        drop(w);
        let path = a.live_path(day, "AAA");
        let grown = std::fs::read(&path).unwrap();
        std::fs::write(&path, &grown[..grown.len() - 3]).unwrap();
        import(&a, day);
        let unsealed = a.original_manifest(day).unwrap().unwrap();
        assert!(!unsealed.live.unwrap().sealed);
        assert!(a.seal_original(day).unwrap());
        let m = a.original_manifest(day).unwrap().unwrap();
        assert!(m.live.unwrap().sealed);
        assert_eq!(m.symbols["AAA"].events, 50, "the torn frame is cut");
        assert!(a.verify_original(day).unwrap().ok());
        // The cut is on record.
        let log = a.read_coverage(day).unwrap().unwrap();
        assert!(log.records.iter().any(|r| matches!(
            &r.event,
            CoverageEvent::TornTail { symbol, .. } if symbol == "AAA"
        )));
        // A second recording of a preserved day never replaces the original.
        std::fs::create_dir_all(a.live_dir(day)).unwrap();
        assert!(a.preserve_live(day).is_err());
        assert!(a.live_dir(day).exists() && a.verify_original(day).unwrap().ok());
    }

    #[test]
    fn retention_removes_only_what_policy_says() {
        let tmp = tempfile::tempdir().unwrap();
        let a = archive(tmp.path());
        let days = [
            d("2026-10-12"),
            d("2026-10-13"),
            d("2026-10-14"),
            d("2026-10-15"),
        ];
        for day in days {
            record(&a, day, 10);
            a.append_coverage(day, &[CoverageRecord::new(0, CoverageEvent::Heartbeat)])
                .unwrap();
        }
        for day in &days[..3] {
            import(&a, *day);
        }
        assert_eq!(a.original_days().unwrap(), days[..3]);
        // Keep the newest two originals: only the oldest goes; imports and live untouched.
        assert_eq!(a.prune_originals(2).unwrap(), [days[0]]);
        assert_eq!(a.original_days().unwrap(), days[1..3]);
        assert_eq!(a.days().unwrap(), days[..3]);
        assert_eq!(a.live_days().unwrap(), [days[3]]);
        assert!(a.prune_originals(2).unwrap().is_empty());
        // Coverage goes only once nothing of its day is left.
        assert!(
            a.prune_coverage(None).unwrap().is_empty(),
            "10-12 is imported"
        );
        a.remove_day(days[0]).unwrap();
        assert_eq!(a.prune_coverage(None).unwrap(), [days[0]]);
        assert_eq!(a.coverage_days().unwrap(), days[1..]);
        // 0 keeps none.
        assert_eq!(a.prune_originals(0).unwrap(), days[1..3]);
        assert_eq!(a.sweep_removed().unwrap(), 0, "nothing left half-removed");
    }

    #[test]
    fn coverage_on_disk_answers_and_an_old_day_is_unverified() {
        let tmp = tempfile::tempdir().unwrap();
        let a = archive(tmp.path());
        let day = d("2026-10-15");
        record(&a, day, 10);
        let s = 1_000_000_000i64;
        // An old day: no coverage file. Never complete.
        let r = a
            .coverage_at(day, "AAA", 0, 100 * s, CoverageBasis::Stream, 200 * s)
            .unwrap();
        assert_eq!(r.collection, Collection::Unverified);
        assert_eq!(r.unverified, [(0, 100 * s)]);
        let recs = [
            CoverageRecord::new(
                0,
                CoverageEvent::ProcessStart {
                    version: "t".into(),
                },
            ),
            CoverageRecord::new(
                0,
                CoverageEvent::Subscription {
                    symbols: Subscription::All,
                },
            ),
            CoverageRecord::new(
                s,
                CoverageEvent::Link {
                    state: LinkStatus::Up,
                    detail: None,
                },
            ),
            CoverageRecord::new(50 * s, CoverageEvent::Heartbeat),
            CoverageRecord::new(
                60 * s,
                CoverageEvent::Link {
                    state: LinkStatus::Down,
                    detail: Some("network".into()),
                },
            ),
            CoverageRecord::new(90 * s, CoverageEvent::ProcessStop),
        ];
        a.append_coverage(day, &recs[..3]).unwrap();
        a.append_coverage(day, &recs[3..]).unwrap();
        let ask = |from: i64, to: i64, basis| {
            a.coverage_at(day, "QUIET", from * s, to * s, basis, 1_000 * s)
                .unwrap()
        };
        assert_eq!(
            ask(2, 59, CoverageBasis::Stream).collection,
            Collection::Complete
        );
        assert_eq!(
            ask(2, 59, CoverageBasis::Recording).collection,
            Collection::Complete
        );
        let r = ask(2, 70, CoverageBasis::Stream);
        assert_eq!(r.collection, Collection::Partial);
        assert_eq!(r.gaps[0].reason, GapReason::LinkDown);
        assert_eq!((r.gaps[0].from, r.gaps[0].to), (60 * s, 70 * s));
        // The recording is gone (retired, not preserved): nothing left to be complete.
        a.retire_live(day).unwrap();
        assert_eq!(
            ask(2, 59, CoverageBasis::Recording).collection,
            Collection::Unverified
        );
        assert_eq!(
            ask(2, 59, CoverageBasis::Stream).collection,
            Collection::Complete
        );
    }
}
