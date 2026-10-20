//! Retention of preserved originals (`crate::original`): a local hot cache of recent sessions,
//! research pins, a durable copy before anything local is released, and a status that says when
//! retention is at risk instead of deleting protected data.
//!
//! # The policy ([`Archive::apply_original_retention`])
//!
//! An original is **kept locally** while any of these holds, and released (deleted) only when
//! none does:
//!
//! 1. **Window.** Its day is one of the last `keep_trading_dates` *completed trading dates*
//!    before the day being recorded (`LIVE_ORIGINAL_DAYS`, at least [`MIN_KEEP_TRADING_DATES`]),
//!    or one of the newest `keep_trading_dates` originals (so a day the recorder missed does not
//!    shorten the window). Trading dates, not calendar days and not directories: Monday's window
//!    of five reaches back to the previous Monday, and a holiday does not count.
//! 2. **Pinned.** `live-original/pins.json` lists it (a day, or symbols of a day: an original is
//!    a complete session and is never split, so a symbol pin keeps the whole day). An unreadable
//!    pins file keeps everything.
//! 3. **Unsealed.** Only a sealed original is copied or released.
//! 4. **No verified durable copy.** With `LIVE_ORIGINAL_DURABLE_DIR` set, every sealed original is
//!    copied there ([`Archive::durable_copy`]) and verified (every file's SHA-256 against the
//!    original's manifest, every record decoded) before a durable manifest is written and the
//!    copy renamed into place. Release also re-checks that the copy is still there. With the
//!    setting off (the default) or the destination missing, nothing outside the window is ever
//!    released: the original stays and the status says `retention_at_risk`.
//!
//! # Status (`ARCHIVE_ROOT/plans/retention-status.json`)
//!
//! Written at every run, with free space, the headroom threshold
//! (`LIVE_ORIGINAL_MIN_HEADROOM_MB`), the per-day size of `live-original/`, `coverage/` and
//! `live/`, what each original is kept for, and `"status": "ok" | "retention_at_risk"` with the
//! reasons: free space under the headroom, originals waiting for a durable copy, a failed copy,
//! an unreadable pins file. At risk is a report, never an action: protected data is not deleted
//! to make room, and the recorder's market path is untouched (housekeeping runs on its own
//! thread; the live writer's own `ARCHIVE_MIN_FREE_MB` floor still stops recording, never the
//! market).
//!
//! # The durable copy (`<dest>/<day>/`)
//!
//! Every file of the original as is (manifest, `quotes.json`, the receipt-order `.swa` files),
//! the day's coverage records as `coverage.jsonl`, and [`DURABLE_MANIFEST`]: file list with
//! sizes and SHA-256s, record count, the pins, and references to what the data is read with:
//! the event contract, archive format and coverage schema versions, the SHA-256 of the
//! archive's `assets.json` and `daily-closes.json` (instrument and corporate-action metadata at
//! copy time), of the day's corrected-import manifest when there is one (a distinct version of
//! the day, never mixed in), of the retention policy, plus whatever the caller adds. A copy is
//! built in `.incoming-<day>` and renamed into place only once verified; an existing copy is
//! never overwritten. The destination root must already exist: an unmounted mount point is
//! "unavailable", never silently filled on the local disk.

use crate::archive::{sha256_file, write_atomic};
use crate::error::ArchiveError;
use crate::lifecycle::REMOVING_PREFIX;
use crate::original::ORIGINALS_DIR;
use crate::reader::SymbolReader;
use crate::{Archive, space};
use chrono::NaiveDate;
use market_events::session::previous_trading_day;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Research pins, inside `live-original/`.
pub const PINS_FILE: &str = "pins.json";
/// Local records of verified durable copies, inside `live-original/` (`<day>.json`).
pub const DURABLE_MARKERS: &str = ".durable";
/// The manifest inside each durable copy.
pub const DURABLE_MANIFEST: &str = "durable-copy.json";
/// The status file, under the archive's `plans/`.
pub const RETENTION_STATUS: &str = "retention-status.json";
/// The window never holds fewer completed trading dates than this (owner, 2026-10-19: five is
/// the hot cache, not the research archive).
pub const MIN_KEEP_TRADING_DATES: usize = 5;
pub const DURABLE_SCHEMA: u32 = 1;

