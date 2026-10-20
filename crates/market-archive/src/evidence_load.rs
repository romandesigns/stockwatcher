//! Stored float and news records (`crate::evidence`) into the context engine's evidence types
//! (`stock_context::{FloatObservation, NewsItem, NewsCoverage}`).
//!
//! The one rule everything here keeps: **`observed_at` is when StockWatcher first had the record**
//! (a float's retrieval, a news version's first sighting, a search's completion), never a
//! provider's publication or refresh time. The engine joins evidence as of each decision's cutoff,
//! so a later retrieval or a later edit can never reach an earlier decision, and today's float can
//! never be read into an older episode.
//!
//! Rules are versioned ([`FLOAT_RULES_VERSION`], [`NEWS_RULES_VERSION`]) and documented in
//! `docs/stock-context/float-news-plan.md` section 6. A changed rule is a new version string; the
//! stored records do not change, so any rule version can be re-run over the same history.

use crate::evidence::{FloatRecord, NewsCoverageRecord, NewsVersionRecord};
use std::collections::{BTreeMap, BTreeSet};
use stock_context::{
    Authenticity, Favorability, FloatObservation, InstrumentId, NewsCoverage, NewsItem,
};

pub const FLOAT_RULES_VERSION: &str = "float-rules-v1";
pub const NEWS_RULES_VERSION: &str = "news-rules-v1";

/// Flags that make a retrieval unusable as float evidence (it still counts for versioning).
/// - `no_row`: the provider returned no usable float figure.
/// - `float_equals_outstanding`: indistinguishable from the provider substituting shares
///   outstanding (FMP SGRX: 799,914 both); shares outstanding is never a float.
/// - `float_exceeds_outstanding`: internally inconsistent.
pub const FLOAT_UNUSABLE: [&str; 3] = [
    "no_row",
    "float_equals_outstanding",
    "float_exceeds_outstanding",
];

/// The quality flags `float-rules-v1` derives from a record's fields. The collector stores them
/// for reading; the loader derives them again, so a stored list never overrides the rules.
pub fn float_quality(r: &FloatRecord) -> Vec<String> {
    let mut q = Vec::new();
    match r.float_shares {
        None => q.push("no_row"),
        Some(0) => q.push("nonpositive_float"),
        Some(f) => match r.outstanding_shares {
            Some(o) if f == o => q.push("float_equals_outstanding"),
            Some(o) if f > o => q.push("float_exceeds_outstanding"),
            None => q.push("outstanding_missing"),
            _ => {}
        },
    }
    if r.float_shares.is_some() && r.free_float_pct.is_none() {
        q.push("free_float_missing");
    }
    if r.source_url.as_deref().is_none_or(str::is_empty) {
        q.push("no_source_document");
    }
    if r.effective_as_of.is_none() {
        // FMP's `date` is its refresh time: a retrieval proves what the provider said then, not
        // that the issuer's float changed (or was true) that day.
        q.push("no_effective_date");
    }
    q.into_iter().map(String::from).collect()
}

/// Every usable float retrieval as an observation, versioned per symbol in retrieval order.
/// Flags added: `unchanged_since_previous_retrieval` / `changed_since_previous_retrieval` (the
/// provider's figure, which is not evidence the issuer's float changed then).
pub fn float_observations(records: &[FloatRecord]) -> Vec<FloatObservation> {
    let mut by_symbol: BTreeMap<&str, Vec<&FloatRecord>> = BTreeMap::new();
    for r in records {
        by_symbol.entry(r.symbol.as_str()).or_default().push(r);
    }
    let mut out = Vec::new();
    for (symbol, mut rows) in by_symbol {
        rows.sort_by_key(|r| r.retrieved_at);
        let mut previous: Option<u64> = None;
        for (i, r) in rows.into_iter().enumerate() {
            let mut quality = float_quality(r);
            for q in &r.quality {
                if !quality.contains(q) {
                    quality.push(q.clone());
                }
            }
            let Some(shares) = r.float_shares else {
                continue;
            };
            if quality.iter().any(|q| FLOAT_UNUSABLE.contains(&q.as_str())) {
                continue;
            }
            if let Some(p) = previous {
                quality.push(
                    if p == shares {
                        "unchanged_since_previous_retrieval"
                    } else {
                        "changed_since_previous_retrieval"
                    }
                    .into(),
                );
            }
            previous = Some(shares);
            quality.push(FLOAT_RULES_VERSION.into());
            out.push(FloatObservation {
                id: format!("float:{}:{symbol}", r.provider),
                version: i as u32 + 1,
                instrument: InstrumentId::symbol(symbol),
                shares,
                provider: r.provider.clone(),
                effective_as_of: r.effective_as_of,
                observed_at: r.retrieved_at,
                quality,
            });
        }
    }
    out
}

