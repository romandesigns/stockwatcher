//! The per-day `manifest.json`.

use crate::bars::BarEntry;
use market_events::Source;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Where an imported (non-fixture, non-live-recorded) file came from, so a re-run of the same
/// import can be recognized and an incomplete one is never mistaken for a full session.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportProvenance {
    /// e.g. `"alpaca"`.
    pub provider: String,
    /// e.g. `"trades"`.
    pub dataset: String,
    /// Endpoint path without credentials, e.g. `/v2/stocks/ABC/trades`.
    pub endpoint: String,
    /// e.g. `"sip"`.
    pub feed: String,
    /// The requested range, RFC 3339 UTC, `[start, end]` as sent to the provider.
    pub start: String,
    pub end: String,
    /// UTC RFC 3339 time the import finished.
    pub imported_at: String,
    /// Pages fetched (each one response).
    pub pages: u64,
    /// True only when every page was fetched and the provider returned no further page token.
    pub complete: bool,
    /// Counts of the provider's raw per-print update markers (Alpaca `u`), e.g. `{"canceled": 2}`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub updates: BTreeMap<String, u64>,
    /// Prints whose time went backwards against the previous one (expected 0 with `sort=asc`).
    #[serde(default)]
    pub out_of_order: u64,
}

impl ImportProvenance {
    /// Same provider, dataset, endpoint, feed and range: a re-run of the same import.
    pub fn same_request(&self, other: &ImportProvenance) -> bool {
        self.provider == other.provider
            && self.dataset == other.dataset
            && self.endpoint == other.endpoint
            && self.feed == other.feed
            && self.start == other.start
            && self.end == other.end
    }
}

pub const MANIFEST_FORMAT: &str = "stockwatcher-archive";

/// One finalized symbol file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolEntry {
    pub symbol: String,
    /// File name inside the day directory, e.g. `"SMPL.swa"`.
    pub file: String,
    /// Every source seen in the file (a fixture file has exactly `["fixture"]`).
    pub sources: Vec<Source>,
    pub events: u64,
    pub trades: u64,
    pub quotes: u64,
    pub corrections: u64,
    pub cancels: u64,
    pub statuses: u64,
    /// Smallest / largest `event_ts` in the file (UTC ns); `None` for an empty file.
    pub first_ts: Option<i64>,
    pub last_ts: Option<i64>,
    pub bytes: u64,
    /// Lowercase hex SHA-256 of the whole file.
    pub sha256: String,
    /// Set for files written by a provider import (absent for fixtures and live recordings).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub import: Option<ImportProvenance>,
}

impl SymbolEntry {
    pub fn is_fixture(&self) -> bool {
        !self.sources.is_empty() && self.sources.iter().all(|s| *s == Source::Fixture)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayManifest {
    /// Always [`MANIFEST_FORMAT`].
    pub format: String,
    pub format_version: u16,
    pub contract_version: u16,
    /// Trading date, `YYYY-MM-DD`.
    pub day: String,
    /// Union of every symbol's sources.
    pub sources: Vec<Source>,
    /// True when every file is fixture data. Fixture data is **never market evidence**.
    pub fixture: bool,
    /// Keyed by symbol, so the JSON is stably ordered.
    pub symbols: BTreeMap<String, SymbolEntry>,
    /// Provider bar datasets (never events), keyed `provider/granularity/SYMBOL`; see `bars`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub bars: BTreeMap<String, BarEntry>,
    /// Present only on a live recording's manifest (`<day>/live/manifest.json`, `crate::live`):
    /// the day is **provisional**, recorded from the live stream as it arrived, and is replaced
    /// by the nightly import. Absent on an imported (final) or fixture day.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live: Option<LiveDay>,
}

/// What a live recording's manifest says about itself.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveDay {
    /// The recorder has moved on to a later day and every file was checked to end on a whole
    /// frame and recounted: nothing is appended any more, so replay may open it. Until then the
    /// per-symbol counts and `bytes` are checkpoints (at most a few seconds behind the files).
    pub sealed: bool,
}

impl DayManifest {
    pub fn new(day: &str) -> Self {
        DayManifest {
            format: MANIFEST_FORMAT.to_string(),
            format_version: crate::format::FORMAT_VERSION,
            contract_version: market_events::CONTRACT_VERSION,
            day: day.to_string(),
            sources: Vec::new(),
            fixture: false,
            symbols: BTreeMap::new(),
            bars: BTreeMap::new(),
            live: None,
        }
    }

    /// Inserts or replaces a symbol and recomputes the day-level fields.
    pub fn upsert(&mut self, entry: SymbolEntry) {
        self.symbols.insert(entry.symbol.clone(), entry);
        self.recompute();
    }

    /// Recomputes the day-level fields from the symbols (after inserting many at once).
    pub fn recompute(&mut self) {
        let mut sources: Vec<Source> = self
            .symbols
            .values()
            .flat_map(|e| e.sources.iter().copied())
            .collect();
        sources.sort();
        sources.dedup();
        self.sources = sources;
        self.fixture = !self.symbols.is_empty() && self.symbols.values().all(|e| e.is_fixture());
    }

    pub fn total_events(&self) -> u64 {
        self.symbols.values().map(|e| e.events).sum()
    }
}
