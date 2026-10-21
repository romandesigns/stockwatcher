//! Alpaca news (`GET /v1beta1/news`, Benzinga stories) into append-only [`NewsVersionRecord`]s and
//! [`NewsCoverageRecord`]s.
//!
//! Evidence (research report 02, rows 29–30): the endpoint answers on this account with stories
//! 3+ years back; `created_at` and `updated_at` are RFC 3339 UTC; a story can be edited after
//! publication (`updated_at` later than `created_at`) and the response then carries only its
//! final state. So:
//!
//! - **A version is what we saw, when we saw it.** Each distinct (`updated_at`, headline,
//!   summary, symbols) of a story becomes a record with `first_observed_at` = our clock when the
//!   page arrived. A later edit is a later record; the earlier one is never changed, so an earlier
//!   cutoff keeps seeing what was known then. A re-fetch of the same version adds nothing.
//! - **Publication time is the provider's `created_at`, kept as delivered.** It is never used as
//!   an observation time: a story first seen at 11:00 that says it was created at 08:59 is
//!   evidence from 11:00 on.
//! - **Coverage only for a complete search.** A window becomes a coverage record only when every
//!   page was fetched (`next_page_token` null). A failure part-way still stores the versions it
//!   saw (they were observed) but claims no coverage, so the news pillar stays UNKNOWN there.
//! - **No bodies.** `include_content=false`; the summary is stored only as a hash.
//!
//! The query has no symbol filter: one request stream covers every symbol Benzinga tags, so the
//! cost does not grow with the watch list (a per-symbol query would be thousands of requests).
//! Whether `start`/`end` filter on `created_at` or `updated_at` is not documented clearly enough
//! to rely on; the collector re-polls a trailing window to catch edits either way.
//!
//! Streaming: Alpaca also has a news WebSocket. No probe has opened it on this account, so it is
//! not used until an entitlement check exists (plan, section 3).

use super::alpaca::{AlpacaCredentials, rfc3339};
use super::http::{FetchError, Http, RetryPolicy, get_with_retry};
use chrono::{DateTime, Utc};
use market_archive::evidence::EVIDENCE_SCHEMA;
use market_archive::{CoverageScope, NewsCoverageRecord, NewsVersionRecord, sha256_hex};
use serde::Deserialize;
use std::collections::{HashMap, VecDeque};
use std::time::Duration;

pub const PROVIDER: &str = "alpaca";
pub const ENDPOINT: &str = "/v1beta1/news";
/// Alpaca's page size ceiling for news.
pub const MAX_LIMIT: u32 = 50;
/// Guard against a provider that never stops paginating.
const MAX_PAGES: u32 = 2_000;

#[derive(PartialEq, Clone, Debug, Deserialize)]
pub struct RawStory {
    pub id: u64,
    #[serde(default)]
    pub headline: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub symbols: Vec<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Deserialize)]
struct Page {
    #[serde(default)]
    news: Vec<RawStory>,
    next_page_token: Option<String>,
}

/// A search window: stories whose time falls in `[start, end]`, all symbols or a list.
#[derive(Clone, Debug)]
pub struct NewsQuery {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub symbols: Option<Vec<String>>,
}

impl NewsQuery {
    fn url(&self, base: &str, page_token: Option<&str>) -> String {
        let mut u = format!(
            "{}{ENDPOINT}?start={}&end={}&limit={MAX_LIMIT}&sort=asc&include_content=false",
            base.trim_end_matches('/'),
            rfc3339(self.start),
            rfc3339(self.end),
        );
        if let Some(s) = &self.symbols {
            u.push_str("&symbols=");
            u.push_str(&s.join(","));
        }
        if let Some(t) = page_token {
            u.push_str("&page_token=");
            u.push_str(&encode(t));
        }
        u
    }

    fn scope(&self) -> CoverageScope {
        match &self.symbols {
            None => CoverageScope::All,
            Some(s) => CoverageScope::Symbols(s.clone()),
        }
    }
}

fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Every story of one search, each with the time its page arrived.
#[derive(Debug, Default)]
pub struct NewsFetch {
    pub stories: Vec<(RawStory, i64)>,
    pub requests: u32,
    /// Every page fetched: the window was searched completely.
    pub complete: bool,
    /// Why it stopped early (`None` when complete).
    pub error: Option<String>,
    /// Arrival of the last page (UTC ns).
    pub finished_at: i64,
}

