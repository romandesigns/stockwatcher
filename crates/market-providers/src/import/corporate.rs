//! Alpaca corporate-action announcements (`GET https://data.alpaca.markets/v1/corporate-actions`,
//! the market-data host, same as the bars) for the stock context's live share-basis check
//! (market_archive::corporate_actions, market_archive::baseline::coverage_for). Every parameter
//! comes from the versioned coverage policy (`corporate-actions-coverage-v1`).
//!
//! Request and response shapes are the ones recorded by the research probes
//! (`reports/probes/alp-corp-actions-{BTOG,SGRX}.json`, 2026-10-19; copies in `tests/fixtures/`):
//!
//! ```text
//! /v1/corporate-actions?symbols=A,B&types=forward_split,reverse_split,unit_split,name_change
//!                      &start=<process from>&end=<process to>&data_quality=all&limit=1000[&page_token=…]
//! {"corporate_actions": {"reverse_splits": [{"id","symbol","new_symbol"?,"ex_date","process_date",
//!                                            "old_rate","new_rate",…}],
//!                        "forward_splits": [...], "unit_splits": [...],
//!                        "name_changes": [{"id","old_symbol","new_symbol","process_date",…}]},
//!  "next_page_token": null}
//! ```
//!
//! `start`/`end` select by **process date**: the window is `[D − lookback, D + lookahead]` and
//! the reader picks actions by their own ex/effective date. `data_quality=all` asks for early,
//! incomplete records too (the default `complete` would hide them). Each record is kept verbatim
//! (`raw`) next to its normalized form.
//!
//! Follow-ups: a name change (or a reverse split with `new_symbol`) whose other ticker was not
//! requested is fetched in a second, bounded pass (`alias_followups_max`), because Alpaca files
//! actions under the ticker in force (report 02, F7) and an old-symbol query alone loses the
//! chain.
//!
//! A fetch never fails as a whole: each chunk either completes (every page parsed, no token left)
//! and its symbols are covered, or its error is recorded and its symbols are not.

use super::alpaca::AlpacaCredentials;
use super::batch::{Paced, RateLimiter, alpaca_headers};
use super::http::{Http, RetryPolicy, get_with_retry};
use chrono::{NaiveDate, Utc};
use market_archive::{
    ACTIONS_SCHEMA, ActionKind, ActionRetrieval, ChunkOutcome, CorporateAction, CoveragePolicy,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

#[derive(Deserialize)]
struct Page {
    #[serde(default)]
    corporate_actions: Option<BTreeMap<String, Vec<Value>>>,
    #[serde(default)]
    next_page_token: Option<String>,
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k)
        .and_then(Value::as_str)
        .map(|x| x.trim().to_ascii_uppercase())
        .filter(|x| !x.is_empty())
}

fn date(v: &Value, k: &str) -> Option<NaiveDate> {
    v.get(k)
        .and_then(Value::as_str)
        .and_then(|x| NaiveDate::parse_from_str(x, "%Y-%m-%d").ok())
}

fn num(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(Value::as_f64).filter(|x| x.is_finite())
}

/// The provider's completeness flag, if the record has one (the field name is not in any
/// recorded response yet, so the likely names are tried).
fn quality(v: &Value) -> Option<String> {
    ["data_quality", "quality", "status"]
        .iter()
        .find_map(|k| v.get(*k).and_then(Value::as_str).map(str::to_string))
}

