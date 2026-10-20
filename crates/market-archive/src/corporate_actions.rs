//! Corporate-action announcements: the coverage policy, every retrieval (append-only), a ledger of
//! every action ever seen with its versions, and late discoveries. `$ARCHIVE_ROOT/corporate-actions/`.
//!
//! ```text
//! corporate-actions/
//!   2026-10-20/                          # the session date D a retrieval was made for
//!     20261020T073000.123Z-1a2b3c4d.json # one ActionRetrieval per fetch, never rewritten
//!   ledger.json                          # id → first_observed, every raw version (rewritten atomically)
//!   late.jsonl                           # one line per action found after it took effect (append-only)
//! ```
//!
//! What it is for: daily bars can only show a split after the split day's bar exists, so a live
//! decision on D needs the provider's announcements to know D's share basis
//! (market_archive::baseline). Facts about the provider that shape this
//! (docs/stock-context/daily-baseline-plan.md §6):
//!
//! - Requests are organized by **process date**, not by ex/effective date, and arrival can be
//!   delayed. So a retrieval asks for a window of process dates around D (policy
//!   `process_lookback_days` / `process_lookahead_days`) and the reader selects actions by their
//!   own ex/effective date, never by the bucket they came in.
//! - `data_quality=all` is requested: the default (`complete`) can omit early, incomplete
//!   records, and an incomplete record is exactly what must turn into "unresolved" rather than
//!   silence. Each action keeps the provider's quality flag when sent, and `complete` says whether
//!   every field the adjustment rule needs is present.
//! - Actions are filed under the **symbol in force** at the time (research report 02, F7), and a
//!   reverse split may carry an optional `new_symbol`. A retrieval therefore follows up name-change
//!   counterparts it did not ask for (bounded) so an old-symbol query does not lose the chain.
//! - **An empty response is never proof of no action.** The strongest statement a covered, fresh,
//!   fully paginated check supports is "no known relevant action", and the policy says what that
//!   permits.
//!
//! Every fetch is kept, including failed ones, with each request chunk's pages and whether it
//! completed. A symbol whose chunk did not complete is not covered: unavailable, never "no action".
//! Incomplete evidence for one symbol never affects another.

use crate::Archive;
use crate::error::ArchiveError;
use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;

pub const CORPORATE_ACTIONS_DIR: &str = "corporate-actions";
pub const ACTIONS_SCHEMA: u32 = 1;

/// The v1 coverage policy, byte for byte.
pub const COVERAGE_V1: &str = include_str!("../policy/corporate-actions-coverage-v1.json");

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct RetryRules {
    pub max_attempts: u32,
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
}

/// `policy/corporate-actions-coverage-v1.json`.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct CoverageBody {
    pub schema: u32,
    pub id: String,
    pub note: String,
    pub endpoint: String,
    pub types: Vec<String>,
    pub data_quality: String,
    pub process_lookback_days: i64,
    pub process_lookahead_days: i64,
    pub symbols_per_request: usize,
    pub page_limit: u32,
    pub retry: RetryRules,
    pub pre_session_fetch_et: String,
    pub pre_session_deadline_et: String,
    pub pre_session_retry_secs: i64,
    pub refresh_every_secs: i64,
    pub refresh_until_et: String,
    pub freshness_secs: i64,
    pub alias_followups_max: usize,
    pub no_known_action_permits_unchanged_basis: bool,
    pub validated: bool,
}

/// The policy with its identity (`id@sha256-prefix` of the exact bytes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoveragePolicy {
    pub body: CoverageBody,
    pub tag: String,
}

impl CoveragePolicy {
    pub fn v1() -> Self {
        Self::parse(COVERAGE_V1).expect("embedded coverage policy parses")
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let body: CoverageBody = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if body.schema != 1 {
            return Err(format!("coverage policy schema {}", body.schema));
        }
        let hash = hex::encode(Sha256::digest(text.as_bytes()));
        Ok(CoveragePolicy {
            tag: format!("{}@{}", body.id, &hash[..12]),
            body,
        })
    }