/// Fetches every page of `q`. Never returns `Err`: what arrived before a failure is kept in
/// [`NewsFetch::stories`] with `complete = false`.
#[allow(clippy::too_many_arguments)]
pub fn fetch_news(
    http: &mut dyn Http,
    data_base: &str,
    creds: &AlpacaCredentials,
    q: &NewsQuery,
    retry: RetryPolicy,
    now: &mut dyn FnMut() -> i64,
    sleep: &mut dyn FnMut(Duration),
    log: &mut dyn FnMut(&str),
) -> NewsFetch {
    let headers = [
        ("APCA-API-KEY-ID", creds.key_id.as_str()),
        ("APCA-API-SECRET-KEY", creds.secret.as_str()),
        ("Accept", "application/json"),
    ];
    let secrets = [creds.key_id.as_str(), creds.secret.as_str()];
    let mut out = NewsFetch::default();
    let mut token: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let url = q.url(data_base, token.as_deref());
        out.requests += 1;
        let resp = match get_with_retry(http, &url, &headers, retry, &secrets, sleep, log) {
            Ok(r) => r,
            Err(e) => {
                out.error = Some(match e {
                    FetchError::Permanent { status, body } if matches!(status, 401..=403) => {
                        format!("not entitled (HTTP {status}): {body}")
                    }
                    e => e.to_string(),
                });
                out.finished_at = now();
                return out;
            }
        };
        let at = now();
        out.finished_at = at;
        let page: Page = match serde_json::from_slice(&resp.body) {
            Ok(p) => p,
            Err(e) => {
                out.error = Some(format!("malformed news page: {e}"));
                return out;
            }
        };
        out.stories.extend(page.news.into_iter().map(|s| (s, at)));
        match page.next_page_token.filter(|t| !t.is_empty()) {
            Some(t) => token = Some(t),
            None => {
                out.complete = true;
                return out;
            }
        }
    }
    out.error = Some(format!("news did not end within {MAX_PAGES} pages"));
    out
}

fn parse_ns(s: Option<&str>) -> Option<i64> {
    DateTime::parse_from_rfc3339(s?)
        .ok()
        .and_then(|d| d.with_timezone(&Utc).timestamp_nanos_opt())
}

/// The record for one story as seen at `observed_at`.
pub fn version_record(s: &RawStory, observed_at: i64) -> NewsVersionRecord {
    let headline = s.headline.trim().to_string();
    let headline_sha256 = sha256_hex(headline.as_bytes());
    let summary_sha256 = sha256_hex(s.summary.trim().as_bytes());
    let mut symbols = s.symbols.clone();
    symbols.sort();
    symbols.dedup();
    let version_key = sha256_hex(
        format!(
            "{}\n{headline_sha256}\n{summary_sha256}\n{}",
            s.updated_at.as_deref().unwrap_or(""),
            symbols.join(",")
        )
        .as_bytes(),
    );
    NewsVersionRecord {
        v: EVIDENCE_SCHEMA,
        provider: PROVIDER.into(),
        source: s.source.clone().unwrap_or_else(|| "unknown".into()),
        article_id: s.id.to_string(),
        created_at: s.created_at.clone(),
        updated_at: s.updated_at.clone(),
        created_at_ns: parse_ns(s.created_at.as_deref()),
        updated_at_ns: parse_ns(s.updated_at.as_deref()),
        first_observed_at: observed_at,
        headline,
        headline_sha256,
        summary_sha256,
        author: s.author.clone().filter(|a| !a.trim().is_empty()),
        symbols,
        url: s.url.clone(),
        version_key,
    }
}

/// Version keys kept per story before the oldest is forgotten.
///
/// An article gets a handful of edits; this is generous for that and bounds a provider that
/// changes one story endlessly. Past it the oldest key is dropped and counted, after which a
/// redelivery of that exact version can no longer be recognised — so the overflow is reported
/// rather than silently changing what the collector believes.
pub const VERSIONS_PER_STORY: usize = 32;

/// Stories the version index keeps before the least recently confirmed is forgotten.
///
/// [`VERSIONS_PER_STORY`] bounds each story's list; without this the number of *stories* grows
/// for the life of the process — a few thousand articles a day, every day a long-running
/// collector stays up. A story that has not been seen for that many newer stories is past every
/// search window the collector asks about, so forgetting it costs at most one re-append if it
/// ever comes back, and that is counted.
pub const STORIES_TRACKED: usize = 20_000;

