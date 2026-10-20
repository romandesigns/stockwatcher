//! Corporate-action announcement retrievals, append-only: `$ARCHIVE_ROOT/corporate-actions/`.
//!
//! ```text
//! corporate-actions/
//!   2026-10-20/                              # the request's `end` date: the trading date D it vouches for
//!     20261020T110002Z-1a2b3c4d.json         # one ActionRetrieval per fetch, never rewritten
//! ```
//!
//! What it is for: daily bars can only show a split after the split day's bar exists, so a live
//! decision on D could never have a checked previous close or a baseline on D's share basis
//! (market_archive::baseline). A pre-session retrieval of Alpaca's announced splits whose
//! effective date is in `(P, D]` lets the loader say "nothing announced for D" or "D is a 1:30
//! reverse split". That is a **weaker kind of evidence** than bar verification (it trusts the
//! feed's completeness and timeliness) and every record built from it says so
//! (`stock_context::ActionsSource::Announced`).
//!
//! Every fetch is kept, including failed ones (`success: false` with the error), with its request
//! parameters, the symbols it requested and the symbols it **covered** (every page of their
//! request chunk arrived). A symbol not covered is unknown, never "no action".
//!
//! Alpaca keys actions by the symbol **in force at the time** (research report 02, F7: SGRX's
//! three reverse splits sit under BTOG). A retrieval therefore also records name changes; the
//! loader refuses to vouch for a symbol whose name changed in `(P, D]` rather than guess which
//! ticker the split was filed under.

use crate::Archive;
use crate::error::ArchiveError;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const CORPORATE_ACTIONS_DIR: &str = "corporate-actions";
pub const ACTIONS_SCHEMA: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitKind {
    Forward,
    Reverse,
}

/// A forward or reverse split as announced (`old_rate` shares become `new_rate`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SplitAction {
    pub id: String,
    pub kind: SplitKind,
    /// The symbol in force on the ex date.
    pub symbol: String,
    pub ex_date: NaiveDate,
    pub old_rate: f64,
    pub new_rate: f64,
}

/// A unit split (units into shares plus warrants or rights). Recorded, never used to restate a
/// price: the share part's ratio alone does not say what one old unit is worth. Field names follow
/// Alpaca's documentation; no recorded response contained one yet, so every field but the id is
/// optional.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UnitSplitAction {
    pub id: String,
    pub old_symbol: Option<String>,
    pub new_symbol: Option<String>,
    pub alternate_symbol: Option<String>,
    pub effective_date: Option<NaiveDate>,
}

/// A ticker change. Recorded; not used for adjustment yet.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NameChange {
    pub id: String,
    pub old_symbol: String,
    pub new_symbol: String,
    pub process_date: NaiveDate,
}

/// One announcement fetch, kept as made.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActionRetrieval {
    pub schema: u32,
    pub retrieved_at: DateTime<Utc>,
    /// `alpaca`.
    pub provider: String,
    /// `/v1/corporate-actions`.
    pub endpoint: String,
    /// Action types requested (`forward_split`, `reverse_split`, `unit_split`, `name_change`).
    pub types: Vec<String>,
    /// Inclusive date range of the request (Alpaca filters each type on its own date field).
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub symbols_requested: Vec<String>,
    /// Sorted. Symbols whose request chunk completed (all pages). Only these are vouched for.
    pub symbols_covered: Vec<String>,
    /// Every chunk completed.
    pub success: bool,
    pub errors: Vec<String>,
    pub requests: u64,
    /// `maintain`, …
    pub command: String,
    pub splits: Vec<SplitAction>,
    pub unit_splits: Vec<UnitSplitAction>,
    pub name_changes: Vec<NameChange>,
}

impl ActionRetrieval {
    pub fn covers(&self, symbol: &str) -> bool {
        self.symbols_covered
            .binary_search_by(|s| s.as_str().cmp(symbol))
            .is_ok()
    }

    /// The file name: retrieval time plus a short hash of the request.
    pub fn file_name(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(self.start.to_string());
        h.update(self.end.to_string());
        for s in &self.symbols_requested {
            h.update(s.as_bytes());
            h.update(b",");
        }
        format!(
            "{}-{}.json",
            self.retrieved_at.format("%Y%m%dT%H%M%SZ"),
            hex::encode(&h.finalize()[..4])
        )
    }

    /// A stable id for evidence provenance: `<end>/<file name>`.
    pub fn id(&self) -> String {
        format!("{}/{}", self.end, self.file_name())
    }
}

impl Archive {
    pub fn corporate_actions_dir(&self, end: NaiveDate) -> PathBuf {
        self.root()
            .join(CORPORATE_ACTIONS_DIR)
            .join(end.format("%Y-%m-%d").to_string())
    }

    /// Appends one retrieval; never overwrites another.
    pub fn write_action_retrieval(&self, r: &ActionRetrieval) -> Result<PathBuf, ArchiveError> {
        let path = self.corporate_actions_dir(r.end).join(r.file_name());
        if path.exists() {
            return Err(ArchiveError::format(&path, "retrieval already exists"));
        }
        let dir = path.parent().expect("has a parent");
        std::fs::create_dir_all(dir).map_err(|e| ArchiveError::io(dir, e))?;
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_vec(r).expect("retrieval serializes");
        std::fs::write(&tmp, json).map_err(|e| ArchiveError::io(&tmp, e))?;
        if let Ok(f) = std::fs::File::open(&tmp) {
            let _ = f.sync_all();
        }
        std::fs::rename(&tmp, &path).map_err(|e| ArchiveError::io(&path, e))?;
        Ok(path)
    }

    /// Every retrieval whose request ended on `end`, oldest first. A file of another schema is an
    /// error, never skipped.
    pub fn read_action_retrievals(
        &self,
        end: NaiveDate,
    ) -> Result<Vec<ActionRetrieval>, ArchiveError> {
        let dir = self.corporate_actions_dir(end);
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(ArchiveError::io(&dir, e)),
        };
        let mut out = Vec::new();
        for p in rd.filter_map(|e| e.ok()).map(|e| e.path()) {
            if p.extension().is_none_or(|x| x != "json") {
                continue;
            }
            let bytes = std::fs::read(&p).map_err(|e| ArchiveError::io(&p, e))?;
            let r: ActionRetrieval = serde_json::from_slice(&bytes)
                .map_err(|e| ArchiveError::format(&p, e.to_string()))?;
            if r.schema != ACTIONS_SCHEMA {
                return Err(ArchiveError::format(
                    &p,
                    format!("corporate actions schema {}", r.schema),
                ));
            }
            out.push(r);
        }
        out.sort_by_key(|r| r.retrieved_at);
        Ok(out)
    }
}