/// One raw record of a group (`reverse_splits`, …) → a normalized action; `None` for a group
/// that is not requested or a record without an id.
pub fn normalize(group: &str, v: &Value) -> Option<CorporateAction> {
    let id = v.get("id").and_then(Value::as_str)?.to_string();
    let q = quality(v);
    let flagged_incomplete = q
        .as_deref()
        .is_some_and(|q| !q.eq_ignore_ascii_case("complete"));
    let mut a = CorporateAction {
        id,
        kind: ActionKind::NameChange,
        symbol: None,
        new_symbol: None,
        alternate_symbol: None,
        effective_date: None,
        process_date: date(v, "process_date"),
        old_rate: num(v, "old_rate"),
        new_rate: num(v, "new_rate"),
        quality: q,
        complete: false,
        raw: v.clone(),
    };
    match group {
        "forward_splits" | "reverse_splits" => {
            a.kind = if group == "forward_splits" {
                ActionKind::ForwardSplit
            } else {
                ActionKind::ReverseSplit
            };
            a.symbol = s(v, "symbol");
            a.new_symbol = s(v, "new_symbol");
            a.effective_date = date(v, "ex_date");
            a.complete = a.symbol.is_some()
                && a.effective_date.is_some()
                && a.old_rate.is_some_and(|x| x > 0.0)
                && a.new_rate.is_some_and(|x| x > 0.0);
        }
        "unit_splits" => {
            a.kind = ActionKind::UnitSplit;
            a.symbol = s(v, "old_symbol");
            a.new_symbol = s(v, "new_symbol");
            a.alternate_symbol = s(v, "alternate_symbol");
            a.effective_date = date(v, "effective_date");
            a.complete = a.symbol.is_some() && a.effective_date.is_some();
        }
        "name_changes" => {
            a.kind = ActionKind::NameChange;
            a.symbol = s(v, "old_symbol");
            a.new_symbol = s(v, "new_symbol");
            a.effective_date = a.process_date;
            a.complete = a.symbol.is_some() && a.new_symbol.is_some() && a.process_date.is_some();
        }
        _ => return None,
    }
    a.complete &= !flagged_incomplete;
    Some(a)
}

struct Fetch<'a> {
    paced: Paced<'a>,
    headers: [(&'a str, &'a str); 3],
    secrets: [&'a str; 2],
    retry: RetryPolicy,
    base: String,
    policy: &'a CoveragePolicy,
    start: NaiveDate,
    end: NaiveDate,
    requests: u64,
}

impl Fetch<'_> {
    /// One chunk, every page. `Ok(actions)` only when the last page had no token.
    fn chunk(
        &mut self,
        symbols: &[String],
        log: &mut dyn FnMut(&str),
    ) -> (u32, Result<Vec<CorporateAction>, String>) {
        let mut sleep = |d: Duration| std::thread::sleep(d);
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0u32;
        loop {
            let mut u = format!(
                "{}/v1/corporate-actions?symbols={}&types={}&start={}&end={}&data_quality={}&limit={}",
                self.base,
                symbols.join("%2C"),
                self.policy.body.types.join("%2C"),
                self.start,
                self.end,
                self.policy.body.data_quality,
                self.policy.body.page_limit,
            );
            if let Some(t) = &token {
                u.push_str("&page_token=");
                u.push_str(t);
            }
            let resp = match get_with_retry(
                &mut self.paced,
                &u,
                &self.headers,
                self.retry,
                &self.secrets,
                &mut sleep,
                log,
            ) {
                Ok(r) => r,
                Err(e) => return (pages, Err(format!("corporate actions: {e}"))),
            };
            self.requests += 1;
            pages += 1;
            let page: Page = match serde_json::from_slice(&resp.body) {
                Ok(p) => p,
                Err(e) => return (pages, Err(format!("corporate actions: {e}"))),
            };
            for (group, records) in page.corporate_actions.unwrap_or_default() {
                for r in &records {
                    if let Some(a) = normalize(&group, r) {
                        out.push(a);
                    }
                }
            }
            match page.next_page_token.filter(|t| !t.is_empty()) {
                Some(t) if token.as_deref() != Some(t.as_str()) => token = Some(t),
                Some(_) => return (pages, Err("corporate actions: repeated page token".into())),
                None => return (pages, Ok(out)),
            }
        }
    }
}

#[derive(Default)]
struct Acc {
    actions: BTreeMap<String, CorporateAction>,
    covered: BTreeSet<String>,
    chunks: Vec<ChunkOutcome>,
}