    fn time(s: &str) -> NaiveTime {
        NaiveTime::parse_from_str(s, "%H:%M").expect("policy times are HH:MM")
    }
    pub fn pre_session_fetch(&self) -> NaiveTime {
        Self::time(&self.body.pre_session_fetch_et)
    }
    pub fn pre_session_deadline(&self) -> NaiveTime {
        Self::time(&self.body.pre_session_deadline_et)
    }
    pub fn refresh_until(&self) -> NaiveTime {
        Self::time(&self.body.refresh_until_et)
    }

    /// The process-date window a retrieval for session date `d` asks for.
    pub fn process_window(&self, d: NaiveDate) -> (NaiveDate, NaiveDate) {
        (
            d - chrono::Duration::days(self.body.process_lookback_days),
            d + chrono::Duration::days(self.body.process_lookahead_days),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    ForwardSplit,
    ReverseSplit,
    UnitSplit,
    NameChange,
}

impl ActionKind {
    pub fn is_split(&self) -> bool {
        matches!(self, ActionKind::ForwardSplit | ActionKind::ReverseSplit)
    }
}

/// One announced action as delivered, normalized, with the raw record kept verbatim.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CorporateAction {
    /// Provider id (stable across versions of the same action).
    pub id: String,
    pub kind: ActionKind,
    /// Splits: the symbol in force on the ex date. Name change / unit split: the old symbol.
    pub symbol: Option<String>,
    /// Name change: the new symbol. Reverse split / unit split: the optional new symbol.
    pub new_symbol: Option<String>,
    /// Unit split: the alternate symbol (warrants, rights).
    pub alternate_symbol: Option<String>,
    /// When it changes the share basis or the ticker: splits `ex_date`, unit splits
    /// `effective_date`, name changes `process_date` (the only date Alpaca gives one).
    pub effective_date: Option<NaiveDate>,
    pub process_date: Option<NaiveDate>,
    pub old_rate: Option<f64>,
    pub new_rate: Option<f64>,
    /// The provider's own completeness/quality flag, when the record carries one. No recorded
    /// response contained one yet (the probes predate `data_quality=all`), so the field is
    /// searched under several names (`data_quality`, `quality`, `status`) and kept as text.
    pub quality: Option<String>,
    /// Every field the adjustment rule needs is present and well formed, and the provider did not
    /// flag the record as incomplete.
    pub complete: bool,
    pub raw: serde_json::Value,
}

impl CorporateAction {
    /// Price multiplier `old/new` of a complete split with whole-number rates.
    pub fn integral_rates(&self) -> Option<(u64, u64)> {
        let ok = |x: f64| x.is_finite() && x >= 1.0 && x.fract() == 0.0;
        match (self.old_rate, self.new_rate) {
            (Some(o), Some(n)) if ok(o) && ok(n) => Some((o as u64, n as u64)),
            _ => None,
        }
    }

    /// Mentions `s` as its symbol, new symbol or alternate symbol.
    pub fn touches(&self, s: &str) -> bool {
        [&self.symbol, &self.new_symbol, &self.alternate_symbol]
            .iter()
            .any(|x| x.as_deref() == Some(s))
    }
}

/// One request chunk of a retrieval.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChunkOutcome {
    pub symbols: Vec<String>,
    pub pages: u32,
    /// Every page fetched and parsed, no next-page token left.
    pub complete: bool,
    pub error: Option<String>,
    /// Requested as a name-change follow-up, not from the watch list.
    pub followup: bool,
}

/// One announcement fetch, kept as made.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActionRetrieval {
    pub schema: u32,
    /// `corporate-actions-coverage-v1@…` in force when it was made.
    pub policy: String,
    pub retrieved_at: DateTime<Utc>,
    pub provider: String,
    pub endpoint: String,
    pub types: Vec<String>,
    pub data_quality: String,
    /// The session date this retrieval was made for.
    pub session_date: NaiveDate,
    /// Inclusive process-date window requested.
    pub process_start: NaiveDate,
    pub process_end: NaiveDate,
    pub symbols_requested: Vec<String>,
    /// Sorted. Symbols whose chunk completed (follow-ups included). Only these are vouched for.
    pub symbols_covered: Vec<String>,
    pub chunks: Vec<ChunkOutcome>,
    /// Every chunk completed.
    pub success: bool,
    pub requests: u64,
    pub command: String,
    pub actions: Vec<CorporateAction>,
}

