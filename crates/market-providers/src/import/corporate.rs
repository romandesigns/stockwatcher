//! Alpaca corporate-action announcements (`GET /v1/corporate-actions` on the data API) for the
//! stock context's live share-basis check (market_archive::corporate_actions,
//! market_archive::baseline::announced).
//!
//! Request and response shapes are the ones recorded by the research probes
//! (`reports/probes/alp-corp-actions-{BTOG,SGRX}.json`, 2026-10-19; copies in
//! `tests/fixtures/`):
//!
//! ```text
//! /v1/corporate-actions?symbols=A,B&types=forward_split,reverse_split,unit_split,name_change
//!                      &start=YYYY-MM-DD&end=YYYY-MM-DD&limit=1000[&page_token=…]
//! {"corporate_actions": {"reverse_splits": [{"id","symbol","ex_date","old_rate","new_rate",…}],
//!                        "forward_splits": [...], "unit_splits": [...],
//!                        "name_changes": [{"id","old_symbol","new_symbol","process_date",…}]},
//!  "next_page_token": null}
//! ```
//!
//! Keyed by the symbol in force at the time (report 02, F7): BTOG's reverse splits are not
//! returned for SGRX, only the rename is. The loader handles that by refusing a symbol renamed in
//! the window; it does not walk rename chains.
//!
//! A fetch never fails as a whole: each chunk of symbols either completes (every page parsed) and
//! its symbols are **covered**, or its error is recorded and its symbols are not. The retrieval is
//! stored either way (append-only), so a gap is visible rather than read as "no actions".

use super::alpaca::AlpacaCredentials;
use super::batch::{Paced, RateLimiter, alpaca_headers};
use super::http::{Http, RetryPolicy, get_with_retry};
use chrono::{NaiveDate, Utc};
use market_archive::{
    ACTIONS_SCHEMA, ActionRetrieval, NameChange, SplitAction, SplitKind, UnitSplitAction,
};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::time::Duration;

/// Types requested. Name changes are recorded (not used for adjustment yet); dividends and
/// spin-offs are not requested: the rule is split-only.
pub const ACTION_TYPES: &[&str] = &[
    "forward_split",
    "reverse_split",
    "unit_split",
    "name_change",
];
/// Symbols per request (keeps URLs short; a day's watch list of ~7,200 is ~72 requests).
pub const SYMBOLS_PER_ACTIONS_REQUEST: usize = 100;
/// Records per page.
pub const ACTIONS_PAGE_LIMIT: u32 = 1000;

#[derive(Deserialize)]
struct RawSplit {
    id: String,
    symbol: String,
    ex_date: NaiveDate,
    old_rate: f64,
    new_rate: f64,
}

#[derive(Deserialize)]
struct RawUnitSplit {
    id: String,
    #[serde(default)]
    old_symbol: Option<String>,
    #[serde(default)]
    new_symbol: Option<String>,
    #[serde(default)]
    alternate_symbol: Option<String>,
    #[serde(default)]
    effective_date: Option<NaiveDate>,
}

#[derive(Deserialize)]
struct RawNameChange {
    id: String,
    old_symbol: String,
    new_symbol: String,
    process_date: NaiveDate,
}

#[derive(Deserialize, Default)]
struct Actions {
    #[serde(default)]
    forward_splits: Vec<RawSplit>,
    #[serde(default)]
    reverse_splits: Vec<RawSplit>,
    #[serde(default)]
    unit_splits: Vec<RawUnitSplit>,
    #[serde(default)]
    name_changes: Vec<RawNameChange>,
}

#[derive(Deserialize)]
struct Page {
    #[serde(default)]
    corporate_actions: Option<Actions>,
    #[serde(default)]
    next_page_token: Option<String>,
}

/// What one chunk returned, before it is known to have completed.
#[derive(Default)]
struct Found {
    splits: BTreeMap<String, SplitAction>,
    units: BTreeMap<String, UnitSplitAction>,
    names: BTreeMap<String, NameChange>,
}

impl Found {
    fn add(&mut self, a: Actions) {
        for (kind, list) in [
            (SplitKind::Forward, a.forward_splits),
            (SplitKind::Reverse, a.reverse_splits),
        ] {
            for s in list {
                self.splits.insert(
                    s.id.clone(),
                    SplitAction {
                        id: s.id,
                        kind,
                        symbol: s.symbol.trim().to_ascii_uppercase(),
                        ex_date: s.ex_date,
                        old_rate: s.old_rate,
                        new_rate: s.new_rate,
                    },
                );
            }
        }
        for u in a.unit_splits {
            let up = |s: Option<String>| s.map(|s| s.trim().to_ascii_uppercase());
            self.units.insert(
                u.id.clone(),
                UnitSplitAction {
                    id: u.id,
                    old_symbol: up(u.old_symbol),
                    new_symbol: up(u.new_symbol),
                    alternate_symbol: up(u.alternate_symbol),
                    effective_date: u.effective_date,
                },
            );
        }
        for n in a.name_changes {
            self.names.insert(
                n.id.clone(),
                NameChange {
                    id: n.id,
                    old_symbol: n.old_symbol.trim().to_ascii_uppercase(),
                    new_symbol: n.new_symbol.trim().to_ascii_uppercase(),
                    process_date: n.process_date,
                },
            );
        }
    }
}

