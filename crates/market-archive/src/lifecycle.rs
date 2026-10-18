//! A day's lifecycle beyond writing it: hidden while a background import fills it, and removed
//! so that no reader ever sees half of it.
//!
//! # The importing marker
//!
//! A day's manifest appears as soon as its first symbol is committed, and an import of a real
//! SIP day then commits ~6,700 more over the next hour or so. Without a marker, the server would
//! offer that day in its menu (and a restart without `REPLAY_DAY` would pick it as the latest)
//! with a few hundred symbols, and a replay opened then would never see the rest: the replay
//! opens a day's files once. So the archive maintainer (`swimport maintain`) drops an empty
//! `.importing` file into a day directory **before** the day's first symbol is written and
//! removes it once the day's first full pass is over; [`Archive::ready_days`] (what replay and
//! the server's day catalog list) leaves such days out. [`Archive::days`] still lists them: the
//! importer and retention need every day with a manifest.
//!
//! The marker is a plain file next to the manifest rather than a manifest field because the
//! manifest is rewritten after every symbol (by `update_manifest`, in parallel workers); a flag in
//! it could be dropped or resurrected by a racing rewrite, and a file's existence cannot.
//!
//! # Removal
//!
//! `remove_dir_all` on a day with thousands of files is not atomic: interrupted (a `docker stop`,
//! a crash) it can leave a directory whose manifest is already gone but whose `.swa` files are
//! not, which [`Archive::days`] no longer lists and so nothing would ever delete. [`Archive::remove_day`]
//! therefore first renames the directory to `.removing-<day>` at the archive root (one atomic
//! rename on the same volume: the day vanishes from every listing at once), then deletes that.
//! [`Archive::sweep_removed`] finishes any such leftover.

use crate::Archive;
use crate::error::ArchiveError;
use chrono::NaiveDate;
use std::path::PathBuf;

/// The marker file name inside a day directory (see the module docs).
pub const IMPORTING_MARKER: &str = ".importing";
/// Prefix of a day directory being removed (see the module docs).
pub const REMOVING_PREFIX: &str = ".removing-";

impl Archive {
    pub fn importing_marker_path(&self, day: NaiveDate) -> PathBuf {
        self.day_dir(day).join(IMPORTING_MARKER)
    }

    /// True while a background import is filling `day` (its marker exists).
    pub fn is_importing(&self, day: NaiveDate) -> bool {
        self.importing_marker_path(day).exists()
    }

    /// Hides `day` from [`Archive::ready_days`] until [`Archive::clear_importing`]. Creates the
    /// day directory if needed, so it can be called before the first symbol is written.
    pub fn mark_importing(&self, day: NaiveDate) -> Result<(), ArchiveError> {
        let dir = self.day_dir(day);
        std::fs::create_dir_all(&dir).map_err(|e| ArchiveError::io(&dir, e))?;
        let path = self.importing_marker_path(day);
        std::fs::write(&path, b"").map_err(|e| ArchiveError::io(&path, e))
    }

    /// Makes `day` visible again. A missing marker is not an error.
    pub fn clear_importing(&self, day: NaiveDate) -> Result<(), ArchiveError> {
        let path = self.importing_marker_path(day);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(ArchiveError::io(&path, e)),
        }
    }

    /// The days a reader may open: [`Archive::days`] without those still being imported.
    pub fn ready_days(&self) -> Result<Vec<NaiveDate>, ArchiveError> {
        Ok(self
            .days()?
            .into_iter()
            .filter(|d| !self.is_importing(*d))
            .collect())
    }

    /// Deletes a day: an atomic rename out of the listing, then the delete (module docs). A day
    /// that does not exist is not an error (someone else removed it first).
    pub fn remove_day(&self, day: NaiveDate) -> Result<(), ArchiveError> {
        let dir = self.day_dir(day);
        let doomed = self
            .root()
            .join(format!("{REMOVING_PREFIX}{}", day.format("%Y-%m-%d")));
        match std::fs::rename(&dir, &doomed) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(ArchiveError::io(&dir, e)),
        }
        std::fs::remove_dir_all(&doomed).map_err(|e| ArchiveError::io(&doomed, e))
    }

    /// Finishes removals an earlier process was interrupted in. Returns how many it deleted.
    pub fn sweep_removed(&self) -> Result<usize, ArchiveError> {
        let rd = match std::fs::read_dir(self.root()) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(ArchiveError::io(self.root(), e)),
        };
        let mut n = 0;
        for e in rd.filter_map(|e| e.ok()) {
            if e.file_name().to_string_lossy().starts_with(REMOVING_PREFIX) {
                let p = e.path();
                std::fs::remove_dir_all(&p).map_err(|err| ArchiveError::io(&p, err))?;
                n += 1;
            }
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ArchiveConfig;
    use crate::writer::FsyncPolicy;

    fn d(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    fn archive(dir: &std::path::Path) -> Archive {
        Archive::new(ArchiveConfig {
            root: dir.join("archive"),
            retention_days: 0,
            min_free_bytes: 0,
        })
    }

    fn day_with_symbol(a: &Archive, day: NaiveDate) {
        let w = a.writer(day, "SMPL", FsyncPolicy::Never).unwrap();
        a.commit(day, w.finalize().unwrap()).unwrap();
    }

    #[test]
    fn an_importing_day_is_listed_but_not_ready_until_cleared() {
        let tmp = tempfile::tempdir().unwrap();
        let a = archive(tmp.path());
        day_with_symbol(&a, d("2026-10-15"));
        a.mark_importing(d("2026-10-16")).unwrap();
        assert_eq!(
            a.days().unwrap(),
            [d("2026-10-15")],
            "a marker alone is no day"
        );
        day_with_symbol(&a, d("2026-10-16"));
        assert_eq!(a.days().unwrap(), [d("2026-10-15"), d("2026-10-16")]);
        assert_eq!(a.ready_days().unwrap(), [d("2026-10-15")]);
        assert!(a.is_importing(d("2026-10-16")));
        a.clear_importing(d("2026-10-16")).unwrap();
        a.clear_importing(d("2026-10-16")).unwrap();
        assert_eq!(a.ready_days().unwrap(), [d("2026-10-15"), d("2026-10-16")]);
    }

    #[test]
    fn removal_takes_the_day_out_of_the_listing_and_sweeps_leftovers() {
        let tmp = tempfile::tempdir().unwrap();
        let a = archive(tmp.path());
        day_with_symbol(&a, d("2026-10-14"));
        day_with_symbol(&a, d("2026-10-15"));
        a.remove_day(d("2026-10-14")).unwrap();
        a.remove_day(d("2026-10-14")).unwrap();
        assert_eq!(a.days().unwrap(), [d("2026-10-15")]);
        assert!(!a.day_dir(d("2026-10-14")).exists());
        // An interrupted removal: renamed away, not yet deleted.
        let left = a.root().join(".removing-2026-10-13");
        std::fs::create_dir_all(&left).unwrap();
        std::fs::write(left.join("X.swa"), b"x").unwrap();
        assert_eq!(a.sweep_removed().unwrap(), 1);
        assert!(!left.exists());
        assert_eq!(a.days().unwrap(), [d("2026-10-15")]);
    }
}
