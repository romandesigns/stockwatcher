//! # market-archive
//!
//! The on-disk archive of normalized [`market_events::VersionedEvent`]s, one file per
//! (trading day, symbol), plus one manifest per day. Replay ([`market-replay`]) streams these files;
//! the importer, the fixture tool and the live market's recorder ([`live`]) write them. A day has
//! up to two versions, and every reader picks one the same way ([`Archive::day_data`]): the
//! nightly import (final) when it has completed the day, else the live recording (provisional,
//! in the day's `live/` directory).
//!
//! # Layout
//!
//! ```text
//! $ARCHIVE_ROOT/
//!   assets.json            # optional: symbol → {name, exchange} (crate::assets)
//!   daily-closes.json      # optional: date → symbol → official close (crate::closes)
//!   2026-10-15/
//!     manifest.json        # symbols, counts, first/last ts, sources, bytes, sha256 per file
//!     SMPL.swa             # one file per symbol (finalized)
//!     FXLO.swa.partial     # a writer that has not finalized (never read by replay)
//!     live/                # the live recording of the day (crate::live): manifest.json with
//!                          #   "live": {"sealed"}, quotes.json, SMPL.swa (a frame per flush)
//! ```
//!
//! The day is the **trading date** of the events' sessions (`market_events::session`), so the
//! overnight session that starts at 20:00 ET the evening before belongs to the next day's file.
//!
//! # File format, version 1 (`.swa`)
//!
//! ```text
//! offset  size  field
//! 0       8     magic  b"SWARCHV\0"
//! 8       2     format version, u16 little-endian (= 1)
//! 10      2     event contract version, u16 LE (= market_events::CONTRACT_VERSION)
//! 12      4     header length H, u32 LE (≤ 64 KiB)
//! 16      H     header: UTF-8 JSON {"symbol","day","created_by"}
//! 16+H    …     ONE zstd frame whose decompressed content is a sequence of records:
//!                 u32 LE length N (1 ≤ N ≤ 1 MiB) followed by N bytes of UTF-8 JSON, one
//!                 `VersionedEvent` ({"v":1,"type":"trade",...}) per record
//! ```
//!
//! Records are in **arrival order** (the order the writer received them), never re-sorted: a live
//! recording keeps late and out-of-order prints exactly where they arrived, which is what replay
//! needs to reproduce the engine's late/out-of-order decisions. A file imported from a provider's
//! historical REST API (sorted by time) is simply a file whose arrival order equals time order.
//!
//! Why JSON records inside zstd rather than a bespoke binary encoding: the contract's persisted
//! form is already the versioned JSON (`"v":1`, readers refuse other versions), so the archive
//! adds no second serialization to keep in step; zstd removes most of JSON's redundancy (a fixture
//! day compresses ~10×); and `swarchive inspect` can show a record as-is. The length prefix gives
//! exact framing and lets the reader refuse a corrupt length before allocating. Decoding cost is
//! irrelevant next to replay pacing; if it ever matters, bump the format version.
//!
//! # Integrity and safety
//!
//! - A writer writes `<SYMBOL>.swa.partial` and on [`SymbolWriter::finalize`] finishes the zstd
//!   frame, fsyncs, and renames it to `<SYMBOL>.swa` (atomic on the same volume); then the manifest
//!   is rewritten atomically (temp file + rename). A crash leaves at most a `.partial` file and the
//!   previous manifest.
//! - The SHA-256 of every finalized file is computed while writing and stored in the manifest;
//!   [`Archive::verify`] recomputes it and decodes every record.
//! - fsync policy ([`FsyncPolicy`]): on finalize only (default), every N records, or never (tests).
//! - Free-space floor: a writer refuses to open, and refuses further writes, when the volume's
//!   available space is below [`ArchiveConfig::min_free_bytes`] (`ArchiveError::InsufficientSpace`).
//! - Retention: [`Archive::prune`] deletes day directories older than
//!   [`ArchiveConfig::retention_days`] (0 = keep forever, the default). The rolling 21-trading-day
//!   window on the VPS is not this: `swimport maintain` (market-providers) owns it, because it
//!   must ask the server which day is being replayed before deleting anything.
//! - A day being filled by a background import carries an `.importing` marker and is left out of
//!   [`Archive::ready_days`]; [`Archive::remove_day`] renames a day out of every listing before
//!   deleting it (see `lifecycle`).
//!
//! Memory: the writer holds one zstd encoder; the reader one decoder and one record buffer
//! (≤ 1 MiB). Neither ever holds a day in memory.
//!
//! [`market-replay`]: ../market_replay/index.html

pub mod assets;
pub mod bars;
pub mod closes;
pub mod config;
pub mod fixture;
pub mod format;
pub mod live;
pub mod manifest;
pub mod reader;
pub mod space;
pub mod writer;

mod archive;
mod error;
mod lifecycle;

pub use archive::{Archive, VerifyReport, VerifySymbol};
pub use assets::{ASSETS_FILE, AssetMap, AssetMeta};
pub use bars::{Bar, BarDataset, BarEntry};
pub use closes::{DAILY_CLOSES_FILE, DailyCloses, PrevCloses, merge_daily_closes};
pub use config::ArchiveConfig;
pub use error::ArchiveError;
pub use lifecycle::{IMPORTING_MARKER, REMOVING_PREFIX};
pub use live::{DayData, LIVE_DIR, LiveQuote, LiveWriter};
pub use manifest::{DayManifest, ImportProvenance, LiveDay, SymbolEntry};
pub use reader::SymbolReader;
pub use writer::{FsyncPolicy, SymbolWriter};

/// Symbols allowed in file names: `A-Z`, `0-9`, `.` and `-`, 1–16 characters (e.g. `BRK.B`).
pub fn valid_symbol(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 16
        && !s.starts_with('.')
        && s.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
}
