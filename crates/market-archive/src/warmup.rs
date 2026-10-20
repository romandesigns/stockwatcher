//! Warmup requests for intraday band entrants: `$ARCHIVE_ROOT/warmup/<D>/<SYMBOL>.json`.
//!
//! A stock that enters the $0.10–$25 band intraday on D without an in-band close in the last 5
//! dates is not on that day's watch list, so it has no split snapshot and no announcement
//! coverage. Whoever notices it (the server, later) asks for a warmup; the archive maintainer
//! (`swimport maintain`, market-providers `import::warmup`) fetches its daily bars, a split
//! snapshot and an announcement check, bounded per run, and marks the request done. Until then
//! (and if its history is short anyway) its evidence stays explicitly insufficient / unavailable.
//!
//! The inbox is files so the server and the maintainer, separate processes, need no channel. A
//! request is idempotent (one file per symbol and date). Writing it is disk I/O: the server must
//! call this off the market-event thread (market-providers `WarmupQueue` buffers requests in
//! memory without blocking and flushes them from a background task).

use crate::{Archive, ArchiveError, valid_symbol};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WarmupRequest {
    pub symbol: String,
    pub session_date: NaiveDate,
    pub requested_at: DateTime<Utc>,
    /// Who asked (e.g. `server:band-entry`).
    pub reason: String,
    /// Filled when done: `ok`, or the error.
    #[serde(default)]
    pub outcome: Option<String>,
    #[serde(default)]
    pub done_at: Option<DateTime<Utc>>,
}

impl Archive {
    fn warmup_dir(&self, d: NaiveDate) -> PathBuf {
        self.root()
            .join("warmup")
            .join(d.format("%Y-%m-%d").to_string())
    }

    /// Files a request; `Ok(false)` when one already exists for that symbol and date.
    pub fn request_warmup(&self, req: &WarmupRequest) -> Result<bool, ArchiveError> {
        if !valid_symbol(&req.symbol) {
            return Err(ArchiveError::InvalidSymbol(req.symbol.clone()));
        }
        let dir = self.warmup_dir(req.session_date);
        let path = dir.join(format!("{}.json", req.symbol));
        if path.exists() || dir.join(format!("{}.done.json", req.symbol)).exists() {
            return Ok(false);
        }
        std::fs::create_dir_all(&dir).map_err(|e| ArchiveError::io(&dir, e))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(req).expect("serializes"))
            .map_err(|e| ArchiveError::io(&tmp, e))?;
        std::fs::rename(&tmp, &path).map_err(|e| ArchiveError::io(&path, e))?;
        Ok(true)
    }

    /// Open requests for `d`, oldest first, at most `max`.
    pub fn pending_warmups(
        &self,
        d: NaiveDate,
        max: usize,
    ) -> Result<Vec<WarmupRequest>, ArchiveError> {
        let dir = self.warmup_dir(d);
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(ArchiveError::io(&dir, e)),
        };
        let mut out = Vec::new();
        for p in rd.filter_map(|e| e.ok()).map(|e| e.path()) {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.ends_with(".json") || name.ends_with(".done.json") {
                continue;
            }
            let b = std::fs::read(&p).map_err(|e| ArchiveError::io(&p, e))?;
            out.push(
                serde_json::from_slice::<WarmupRequest>(&b)
                    .map_err(|e| ArchiveError::format(&p, e.to_string()))?,
            );
        }
        out.sort_by(|a, b| {
            a.requested_at
                .cmp(&b.requested_at)
                .then(a.symbol.cmp(&b.symbol))
        });
        out.truncate(max);
        Ok(out)
    }

    /// Closes a request with its outcome (`<SYMBOL>.done.json`); the open file is removed.
    pub fn finish_warmup(
        &self,
        mut req: WarmupRequest,
        outcome: String,
        at: DateTime<Utc>,
    ) -> Result<(), ArchiveError> {
        let dir = self.warmup_dir(req.session_date);
        let open = dir.join(format!("{}.json", req.symbol));
        let done = dir.join(format!("{}.done.json", req.symbol));
        req.outcome = Some(outcome);
        req.done_at = Some(at);
        std::fs::create_dir_all(&dir).map_err(|e| ArchiveError::io(&dir, e))?;
        std::fs::write(&done, serde_json::to_vec(&req).expect("serializes"))
            .map_err(|e| ArchiveError::io(&done, e))?;
        match std::fs::remove_file(&open) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(ArchiveError::io(&open, e)),
        }
    }
}
