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

use crate::evidence::{
    FilingRecord, FloatRecord, NewsCoverageRecord, NewsVersionRecord, SourceNewsRecord,
};
use std::collections::{BTreeMap, BTreeSet};
use stock_context::{
    Authenticity, Favorability, FloatObservation, InstrumentId, NewsCoverage, NewsItem,
    OutstandingObservation, SourceKind, SourceNews, Tag,
};

pub const FLOAT_RULES_VERSION: &str = "float-rules-v1";

pub const NEWS_RULES_V1: &str = "news-rules-v1";
pub const NEWS_RULES_V2: &str = "news-rules-v2";
/// The rules a new run normalizes stories under. Older identities stay callable
/// ([`news_items_with`]) so the same stored history can be re-read exactly as a past decision
/// read it.
pub const NEWS_RULES_VERSION: &str = NEWS_RULES_V2;

/// Which headline rules to classify under.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NewsRules {
    /// The first rules: any phrase in [`NOISE_RULES`] anywhere in the headline makes the whole
    /// headline commentary. Kept so stored stories re-normalize exactly.
    V1,
    /// [`SCREEN_RULES`] still disqualify the headline wherever they appear, but a
    /// [`PRICE_COMMENTARY_RULES`] remark appended to a clause that states a company event no
    /// longer cancels the event.
    #[default]
    V2,
}

impl NewsRules {
    pub fn tag(self) -> &'static str {
        match self {
            NewsRules::V1 => NEWS_RULES_V1,
            NewsRules::V2 => NEWS_RULES_V2,
        }
    }
}

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

pub const OUTSTANDING_RULES_VERSION: &str = "outstanding-rules-v1";