impl ActionRetrieval {
    pub fn covers(&self, symbol: &str) -> bool {
        self.symbols_covered
            .binary_search_by(|s| s.as_str().cmp(symbol))
            .is_ok()
    }

    /// The file name: retrieval time (ms) plus a short hash of the request.
    pub fn file_name(&self) -> String {
        let mut h = Sha256::new();
        h.update(self.process_start.to_string());
        h.update(self.process_end.to_string());
        for s in &self.symbols_requested {
            h.update(s.as_bytes());
            h.update(b",");
        }
        format!(
            "{}-{}.json",
            self.retrieved_at.format("%Y%m%dT%H%M%S%.3fZ"),
            hex::encode(&h.finalize()[..4])
        )
    }

    /// A stable id for evidence provenance: `<session date>/<file name>`.
    pub fn id(&self) -> String {
        format!("{}/{}", self.session_date, self.file_name())
    }
}

/// Every version of one action as seen over time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub first_observed: DateTime<Utc>,
    pub first_retrieval: String,
    /// `(observed_at, retrieval id, normalized action)`; a new version only when it changed.
    pub versions: Vec<(DateTime<Utc>, String, CorporateAction)>,
}

pub type ActionLedger = BTreeMap<String, LedgerEntry>;

/// An action first seen (or first seen with its current terms) after it had already taken
/// effect. Recorded with the evidence records it could have affected; nothing is rewritten.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LateAction {
    pub action_id: String,
    pub symbol: Option<String>,
    pub effective_date: NaiveDate,
    pub discovered_at: DateTime<Utc>,
    pub retrieval: String,
    /// `new` (first sighting) or `changed` (terms changed in a later version).
    pub kind: String,
    /// Evidence record ids (market_archive::baseline) for the trading dates from the effective
    /// date through the discovery date: the decisions that may have used a wrong basis. Decision
    /// ids proper come once decisions are logged (server increment 2b).
    pub affected_records: Vec<String>,
}

fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> Result<(), ArchiveError> {
    let dir = path.parent().expect("has a parent");
    std::fs::create_dir_all(dir).map_err(|e| ArchiveError::io(dir, e))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes).map_err(|e| ArchiveError::io(&tmp, e))?;
    if let Ok(f) = std::fs::File::open(&tmp) {
        let _ = f.sync_all();
    }
    std::fs::rename(&tmp, path).map_err(|e| ArchiveError::io(path, e))
}

impl Archive {
    pub fn corporate_actions_root(&self) -> PathBuf {
        self.root().join(CORPORATE_ACTIONS_DIR)
    }

    pub fn corporate_actions_dir(&self, session: NaiveDate) -> PathBuf {
        self.corporate_actions_root()
            .join(session.format("%Y-%m-%d").to_string())
    }

    /// Appends one retrieval; never overwrites another.
    pub fn write_action_retrieval(&self, r: &ActionRetrieval) -> Result<PathBuf, ArchiveError> {
        let path = self
            .corporate_actions_dir(r.session_date)
            .join(r.file_name());
        if path.exists() {
            return Err(ArchiveError::format(&path, "retrieval already exists"));
        }
        write_atomic(&path, &serde_json::to_vec(r).expect("retrieval serializes"))?;
        Ok(path)
    }

