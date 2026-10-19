//! Appending writer for one (day, symbol) file.

use crate::error::ArchiveError;
use crate::format::{FileHeader, encode_header, encode_record};
use crate::manifest::SymbolEntry;
use crate::space::ensure_floor;
use market_events::{MarketEvent, Source};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

/// When the writer forces data to stable storage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FsyncPolicy {
    /// Only when finalizing (default). A crash loses the open `.partial` file's tail.
    OnFinalize,
    /// Flush a zstd block and fsync every N records (bounded loss for live recording).
    EveryRecords(u64),
    /// Never fsync (tests and throwaway fixtures).
    Never,
}

/// Re-check the free-space floor after this many bytes written.
const SPACE_RECHECK_BYTES: u64 = 8 * 1024 * 1024;

/// Hashes and counts every byte on its way to the file.
struct HashingWriter {
    inner: BufWriter<File>,
    hasher: Sha256,
    written: u64,
}

impl Write for HashingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Writes `<SYMBOL>.swa.partial`; [`SymbolWriter::finalize`] makes it `<SYMBOL>.swa`.
pub struct SymbolWriter {
    partial: PathBuf,
    final_path: PathBuf,
    encoder: Option<zstd::stream::write::Encoder<'static, HashingWriter>>,
    fsync: FsyncPolicy,
    floor: u64,
    next_space_check: u64,
    entry: SymbolEntry,
    since_sync: u64,
}

impl SymbolWriter {
    /// Creates (truncating) the partial file. Refuses below the free-space floor.
    pub(crate) fn create(
        dir: &Path,
        header: FileHeader,
        fsync: FsyncPolicy,
        floor: u64,
    ) -> Result<Self, ArchiveError> {
        std::fs::create_dir_all(dir).map_err(|e| ArchiveError::io(dir, e))?;
        ensure_floor(dir, floor)?;
        let file_name = format!("{}.{}", header.symbol, crate::format::EXT);
        let final_path = dir.join(&file_name);
        let partial = dir.join(format!("{file_name}{}", crate::format::PARTIAL_SUFFIX));
        let file = File::create(&partial).map_err(|e| ArchiveError::io(&partial, e))?;
        let mut hw = HashingWriter {
            inner: BufWriter::with_capacity(256 * 1024, file),
            hasher: Sha256::new(),
            written: 0,
        };
        hw.write_all(&encode_header(&header))
            .map_err(|e| ArchiveError::io(&partial, e))?;
        // Level 6: ~10× on event JSON at well over replay speed; 3 (the default) is ~15% larger.
        let encoder =
            zstd::stream::write::Encoder::new(hw, 6).map_err(|e| ArchiveError::io(&partial, e))?;
        Ok(SymbolWriter {
            partial,
            final_path,
            encoder: Some(encoder),
            fsync,
            floor,
            next_space_check: SPACE_RECHECK_BYTES,
            entry: SymbolEntry {
                symbol: header.symbol,
                file: file_name,
                ..SymbolEntry::default()
            },
            since_sync: 0,
        })
    }

    pub fn path(&self) -> &Path {
        &self.partial
    }

    /// Appends one event (as a `{"v":1,...}` record) in arrival order.
    pub fn append(&mut self, event: &MarketEvent) -> Result<(), ArchiveError> {
        let json = encode_record(event).map_err(|m| ArchiveError::format(&self.partial, m))?;
        let enc = self.encoder.as_mut().expect("not finalized");
        let path = &self.partial;
        enc.write_all(&(json.len() as u32).to_le_bytes())
            .and_then(|_| enc.write_all(&json))
            .map_err(|e| ArchiveError::io(path, e))?;
        let written = enc.get_ref().written;
        self.count(event);
        if written >= self.next_space_check {
            self.next_space_check = written + SPACE_RECHECK_BYTES;
            ensure_floor(&self.partial, self.floor)?;
        }
        if let FsyncPolicy::EveryRecords(n) = self.fsync {
            self.since_sync += 1;
            if self.since_sync >= n.max(1) {
                self.since_sync = 0;
                self.sync()?;
            }
        }
        Ok(())
    }

    fn count(&mut self, event: &MarketEvent) {
        count_event(&mut self.entry, event);
    }

    fn sync(&mut self) -> Result<(), ArchiveError> {
        let enc = self.encoder.as_mut().expect("not finalized");
        let path = &self.partial;
        enc.flush().map_err(|e| ArchiveError::io(path, e))?;
        let hw = enc.get_mut();
        hw.inner.flush().map_err(|e| ArchiveError::io(path, e))?;
        hw.inner
            .get_ref()
            .sync_data()
            .map_err(|e| ArchiveError::io(path, e))
    }

    /// Finishes the zstd frame, fsyncs (unless [`FsyncPolicy::Never`]), and atomically renames
    /// the partial file to its final name. Returns the manifest entry (with size and SHA-256).
    pub fn finalize(mut self) -> Result<SymbolEntry, ArchiveError> {
        let enc = self.encoder.take().expect("finalize once");
        let mut hw = enc
            .finish()
            .map_err(|e| ArchiveError::io(&self.partial, e))?;
        hw.inner
            .flush()
            .map_err(|e| ArchiveError::io(&self.partial, e))?;
        let file = hw
            .inner
            .into_inner()
            .map_err(|e| ArchiveError::io(&self.partial, e.into_error()))?;
        if self.fsync != FsyncPolicy::Never {
            file.sync_all()
                .map_err(|e| ArchiveError::io(&self.partial, e))?;
        }
        drop(file);
        std::fs::rename(&self.partial, &self.final_path)
            .map_err(|e| ArchiveError::io(&self.final_path, e))?;
        self.entry.bytes = hw.written;
        self.entry.sha256 = hex::encode(hw.hasher.finalize());
        Ok(std::mem::take(&mut self.entry))
    }
}

impl Drop for SymbolWriter {
    fn drop(&mut self) {
        // Dropped without finalize: leave the .partial file (evidence of an interrupted write),
        // but flush what we have so it is at least inspectable.
        if let Some(enc) = self.encoder.as_mut() {
            let _ = enc.flush();
        }
    }
}

/// Adds one event to a manifest entry's counts, sources and time range.
pub(crate) fn count_event(e: &mut SymbolEntry, event: &MarketEvent) {
    e.events += 1;
    let origin: Source = match event {
        MarketEvent::Trade(t) => {
            e.trades += 1;
            t.source
        }
        MarketEvent::Quote(q) => {
            e.quotes += 1;
            q.source
        }
        MarketEvent::Correction(c) => {
            e.corrections += 1;
            c.source
        }
        MarketEvent::Cancel(c) => {
            e.cancels += 1;
            c.source
        }
        MarketEvent::Status(s) => {
            e.statuses += 1;
            s.source
        }
    };
    if !e.sources.contains(&origin) {
        e.sources.push(origin);
        e.sources.sort();
    }
    let ts = event.event_ts().nanos();
    e.first_ts = Some(e.first_ts.map_or(ts, |f| f.min(ts)));
    e.last_ts = Some(e.last_ts.map_or(ts, |l| l.max(ts)));
}