/// Every usable shares-outstanding figure as an observation (`outstanding-rules-v1`), versioned
/// per symbol in retrieval order, for the float pillar's pillars-v5 fallback. Never a float.
///
/// Usable: a positive figure on a record whose float does not exceed it. A record with
/// `float_exceeds_outstanding` is internally inconsistent, so neither number bounds the other and
/// the outstanding figure is dropped with the float. A `no_row` record (null float) keeps its
/// outstanding figure: that is exactly the case the fallback is for (2026-11-20: 1,026 of 3,409
/// FMP rows had a null float, many with an outstanding figure).
pub fn outstanding_observations(records: &[FloatRecord]) -> Vec<OutstandingObservation> {
    let mut by_symbol: BTreeMap<&str, Vec<&FloatRecord>> = BTreeMap::new();
    for r in records {
        by_symbol.entry(r.symbol.as_str()).or_default().push(r);
    }
    let mut out = Vec::new();
    for (symbol, mut rows) in by_symbol {
        rows.sort_by_key(|r| r.retrieved_at);
        for (i, r) in rows.into_iter().enumerate() {
            let Some(shares) = r.outstanding_shares.filter(|&o| o > 0) else {
                continue;
            };
            if r.float_shares.is_some_and(|f| f > shares) {
                continue;
            }
            let mut quality = vec![OUTSTANDING_RULES_VERSION.to_string()];
            if r.float_shares.is_none() {
                quality.push("float_missing".into());
            }
            out.push(OutstandingObservation {
                id: format!("outstanding:{}:{symbol}", r.provider),
                version: i as u32 + 1,
                instrument: InstrumentId::symbol(symbol),
                shares,
                provider: r.provider.clone(),
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
///
/// `news-rules-v1` applies all of these to the whole headline. `news-rules-v2` partitions exactly
/// the same phrases into [`SCREEN_RULES`] and [`PRICE_COMMENTARY_RULES`] (nothing is dropped;
/// `v2_drops_no_phrase_v1_used` asserts it) and treats the two differently.
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

/// `news-rules-v2`: the headline's own subject is a screen, a roundup or an analyst note. These
/// disqualify wherever they appear — "12 Stocks Moving In Monday's Mid-Day Session: TEST Receives
/// FDA Approval" is a list, not TEST's announcement.
pub const SCREEN_RULES: [&str; 10] = [
    "whale alerts",
    "unusual options",
    "options activity",
    "stocks moving",
    "movers",
    "what's moving",
    "mid-day",
    "midday",
    "price target",
    "analyst ratings",
];

/// `news-rules-v2`: a remark about the tape. On its own it is commentary, but publishers append
/// it to real announcements ("TEST Receives FDA Approval, Shares Trading Higher"), so it only
/// disqualifies a headline in which no clause states a company event by itself.
pub const PRICE_COMMENTARY_RULES: [&str; 4] = [
    "52-week",
    "shares are trading",
    "trading higher",
    "trading lower",
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

/// Clauses of a headline: publishers append a remark after a comma, a semicolon, a colon or a
/// spaced dash. A bare `-` is not a boundary (it is inside "mid-day", "at-the-market", "52-week").
fn clauses(headline: &str) -> Vec<String> {
    headline
        .replace(" - ", " , ")
        .split([',', ';', ':', '\u{2013}', '\u{2014}', '|'])
        .map(str::to_string)
        .collect()
}

fn event_of(w: &str) -> Option<&'static str> {
    EVENT_RULES
        .iter()
        .find(|(_, phrases)| phrases.iter().any(|p| has_phrase(w, p)))
        .map(|(e, _)| *e)
}

/// `(event_type, noise)` for a headline under the current rules ([`NEWS_RULES_VERSION`]).
pub fn classify_headline(headline: &str) -> (&'static str, bool) {
    classify_headline_with(NewsRules::default(), headline)
}

/// `(event_type, noise)` for a headline under `rules`.
///
/// The event type is the first [`EVENT_RULES`] category with a phrase in the headline, the same
/// under every version. Only what counts as commentary differs: v1 disqualifies on any
/// [`NOISE_RULES`] phrase, v2 on a [`SCREEN_RULES`] phrase, or on a [`PRICE_COMMENTARY_RULES`]
/// phrase when **no** clause states a company event without one of its own.
pub fn classify_headline_with(rules: NewsRules, headline: &str) -> (&'static str, bool) {
    let w = words(headline);
    let event = event_of(&w).unwrap_or("other");
    let commentary = |text: &str| PRICE_COMMENTARY_RULES.iter().any(|p| has_phrase(text, p));
    let noise = match rules {
        NewsRules::V1 => NOISE_RULES.iter().any(|p| has_phrase(&w, p)),
        NewsRules::V2 => {
            if SCREEN_RULES.iter().any(|p| has_phrase(&w, p)) {
                true
            } else if !commentary(&w) {
                false
            } else {
                !clauses(headline).iter().any(|c| {
                    let cw = words(c);
                    event_of(&cw).is_some() && !commentary(&cw)
                })
            }
        }
    };
    (event, noise)
}

fn names_ticker(headline: &str, symbol: &str) -> bool {
    headline
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '.'))
        .any(|t| t == symbol)
}

/// The provider's own revision time for a record: when it says the article was last changed.
///
/// `updated_at` is the article's revision; `created_at` is a fallback for a provider that sends no
/// update time on a first publication. `None` when neither parses, which cannot be ordered
/// against records that do.
fn revision_at(r: &NewsVersionRecord) -> Option<i64> {
    r.updated_at_ns.or(r.created_at_ns)
}

/// Versions per story, numbered by **first sighting**, with the provider's revision time carried
/// alongside.
///
/// The numbering is observation order deliberately, and it is an identity rather than a ranking.
/// Once a version has been observed its ordinal is fixed: nothing a later refresh discovers can
/// change it, so `(id, version)` is stable and a decision that cited it still means the same
/// thing afterwards. Numbering by the provider's revision time instead would renumber a story's
/// earlier versions the moment an older revision was discovered late — rewriting the identity of
/// evidence that had already been published.
///
/// Which version is **current** is a different question, and not one this function answers. It
/// depends on the cutoff, because it must be settled over the versions visible then and no
/// others (`stock_context::EvidenceStore::news_stories`).
///
/// Each canonical revision appears **once** per story, at its earliest observation (a re-fetch is
/// not an edit). Collapsing only *adjacent* duplicates was not enough: a redelivery separated from
/// its original by a late older revision — rev11 seen at 09:00, rev08 at 11:00, rev11 again at
/// 12:00, which is what a forgotten cache entry produces — left two copies of rev11, and the later
/// one won the revision-order tiebreak. The story's current version then carried the 12:00
/// sighting instead of the 09:00 one, moving the observation time and the evidence identity of
/// something already published.
///
/// A genuine edit back to an earlier text is not affected: the provider stamps it with a new
/// revision time, which is part of the canonical key, so it is a distinct version and history
/// keeps both.
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
        // Canonical content, not the raw key: the same revision of the same text written with a
        // different timestamp format is one version, not two.
        let key = |r: &NewsVersionRecord| r.content_key();
        rows.sort_by_key(|r| (r.first_observed_at, key(r)));
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut n = 0;
        for r in rows {
            if !seen.insert(key(r)) {
                continue;
            }
            n += 1;
            out.push((n, r));
        }
    }
    out
}

/// Every news version as one `NewsItem` per tagged symbol (only `symbols` when given), under the
/// current rules ([`NEWS_RULES_VERSION`]).
pub fn news_items(
    records: &[NewsVersionRecord],
    symbols: Option<&BTreeSet<String>>,
) -> Vec<NewsItem> {
    news_items_with(NewsRules::default(), records, symbols)
}

/// [`news_items`] under an explicit rules version, so a past decision's classification can be
/// reproduced from the same stored stories.
///
/// A version that no longer tags a symbol an earlier version tagged also emits a **tombstone**
/// for that symbol ([`Tag::Removed`]), observed when the edit was first seen. Without it the
/// engine keeps reading the last version that named the symbol, so a story retagged away from it
/// would qualify for ever. The tombstone is also marked neither relevant nor substantive, so a
/// reader that ignores `tag` cannot qualify on it either.
pub fn news_items_with(
    rules: NewsRules,
    records: &[NewsVersionRecord],
    symbols: Option<&BTreeSet<String>>,
) -> Vec<NewsItem> {
    // Every symbol any version of a story mentions, and when we first learned of that mention.
    //
    // A version has to produce a statement about *all* of them, not only about the symbols an
    // earlier version happened to tag. Otherwise a symbol first revealed by a late-arriving older
    // revision never learns that a newer revision dropped it: suppose revision 10 is seen first
    // and tags only WXYZ, and revision 08 arrives two hours later tagging ABCD. ABCD's only
    // visible version is then the older one that tags it, so ABCD qualifies on a story whose
    // current content does not mention it at all.
    //
    // The tombstone for such a symbol is observed when the symbol's relation to the story became
    // known, never when the version itself was seen — stamping it earlier would insert a record
    // at a time nothing knew it existed, and a replay of that time would not be reproducible.
    let mut revealed: BTreeMap<(&str, &str), BTreeMap<&str, i64>> = BTreeMap::new();
    for (_, r) in versions(records) {
        let key = (r.provider.as_str(), r.article_id.as_str());
        let per_story = revealed.entry(key).or_default();
        for sym in &r.symbols {
            let e = per_story.entry(sym.as_str()).or_insert(r.first_observed_at);
            *e = (*e).min(r.first_observed_at);
        }
    }

    let mut out = Vec::new();
    for (version, r) in versions(records) {
        let this = (r.provider.as_str(), r.article_id.as_str());
        let all = revealed.get(&this).cloned().unwrap_or_default();
        let (event_type, noise) = classify_headline_with(rules, &r.headline);
        let credible = r.provider == "alpaca"
            && CREDIBLE_SOURCES.contains(&r.source.to_ascii_lowercase().as_str());
        let tagged_now: BTreeSet<&str> = r.symbols.iter().map(String::as_str).collect();
        let item = |sym: &str, tag: Tag, relevant: bool, substantive: bool, observed_at: i64| {
            NewsItem {
                id: format!("news:{}:{}", r.provider, r.article_id),
                version,
                content_key: r.content_key(),
                revision_at: revision_at(r),
                instrument: InstrumentId::symbol(sym),
                event_type: event_type.into(),
                source: format!("{}/{}", r.provider, r.source),
                credible_source: credible,
                relevant,
                substantive,
                // A news provider reporting a story: issuer or regulator confirmation is a
                // separate, later enrichment.
                authenticity: Authenticity::ReportedUnverified,
                favorability: Favorability::Undetermined,
                published_at: r.created_at_ns,
                observed_at,
                tag,
                rules: rules.tag().into(),
                url: r.url.clone(),
                headline: Some(r.headline.clone()),
                halt: None,
            }
        };
        for (sym, revealed_at) in &all {
            if symbols.is_some_and(|s| !s.contains(*sym)) {
                continue;
            }
            if tagged_now.contains(sym) {
                let relevant =
                    r.symbols.len() <= MAX_SYMBOLS_FOR_RELEVANCE || names_ticker(&r.headline, sym);
                out.push(item(
                    sym,
                    Tag::Present,
                    relevant,
                    event_type != "other" && !noise,
                    r.first_observed_at,
                ));
            } else {
                out.push(item(
                    sym,
                    Tag::Removed,
                    false,
                    false,
                    r.first_observed_at.max(*revealed_at),
                ));
            }
        }
    }
    out
}

/// The rules SEC filings are turned into news items under ([`filing_items`]).
pub const FILING_RULES_VERSION: &str = "filing-rules-v1";

/// `NewsItem::event_type` of an 8-K or 6-K.
pub const CURRENT_REPORT_EVENT: &str = "current_report";
/// `NewsItem::event_type` of an 8-K/A or 6-K/A.
pub const AMENDMENT_EVENT: &str = "current_report_amendment";

/// What an SEC form is to the Ross Panel (`filing-rules-v1`, owner brief 2026-11-22).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilingClass {
    /// 8-K, 6-K: the issuer's own current report of a material event (a contract, results, a
    /// trial readout, an offering's pricing, a delisting notice...). Passes the news pillar under
    /// pillars-v6; favorability is not judged, as for a Benzinga story (adverse news is news).
    CurrentReport,
    /// 8-K/A, 6-K/A: recorded, never a pass. An amendment restates or completes an earlier
    /// report (most often the financial statements an acquisition 8-K promised); whatever news
    /// it carried was the original's, which passed on its own date.
    Amendment,
    /// Registration statements and prospectuses (S-1, S-3, F-1, F-3 with their /A, ASR and MEF
    /// variants; 424B*): dilution, not a bullish catalyst. Recorded and flagged on the row
    /// ("Offering filed"), never a pass.
    Offering,
}