/// A headline's event type under `news-rules-v1`: the first category (in this order) with a
/// phrase that appears as whole words in the lowercased headline, else `other`. Order matters:
/// "registered direct offering" is financing before "agreement" could make it commercial.
pub const EVENT_RULES: [(&str, &[&str]); 7] = [
    (
        "regulatory_decision",
        &[
            "fda",
            "approval",
            "approves",
            "approved",
            "clearance",
            "510(k)",
            "ema",
            "chmp",
            "complete response letter",
            "crl",
            "breakthrough therapy",
            "fast track",
            "orphan drug",
            "emergency use authorization",
        ],
    ),
    (
        "clinical_result",
        &[
            "phase 1",
            "phase 2",
            "phase 3",
            "phase i",
            "phase ii",
            "phase iii",
            "topline",
            "trial",
            "study results",
            "primary endpoint",
            "clinical data",
        ],
    ),
    (
        "earnings",
        &[
            "earnings",
            "quarterly results",
            "q1 results",
            "q2 results",
            "q3 results",
            "q4 results",
            "fiscal year results",
            "eps",
            "reports q1",
            "reports q2",
            "reports q3",
            "reports q4",
        ],
    ),
    (
        "guidance",
        &[
            "guidance",
            "outlook",
            "raises forecast",
            "lowers forecast",
            "reaffirms",
        ],
    ),
    (
        "financing",
        &[
            "offering",
            "private placement",
            "registered direct",
            "pricing of",
            "warrants",
            "convertible",
            "at-the-market",
            "financing",
            "credit facility",
        ],
    ),
    (
        "corporate_action",
        &[
            "reverse split",
            "reverse stock split",
            "stock split",
            "merger",
            "acquisition",
            "to acquire",
            "acquires",
            "to be acquired",
            "delisting",
            "nasdaq notice",
            "regains compliance",
            "name change",
            "ticker change",
            "dividend",
            "buyback",
            "repurchase",
            "spin-off",
            "spinoff",
        ],
    ),
    (
        "commercial_agreement",
        &[
            "agreement",
            "partnership",
            "partners with",
            "collaboration",
            "contract",
            "awarded",
            "purchase order",
            "license",
            "licensing",
            "distribution deal",
            "supply deal",
        ],
    ),
];

/// Headlines that are market commentary or screens, not news about the company itself: never
/// substantive whatever keyword they contain ("10 stocks with whale alerts", movers lists).
pub const NOISE_RULES: [&str; 14] = [
    "whale alerts",
    "unusual options",
    "options activity",
    "stocks moving",
    "movers",
    "what's moving",
    "mid-day",
    "midday",
    "52-week",
    "shares are trading",
    "trading higher",
    "trading lower",
    "price target",
    "analyst ratings",
];

/// More symbols than this on one story and it is a roundup: relevant to a symbol only when its
/// headline names that ticker.
pub const MAX_SYMBOLS_FOR_RELEVANCE: usize = 3;

/// Sources a news-provider record is credible for reporting (that a story was published, not that
/// its claim is true): Benzinga is the source Alpaca's news API serves.
pub const CREDIBLE_SOURCES: [&str; 1] = ["benzinga"];

fn words(text: &str) -> String {
    let mut s = String::with_capacity(text.len() + 2);
    s.push(' ');
    for c in text.chars().flat_map(char::to_lowercase) {
        // Keep the characters our phrases contain; everything else separates words.
        if c.is_alphanumeric() || matches!(c, '(' | ')' | '-' | '\'') {
            s.push(c);
        } else {
            s.push(' ');
        }
    }
    s.push(' ');
    s
}

