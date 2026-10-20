//! The live recording: what the live market receives, written into the archive itself as it
//! arrives, so one store serves both the dashboard after a restart (today's history) and replay
//! (a provisional day until the nightly import replaces it).
//!
//! # Layout
//!
//! ```text
//! $ARCHIVE_ROOT/2026-10-19/
//!   live/                    # the live recording of the trading date (provisional)
//!     manifest.json          # a DayManifest with "live": {"sealed": …}
//!     quotes.json            # the live market's quote rows (LiveQuote), a startup summary
//!     SMPL.swa               # one file per symbol, arrival order, one zstd frame per flush
//!   manifest.json, *.swa     # the nightly import (final), once it runs
//!   .importing               # while that import fills the day
//! $ARCHIVE_ROOT/live-original/2026-10-19/   # live/ moved here unchanged once the import is done
//! ```
//!
//! The day is the trading date of the receive time ([`market_events::session::trading_date`]):
//! overnight belongs to the next trading day, as for the import.
//!
//! # Why a `live/` directory inside the day, and not `<SYMBOL>.swa` beside the import's
//!
//! The nightly import must **replace** the recording atomically, and it fills a day symbol by
//! symbol over the better part of an hour. Files side by side would have readers see a mix of
//! recorded and imported symbols for that hour, and the importer would have to overwrite files
//! the recorder may still hold. In a subdirectory both coexist untouched, and the switch is the
//! import's existing one: the day stays hidden behind `.importing` while it fills, and
//! [`Archive::finish_import`] clears the marker (every reader now takes the final data:
//! [`Archive::day_data`]) and then moves `live/` out of the day to its preserved original
//! (`crate::original`: the receipt-order history forward research needs, which the import cannot
//! reproduce). The recorder refuses a day that has an
//! import's manifest or marker ([`Archive::accepts_live`]), checked at every flush, so live
//! never writes over or beside imported data.
//!
//! # One file per symbol, frames per flush, no `.partial`
//!
//! - **Per symbol**, like the archive, so the dashboard can load one symbol's day without
//!   decoding the market's (the server loads a symbol when a chart asks for it), and replay opens
//!   the day exactly like an imported one.
//! - **Frames.** A whole-market recording touches thousands of symbols a second; a streaming
//!   zstd encoder per symbol would hold megabytes each. Instead each symbol's records are
//!   buffered raw and written as one complete zstd frame with one write once they reach
//!   [`FRAME_MAX_BYTES`] or the oldest is [`FRAME_MAX_AGE`] old, and at every barrier, day
//!   change and clean stop. A zstd stream may consist of many frames and the archive's reader
//!   decodes them all, so the file format is the archive's v1 unchanged. Why not a frame per
//!   symbol every second: measured on a synthetic whole-market morning (see the docs), that is
//!   134 B/event on disk and only ~20k events/s of writer throughput on the dev machine, less
//!   than the open's peak; size/age frames keep busy symbols in large frames. The cost is that a
//!   process **killed** (not stopped) loses up to `FRAME_MAX_AGE` of a quiet symbol's prints; a
//!   clean stop (every deploy) writes everything first.
//! - **Readable while written, and after a crash.** A file is only ever extended by whole
//!   frames; a reader stops cleanly at the end of the last one. A process killed during a write
//!   leaves a torn last frame: readers stop before it (one error, the end of the data), and the
//!   next run truncates it ([`repair_tail`]) before appending, starting its scan at the length
//!   the last manifest checkpoint recorded, so it does not re-read the day. There is no
//!   `.partial` + rename step because there is no moment the file is "finished" until the day
//!   is: every flushed frame is already final.
//! - **The manifest is a checkpoint** (every few seconds and at the end), with the counts and
//!   committed length per symbol. **Sealing** ([`Archive::seal_live`]), once the recorder has
//!   moved on to a later day, truncates any torn tail, recounts every file exactly, rewrites it
//!   as one frame (per-second frames compress about 3× worse than one frame over the day;
//!   measured), records its SHA-256 and marks the day `sealed`: only then does replay offer it.
//! - **Torn tails are recorded.** Every truncation (on reopening a file, or when sealing) appends
//!   a `torn_tail` record to the day's coverage file (`crate::coverage`): the cut frame held
//!   events that are now missing from the recording.
//!
//! Symbol-less events (market-wide statuses) are not recorded: nothing downstream uses them.