/// The class of an EDGAR form type, `None` for every form the panel ignores (Form 4, 13D/G,
/// 10-Q, 8-K12B and the rest are not collected at all).
pub fn filing_class(form: &str) -> Option<FilingClass> {
    match form {
        "8-K" | "6-K" => Some(FilingClass::CurrentReport),
        "8-K/A" | "6-K/A" => Some(FilingClass::Amendment),
        "S-1" | "S-1/A" | "S-1MEF" | "S-3" | "S-3/A" | "S-3ASR" | "S-3MEF" | "F-1" | "F-1/A"
        | "F-1MEF" | "F-3" | "F-3/A" | "F-3ASR" | "F-3MEF" => Some(FilingClass::Offering),
        f if f.starts_with("424B") => Some(FilingClass::Offering),
        _ => None,
    }
}

/// Every SEC filing as one `NewsItem` per listed symbol (only `symbols` when given), under
/// [`FILING_RULES_VERSION`].
///
/// - `id` `news:sec_edgar:<accession>`, `version` 1: a filing is never edited (an amendment is
///   another accession), so a story has one version and is always ordered.
/// - `published_at` is the record's `accepted_at_ns`: the EDGAR acceptance time, or the
///   earliest acceptance its filing date allows for a daily-index row
///   (`crate::evidence::DATED_BY_DAILY_INDEX`). The pillar never counts a filing before it, nor
///   before `observed_at`, our first sighting.
/// - `relevant`: the CIK's own tickers, so always; `credible_source`: the filing is at the
///   regulator; `authenticity`: [`Authenticity::IssuerAnnounced`] (the issuer filed it, nobody
///   reported it).
/// - `substantive` only for a [`FilingClass::CurrentReport`]; offerings carry
///   `stock_context::OFFERING_EVENT` so a row can flag them.
///
/// The same accession recorded twice (the feed and a daily index) keeps its first sighting.
pub fn filing_items(records: &[FilingRecord], symbols: Option<&BTreeSet<String>>) -> Vec<NewsItem> {
    let mut first: BTreeMap<&str, &FilingRecord> = BTreeMap::new();
    for r in records {
        first
            .entry(r.accession.as_str())
            .and_modify(|e| {
                if (r.first_observed_at, r.accepted_at_ns) < (e.first_observed_at, e.accepted_at_ns)
                {
                    *e = r;
                }
            })
            .or_insert(r);
    }
    let mut out = Vec::new();
    for r in first.values() {
        let Some(class) = filing_class(&r.form) else {
            continue;
        };
        let event_type = match class {
            FilingClass::CurrentReport => CURRENT_REPORT_EVENT,
            FilingClass::Amendment => AMENDMENT_EVENT,
            FilingClass::Offering => stock_context::OFFERING_EVENT,
        };
        let mut syms: Vec<&str> = r.symbols.iter().map(String::as_str).collect();
        syms.sort_unstable();
        syms.dedup();
        for sym in syms {
            if symbols.is_some_and(|s| !s.contains(sym)) {
                continue;
            }
            out.push(NewsItem::from_source(&SourceNews {
                provider: r.provider.clone(),
                source: r.form.clone(),
                id: r.accession.clone(),
                symbol: sym.to_string(),
                published_at: r.accepted_at_ns,
                observed_at: r.first_observed_at,
                version: 1,
                headline: Some(format!("{} {}", r.form, r.company)),
                url: Some(r.url.clone()),
                kind: match class {
                    FilingClass::CurrentReport => SourceKind::Catalyst,
                    FilingClass::Amendment => SourceKind::Recorded,
                    FilingClass::Offering => SourceKind::Offering,
                },
                event_type: Some(event_type.to_string()),
                rules: FILING_RULES_VERSION.into(),
                halt: None,
            }));
        }
    }
    out
}