fn has_phrase(w: &str, phrase: &str) -> bool {
    w.contains(&format!(" {phrase} "))
}

/// `(event_type, noise)` for a headline.
pub fn classify_headline(headline: &str) -> (&'static str, bool) {
    let w = words(headline);
    let noise = NOISE_RULES.iter().any(|p| has_phrase(&w, p));
    let event = EVENT_RULES
        .iter()
        .find(|(_, phrases)| phrases.iter().any(|p| has_phrase(&w, p)))
        .map_or("other", |(e, _)| e);
    (event, noise)
}

fn names_ticker(headline: &str, symbol: &str) -> bool {
    headline
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '.'))
        .any(|t| t == symbol)
}

/// Versions per story: records sorted by first sighting; consecutive identical `version_key`s
/// collapse into the first (a re-fetch is not an edit), while a story that changes back to an
/// earlier text is a new version (history is kept, not deduplicated).
fn versions(records: &[NewsVersionRecord]) -> Vec<(u32, &NewsVersionRecord)> {
    let mut by_story: BTreeMap<(&str, &str), Vec<&NewsVersionRecord>> = BTreeMap::new();
    for r in records {
        by_story
            .entry((r.provider.as_str(), r.article_id.as_str()))
            .or_default()
            .push(r);
    }
    let mut out = Vec::new();
    for (_, mut rows) in by_story {
        rows.sort_by_key(|r| r.first_observed_at);
        let mut last: Option<&str> = None;
        let mut n = 0;
        for r in rows {
            if last == Some(r.version_key.as_str()) {
                continue;
            }
            last = Some(r.version_key.as_str());
            n += 1;
            out.push((n, r));
        }
    }
    out
}

/// Every news version as one `NewsItem` per tagged symbol (only `symbols` when given).
pub fn news_items(
    records: &[NewsVersionRecord],
    symbols: Option<&BTreeSet<String>>,
) -> Vec<NewsItem> {
    let mut out = Vec::new();
    for (version, r) in versions(records) {
        let (event_type, noise) = classify_headline(&r.headline);
        let credible = r.provider == "alpaca"
            && CREDIBLE_SOURCES.contains(&r.source.to_ascii_lowercase().as_str());
        for sym in &r.symbols {
            if symbols.is_some_and(|s| !s.contains(sym)) {
                continue;
            }
            let relevant =
                r.symbols.len() <= MAX_SYMBOLS_FOR_RELEVANCE || names_ticker(&r.headline, sym);
            out.push(NewsItem {
                id: format!("news:{}:{}", r.provider, r.article_id),
                version,
                instrument: InstrumentId::symbol(sym),
                event_type: event_type.into(),
                source: format!("{}/{}", r.provider, r.source),
                credible_source: credible,
                relevant,
                substantive: event_type != "other" && !noise,
                // A news provider reporting a story: issuer or regulator confirmation is a
                // separate, later enrichment.
                authenticity: Authenticity::ReportedUnverified,
                favorability: Favorability::Undetermined,
                published_at: r.created_at_ns,
                observed_at: r.first_observed_at,
            });
        }
    }
    out
}

/// Coverage per symbol. Records are taken in completion order; a search whose window starts at or
/// before the end of the symbol's current run extends it (polls overlap), anything else starts a
/// new run. Each record becomes one `NewsCoverage` from its run's start through the furthest end
/// seen so far, observed when that search completed.
///
/// `min_interval_secs` thins the output (0 keeps every record): a record is dropped when the
/// previous kept one in the same run completed less than this long before it. Dropping only ever
/// makes a cutoff see an older, shorter run (UNKNOWN instead of FAIL), never more coverage.
pub fn news_coverage(
    records: &[NewsCoverageRecord],
    symbols: &[String],
    min_interval_secs: i64,
) -> Vec<NewsCoverage> {
    let mut rows: Vec<&NewsCoverageRecord> = records.iter().collect();
    rows.sort_by_key(|r| r.observed_at);
    let mut out = Vec::new();
    for sym in symbols {
        let mine: Vec<&NewsCoverageRecord> = rows
            .iter()
            .copied()
            .filter(|r| r.scope.covers(sym))
            .collect();
        runs(
            &mine,
            InstrumentId::symbol(sym),
            min_interval_secs,
            &mut out,
        );
    }
    out
}