/// Which versions of each story are **durably stored**, so a re-fetch adds nothing, an edit adds
/// one, and a redelivery of an older version through another transport adds nothing either.
///
/// Built from the stored records at start-up (the loader applies the same rule, so a story whose
/// earlier versions fell outside what was loaded is still collapsed correctly there).
///
/// # Durable, not merely seen
///
/// The two are different and conflating them lost evidence. This used to mark a version seen as
/// it handed the record to its caller, before the caller had appended it. An append that failed
/// left the version marked in memory and absent from disk: the next search suppressed it as
/// already known, and then wrote a **coverage** record saying the window had been searched. So
/// the story was permanently missing while coverage claimed it had been looked for — which is a
/// false FAIL for the news pillar, the one direction the lower-bound argument cannot absorb.
///
/// [`NewsVersions::propose`] therefore changes nothing, and [`NewsVersions::commit`] is called
/// only after the records are on disk.
///
/// # Every known version, not just the latest
///
/// Keeping only the latest key meant a story that went v1 → v2 and was then redelivered as v1
/// (a REST page behind a stream, or the reverse) did not match, and a *new* record was written
/// for v1 with a new `first_observed_at`. That fabricates a revision out of a transport toggle
/// and backdates nothing: the story reads as having changed when it did not.
#[derive(Debug, Default)]
pub struct NewsVersions {
    /// Every durable version key per story, oldest first.
    known: HashMap<String, VecDeque<String>>,
    /// Story ids in the order they were last confirmed, least recent first: what to forget when
    /// [`STORIES_TRACKED`] is reached.
    order: VecDeque<String>,
    /// Keys dropped at [`VERSIONS_PER_STORY`]. A redelivery of one of those can no longer be
    /// recognised, so it is a number a caller can report rather than a silent change of rule.
    forgotten: u64,
    /// Whole stories dropped at [`STORIES_TRACKED`], for the same reason.
    stories_forgotten: u64,
}

impl NewsVersions {
    pub fn from_records(records: &[NewsVersionRecord]) -> Self {
        let mut v = NewsVersions::default();
        v.commit(records);
        v
    }

    /// Records whose versions are now on disk. Called **after** a successful append.
    ///
    /// Keyed on canonical content ([`NewsVersionRecord::content_key`]) rather than the raw
    /// `version_key`, so the same revision of the same text delivered by two transports with
    /// differently formatted timestamps is recognised as one version instead of being appended
    /// twice.
    pub fn commit(&mut self, records: &[NewsVersionRecord]) {
        for r in records {
            let key = r.content_key();
            let fresh = !self.known.contains_key(&r.article_id);
            let keys = self.known.entry(r.article_id.clone()).or_default();
            if keys.iter().any(|k| *k == key) {
                continue;
            }
            if keys.len() == VERSIONS_PER_STORY {
                keys.pop_front();
                self.forgotten += 1;
            }
            keys.push_back(key);
            // Most recently confirmed goes to the back; a story confirmed again moves there.
            if !fresh && let Some(i) = self.order.iter().position(|id| *id == r.article_id) {
                self.order.remove(i);
            }
            self.order.push_back(r.article_id.clone());
            while self.order.len() > STORIES_TRACKED {
                if let Some(oldest) = self.order.pop_front()
                    && let Some(dropped) = self.known.remove(&oldest)
                {
                    self.forgotten += dropped.len() as u64;
                    self.stories_forgotten += 1;
                }
            }
        }
    }

    /// The versions in `stories` that are not durable yet, as records to append.
    ///
    /// Changes nothing: until the caller has stored these and called [`NewsVersions::commit`],
    /// nothing here believes it has seen them. Duplicates **within** the batch collapse too, so
    /// one search returning a story twice proposes it once.
    pub fn propose(&self, stories: &[(RawStory, i64)]) -> Vec<NewsVersionRecord> {
        let mut out: Vec<NewsVersionRecord> = Vec::new();
        for (s, at) in stories {
            let r = version_record(s, *at);
            let key = r.content_key();
            let durable = self
                .known
                .get(&r.article_id)
                .is_some_and(|keys| keys.iter().any(|k| *k == key));
            let proposed = out
                .iter()
                .any(|p| p.article_id == r.article_id && p.content_key() == key);
            if durable || proposed {
                continue;
            }
            out.push(r);
        }
        out
    }

    /// Version keys dropped at either bound.
    pub fn forgotten(&self) -> u64 {
        self.forgotten
    }

    /// Whole stories dropped at [`STORIES_TRACKED`].
    pub fn stories_forgotten(&self) -> u64 {
        self.stories_forgotten
    }

    /// Stories with at least one durable version.
    pub fn len(&self) -> usize {
        self.known.len()
    }

    pub fn is_empty(&self) -> bool {
        self.known.is_empty()
    }
}

/// The coverage record of a complete search (`None` when it was not complete).
pub fn coverage_record(q: &NewsQuery, f: &NewsFetch) -> Option<NewsCoverageRecord> {
    if !f.complete {
        return None;
    }
    let from = q.start.timestamp_nanos_opt()?;
    let through = q.end.timestamp_nanos_opt()?;
    Some(NewsCoverageRecord {
        v: EVIDENCE_SCHEMA,
        id: format!("newscov:{PROVIDER}:{from}-{through}@{}", f.finished_at),
        provider: PROVIDER.into(),
        // What Alpaca's news API serves; a story with another `source` would still be stored
        // under its own source, and the loader's credibility rule decides per story.
        sources: vec!["benzinga".into()],
        scope: q.scope(),
        covered_from: from,
        covered_through: through,
        observed_at: f.finished_at,
        requests: f.requests,
        stories: f.stories.len() as u32,
    })
}