/// Every stored [`SourceNewsRecord`] (a press wire's items) as news items through the seam
/// (`stock_context::NewsItem::from_source`), one per listed symbol (only `symbols` when given).
/// A record whose `kind` is not one of the seam's is skipped, never guessed.
pub fn source_news_items(
    records: &[SourceNewsRecord],
    symbols: Option<&BTreeSet<String>>,
) -> Vec<NewsItem> {
    let mut out = Vec::new();
    for r in records {
        let kind = match r.kind.as_str() {
            "catalyst" => SourceKind::Catalyst,
            "offering" => SourceKind::Offering,
            "pending" => SourceKind::Pending,
            "recorded" => SourceKind::Recorded,
            _ => continue,
        };
        for sym in &r.symbols {
            if symbols.is_some_and(|s| !s.contains(sym)) {
                continue;
            }
            out.push(NewsItem::from_source(&SourceNews {
                provider: r.provider.clone(),
                source: r.source.clone(),
                id: r.id.clone(),
                symbol: sym.clone(),
                published_at: r.published_at,
                observed_at: r.first_observed_at,
                version: r.version,
                headline: r.headline.clone(),
                url: r.url.clone(),
                kind,
                event_type: r.event_type.clone(),
                rules: r.rules.clone(),
                halt: None,
            }));
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

#[cfg(test)]
mod outstanding_tests {
    use super::*;

    fn rec(sym: &str, float: Option<u64>, out: Option<u64>, at: i64) -> FloatRecord {
        FloatRecord {
            v: crate::evidence::EVIDENCE_SCHEMA,
            symbol: sym.into(),
            provider: "fmp".into(),
            endpoint: "/stable/shares-float-all?page=0&limit=1000".into(),
            float_shares: float,
            outstanding_shares: out,
            free_float_pct: None,
            source_url: None,
            provider_date: None,
            effective_as_of: None,
            retrieved_at: at,
            response_sha256: "x".into(),
            quality: vec![],
        }
    }

    #[test]
    fn a_null_float_keeps_its_outstanding_figure_and_nothing_inconsistent_is_used() {
        let rows = [
            rec("NULL", None, Some(7_000_000), 1),
            rec("ZERO", Some(0), Some(0), 1),
            rec("NONE", None, None, 1),
            rec("BAD", Some(9_000_000), Some(5_000_000), 1),
            rec("OK", Some(4_000_000), Some(6_000_000), 1),
            rec("OK", None, Some(6_500_000), 2),
        ];
        let obs = outstanding_observations(&rows);
        let got: Vec<_> = obs
            .iter()
            .map(|o| (o.instrument.0.as_str(), o.shares, o.version))
            .collect();
        assert_eq!(
            got,
            vec![
                ("sym:NULL", 7_000_000, 1),
                ("sym:OK", 6_000_000, 1),
                ("sym:OK", 6_500_000, 2)
            ]
        );
        assert!(obs[0].quality.contains(&"float_missing".to_string()));
        assert_eq!(obs[0].id, "outstanding:fmp:NULL");
        // The float loader is unchanged: a null float is still not a float (the zero row stays
        // as it was, a record the pillar calls `nonpositive_float`).
        let floats = float_observations(&rows);
        assert!(floats.iter().all(|f| f.instrument.0 != "sym:NULL"));
    }
}

#[cfg(test)]
mod filing_tests {
    use super::*;
    use crate::evidence::{DATED_BY_ACCEPTANCE, EVIDENCE_SCHEMA};

    fn rec(acc: &str, form: &str, syms: &[&str], accepted: i64, observed: i64) -> FilingRecord {
        FilingRecord {
            v: EVIDENCE_SCHEMA,
            provider: "sec_edgar".into(),
            source: "current_feed".into(),
            accession: acc.into(),
            cik: 1234567,
            company: "Acme Corp".into(),
            form: form.into(),
            accepted_at: None,
            accepted_at_ns: accepted,
            dated_by: DATED_BY_ACCEPTANCE.into(),
            filed: None,
            items: vec![],
            symbols: syms.iter().map(|s| s.to_string()).collect(),
            url: format!("https://www.sec.gov/Archives/edgar/data/1234567/{acc}-index.htm"),
            first_observed_at: observed,
        }
    }

    #[test]
    fn forms_are_classified_and_the_rest_ignored() {
        use FilingClass::*;
        for (form, want) in [
            ("8-K", Some(CurrentReport)),
            ("6-K", Some(CurrentReport)),
            ("8-K/A", Some(Amendment)),
            ("6-K/A", Some(Amendment)),
            ("S-1", Some(Offering)),
            ("S-1/A", Some(Offering)),
            ("S-3ASR", Some(Offering)),
            ("F-1", Some(Offering)),
            ("F-3", Some(Offering)),
            ("424B5", Some(Offering)),
            ("424B3", Some(Offering)),
            ("4", None),
            ("SC 13D", None),
            ("SC 13G/A", None),
            ("10-Q", None),
            ("8-K12B", None),
            ("S-8", None),
        ] {
            assert_eq!(filing_class(form), want, "{form}");
        }
    }

    #[test]
    fn a_current_report_is_substantive_an_offering_and_an_amendment_are_not() {
        let items = filing_items(
            &[
                rec("0001-26-000001", "8-K", &["ACME", "ACMEW"], 10, 20),
                rec("0001-26-000002", "424B5", &["ACME"], 30, 40),
                rec("0001-26-000003", "8-K/A", &["ACME"], 50, 60),
                rec("0001-26-000004", "4", &["ACME"], 70, 80),
            ],
            None,
        );
        assert_eq!(
            items.len(),
            4,
            "the 8-K for both tickers, the 424B5, the 8-K/A"
        );
        let k = items.iter().find(|n| n.source == "sec_edgar/8-K").unwrap();
        assert!(k.substantive && k.relevant && k.credible_source);
        assert_eq!(k.authenticity, Authenticity::IssuerAnnounced);
        assert_eq!((k.published_at, k.observed_at), (Some(10), 20));
        assert_eq!(k.id, "news:sec_edgar:0001-26-000001");
        assert_eq!(k.event_type, CURRENT_REPORT_EVENT);
        assert!(k.url.as_deref().unwrap().ends_with("-index.htm"));
        let o = items
            .iter()
            .find(|n| n.source == "sec_edgar/424B5")
            .unwrap();
        assert!(!o.substantive);
        assert_eq!(o.event_type, stock_context::OFFERING_EVENT);
        let a = items
            .iter()
            .find(|n| n.source == "sec_edgar/8-K/A")
            .unwrap();
        assert!(!a.substantive);
        // The symbol filter.
        let only: BTreeSet<String> = ["ACMEW".to_string()].into();
        let f = filing_items(
            &[rec("0001-26-000001", "8-K", &["ACME", "ACMEW"], 10, 20)],
            Some(&only),
        );
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].instrument, InstrumentId::symbol("ACMEW"));
    }

    #[test]
    fn one_accession_recorded_twice_keeps_its_first_sighting() {
        let mut late = rec("0001-26-000009", "6-K", &["FRGN"], 100, 900);
        late.source = "daily_index".into();
        let items = filing_items(
            &[late, rec("0001-26-000009", "6-K", &["FRGN"], 150, 200)],
            None,
        );
        assert_eq!(items.len(), 1);
        assert_eq!(
            (items[0].observed_at, items[0].published_at),
            (200, Some(150))
        );
    }
}
