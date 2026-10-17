use crate::bars::{BARS_DIR, BarDataset, BarEntry};
use crate::config::ArchiveConfig;
use crate::error::ArchiveError;
use crate::format::{FileHeader, MANIFEST};
use crate::manifest::{DayManifest, SymbolEntry};
use crate::reader::SymbolReader;
use crate::writer::{FsyncPolicy, SymbolWriter};
use crate::{space, valid_symbol};
use chrono::{Duration, NaiveDate};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Serializes manifest read-modify-writes within the process (see `update_manifest`).
static MANIFEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The archive rooted at [`ArchiveConfig::root`].
#[derive(Clone, Debug)]
pub struct Archive {
    cfg: ArchiveConfig,
}

/// Result of [`Archive::verify`] for one symbol.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifySymbol {
    pub symbol: String,
    pub sha256_ok: bool,
    pub records_ok: bool,
    pub problems: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifyReport {
    pub day: NaiveDate,
    pub symbols: Vec<VerifySymbol>,
    /// Files in the directory that the manifest does not list (including `.partial`).
    pub unlisted: Vec<String>,
}

impl VerifyReport {
    pub fn ok(&self) -> bool {
        self.symbols.iter().all(|s| s.problems.is_empty())
    }
}

impl Archive {
    pub fn new(cfg: ArchiveConfig) -> Self {
        Archive { cfg }
    }

    pub fn config(&self) -> &ArchiveConfig {
        &self.cfg
    }

    pub fn root(&self) -> &Path {
        &self.cfg.root
    }

    pub fn day_dir(&self, day: NaiveDate) -> PathBuf {
        self.cfg.root.join(day.format("%Y-%m-%d").to_string())
    }

    pub fn manifest_path(&self, day: NaiveDate) -> PathBuf {
        self.day_dir(day).join(MANIFEST)
    }