use crate::archive::{read_manifest_file, write_atomic, write_manifest_file};
use crate::coverage::{CoverageEvent, CoverageRecord};
use crate::error::ArchiveError;
use crate::format::{EXT, FORMAT_VERSION, FileHeader, MAGIC, encode_header, encode_record};
use crate::lifecycle::REMOVING_PREFIX;
use crate::manifest::{DayManifest, LiveDay, SymbolEntry};
use crate::reader::SymbolReader;
use crate::space::ensure_floor;
use crate::writer::count_event;
use crate::{Archive, valid_symbol};
use chrono::NaiveDate;
use market_events::{MarketEvent, Price};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The live recording's directory inside a day.
pub const LIVE_DIR: &str = "live";
/// The live market's quote rows inside it.
pub const LIVE_QUOTES: &str = "quotes.json";
/// Open files a writer keeps; beyond this it closes them all (the busy ones reopen at once).
/// Above the ~7,000 symbols of a whole-market day, so a recording reopens nothing: with 4,096
/// every 10 s age cut reopened thousands of files (measured: most of the writer's time on the
/// dev machine). The server's container allows 65,536 descriptors (infra/vps/docker-compose.yml).
const MAX_OPEN_FILES: usize = 16_384;
/// Level for the per-flush frames: they are small, and the writer must keep up with the open.
const FRAME_LEVEL: i32 = 3;
/// A symbol's pending records become a frame once they reach this size...
pub const FRAME_MAX_BYTES: usize = 64 * 1024;
/// ...or once the oldest of them is this old (module docs, "Frames").
pub const FRAME_MAX_AGE: Duration = Duration::from_secs(10);

/// One quote-board row as the live market saved it: enough to list the symbol (price band,
/// change, volume) at startup without reading its events. Prices in micro-dollars.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveQuote {
    pub symbol: String,
    pub day: NaiveDate,
    pub last: Option<Price>,
    /// Execution time of `last` (UTC ns).
    pub last_ts: i64,
    pub first: Option<Price>,
    pub open: Option<Price>,
    pub volume: u64,
    pub trades: u64,
}

/// Which data a day has, as every reader must choose it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DayData {
    /// Imported and complete (the import's manifest, no `.importing` marker). Always preferred.
    Final,
    /// Only the live recording. `sealed`: nothing is appended any more.
    Provisional { sealed: bool },
}

impl Archive {
    pub fn live_dir(&self, day: NaiveDate) -> PathBuf {
        self.day_dir(day).join(LIVE_DIR)
    }

    pub fn live_manifest_path(&self, day: NaiveDate) -> PathBuf {
        self.live_dir(day).join(crate::format::MANIFEST)
    }

    pub fn live_path(&self, day: NaiveDate, symbol: &str) -> PathBuf {
        self.live_dir(day).join(format!("{symbol}.{EXT}"))
    }

    pub fn live_manifest(&self, day: NaiveDate) -> Result<Option<DayManifest>, ArchiveError> {
        read_manifest_file(&self.live_manifest_path(day))
    }

    /// A recorded symbol's events, at most the first `limit` bytes (a length the writer reported,
    /// see [`LiveWriter::barrier`]); `None` reads to the last whole frame.
    pub fn live_reader(
        &self,
        day: NaiveDate,
        symbol: &str,
        limit: Option<u64>,
    ) -> Result<SymbolReader, ArchiveError> {
        if !valid_symbol(symbol) {
            return Err(ArchiveError::InvalidSymbol(symbol.to_string()));
        }
        SymbolReader::open_limited(&self.live_path(day, symbol), limit)
    }