    /// Every retrieval made for session date `d`, oldest first. Another schema is an error.
    pub fn read_action_retrievals(
        &self,
        d: NaiveDate,
    ) -> Result<Vec<ActionRetrieval>, ArchiveError> {
        let dir = self.corporate_actions_dir(d);
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

    pub fn read_action_ledger(&self) -> Result<ActionLedger, ArchiveError> {
        let p = self.corporate_actions_root().join("ledger.json");
        match std::fs::read(&p) {
            Ok(b) => {
                serde_json::from_slice(&b).map_err(|e| ArchiveError::format(&p, e.to_string()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ActionLedger::new()),
            Err(e) => Err(ArchiveError::io(&p, e)),
        }
    }

    /// Folds a retrieval into the ledger (first sighting, new versions) and appends every late
    /// discovery to `late.jsonl` (see [`late_actions`]). Returns the late actions.
    pub fn record_actions(
        &self,
        r: &ActionRetrieval,
        affected: &dyn Fn(&CorporateAction, NaiveDate, NaiveDate) -> Vec<String>,
    ) -> Result<Vec<LateAction>, ArchiveError> {
        let mut ledger = self.read_action_ledger()?;
        let late = late_actions(&mut ledger, r, affected);
        let p = self.corporate_actions_root().join("ledger.json");
        write_atomic(&p, &serde_json::to_vec(&ledger).expect("ledger serializes"))?;
        if !late.is_empty() {
            let lp = self.corporate_actions_root().join("late.jsonl");
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&lp)
                .map_err(|e| ArchiveError::io(&lp, e))?;
            for l in &late {
                let mut line = serde_json::to_vec(l).expect("late action serializes");
                line.push(b'\n');
                f.write_all(&line).map_err(|e| ArchiveError::io(&lp, e))?;
            }
            let _ = f.sync_all();
        }
        Ok(late)
    }

    pub fn read_late_actions(&self) -> Result<Vec<LateAction>, ArchiveError> {
        let p = self.corporate_actions_root().join("late.jsonl");
        let text = match std::fs::read_to_string(&p) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(ArchiveError::io(&p, e)),
        };
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).map_err(|e| ArchiveError::format(&p, e.to_string())))
            .collect()
    }
}

/// Updates `ledger` with `r` and returns the late discoveries: an action (first seen, or seen
/// with changed terms) whose effective date's 04:00 ET session start had already passed at
/// retrieval. `affected(action, from, through)` names the evidence records of that stretch.
pub fn late_actions(
    ledger: &mut ActionLedger,
    r: &ActionRetrieval,
    affected: &dyn Fn(&CorporateAction, NaiveDate, NaiveDate) -> Vec<String>,
) -> Vec<LateAction> {
    let mut late = Vec::new();
    let found_on = r
        .retrieved_at
        .with_timezone(&chrono_tz::America::New_York)
        .date_naive();
    for a in &r.actions {
        let kind = match ledger.get_mut(&a.id) {
            None => {
                ledger.insert(
                    a.id.clone(),
                    LedgerEntry {
                        first_observed: r.retrieved_at,
                        first_retrieval: r.id(),
                        versions: vec![(r.retrieved_at, r.id(), a.clone())],
                    },
                );
                "new"
            }
            Some(e) => {
                let last = &e.versions.last().expect("non-empty").2;
                if last.raw == a.raw {
                    continue;
                }
                let terms_changed = last.effective_date != a.effective_date
                    || last.old_rate != a.old_rate
                    || last.new_rate != a.new_rate
                    || last.symbol != a.symbol
                    || last.new_symbol != a.new_symbol;
                e.versions.push((r.retrieved_at, r.id(), a.clone()));
                if !terms_changed {
                    continue;
                }
                "changed"
            }
        };
        let Some(eff) = a.effective_date else {
            continue;
        };
        let started = market_events::session::sessions_for(eff)
            .into_iter()
            .find(|s| s.0.kind == market_events::SessionKind::Pre)
            .map(|s| s.1.to_utc());
        if started.is_some_and(|t| r.retrieved_at > t) {
            late.push(LateAction {
                action_id: a.id.clone(),
                symbol: a.new_symbol.clone().or_else(|| a.symbol.clone()),
                effective_date: eff,
                discovered_at: r.retrieved_at,
                retrieval: r.id(),
                kind: kind.into(),
                affected_records: affected(a, eff, found_on.max(eff)),
            });
        }
    }
    late
}