/// Market-wide coverage ([`InstrumentId::market`]): the runs of the searches made without a
/// symbol filter (`CoverageScope::All`, which is every search the collector makes), by the rules
/// of [`news_coverage`]. One record per kept search instead of one per search and symbol: the
/// engine serves a market record to every instrument (`EvidenceStore::coverage`). A search for
/// named symbols is not part of these runs; give those symbols [`news_coverage`] too.
pub fn market_news_coverage(
    records: &[NewsCoverageRecord],
    min_interval_secs: i64,
) -> Vec<NewsCoverage> {
    let mut rows: Vec<&NewsCoverageRecord> = records
        .iter()
        .filter(|r| r.scope == crate::CoverageScope::All)
        .collect();
    rows.sort_by_key(|r| r.observed_at);
    let mut out = Vec::new();
    runs(&rows, InstrumentId::market(), min_interval_secs, &mut out);
    out
}

/// The symbols a search for named symbols covered (`CoverageScope::Symbols`), across `records`.
pub fn scoped_symbols(records: &[NewsCoverageRecord]) -> BTreeSet<String> {
    records
        .iter()
        .filter_map(|r| match &r.scope {
            crate::CoverageScope::Symbols(s) => Some(s.iter().cloned()),
            crate::CoverageScope::All => None,
        })
        .flatten()
        .collect()
}

/// [`news_coverage`]'s run rule over `rows` (already in completion order) for one instrument.
fn runs(
    rows: &[&NewsCoverageRecord],
    instrument: InstrumentId,
    min_interval_secs: i64,
    out: &mut Vec<NewsCoverage>,
) {
    let min_gap = min_interval_secs.max(0) * 1_000_000_000;
    let mut run: Option<(i64, i64)> = None;
    let mut last_kept: Option<i64> = None;
    for r in rows {
        let (start, through, fresh_run) = match run {
            Some((s, t)) if r.covered_from <= t => (s, t.max(r.covered_through), false),
            _ => (r.covered_from, r.covered_through, true),
        };
        run = Some((start, through));
        if !fresh_run && last_kept.is_some_and(|k| r.observed_at - k < min_gap) {
            continue;
        }
        last_kept = Some(r.observed_at);
        out.push(NewsCoverage {
            id: r.id.clone(),
            instrument: instrument.clone(),
            sources: r
                .sources
                .iter()
                .map(|s| format!("{}/{s}", r.provider))
                .collect(),
            covered_from: start,
            covered_through: through,
            observed_at: r.observed_at,
        });
    }
}

/// Everything one run needs, ready for `Engine::add_*`.
#[derive(Clone, Debug, Default)]
pub struct LoadedEvidence {
    pub floats: Vec<FloatObservation>,
    pub news: Vec<NewsItem>,
    pub coverage: Vec<NewsCoverage>,
}

impl LoadedEvidence {
    /// Records for `symbols` from the archive. `since` (UTC ns) skips older monthly news files.
    pub fn load(
        archive: &crate::Archive,
        symbols: &BTreeSet<String>,
        since: Option<i64>,
        coverage_min_interval_secs: i64,
    ) -> Result<Self, crate::ArchiveError> {
        let floats = archive
            .read_float()?
            .into_iter()
            .filter(|r| symbols.contains(&r.symbol))
            .collect::<Vec<_>>();
        let news = archive.read_news(since)?;
        let cov = archive.read_news_coverage(since)?;
        let list: Vec<String> = symbols.iter().cloned().collect();
        Ok(LoadedEvidence {
            floats: float_observations(&floats),
            news: news_items(&news, Some(symbols)),
            coverage: news_coverage(&cov, &list, coverage_min_interval_secs),
        })
    }

    pub fn apply(self, engine: &mut stock_context::Engine) {
        for f in self.floats {
            engine.add_float(f);
        }
        for n in self.news {
            engine.add_news(n);
        }
        for c in self.coverage {
            engine.add_coverage(c);
        }
    }
}