    /// Days with a live recording (a live manifest), ascending.
    pub fn live_days(&self) -> Result<Vec<NaiveDate>, ArchiveError> {
        let rd = match std::fs::read_dir(self.root()) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(ArchiveError::io(self.root(), e)),
        };
        let mut days: Vec<NaiveDate> = rd
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let d = NaiveDate::parse_from_str(&name, "%Y-%m-%d").ok()?;
                self.live_manifest_path(d).is_file().then_some(d)
            })
            .collect();
        days.sort();
        Ok(days)
    }

    /// True when the import has finished the day: its manifest exists and no marker.
    pub fn is_final(&self, day: NaiveDate) -> bool {
        self.manifest_path(day).is_file() && !self.is_importing(day)
    }

    /// True when the live recording may write into `day`: no import has started it.
    pub fn accepts_live(&self, day: NaiveDate) -> bool {
        !self.manifest_path(day).exists() && !self.is_importing(day)
    }

    /// The data a reader should use for `day`: the final import whenever it is complete, else the
    /// live recording, else none. An import in progress does not count yet: until its marker is
    /// cleared the recording is what there is.
    pub fn day_data(&self, day: NaiveDate) -> Option<DayData> {
        if self.is_final(day) {
            return Some(DayData::Final);
        }
        match self.live_manifest(day) {
            Ok(Some(m)) => Some(DayData::Provisional {
                sealed: m.live.is_some_and(|l| l.sealed),
            }),
            _ => None,
        }
    }

    /// Ends an import of `day`: clears its marker (from here every reader takes the imported
    /// data, [`Archive::day_data`]), then moves the live recording, which the import replaces for
    /// every reader, to its preserved original unchanged ([`Archive::preserve_live`]; how long it
    /// is kept is `crate::retention`). The importer calls this instead of
    /// [`Archive::clear_importing`].
    ///
    /// It does not seal: the import runs in another process, and sealing rewrites the files the
    /// recorder's own sealing thread may be working on. A recording moved unsealed is sealed by
    /// the recorder's housekeeping ([`Archive::seal_original`]).
    pub fn finish_import(&self, day: NaiveDate) -> Result<(), ArchiveError> {
        self.clear_importing(day)?;
        self.preserve_live(day).map(|_| ())
    }

    /// Removes a day's live recording (renamed out of every listing first, like
    /// [`Archive::remove_day`]); a day directory left empty goes too. Returns whether there was one.
    pub fn retire_live(&self, day: NaiveDate) -> Result<bool, ArchiveError> {
        let dir = self.live_dir(day);
        let doomed = self.root().join(format!(
            "{REMOVING_PREFIX}{}-{LIVE_DIR}",
            day.format("%Y-%m-%d")
        ));
        match std::fs::rename(&dir, &doomed) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(ArchiveError::io(&dir, e)),
        }
        std::fs::remove_dir_all(&doomed).map_err(|e| ArchiveError::io(&doomed, e))?;
        // Only an empty directory is removed (`remove_dir` refuses anything else).
        let _ = std::fs::remove_dir(self.day_dir(day));
        Ok(true)
    }

    /// Keeps the newest `keep` live recordings in `live/` and moves the rest to their preserved
    /// originals ([`Archive::preserve_live`]), **never** `recording` (the day the recorder is
    /// writing). A recording normally leaves `live/` when the import that replaces it finishes;
    /// this catches days no import ever finished. It is moved, not deleted: a day no import
    /// replaced has no other copy at all, so it is the receipt-order original like any other,
    /// under the originals' retention (`crate::retention`). A day that already has an original
    /// keeps its recording in `live/` (an original is never overwritten). Returns the days moved.
    pub fn prune_live(
        &self,
        keep: usize,
        recording: Option<NaiveDate>,
    ) -> Result<Vec<NaiveDate>, ArchiveError> {
        let days = self.live_days()?;
        let cut = days.len().saturating_sub(keep);
        let mut out = Vec::new();
        for d in days[..cut]
            .iter()
            .copied()
            .filter(|d| Some(*d) != recording)
        {
            match self.preserve_live(d) {
                Ok(true) => out.push(d),
                Ok(false) => {}
                // Already preserved: leave both as they are.
                Err(ArchiveError::Format { .. }) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(out)
    }

    /// Saves the live market's quote rows of `day` (atomic). Skipped, returning false, when the day
    /// has no recording or is final (the import's own data then speaks for the day).
    pub fn write_live_quotes(
        &self,
        day: NaiveDate,
        rows: &[LiveQuote],
    ) -> Result<bool, ArchiveError> {
        if !self.live_dir(day).is_dir() || !self.accepts_live(day) {
            return Ok(false);
        }
        let json = serde_json::to_vec(rows).expect("quotes serialize");
        write_atomic(&self.live_dir(day).join(LIVE_QUOTES), &json)?;
        Ok(true)
    }

    pub fn read_live_quotes(&self, day: NaiveDate) -> Result<Option<Vec<LiveQuote>>, ArchiveError> {
        let path = self.live_dir(day).join(LIVE_QUOTES);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(ArchiveError::io(&path, e)),
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| ArchiveError::format(&path, e.to_string()))
    }

    /// Seals a day the recorder has left (module docs): every file truncated to its last whole
    /// frame, recounted by decoding it, hashed; the manifest rewritten with `sealed: true`.
    /// Decodes the whole day once, so callers run it off any hot path.
    pub fn seal_live(&self, day: NaiveDate) -> Result<DayManifest, ArchiveError> {
        self.seal_dir(day, &self.live_dir(day))
    }

    /// [`Archive::seal_live`] of the recording in `dir` (`live/`, or a preserved original moved
    /// before it was sealed).
    pub(crate) fn seal_dir(&self, day: NaiveDate, dir: &Path) -> Result<DayManifest, ArchiveError> {
        let manifest_path = dir.join(crate::format::MANIFEST);
        let mut m = read_manifest_file(&manifest_path)?.unwrap_or_else(|| new_live_manifest(day));
        let rd = std::fs::read_dir(dir).map_err(|e| ArchiveError::io(dir, e))?;
        let mut files: Vec<PathBuf> = Vec::new();
        for p in rd.filter_map(|e| e.ok().map(|e| e.path())) {
            if p.extension().is_some_and(|x| x == EXT) {
                files.push(p);
            } else if p.to_string_lossy().ends_with(&format!(".{EXT}.tmp")) {
                // A compaction interrupted (a crash, or the directory moved under it).
                let _ = std::fs::remove_file(&p);
            }
        }
        files.sort();
        m.symbols.clear();
        for path in files {
            let before = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let kept = repair_tail(&path, 0)?;
            let reader = SymbolReader::open(&path)?;
            let header = reader.header().clone();
            let symbol = header.symbol.clone();
            if kept < before {
                self.note_torn_tail(day, &symbol, kept, before - kept);
            }
            let mut entry = SymbolEntry {
                symbol: symbol.clone(),
                file: path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                ..SymbolEntry::default()
            };
            let mut events = Vec::new();
            for r in reader {
                let e = r?.event;
                count_event(&mut entry, &e);
                events.push(e);
            }
            compact(&path, &header, &events);
            let (sha, bytes) = crate::archive::sha256_file(&path)?;
            entry.bytes = bytes;
            entry.sha256 = sha;
            m.symbols.insert(symbol, entry);
        }
        m.recompute();
        m.live = Some(LiveDay { sealed: true });
        write_manifest_file(&manifest_path, &m)?;
        Ok(m)
    }

    /// Records a truncated torn tail in the day's coverage (best effort: the repair itself must
    /// not fail because the audit line could not be written).
    fn note_torn_tail(&self, day: NaiveDate, symbol: &str, kept: u64, cut: u64) {
        let _ = self.append_coverage(
            day,
            &[CoverageRecord::now(CoverageEvent::TornTail {
                symbol: symbol.to_string(),
                kept_bytes: kept,
                cut_bytes: cut,
            })],
        );
    }
}