/// One announcement retrieval for `symbols` over `start..=end` (normally P..=D, made pre-market
/// on D), paced at `per_minute` by its own limiter. Never returns an error: failures are in the
/// record (`success: false`, `errors`, symbols missing from `symbols_covered`).
#[allow(clippy::too_many_arguments)]
pub fn fetch_announcements(
    http: &mut dyn Http,
    data_base: &str,
    creds: &AlpacaCredentials,
    symbols: &[String],
    start: NaiveDate,
    end: NaiveDate,
    per_minute: u32,
    command: &str,
    log: &mut dyn FnMut(&str),
) -> ActionRetrieval {
    let mut symbols: Vec<String> = symbols
        .iter()
        .map(|s| s.trim().to_ascii_uppercase())
        .filter(|s| !s.is_empty())
        .collect();
    symbols.sort();
    symbols.dedup();
    let headers = alpaca_headers(creds);
    let secrets = [creds.key_id.as_str(), creds.secret.as_str()];
    let mut sleep = |d: Duration| std::thread::sleep(d);
    let mut paced = Paced {
        inner: http,
        limiter: RateLimiter::per_minute(per_minute),
    };
    let mut all = Found::default();
    let mut covered = Vec::new();
    let mut errors = Vec::new();
    let mut requests = 0u64;
    for chunk in symbols.chunks(SYMBOLS_PER_ACTIONS_REQUEST) {
        let mut found = Found::default();
        let mut token: Option<String> = None;
        let outcome: Result<(), String> = loop {
            let mut u = format!(
                "{}/v1/corporate-actions?symbols={}&types={}&start={start}&end={end}&limit={ACTIONS_PAGE_LIMIT}",
                data_base.trim_end_matches('/'),
                chunk.join("%2C"),
                ACTION_TYPES.join("%2C"),
            );
            if let Some(t) = &token {
                u.push_str("&page_token=");
                u.push_str(t);
            }
            let resp = match get_with_retry(
                &mut paced,
                &u,
                &headers,
                RetryPolicy::default(),
                &secrets,
                &mut sleep,
                log,
            ) {
                Ok(r) => r,
                Err(e) => break Err(format!("corporate actions: {e}")),
            };
            requests += 1;
            let page: Page = match serde_json::from_slice(&resp.body) {
                Ok(p) => p,
                Err(e) => break Err(format!("corporate actions: {e}")),
            };
            found.add(page.corporate_actions.unwrap_or_default());
            match page.next_page_token.filter(|t| !t.is_empty()) {
                Some(t) if token.as_deref() != Some(t.as_str()) => token = Some(t),
                Some(_) => break Err("corporate actions: repeated page token".into()),
                None => break Ok(()),
            }
        };
        match outcome {
            Ok(()) => {
                covered.extend(chunk.iter().cloned());
                all.splits.extend(found.splits);
                all.units.extend(found.units);
                all.names.extend(found.names);
            }
            Err(e) => {
                log(&format!(
                    "{e} ({} symbols from {} not covered)",
                    chunk.len(),
                    chunk[0]
                ));
                errors.push(format!("{} ({}..): {e}", chunk.len(), chunk[0]));
            }
        }
    }
    covered.sort();
    let success = errors.is_empty();
    log(&format!(
        "corporate actions {start}..{end}: {}/{} symbols covered, {} splits, {} unit splits, {} name changes, {requests} requests",
        covered.len(),
        symbols.len(),
        all.splits.len(),
        all.units.len(),
        all.names.len()
    ));
    ActionRetrieval {
        schema: ACTIONS_SCHEMA,
        retrieved_at: Utc::now(),
        provider: "alpaca".into(),
        endpoint: "/v1/corporate-actions".into(),
        types: ACTION_TYPES.iter().map(|s| s.to_string()).collect(),
        start,
        end,
        symbols_requested: symbols,
        symbols_covered: covered,
        success,
        errors,
        requests,
        command: command.into(),
        splits: all.splits.into_values().collect(),
        unit_splits: all.units.into_values().collect(),
        name_changes: all.names.into_values().collect(),
    }
}