fn run(
    f: &mut Fetch,
    acc: &mut Acc,
    list: &[String],
    per: usize,
    followup: bool,
    log: &mut dyn FnMut(&str),
) {
    for chunk in list.chunks(per) {
        let (pages, res) = f.chunk(chunk, log);
        acc.chunks.push(ChunkOutcome {
            symbols: chunk.to_vec(),
            pages,
            complete: res.is_ok(),
            error: res.as_ref().err().cloned(),
            followup,
        });
        match res {
            Ok(list) => {
                acc.covered.extend(chunk.iter().cloned());
                for a in list {
                    acc.actions.insert(a.id.clone(), a);
                }
            }
            Err(e) => log(&format!("{e} ({} symbols from {})", chunk.len(), chunk[0])),
        }
    }
}

/// One announcement retrieval for session date `d` and `symbols` (module docs), paced at
/// `per_minute` by its own limiter. Never returns an error: failures are in the record.
#[allow(clippy::too_many_arguments)]
pub fn fetch_announcements(
    http: &mut dyn Http,
    data_base: &str,
    creds: &AlpacaCredentials,
    policy: &CoveragePolicy,
    symbols: &[String],
    d: NaiveDate,
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
    let (start, end) = policy.process_window(d);
    let b = &policy.body.retry;
    let mut f = Fetch {
        paced: Paced {
            inner: http,
            limiter: RateLimiter::per_minute(per_minute),
        },
        headers: alpaca_headers(creds),
        secrets: [creds.key_id.as_str(), creds.secret.as_str()],
        retry: RetryPolicy {
            max_attempts: b.max_attempts,
            base_delay: Duration::from_millis(b.base_delay_ms),
            max_delay: Duration::from_millis(b.max_delay_ms),
        },
        base: data_base.trim_end_matches('/').to_string(),
        policy,
        start,
        end,
        requests: 0,
    };
    let mut acc = Acc::default();
    let per = policy.body.symbols_per_request.max(1);
    run(&mut f, &mut acc, &symbols, per, false, log);
    let Acc {
        mut actions,
        mut covered,
        mut chunks,
    } = acc;
    // Follow-up pass: tickers on the other side of a rename that were not requested.
    let requested: BTreeSet<&String> = symbols.iter().collect();
    let mut extra: Vec<String> = actions
        .values()
        .filter(|a| {
            a.kind == ActionKind::NameChange
                || (a.kind == ActionKind::ReverseSplit && a.new_symbol.is_some())
        })
        .flat_map(|a| [a.symbol.clone(), a.new_symbol.clone()])
        .flatten()
        .filter(|s| !requested.contains(s))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if extra.len() > policy.body.alias_followups_max {
        log(&format!(
            "corporate actions: {} rename follow-ups, only {} fetched",
            extra.len(),
            policy.body.alias_followups_max
        ));
        extra.truncate(policy.body.alias_followups_max);
    }
    if !extra.is_empty() {
        let mut more = Acc::default();
        run(&mut f, &mut more, &extra, per, true, log);
        actions.extend(more.actions);
        covered.extend(more.covered);
        chunks.extend(more.chunks);
    }
    let success = chunks.iter().all(|c| c.complete);
    log(&format!(
        "corporate actions for {d} (process {start}..{end}): {}/{} symbols covered (+{} follow-ups), {} actions, {} requests",
        covered.iter().filter(|s| requested.contains(s)).count(),
        symbols.len(),
        extra.len(),
        actions.len(),
        f.requests
    ));
    ActionRetrieval {
        schema: ACTIONS_SCHEMA,
        policy: policy.tag.clone(),
        retrieved_at: Utc::now(),
        provider: "alpaca".into(),
        endpoint: policy.body.endpoint.clone(),
        types: policy.body.types.clone(),
        data_quality: policy.body.data_quality.clone(),
        session_date: d,
        process_start: start,
        process_end: end,
        symbols_requested: symbols,
        symbols_covered: covered.into_iter().collect(),
        chunks,
        success,
        requests: f.requests,
        command: command.into(),
        actions: actions.into_values().collect(),
    }
}