    /// Days that have a manifest, ascending. A missing root is an empty archive.
    pub fn days(&self) -> Result<Vec<NaiveDate>, ArchiveError> {
        let rd = match std::fs::read_dir(&self.cfg.root) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(ArchiveError::io(&self.cfg.root, e)),
        };
        let mut days: Vec<NaiveDate> = rd
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let d = NaiveDate::parse_from_str(&name, "%Y-%m-%d").ok()?;
                e.path().join(MANIFEST).is_file().then_some(d)
            })
            .collect();
        days.sort();
        Ok(days)
    }

    pub fn manifest(&self, day: NaiveDate) -> Result<Option<DayManifest>, ArchiveError> {
        let path = self.manifest_path(day);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(ArchiveError::io(&path, e)),
        };
        let m: DayManifest =
            serde_json::from_slice(&bytes).map_err(|e| ArchiveError::Manifest {
                path: path.clone(),
                message: e.to_string(),
            })?;
        if m.format != crate::manifest::MANIFEST_FORMAT
            || m.format_version != crate::format::FORMAT_VERSION
        {
            return Err(ArchiveError::Manifest {
                path,
                message: format!("unsupported {} v{}", m.format, m.format_version),
            });
        }
        Ok(Some(m))
    }

    /// Opens a writer for `(day, symbol)`. Refuses below the free-space floor.
    pub fn writer(
        &self,
        day: NaiveDate,
        symbol: &str,
        fsync: FsyncPolicy,
    ) -> Result<SymbolWriter, ArchiveError> {
        if !valid_symbol(symbol) {
            return Err(ArchiveError::InvalidSymbol(symbol.to_string()));
        }
        SymbolWriter::create(
            &self.day_dir(day),
            FileHeader {
                symbol: symbol.to_string(),
                day: day.format("%Y-%m-%d").to_string(),
                created_by: format!("market-archive {}", env!("CARGO_PKG_VERSION")),
            },
            fsync,
            self.cfg.min_free_bytes,
        )
    }

    /// Records a finalized file in the day's manifest (atomic rewrite).
    pub fn commit(&self, day: NaiveDate, entry: SymbolEntry) -> Result<DayManifest, ArchiveError> {
        self.update_manifest(day, |m| m.upsert(entry))
    }

    /// Writes a provider bar dataset (never events) to `bars/<provider>-<granularity>/<SYMBOL>.json`
    /// atomically and records it, with its checksum, in the day manifest's `bars` map.
    pub fn write_bars(&self, day: NaiveDate, ds: &BarDataset) -> Result<BarEntry, ArchiveError> {
        if !valid_symbol(&ds.symbol) {
            return Err(ArchiveError::InvalidSymbol(ds.symbol.clone()));
        }
        let safe =
            |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        if !safe(&ds.provider) || !safe(&ds.granularity) {
            return Err(ArchiveError::format(
                self.day_dir(day),
                format!(
                    "invalid provider/granularity {:?}/{:?}",
                    ds.provider, ds.granularity
                ),
            ));
        }
        let rel = format!(
            "{BARS_DIR}/{}-{}/{}.json",
            ds.provider, ds.granularity, ds.symbol
        );
        let path = self.day_dir(day).join(&rel);
        let dir = path.parent().expect("has a parent").to_path_buf();
        std::fs::create_dir_all(&dir).map_err(|e| ArchiveError::io(&dir, e))?;
        space::ensure_floor(&dir, self.cfg.min_free_bytes)?;
        let json = serde_json::to_vec_pretty(ds).expect("bar dataset serializes");
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &json).map_err(|e| ArchiveError::io(&tmp, e))?;
        if let Ok(f) = std::fs::File::open(&tmp) {
            let _ = f.sync_all();
        }
        std::fs::rename(&tmp, &path).map_err(|e| ArchiveError::io(&path, e))?;
        let entry = BarEntry {
            provider: ds.provider.clone(),
            granularity: ds.granularity.clone(),
            symbol: ds.symbol.clone(),
            file: rel,
            bars: ds.bars.len() as u64,
            first_ts: ds.bars.first().map(|b| b.t),
            last_ts: ds.bars.last().map(|b| b.t),
            query: ds.query.clone(),
            fetched_at: ds.fetched_at.clone(),
            bytes: json.len() as u64,
            sha256: hex::encode(Sha256::digest(&json)),
        };
        let key = BarEntry::key(&ds.provider, &ds.granularity, &ds.symbol);
        let recorded = entry.clone();
        self.update_manifest(day, move |m| {
            m.bars.insert(key, recorded);
        })?;
        Ok(entry)
    }

    /// Reads a bar dataset recorded in the day manifest.
    pub fn read_bars(
        &self,
        day: NaiveDate,
        provider: &str,
        granularity: &str,
        symbol: &str,
    ) -> Result<Option<BarDataset>, ArchiveError> {
        let Some(m) = self.manifest(day)? else {
            return Ok(None);
        };
        let Some(e) = m.bars.get(&BarEntry::key(provider, granularity, symbol)) else {
            return Ok(None);
        };
        let path = self.day_dir(day).join(&e.file);
        let bytes = std::fs::read(&path).map_err(|err| ArchiveError::io(&path, err))?;
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|err| ArchiveError::format(&path, err.to_string()))
    }

    /// Read-modify-write of the day manifest, then an atomic rename.
    fn update_manifest(
        &self,
        day: NaiveDate,
        f: impl FnOnce(&mut DayManifest),
    ) -> Result<DayManifest, ArchiveError> {
        // One writer at a time in this process: parallel importers finishing symbols of the same
        // day would otherwise each read the old manifest and drop each other's entries.
        let _guard = MANIFEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let key = day.format("%Y-%m-%d").to_string();
        let mut m = self
            .manifest(day)?
            .unwrap_or_else(|| DayManifest::new(&key));
        f(&mut m);
        let path = self.manifest_path(day);
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_vec_pretty(&m).expect("manifest serializes");
        std::fs::write(&tmp, json).map_err(|e| ArchiveError::io(&tmp, e))?;
        if let Ok(f) = std::fs::File::open(&tmp) {
            let _ = f.sync_all();
        }
        std::fs::rename(&tmp, &path).map_err(|e| ArchiveError::io(&path, e))?;
        Ok(m)
    }

    /// Streaming reader for a finalized `(day, symbol)` file.
    pub fn reader(&self, day: NaiveDate, symbol: &str) -> Result<SymbolReader, ArchiveError> {
        if !valid_symbol(symbol) {
            return Err(ArchiveError::InvalidSymbol(symbol.to_string()));
        }
        SymbolReader::open(
            &self
                .day_dir(day)
                .join(format!("{symbol}.{}", crate::format::EXT)),
        )
    }

    /// Recomputes every listed file's SHA-256 and size, decodes every record and compares the
    /// counts with the manifest.
    pub fn verify(&self, day: NaiveDate) -> Result<VerifyReport, ArchiveError> {
        let m = self
            .manifest(day)?
            .ok_or_else(|| ArchiveError::NotFound(self.manifest_path(day)))?;
        let dir = self.day_dir(day);
        let mut symbols = Vec::new();
        for entry in m.symbols.values() {
            let path = dir.join(&entry.file);
            let mut problems = Vec::new();
            let (sha, bytes) = match sha256_file(&path) {
                Ok(x) => x,
                Err(e) => {
                    problems.push(e.to_string());
                    symbols.push(VerifySymbol {
                        symbol: entry.symbol.clone(),
                        sha256_ok: false,
                        records_ok: false,
                        problems,
                    });
                    continue;
                }
            };
            let sha256_ok = sha == entry.sha256 && bytes == entry.bytes;
            if !sha256_ok {
                problems.push(format!(
                    "checksum mismatch: file {sha} ({bytes} B), manifest {} ({} B)",
                    entry.sha256, entry.bytes
                ));
            }
            let mut count = 0u64;
            let mut records_ok = true;
            match SymbolReader::open(&path) {
                Ok(r) => {
                    if r.header().symbol != entry.symbol {
                        problems.push(format!("header symbol {:?}", r.header().symbol));
                        records_ok = false;
                    }
                    for ev in r {
                        match ev {
                            Ok(_) => count += 1,
                            Err(e) => {
                                problems.push(e.to_string());
                                records_ok = false;
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    problems.push(e.to_string());
                    records_ok = false;
                }
            }
            if records_ok && count != entry.events {
                problems.push(format!(
                    "{count} records decoded, manifest says {}",
                    entry.events
                ));
                records_ok = false;
            }
            symbols.push(VerifySymbol {
                symbol: entry.symbol.clone(),
                sha256_ok,
                records_ok,
                problems,
            });
        }
        for b in m.bars.values() {
            let path = dir.join(&b.file);
            let mut problems = Vec::new();
            match sha256_file(&path) {
                Ok((sha, bytes)) if sha == b.sha256 && bytes == b.bytes => {}
                Ok((sha, bytes)) => problems.push(format!(
                    "checksum mismatch: file {sha} ({bytes} B), manifest {} ({} B)",
                    b.sha256, b.bytes
                )),
                Err(e) => problems.push(e.to_string()),
            }
            if problems.is_empty() {
                let decoded = std::fs::read(&path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<BarDataset>(&bytes).ok());
                match decoded {
                    Some(ds) if ds.bars.len() as u64 == b.bars => {}
                    Some(ds) => problems.push(format!(
                        "{} bars decoded, manifest says {}",
                        ds.bars.len(),
                        b.bars
                    )),
                    None => problems.push("bar dataset does not decode".into()),
                }
            }
            symbols.push(VerifySymbol {
                symbol: format!("{} ({}/{} bars)", b.symbol, b.provider, b.granularity),
                sha256_ok: problems.is_empty(),
                records_ok: problems.is_empty(),
                problems,
            });
        }
        let mut unlisted: Vec<String> = std::fs::read_dir(&dir)
            .map_err(|e| ArchiveError::io(&dir, e))?
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|n| n != MANIFEST && n != BARS_DIR && !m.symbols.values().any(|e| &e.file == n))
            .collect();
        unlisted.sort();
        Ok(VerifyReport {
            day,
            symbols,
            unlisted,
        })
    }

    /// Deletes day directories whose trading date is more than `retention_days` before `today`.
    /// `today` is passed in (an ET trading date) so nothing depends on the process timezone.
    /// Returns the deleted days. No-op when retention is 0.
    pub fn prune(&self, today: NaiveDate) -> Result<Vec<NaiveDate>, ArchiveError> {
        if self.cfg.retention_days == 0 {
            return Ok(Vec::new());
        }
        let cutoff = today - Duration::days(i64::from(self.cfg.retention_days));
        let mut deleted = Vec::new();
        for day in self.days()? {
            if day < cutoff {
                let dir = self.day_dir(day);
                std::fs::remove_dir_all(&dir).map_err(|e| ArchiveError::io(&dir, e))?;
                deleted.push(day);
            }
        }
        Ok(deleted)
    }

    /// Bytes available on the archive volume.
    pub fn free_bytes(&self) -> Result<u64, ArchiveError> {
        space::available_bytes(&self.cfg.root)
    }
}

/// `(lowercase hex sha256, size)` of a file, streamed.
pub fn sha256_file(path: &Path) -> Result<(String, u64), ArchiveError> {
    let mut f = std::fs::File::open(path).map_err(|e| ArchiveError::io(path, e))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut total = 0u64;
    loop {
        let n = f.read(&mut buf).map_err(|e| ArchiveError::io(path, e))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        total += n as u64;
    }
    Ok((hex::encode(h.finalize()), total))
}