/// Adds `d`'s counts, sources and time range to `into`.
fn merge_counts(into: &mut SymbolEntry, d: &SymbolEntry) {
    into.events += d.events;
    into.trades += d.trades;
    into.quotes += d.quotes;
    into.corrections += d.corrections;
    into.cancels += d.cancels;
    into.statuses += d.statuses;
    for s in &d.sources {
        if !into.sources.contains(s) {
            into.sources.push(*s);
        }
    }
    into.sources.sort();
    into.first_ts = match (into.first_ts, d.first_ts) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    into.last_ts = match (into.last_ts, d.last_ts) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };
}

/// Rewrites a sealed file as one zstd frame at the archive's level (temp file, fsync, rename).
/// The per-flush frames of a recording are small (a busy second of one symbol, often a single
/// print), so they compress far worse than one frame over the day (measured in
/// `docs/architecture/simulator-and-chart.md`, "Live recording"); a sealed day is never appended
/// to again, so it can take the archive's own shape. Best effort: on any failure (a reader holding
/// the file on Windows) the frames stay as they are, which reads the same.
fn compact(path: &Path, header: &FileHeader, events: &[MarketEvent]) {
    let tmp = path.with_extension(format!("{EXT}.tmp"));
    let res = (|| -> std::io::Result<()> {
        let file = File::create(&tmp)?;
        let mut w = std::io::BufWriter::with_capacity(256 * 1024, file);
        w.write_all(&encode_header(header))?;
        let mut enc = zstd::stream::write::Encoder::new(w, 6)?;
        for e in events {
            let rec = encode_record(e).map_err(std::io::Error::other)?;
            enc.write_all(&(rec.len() as u32).to_le_bytes())?;
            enc.write_all(&rec)?;
        }
        let mut w = enc.finish()?;
        w.flush()?;
        w.get_ref().sync_all()?;
        drop(w);
        std::fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

fn new_live_manifest(day: NaiveDate) -> DayManifest {
    let mut m = DayManifest::new(&day.format("%Y-%m-%d").to_string());
    m.live = Some(LiveDay { sealed: false });
    m
}

/// Truncates a recorded file to its last whole zstd frame and returns its length. `from` is a
/// length known to end on a frame (the last checkpoint), so only what follows it is scanned; 0
/// scans from the header. Frames are walked by their headers (`find_frame_compressed_size`), never
/// decompressed. A file too short to hold its header is emptied back to a bare header.
pub fn repair_tail(path: &Path, from: u64) -> Result<u64, ArchiveError> {
    let io = |e| ArchiveError::io(path, e);
    let mut f = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(io)?;
    let len = f.metadata().map_err(io)?.len();
    let mut prefix = [0u8; 16];
    if len < 16 || f.read_exact(&mut prefix).is_err() || &prefix[0..8] != MAGIC {
        return Err(ArchiveError::format(path, "not a .swa file"));
    }
    let fv = u16::from_le_bytes([prefix[8], prefix[9]]);
    if fv != FORMAT_VERSION {
        return Err(ArchiveError::format(path, format!("format version {fv}")));
    }
    let header_end = 16
        + u64::from(u32::from_le_bytes([
            prefix[12], prefix[13], prefix[14], prefix[15],
        ]));
    if header_end > len {
        return Err(ArchiveError::format(path, "truncated header"));
    }
    let start = if from >= header_end && from <= len {
        from
    } else {
        header_end
    };
    f.seek(SeekFrom::Start(start)).map_err(io)?;
    let mut rest = Vec::with_capacity((len - start) as usize);
    f.read_to_end(&mut rest).map_err(io)?;
    let mut pos = 0usize;
    while pos < rest.len() {
        match zstd::zstd_safe::find_frame_compressed_size(&rest[pos..]) {
            Ok(n) if n > 0 && pos + n <= rest.len() => pos += n,
            _ => break,
        }
    }
    let good = start + pos as u64;
    if good < len {
        f.set_len(good).map_err(io)?;
        f.sync_all().map_err(io)?;
    }
    Ok(good)
}

struct LiveSym {
    /// Raw records (u32 length + JSON) since the last flush.
    pending: Vec<u8>,
    pending_n: u64,
    /// Counts of the pending records (merged into `entry` when they are written).
    pending_entry: SymbolEntry,
    /// When the oldest pending record was appended.
    pending_since: Option<Instant>,
    /// Committed file length; `None` until this run opened (and repaired or created) the file.
    len: Option<u64>,
    /// Records this run has written to the file.
    run_records: u64,
    /// The manifest entry: the last checkpoint's, plus this run's appends.
    entry: SymbolEntry,
}

/// Appends a live recording of one day (module docs). Synchronous file I/O: run it on its own
/// thread. Not safe for two processes on the same day (there is one live market per archive).
pub struct LiveWriter {
    archive: Archive,
    day: NaiveDate,
    dir: PathBuf,
    floor: u64,
    created_by: String,
    manifest: DayManifest,
    syms: HashMap<String, LiveSym>,
    /// Symbols with pending records.
    dirty: Vec<String>,
    files: HashMap<String, File>,
    /// Symbols whose entry changed since the last checkpoint.
    touched: HashSet<String>,
    skipped: u64,
    /// One compression context for every frame (making one per frame dominated the cost).
    compressor: zstd::bulk::Compressor<'static>,
}

impl LiveWriter {
    /// Opens (or continues) the recording of `day`. Refuses a day with imported data and a
    /// volume below `floor`.
    pub fn open(
        archive: &Archive,
        day: NaiveDate,
        floor: u64,
        created_by: &str,
    ) -> Result<Self, ArchiveError> {
        if !archive.accepts_live(day) {
            return Err(ArchiveError::DayIsFinal(day));
        }
        let dir = archive.live_dir(day);
        std::fs::create_dir_all(&dir).map_err(|e| ArchiveError::io(&dir, e))?;
        ensure_floor(&dir, floor)?;
        let mut manifest = archive
            .live_manifest(day)?
            .unwrap_or_else(|| new_live_manifest(day));
        // Back to appending: a sealed day reopened (a clock stepped back) is no longer sealed.
        manifest.live = Some(LiveDay { sealed: false });
        write_manifest_file(&archive.live_manifest_path(day), &manifest)?;
        Ok(LiveWriter {
            archive: archive.clone(),
            day,
            dir,
            floor,
            created_by: created_by.to_string(),
            manifest,
            syms: HashMap::new(),
            dirty: Vec::new(),
            files: HashMap::new(),
            touched: HashSet::new(),
            skipped: 0,
            compressor: zstd::bulk::Compressor::new(FRAME_LEVEL)
                .map_err(|e| ArchiveError::io(archive.live_dir(day), e))?,
        })
    }

    pub fn day(&self) -> NaiveDate {
        self.day
    }

    /// Events not recorded because they name no (valid) symbol.
    pub fn skipped(&self) -> u64 {
        self.skipped
    }

    /// Buffers one event; written at the next [`LiveWriter::flush`]. Returns false when it was
    /// skipped (no symbol, or not a symbol the archive can name a file after).
    pub fn append(&mut self, event: &MarketEvent) -> Result<bool, ArchiveError> {
        let Some(symbol) = event.symbol().filter(|s| valid_symbol(s)) else {
            self.skipped += 1;
            return Ok(false);
        };
        let record = encode_record(event).map_err(|m| ArchiveError::format(&self.dir, m))?;
        if !self.syms.contains_key(symbol) {
            let entry = self
                .manifest
                .symbols
                .get(symbol)
                .cloned()
                .unwrap_or_else(|| SymbolEntry {
                    symbol: symbol.to_string(),
                    file: format!("{symbol}.{EXT}"),
                    ..SymbolEntry::default()
                });
            self.syms.insert(
                symbol.to_string(),
                LiveSym {
                    pending: Vec::new(),
                    pending_n: 0,
                    pending_entry: SymbolEntry::default(),
                    pending_since: None,
                    len: None,
                    run_records: 0,
                    entry,
                },
            );
        }
        let s = self.syms.get_mut(symbol).expect("inserted");
        if s.pending.is_empty() {
            self.dirty.push(symbol.to_string());
            s.pending_since = Some(Instant::now());
        }
        s.pending
            .extend_from_slice(&(record.len() as u32).to_le_bytes());
        s.pending.extend_from_slice(&record);
        s.pending_n += 1;
        count_event(&mut s.pending_entry, event);
        Ok(true)
    }

    /// Writes every symbol's pending records as one zstd frame each. Refuses (and writes
    /// nothing) once the day has imported data or the volume is below the floor.
    pub fn flush(&mut self) -> Result<(), ArchiveError> {
        self.flush_where(|_, _| true)
    }

    /// Writes the frames that are due: a symbol's pending records once they reach
    /// [`FRAME_MAX_BYTES`] or the oldest is [`FRAME_MAX_AGE`] old (module docs, "Frames").
    pub fn flush_due(&mut self) -> Result<(), ArchiveError> {
        let now = Instant::now();
        self.flush_where(|bytes, since| {
            bytes >= FRAME_MAX_BYTES
                || since.is_some_and(|t| now.duration_since(t) >= FRAME_MAX_AGE)
        })
    }

    fn flush_where(
        &mut self,
        due: impl Fn(usize, Option<Instant>) -> bool,
    ) -> Result<(), ArchiveError> {
        if self.dirty.is_empty() {
            return Ok(());
        }
        let (now, later): (Vec<String>, Vec<String>) =
            std::mem::take(&mut self.dirty).into_iter().partition(|s| {
                self.syms
                    .get(s)
                    .is_some_and(|x| due(x.pending.len(), x.pending_since))
            });
        self.dirty = later;
        if now.is_empty() {
            return Ok(());
        }
        if !self.archive.accepts_live(self.day) {
            self.dirty.extend(now);
            return Err(ArchiveError::DayIsFinal(self.day));
        }
        ensure_floor(&self.dir, self.floor)?;
        for symbol in now {
            self.flush_symbol(&symbol)?;
        }
        Ok(())
    }

    fn flush_symbol(&mut self, symbol: &str) -> Result<(), ArchiveError> {
        let path = self.archive.live_path(self.day, symbol);
        let pending = match self.syms.get_mut(symbol) {
            Some(s) if !s.pending.is_empty() => std::mem::take(&mut s.pending),
            _ => return Ok(()),
        };
        let frame = self
            .compressor
            .compress(&pending)
            .map_err(|e| ArchiveError::io(&path, e))?;
        self.open_file(symbol)?;
        let f = self.files.get_mut(symbol).expect("opened");
        f.write_all(&frame)
            .map_err(|e| ArchiveError::io(&path, e))?;
        let s = self.syms.get_mut(symbol).expect("present");
        let len = s.len.expect("opened") + frame.len() as u64;
        s.len = Some(len);
        s.entry.bytes = len;
        merge_counts(&mut s.entry, &std::mem::take(&mut s.pending_entry));
        s.run_records += s.pending_n;
        s.pending_n = 0;
        s.pending_since = None;
        // Reuse the allocation for the next frame's records.
        s.pending = pending;
        s.pending.clear();
        self.touched.insert(symbol.to_string());
        Ok(())
    }

    /// Makes sure the symbol's file is open for appending; the first time in this run it is
    /// created with its header or repaired (a torn tail from a killed run is cut).
    fn open_file(&mut self, symbol: &str) -> Result<(), ArchiveError> {
        if self.files.contains_key(symbol) {
            return Ok(());
        }
        let path = self.archive.live_path(self.day, symbol);
        let s = self.syms.get_mut(symbol).expect("present");
        if s.len.is_none() {
            let len = if path.exists() {
                let before = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                let kept = repair_tail(&path, s.entry.bytes)?;
                if kept < before {
                    self.archive
                        .note_torn_tail(self.day, symbol, kept, before - kept);
                }
                kept
            } else {
                let header = encode_header(&FileHeader {
                    symbol: symbol.to_string(),
                    day: self.day.format("%Y-%m-%d").to_string(),
                    created_by: self.created_by.clone(),
                });
                let mut f = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .map_err(|e| ArchiveError::io(&path, e))?;
                f.write_all(&header)
                    .map_err(|e| ArchiveError::io(&path, e))?;
                header.len() as u64
            };
            s.len = Some(len);
            s.entry.bytes = len;
        }
        if self.files.len() >= MAX_OPEN_FILES {
            self.files.clear();
        }
        let f = OpenOptions::new()
            .append(true)
            .open(&path)
            .map_err(|e| ArchiveError::io(&path, e))?;
        self.files.insert(symbol.to_string(), f);
        Ok(())
    }

    /// Flushes one symbol and reports what this run has written of it: `(records, file length)`.
    /// Every event appended before the call is then in the file, below that length; a reader
    /// limited to it sees exactly those (plus earlier runs'). For a symbol this run has not
    /// touched: `(0, current length)`.
    pub fn barrier(&mut self, symbol: &str) -> Result<(u64, u64), ArchiveError> {
        if self.syms.get(symbol).is_some_and(|s| !s.pending.is_empty()) {
            if !self.archive.accepts_live(self.day) {
                return Err(ArchiveError::DayIsFinal(self.day));
            }
            self.flush_symbol(symbol)?;
            self.dirty.retain(|s| s != symbol);
        }
        match self.syms.get(symbol) {
            Some(s) if s.len.is_some() => Ok((s.run_records, s.len.unwrap_or(0))),
            _ => {
                let len = std::fs::metadata(self.archive.live_path(self.day, symbol))
                    .map(|m| m.len())
                    .unwrap_or(0);
                Ok((0, len))
            }
        }
    }

    /// Flushes everything and checkpoints the manifest.
    pub fn sync(&mut self) -> Result<(), ArchiveError> {
        self.flush()?;
        self.commit()
    }

    /// Checkpoints the manifest (atomic, fsynced) without cutting frames that are not due: the
    /// recorder's periodic step. The symbol files themselves are not fsynced: a whole-market
    /// recording touches ~7,000 files, and an fdatasync each every few seconds would cost the
    /// writer more than the market gives it time for. What `write` returned is in the OS's page
    /// cache, so a stopped, killed or crashed **process** loses nothing of it; only a power loss
    /// or kernel crash can lose the last seconds the kernel had not written back, and the
    /// recording is provisional (the import replaces it) and repaired to its last whole frame.
    /// A sealed day is fsynced file by file when it is compacted.
    pub fn commit(&mut self) -> Result<(), ArchiveError> {
        self.checkpoint()
    }

    /// Rewrites the manifest with every written symbol's counts and committed length (atomic):
    /// the counts cover exactly what is in the files.
    fn checkpoint(&mut self) -> Result<(), ArchiveError> {
        if self.touched.is_empty() {
            return Ok(());
        }
        for symbol in std::mem::take(&mut self.touched) {
            if let Some(s) = self.syms.get(&symbol) {
                self.manifest.symbols.insert(symbol, s.entry.clone());
            }
        }
        self.manifest.recompute();
        if !self.archive.accepts_live(self.day) {
            return Err(ArchiveError::DayIsFinal(self.day));
        }
        write_manifest_file(&self.archive.live_manifest_path(self.day), &self.manifest)
    }

    /// Flushes, syncs and checkpoints: everything appended is durable and listed.
    pub fn finish(mut self) -> Result<(), ArchiveError> {
        self.sync()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ArchiveConfig;
    use crate::writer::FsyncPolicy;
    use market_events::{Status, StatusKind, Trade, UnixNanos};

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 15).unwrap()
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

    fn read(a: &Archive, sym: &str, limit: Option<u64>) -> (Vec<MarketEvent>, bool) {
        let mut out = Vec::new();
        let mut torn = false;
        for r in a.live_reader(day(), sym, limit).unwrap() {
            match r {
                Ok(v) => out.push(v.event),
                Err(_) => torn = true,
            }
        }
        (out, torn)
    }

    /// A reader during the recording sees every flushed frame, cleanly, and the manifest says
    /// the day is provisional and unsealed.
    #[test]
    fn a_recording_reads_back_while_it_is_still_being_written() {
        let tmp = tempfile::tempdir().unwrap();
        let a = archive(tmp.path());
        let mut w = LiveWriter::open(&a, day(), 0, "test").unwrap();
        let mut sent = Vec::new();
        for i in 0..3_000 {
            let e = trade(if i % 3 == 0 { "AAA" } else { "BBB" }, i);
            w.append(&e).unwrap();
            sent.push(e);
            if i % 500 == 499 {
                w.flush().unwrap();
                let (got, torn) = read(&a, "BBB", None);
                assert!(!torn, "only whole frames on disk");
                let want: Vec<_> = sent
                    .iter()
                    .filter(|e| e.symbol() == Some("BBB"))
                    .cloned()
                    .collect();
                assert_eq!(got, want);
            }
        }
        // Not flushed yet: invisible, and nothing torn.
        w.append(&trade("BBB", 9_999)).unwrap();
        assert!(!read(&a, "BBB", None).1);
        w.sync().unwrap();
        assert_eq!(
            a.day_data(day()),
            Some(DayData::Provisional { sealed: false })
        );
        let m = a.live_manifest(day()).unwrap().unwrap();
        assert_eq!(m.symbols["AAA"].events, 1_000);
        assert_eq!(m.symbols["BBB"].events, 2_001);
        assert_eq!(read(&a, "BBB", None).0.len(), 2_001);
        // A status without a symbol is not recorded.
        let st = Status {
            symbol: None,
            event_ts: UnixNanos(1_792_075_200_000_000_000),
            receive_ts: None,
            source: market_events::Source::Fixture,
            kind: StatusKind::Other,
            code: String::new(),
            reason: String::new(),
            tape: market_events::Tape::Unknown,
        };
        assert!(!w.append(&MarketEvent::Status(st)).unwrap());
        assert_eq!(w.skipped(), 1);
        w.finish().unwrap();
    }

    /// A barrier length bounds a reader exactly, whatever is written after it.
    #[test]
    fn a_barrier_bounds_what_a_reader_sees() {
        let tmp = tempfile::tempdir().unwrap();
        let a = archive(tmp.path());
        let mut w = LiveWriter::open(&a, day(), 0, "test").unwrap();
        for i in 0..10 {
            w.append(&trade("AAA", i)).unwrap();
        }
        let (n, len) = w.barrier("AAA").unwrap();
        assert_eq!(n, 10);
        for i in 10..20 {
            w.append(&trade("AAA", i)).unwrap();
        }
        w.flush().unwrap();
        assert_eq!(read(&a, "AAA", Some(len)).0.len(), 10);
        assert_eq!(read(&a, "AAA", None).0.len(), 20);
        assert_eq!(w.barrier("ZZZ").unwrap(), (0, 0));
    }

    /// Killed in the middle of writing a frame: readers stop before it, and the next run cuts it
    /// off and carries on appending readable frames.
    #[test]
    fn a_crash_mid_write_stays_readable_and_the_next_run_repairs_it() {
        let tmp = tempfile::tempdir().unwrap();
        let a = archive(tmp.path());
        let mut w = LiveWriter::open(&a, day(), 0, "test").unwrap();
        for i in 0..100 {
            w.append(&trade("AAA", i)).unwrap();
        }
        w.sync().unwrap();
        for i in 100..200 {
            w.append(&trade("AAA", i)).unwrap();
        }
        w.flush().unwrap();
        drop(w);
        // A torn write: the last frame lost its end.
        let path = a.live_path(day(), "AAA");
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() - 9]).unwrap();
        let (got, torn) = read(&a, "AAA", None);
        assert!(torn);
        assert_eq!(got.len(), 100, "everything before the torn frame ({torn})");

        let mut w = LiveWriter::open(&a, day(), 0, "test").unwrap();
        for i in 200..250 {
            w.append(&trade("AAA", i)).unwrap();
        }
        w.finish().unwrap();
        let (got, torn) = read(&a, "AAA", None);
        assert!(!torn, "repaired before appending");
        let seqs: Vec<u64> = got
            .iter()
            .map(|e| match e {
                MarketEvent::Trade(t) => t.source_seq,
                _ => 0,
            })
            .collect();
        assert_eq!(seqs, (0..100).chain(200..250).collect::<Vec<_>>());
        // Sealing recounts exactly and hashes.
        let m = a.seal_live(day()).unwrap();
        assert_eq!(m.symbols["AAA"].events, 150);
        assert!(!m.symbols["AAA"].sha256.is_empty());
        assert_eq!(
            a.day_data(day()),
            Some(DayData::Provisional { sealed: true })
        );
    }

    /// The import wins: once it has started a day the recorder refuses it, and finishing the
    /// import switches every reader to the final data and retires the recording.
    #[test]
    fn the_import_replaces_a_recorded_day_and_live_never_writes_into_it() {
        let tmp = tempfile::tempdir().unwrap();
        let a = archive(tmp.path());
        let mut w = LiveWriter::open(&a, day(), 0, "test").unwrap();
        w.append(&trade("AAA", 1)).unwrap();
        w.sync().unwrap();
        a.write_live_quotes(day(), &[]).unwrap();
        assert_eq!(
            a.days().unwrap(),
            Vec::<NaiveDate>::new(),
            "not an archive day"
        );
        assert_eq!(a.live_days().unwrap(), [day()]);

        // The nightly import starts the day.
        a.mark_importing(day()).unwrap();
        w.append(&trade("AAA", 2)).unwrap();
        assert!(matches!(w.flush(), Err(ArchiveError::DayIsFinal(_))));
        // The recorder drops a refused day's writer (closing its files).
        drop(w);
        assert!(matches!(
            LiveWriter::open(&a, day(), 0, "test"),
            Err(ArchiveError::DayIsFinal(_))
        ));
        assert!(!a.write_live_quotes(day(), &[]).unwrap());
        let iw = a.writer(day(), "AAA", FsyncPolicy::Never).unwrap();
        a.commit(day(), iw.finalize().unwrap()).unwrap();
        // Still importing: the recording is what readers use.
        assert_eq!(
            a.day_data(day()),
            Some(DayData::Provisional { sealed: false })
        );
        a.finish_import(day()).unwrap();
        assert_eq!(a.day_data(day()), Some(DayData::Final));
        assert!(!a.live_dir(day()).exists());
        assert_eq!(a.live_days().unwrap(), Vec::<NaiveDate>::new());
        assert_eq!(a.ready_days().unwrap(), [day()]);
        // And stays refused afterwards.
        assert!(LiveWriter::open(&a, day(), 0, "test").is_err());
    }

    #[test]
    fn retention_never_removes_the_day_being_recorded() {
        let tmp = tempfile::tempdir().unwrap();
        let a = Archive::new(ArchiveConfig {
            root: tmp.path().join("archive"),
            retention_days: 1,
            min_free_bytes: 0,
        });
        let days: Vec<NaiveDate> = (12..=15)
            .map(|d| NaiveDate::from_ymd_opt(2026, 10, d).unwrap())
            .collect();
        for d in &days {
            let mut w = LiveWriter::open(&a, *d, 0, "test").unwrap();
            w.append(&trade("AAA", 1)).unwrap();
            w.finish().unwrap();
        }
        // The recorder is (still) on the oldest one: kept, whatever the count says.
        let pruned = a.prune_live(1, Some(days[0])).unwrap();
        assert_eq!(pruned, days[1..3]);
        assert_eq!(a.live_days().unwrap(), [days[0], days[3]]);
        // Moved to their originals, not deleted: no import ever replaced them.
        assert_eq!(a.original_days().unwrap(), days[1..3]);
        // The archive's own retention lists imported days only: a recording is not one.
        assert!(
            a.prune(days[3] + chrono::Duration::days(30))
                .unwrap()
                .is_empty()
        );
        assert_eq!(a.live_days().unwrap(), [days[0], days[3]]);
    }
}