/// The originals' retention settings (`LIVE_ORIGINAL_*`, read in `market-providers`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OriginalPolicy {
    /// Completed trading dates kept locally; raised to [`MIN_KEEP_TRADING_DATES`].
    pub keep_trading_dates: usize,
    /// Where durable copies go. `None`: off, nothing outside the window is released.
    pub durable_dir: Option<PathBuf>,
    /// Free space under this reports `retention_at_risk`.
    pub min_headroom_bytes: u64,
    /// Extra references recorded in every durable copy (versions, policy hashes).
    pub references: BTreeMap<String, String>,
}

impl Default for OriginalPolicy {
    fn default() -> Self {
        OriginalPolicy {
            keep_trading_dates: MIN_KEEP_TRADING_DATES,
            durable_dir: None,
            min_headroom_bytes: 10 * 1024 * 1024 * 1024,
            references: BTreeMap::new(),
        }
    }
}

impl OriginalPolicy {
    pub fn keep(&self) -> usize {
        self.keep_trading_dates.max(MIN_KEEP_TRADING_DATES)
    }

    /// SHA-256 of the policy's own settings, recorded with every durable copy.
    fn hash(&self) -> String {
        let j = serde_json::json!({
            "keep_trading_dates": self.keep(),
            "min_headroom_bytes": self.min_headroom_bytes,
            "durable": self.durable_dir.is_some(),
        });
        hex::encode(Sha256::digest(j.to_string().as_bytes()))
    }
}

/// One research pin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pin {
    pub day: NaiveDate,
    /// Empty: the whole day. Otherwise the symbols of interest (the whole day is still kept).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub symbols: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinsFile {
    pub v: u32,
    pub pins: Vec<Pin>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DurableFile {
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
}

/// [`DURABLE_MANIFEST`] (module docs).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DurableManifest {
    pub v: u32,
    pub day: NaiveDate,
    /// UTC RFC 3339.
    pub copied_at: String,
    /// Where it was copied from, relative to the archive root.
    pub source: String,
    /// The original's files, as copied and verified.
    pub files: Vec<DurableFile>,
    /// Events decoded from the copied `.swa` files (equal to the original's manifest).
    pub records: u64,
    pub coverage: Option<DurableFile>,
    pub pins: Vec<Pin>,
    pub references: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionState {
    Ok,
    RetentionAtRisk,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kept {
    Window,
    Pinned,
    Unsealed,
    PinsUnreadable,
    AwaitingDurableCopy,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OriginalStatus {
    pub day: NaiveDate,
    pub bytes: u64,
    pub sealed: bool,
    pub pinned: bool,
    pub durable_copy: bool,
    /// `None`: released this run.
    pub kept: Option<Kept>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DayBytes {
    pub day: NaiveDate,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DurableStatus {
    pub configured: bool,
    pub dir: Option<String>,
    pub available: bool,
}

/// The status file (module docs).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RetentionStatus {
    pub v: u32,
    pub generated_at: String,
    pub status: RetentionState,
    pub reasons: Vec<String>,
    pub free_bytes: u64,
    pub min_headroom_bytes: u64,
    pub keep_trading_dates: usize,
    /// The completed trading dates the window protects, newest first.
    pub window: Vec<NaiveDate>,
    pub durable: DurableStatus,
    pub originals: Vec<OriginalStatus>,
    pub coverage: Vec<DayBytes>,
    pub live: Vec<DayBytes>,
    pub originals_bytes: u64,
    pub coverage_bytes: u64,
    pub live_bytes: u64,
    pub copied: Vec<NaiveDate>,
    pub released: Vec<NaiveDate>,
}

/// Total size of the files under `path` (0 when missing).
fn tree_bytes(path: &Path) -> u64 {
    let Ok(md) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if md.is_file() {
        return md.len();
    }
    std::fs::read_dir(path)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| tree_bytes(&e.path()))
                .sum()
        })
        .unwrap_or(0)
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

impl Archive {
    pub fn pins_path(&self) -> PathBuf {
        self.originals_dir().join(PINS_FILE)
    }

    /// The research pins; none when the file is missing.
    pub fn read_pins(&self) -> Result<Vec<Pin>, ArchiveError> {
        let path = self.pins_path();
        match std::fs::read(&path) {
            Ok(b) => serde_json::from_slice::<PinsFile>(&b)
                .map(|f| f.pins)
                .map_err(|e| ArchiveError::format(&path, e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(ArchiveError::io(&path, e)),
        }
    }

    /// Adds a pin (atomic rewrite of the pins file).
    pub fn add_pin(&self, pin: Pin) -> Result<(), ArchiveError> {
        let mut pins = self.read_pins()?;
        if !pins.contains(&pin) {
            pins.push(pin);
        }
        let dir = self.originals_dir();
        std::fs::create_dir_all(&dir).map_err(|e| ArchiveError::io(&dir, e))?;
        let json = serde_json::to_vec_pretty(&PinsFile { v: 1, pins }).expect("pins serialize");
        write_atomic(&self.pins_path(), &json)
    }

    fn durable_marker_path(&self, day: NaiveDate) -> PathBuf {
        self.originals_dir()
            .join(DURABLE_MARKERS)
            .join(format!("{}.json", day.format("%Y-%m-%d")))
    }

    /// The local record of `day`'s verified durable copy, if one was made.
    pub fn durable_marker(&self, day: NaiveDate) -> Option<DurableManifest> {
        let b = std::fs::read(self.durable_marker_path(day)).ok()?;
        serde_json::from_slice(&b).ok()
    }

    /// True when `day` has a verified durable copy under `dest` that is still there: its
    /// manifest equals the local record and every file has its recorded size.
    pub fn durable_copy_present(&self, day: NaiveDate, dest: &Path) -> bool {
        let Some(local) = self.durable_marker(day) else {
            return false;
        };
        let dir = dest.join(day.format("%Y-%m-%d").to_string());
        let remote: Option<DurableManifest> = std::fs::read(dir.join(DURABLE_MANIFEST))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        remote.is_some_and(|r| {
            r == local
                && r.files
                    .iter()
                    .chain(r.coverage.iter())
                    .all(|f| std::fs::metadata(dir.join(&f.name)).is_ok_and(|m| m.len() == f.bytes))
        })
    }

    /// Copies `day`'s sealed original, with its coverage records, to `<dest>/<day>/` and verifies
    /// it (module docs). Idempotent: an existing verified copy just refreshes the local record;
    /// a different copy already there is an error, never overwritten.
    pub fn durable_copy(
        &self,
        day: NaiveDate,
        dest: &Path,
        policy: &OriginalPolicy,
    ) -> Result<DurableManifest, ArchiveError> {
        if !dest.is_dir() {
            return Err(ArchiveError::NotFound(dest.to_path_buf()));
        }
        let src = self.original_dir(day);
        let m = self
            .original_manifest(day)?
            .ok_or_else(|| ArchiveError::NotFound(self.original_manifest_path(day)))?;
        if !m.live.as_ref().is_some_and(|l| l.sealed) {
            return Err(ArchiveError::format(&src, "not sealed: not copied yet"));
        }
        let name = day.format("%Y-%m-%d").to_string();
        let fin = dest.join(&name);
        if fin.exists() {
            let there: Option<DurableManifest> = std::fs::read(fin.join(DURABLE_MANIFEST))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok());
            return match there {
                Some(d) if d.source == format!("{ORIGINALS_DIR}/{name}") => {
                    // Only a copy whose files match this original, and are all still there
                    // byte for byte, counts.
                    let ours = self.original_file_list(&src)?;
                    let intact = d.files.iter().chain(d.coverage.iter()).all(|f| {
                        sha256_file(&fin.join(&f.name))
                            .is_ok_and(|(sha, bytes)| sha == f.sha256 && bytes == f.bytes)
                    });
                    if d.files == ours && intact {
                        self.write_durable_marker(day, &d)?;
                        Ok(d)
                    } else {
                        Err(ArchiveError::format(
                            &fin,
                            "a different or damaged durable copy of this day exists; not overwritten",
                        ))
                    }
                }
                _ => Err(ArchiveError::format(
                    &fin,
                    "destination exists without a valid durable manifest; not overwritten",
                )),
            };
        }
        let incoming = dest.join(format!(".incoming-{name}"));
        if incoming.exists() {
            std::fs::remove_dir_all(&incoming).map_err(|e| ArchiveError::io(&incoming, e))?;
        }
        std::fs::create_dir_all(&incoming).map_err(|e| ArchiveError::io(&incoming, e))?;
        let res = self.fill_durable(day, &src, &incoming, &m, policy);
        let manifest = match res {
            Ok(d) => d,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&incoming);
                return Err(e);
            }
        };
        std::fs::rename(&incoming, &fin).map_err(|e| ArchiveError::io(&fin, e))?;
        self.write_durable_marker(day, &manifest)?;
        Ok(manifest)
    }

    fn write_durable_marker(
        &self,
        day: NaiveDate,
        d: &DurableManifest,
    ) -> Result<(), ArchiveError> {
        let path = self.durable_marker_path(day);
        let dir = path.parent().expect("has a parent").to_path_buf();
        std::fs::create_dir_all(&dir).map_err(|e| ArchiveError::io(&dir, e))?;
        write_atomic(&path, &serde_json::to_vec_pretty(d).expect("serializes"))
    }

    /// `(name, bytes, sha256)` of every file of an original directory, by name.
    fn original_file_list(&self, src: &Path) -> Result<Vec<DurableFile>, ArchiveError> {
        let mut names: Vec<String> = std::fs::read_dir(src)
            .map_err(|e| ArchiveError::io(src, e))?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        names.sort();
        names
            .into_iter()
            .map(|n| {
                let (sha256, bytes) = sha256_file(&src.join(&n))?;
                Ok(DurableFile {
                    name: n,
                    bytes,
                    sha256,
                })
            })
            .collect()
    }

    fn fill_durable(
        &self,
        day: NaiveDate,
        src: &Path,
        to: &Path,
        m: &crate::DayManifest,
        policy: &OriginalPolicy,
    ) -> Result<DurableManifest, ArchiveError> {
        let files = self.original_file_list(src)?;
        let copy = |from: &Path, dst: &Path| -> Result<(), ArchiveError> {
            std::fs::copy(from, dst).map_err(|e| ArchiveError::io(dst, e))?;
            std::fs::OpenOptions::new()
                .write(true)
                .open(dst)
                .and_then(|f| f.sync_all())
                .map_err(|e| ArchiveError::io(dst, e))
        };
        for f in &files {
            let dst = to.join(&f.name);
            copy(&src.join(&f.name), &dst)?;
            let (sha, bytes) = sha256_file(&dst)?;
            if sha != f.sha256 || bytes != f.bytes {
                return Err(ArchiveError::format(&dst, "copy differs from the original"));
            }
        }
        // Against the original's own manifest, and readable record by record.
        let mut records = 0u64;
        for e in m.symbols.values() {
            let dst = to.join(&e.file);
            let (sha, bytes) = sha256_file(&dst)?;
            if sha != e.sha256 || bytes != e.bytes {
                return Err(ArchiveError::format(
                    &dst,
                    "copy does not match the original's manifest",
                ));
            }
            let mut n = 0u64;
            for r in SymbolReader::open(&dst)? {
                r?;
                n += 1;
            }
            if n != e.events {
                return Err(ArchiveError::format(
                    &dst,
                    format!("{n} records decoded, manifest says {}", e.events),
                ));
            }
            records += n;
        }
        let coverage = match self.coverage_path(day) {
            p if p.is_file() => {
                let dst = to.join("coverage.jsonl");
                copy(&p, &dst)?;
                let (sha256, bytes) = sha256_file(&dst)?;
                Some(DurableFile {
                    name: "coverage.jsonl".into(),
                    bytes,
                    sha256,
                })
            }
            _ => None,
        };
        let mut references = BTreeMap::new();
        let mut file_ref = |key: &str, path: PathBuf| {
            if let Ok((sha, bytes)) = sha256_file(&path) {
                references.insert(format!("{key}.sha256"), sha);
                references.insert(format!("{key}.bytes"), bytes.to_string());
            }
        };
        file_ref("assets.json", self.root().join(crate::ASSETS_FILE));
        file_ref(
            "daily-closes.json",
            self.root().join(crate::DAILY_CLOSES_FILE),
        );
        file_ref("import_manifest", self.manifest_path(day));
        references.insert(
            "event_contract_version".into(),
            market_events::CONTRACT_VERSION.to_string(),
        );
        references.insert(
            "archive_format_version".into(),
            crate::format::FORMAT_VERSION.to_string(),
        );
        references.insert(
            "coverage_schema".into(),
            crate::coverage::COVERAGE_SCHEMA.to_string(),
        );
        references.insert(
            "market_archive_version".into(),
            env!("CARGO_PKG_VERSION").into(),
        );
        references.insert("retention_policy.sha256".into(), policy.hash());
        references.extend(policy.references.clone());
        let pins = self
            .read_pins()
            .unwrap_or_default()
            .into_iter()
            .filter(|p| p.day == day)
            .collect();
        let d = DurableManifest {
            v: DURABLE_SCHEMA,
            day,
            copied_at: now_rfc3339(),
            source: format!("{ORIGINALS_DIR}/{}", day.format("%Y-%m-%d")),
            files,
            records,
            coverage,
            pins,
            references,
        };
        write_atomic(
            &to.join(DURABLE_MANIFEST),
            &serde_json::to_vec_pretty(&d).expect("serializes"),
        )?;
        Ok(d)
    }

    /// The completed trading dates before `today` the window protects, newest first.
    pub fn retention_window(today: NaiveDate, keep: usize) -> Vec<NaiveDate> {
        let mut out = Vec::with_capacity(keep);
        let mut d = today;
        for _ in 0..keep {
            d = previous_trading_day(d);
            out.push(d);
        }
        out
    }

    /// Applies the originals' policy (module docs) with `today` the day being recorded: copies,
    /// releases what may go, writes the status file. Never deletes a protected original.
    pub fn apply_original_retention(
        &self,
        today: NaiveDate,
        policy: &OriginalPolicy,
    ) -> Result<RetentionStatus, ArchiveError> {
        let keep = policy.keep();
        let mut reasons = Vec::new();
        let days = self.original_days()?;
        let (pins, pins_ok) = match self.read_pins() {
            Ok(p) => (p, true),
            Err(e) => {
                reasons.push(format!("pins file unreadable, nothing released: {e}"));
                (Vec::new(), false)
            }
        };
        let pinned: BTreeSet<NaiveDate> = pins.iter().map(|p| p.day).collect();
        let dest = policy.durable_dir.as_deref();
        let available = dest.is_some_and(Path::is_dir);
        let sealed = |d: NaiveDate| {
            self.original_manifest(d)
                .ok()
                .flatten()
                .and_then(|m| m.live)
                .is_some_and(|l| l.sealed)
        };
        let mut copied = Vec::new();
        if let (Some(dest), true) = (dest, available) {
            for d in days.iter().copied().filter(|d| sealed(*d)) {
                if self.durable_copy_present(d, dest) {
                    continue;
                }
                match self.durable_copy(d, dest, policy) {
                    Ok(_) => copied.push(d),
                    Err(e) => reasons.push(format!("durable copy of {d} failed: {e}")),
                }
            }
        }
        let window = Self::retention_window(today, keep);
        let newest: BTreeSet<NaiveDate> = days.iter().rev().take(keep).copied().collect();
        let mut originals = Vec::new();
        let mut released = Vec::new();
        let mut awaiting = Vec::new();
        for d in days.iter().copied() {
            let durable = dest.is_some_and(|p| self.durable_copy_present(d, p));
            let is_sealed = sealed(d);
            let kept = if d >= today || window.contains(&d) || newest.contains(&d) {
                Some(Kept::Window)
            } else if pinned.contains(&d) {
                Some(Kept::Pinned)
            } else if !pins_ok {
                Some(Kept::PinsUnreadable)
            } else if !is_sealed {
                Some(Kept::Unsealed)
            } else if !durable {
                awaiting.push(d);
                Some(Kept::AwaitingDurableCopy)
            } else {
                None
            };
            let bytes = tree_bytes(&self.original_dir(d));
            if kept.is_none() {
                self.release_original(d)?;
                released.push(d);
            }
            originals.push(OriginalStatus {
                day: d,
                bytes,
                sealed: is_sealed,
                pinned: pinned.contains(&d),
                durable_copy: durable,
                kept,
            });
        }
        if !awaiting.is_empty() {
            let why = match (dest, available) {
                (None, _) => "LIVE_ORIGINAL_DURABLE_DIR is not set".to_string(),
                (Some(p), false) => format!("{} is not available", p.display()),
                (Some(_), true) => "the copy failed".to_string(),
            };
            reasons.push(format!(
                "{} original(s) past the window kept without a durable copy ({why}): {}",
                awaiting.len(),
                awaiting
                    .iter()
                    .map(|d| d.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        let free = space::available_bytes(self.root())?;
        if free < policy.min_headroom_bytes {
            reasons.push(format!(
                "free space {} MiB is below the headroom of {} MiB (LIVE_ORIGINAL_MIN_HEADROOM_MB); protected originals are not deleted to make room",
                free / (1024 * 1024),
                policy.min_headroom_bytes / (1024 * 1024)
            ));
        }
        let coverage: Vec<DayBytes> = self
            .coverage_days()?
            .into_iter()
            .map(|d| DayBytes {
                day: d,
                bytes: tree_bytes(&self.coverage_path(d)),
            })
            .collect();
        let live: Vec<DayBytes> = self
            .live_days()?
            .into_iter()
            .map(|d| DayBytes {
                day: d,
                bytes: tree_bytes(&self.live_dir(d)),
            })
            .collect();
        let status = RetentionStatus {
            v: 1,
            generated_at: now_rfc3339(),
            status: if reasons.is_empty() {
                RetentionState::Ok
            } else {
                RetentionState::RetentionAtRisk
            },
            reasons,
            free_bytes: free,
            min_headroom_bytes: policy.min_headroom_bytes,
            keep_trading_dates: keep,
            window,
            durable: DurableStatus {
                configured: dest.is_some(),
                dir: dest.map(|p| p.display().to_string()),
                available,
            },
            originals_bytes: originals
                .iter()
                .filter(|o| o.kept.is_some())
                .map(|o| o.bytes)
                .sum(),
            coverage_bytes: coverage.iter().map(|c| c.bytes).sum(),
            live_bytes: live.iter().map(|c| c.bytes).sum(),
            originals,
            coverage,
            live,
            copied,
            released,
        };
        let plans = self.root().join("plans");
        std::fs::create_dir_all(&plans).map_err(|e| ArchiveError::io(&plans, e))?;
        write_atomic(
            &plans.join(RETENTION_STATUS),
            &serde_json::to_vec_pretty(&status).expect("status serializes"),
        )?;
        Ok(status)
    }

    /// Removes a local original (renamed out of the listing first).
    fn release_original(&self, d: NaiveDate) -> Result<(), ArchiveError> {
        let dir = self.original_dir(d);
        let doomed = self.root().join(format!(
            "{REMOVING_PREFIX}{}-{ORIGINALS_DIR}",
            d.format("%Y-%m-%d")
        ));
        match std::fs::rename(&dir, &doomed) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(ArchiveError::io(&dir, e)),
        }
        std::fs::remove_dir_all(&doomed).map_err(|e| ArchiveError::io(&doomed, e))
    }

    /// The last status file written, as raw JSON (for the server or ops).
    pub fn read_retention_status(&self) -> Option<serde_json::Value> {
        let b = std::fs::read(self.root().join("plans").join(RETENTION_STATUS)).ok()?;
        serde_json::from_slice(&b).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coverage::{CoverageEvent, CoverageRecord};
    use crate::writer::FsyncPolicy;
    use crate::{ArchiveConfig, LiveWriter};
    use market_events::{MarketEvent, Price, Trade, UnixNanos};

    fn d(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    /// Monday 2026-10-19 is the day being recorded.
    fn today() -> NaiveDate {
        d("2026-10-19")
    }

    /// Two weeks of trading dates: 10-06..=10-09 lie outside a five-date window.
    const DAYS: [&str; 9] = [
        "2026-10-06",
        "2026-10-07",
        "2026-10-08",
        "2026-10-09",
        "2026-10-12",
        "2026-10-13",
        "2026-10-14",
        "2026-10-15",
        "2026-10-16",
    ];

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
        tr.receive_ts = Some(t);
        MarketEvent::Trade(tr)
    }

    /// A recorded, sealed day the import then replaced: a sealed original.
    fn original(a: &Archive, day: NaiveDate) {
        let mut w = LiveWriter::open(a, day, 0, "test").unwrap();
        for i in 0..40 {
            w.append(&trade(if i % 2 == 0 { "AAA" } else { "BBB" }, i))
                .unwrap();
        }
        w.finish().unwrap();
        a.seal_live(day).unwrap();
        a.append_coverage(day, &[CoverageRecord::new(1, CoverageEvent::Heartbeat)])
            .unwrap();
        import(a, day, 1);
    }

    fn import(a: &Archive, day: NaiveDate, seq: u64) {
        a.mark_importing(day).unwrap();
        let mut w = a.writer(day, "AAA", FsyncPolicy::Never).unwrap();
        w.append(&trade("AAA", seq)).unwrap();
        a.commit(day, w.finalize().unwrap()).unwrap();
        a.finish_import(day).unwrap();
    }

    fn setup(dir: &Path) -> Archive {
        let a = archive(dir);
        for s in DAYS {
            original(&a, d(s));
        }
        a
    }

    fn policy(dest: Option<PathBuf>) -> OriginalPolicy {
        OriginalPolicy {
            min_headroom_bytes: 0,
            durable_dir: dest,
            ..OriginalPolicy::default()
        }
    }

    fn kept(s: &RetentionStatus, day: &str) -> Option<Kept> {
        s.originals.iter().find(|o| o.day == d(day)).unwrap().kept
    }

    #[test]
    fn the_window_counts_completed_trading_dates_not_calendar_days() {
        // Monday: five trading dates reach back to the previous Monday, over the weekend.
        assert_eq!(
            Archive::retention_window(today(), 5),
            [
                "2026-10-16",
                "2026-10-15",
                "2026-10-14",
                "2026-10-13",
                "2026-10-12"
            ]
            .map(d)
        );
        // Never fewer than five.
        let p = OriginalPolicy {
            keep_trading_dates: 2,
            ..OriginalPolicy::default()
        };
        assert_eq!(p.keep(), MIN_KEEP_TRADING_DATES);
    }

    #[test]
    fn without_a_durable_store_nothing_past_the_window_is_released() {
        let tmp = tempfile::tempdir().unwrap();
        let a = setup(tmp.path());
        let s = a.apply_original_retention(today(), &policy(None)).unwrap();
        assert!(s.released.is_empty());
        assert_eq!(a.original_days().unwrap().len(), DAYS.len());
        assert_eq!(s.status, RetentionState::RetentionAtRisk);
        assert!(s.reasons[0].contains("LIVE_ORIGINAL_DURABLE_DIR is not set"));
        assert_eq!(kept(&s, "2026-10-06"), Some(Kept::AwaitingDurableCopy));
        assert_eq!(kept(&s, "2026-10-12"), Some(Kept::Window));
        assert!(s.originals.iter().all(|o| o.bytes > 0));
        assert_eq!(s.coverage.len(), DAYS.len());
        // The status the server and ops read.
        let file = a.read_retention_status().unwrap();
        assert_eq!(file["status"], "retention_at_risk");
        assert_eq!(file["originals"].as_array().unwrap().len(), DAYS.len());
        // An unavailable destination (an unmounted path) is the same: kept, at risk, and the
        // path is not created on the local disk.
        let gone = tmp.path().join("pi-mount");
        let s = a
            .apply_original_retention(today(), &policy(Some(gone.clone())))
            .unwrap();
        assert!(s.released.is_empty() && !gone.exists());
        assert!(!s.durable.available);
        assert_eq!(s.status, RetentionState::RetentionAtRisk);
    }

    #[test]
    fn a_verified_durable_copy_releases_only_what_is_neither_in_the_window_nor_pinned() {
        let tmp = tempfile::tempdir().unwrap();
        let a = setup(tmp.path());
        a.add_pin(Pin {
            day: d("2026-10-07"),
            symbols: vec!["AAA".into()],
            reason: "first expansion candle study".into(),
        })
        .unwrap();
        let before = std::fs::read(a.original_dir(d("2026-10-06")).join("AAA.swa")).unwrap();
        let dest = tmp.path().join("research");
        std::fs::create_dir_all(&dest).unwrap();
        let s = a
            .apply_original_retention(today(), &policy(Some(dest.clone())))
            .unwrap();
        assert_eq!(s.status, RetentionState::Ok, "{:?}", s.reasons);
        assert_eq!(
            s.copied.len(),
            DAYS.len(),
            "every sealed original is copied"
        );
        assert_eq!(
            s.released,
            ["2026-10-06", "2026-10-08", "2026-10-09"].map(d)
        );
        assert_eq!(kept(&s, "2026-10-07"), Some(Kept::Pinned));
        assert_eq!(a.original_days().unwrap().len(), DAYS.len() - 3);
        // The copy: byte-identical receipt-order data, coverage, manifest with references.
        let copy = dest.join("2026-10-06");
        assert_eq!(std::fs::read(copy.join("AAA.swa")).unwrap(), before);
        assert!(copy.join("coverage.jsonl").is_file());
        let m: DurableManifest =
            serde_json::from_slice(&std::fs::read(copy.join(DURABLE_MANIFEST)).unwrap()).unwrap();
        assert_eq!(m.records, 40);
        assert!(m.files.iter().any(|f| f.name == "manifest.json"));
        assert!(m.references.contains_key("import_manifest.sha256"));
        assert!(m.references.contains_key("retention_policy.sha256"));
        assert_eq!(m.references["event_contract_version"], "1");
        let pinned: DurableManifest = serde_json::from_slice(
            &std::fs::read(dest.join("2026-10-07").join(DURABLE_MANIFEST)).unwrap(),
        )
        .unwrap();
        assert_eq!(pinned.pins[0].symbols, ["AAA"]);
        // A second run copies nothing and releases nothing more.
        let s = a
            .apply_original_retention(today(), &policy(Some(dest.clone())))
            .unwrap();
        assert!(s.copied.is_empty() && s.released.is_empty());
    }

    #[test]
    fn a_damaged_or_missing_copy_never_lets_the_original_go() {
        let tmp = tempfile::tempdir().unwrap();
        let a = setup(tmp.path());
        let dest = tmp.path().join("research");
        std::fs::create_dir_all(&dest).unwrap();
        let day = d("2026-10-06");
        a.durable_copy(day, &dest, &policy(Some(dest.clone())))
            .unwrap();
        assert!(a.durable_copy_present(day, &dest));
        // Damaged after the copy: not present any more, not re-copied over, not released.
        std::fs::write(dest.join("2026-10-06").join("AAA.swa"), b"x").unwrap();
        assert!(!a.durable_copy_present(day, &dest));
        let s = a
            .apply_original_retention(today(), &policy(Some(dest.clone())))
            .unwrap();
        assert_eq!(kept(&s, "2026-10-06"), Some(Kept::AwaitingDurableCopy));
        assert!(s.reasons.iter().any(|r| r.contains("damaged")));
        assert!(a.original_dir(day).is_dir());
        // An original that no longer matches its own manifest is never copied.
        let day = d("2026-10-16");
        let dest = tmp.path().join("research-2");
        std::fs::create_dir_all(&dest).unwrap();
        let p = a.original_dir(day).join("BBB.swa");
        let mut b = std::fs::read(&p).unwrap();
        let n = b.len();
        b[n - 1] ^= 0xff;
        std::fs::write(&p, b).unwrap();
        assert!(a.durable_copy(day, &dest, &policy(None)).is_err());
        assert!(!dest.join("2026-10-16").exists());
        assert!(!dest.join(".incoming-2026-10-16").exists());
    }

    #[test]
    fn low_headroom_is_reported_and_deletes_nothing_protected() {
        let tmp = tempfile::tempdir().unwrap();
        let a = setup(tmp.path());
        let p = OriginalPolicy {
            min_headroom_bytes: u64::MAX,
            ..OriginalPolicy::default()
        };
        let s = a.apply_original_retention(today(), &p).unwrap();
        assert_eq!(s.status, RetentionState::RetentionAtRisk);
        assert!(s.reasons.iter().any(|r| r.contains("headroom")));
        assert!(s.released.is_empty());
        // An unreadable pins file keeps everything too.
        std::fs::write(a.pins_path(), b"{not json").unwrap();
        let dest = tmp.path().join("research");
        std::fs::create_dir_all(&dest).unwrap();
        let s = a
            .apply_original_retention(today(), &policy(Some(dest)))
            .unwrap();
        assert!(s.released.is_empty());
        assert_eq!(kept(&s, "2026-10-06"), Some(Kept::PinsUnreadable));
    }

    #[test]
    fn a_corrected_reimport_never_overwrites_the_original() {
        let tmp = tempfile::tempdir().unwrap();
        let a = archive(tmp.path());
        let day = d("2026-10-15");
        original(&a, day);
        let before = a.original_file_list(&a.original_dir(day)).unwrap();
        let import_before = std::fs::read(a.manifest_path(day)).unwrap();
        // A revised import of the same day: a new final version, a distinct one.
        import(&a, day, 7);
        assert_ne!(std::fs::read(a.manifest_path(day)).unwrap(), import_before);
        assert_eq!(a.original_file_list(&a.original_dir(day)).unwrap(), before);
        assert!(a.verify_original(day).unwrap().ok());
    }
}
