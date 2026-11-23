//! What a release is (catalyst or offering), and when two releases are the same one.

use std::collections::BTreeSet;

/// A release's kind for the news pillar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireKind {
    /// The default: an issuer announcing something.
    Catalyst,
    /// Raising money by selling stock or warrants. It must **not** pass the news pillar (a
    /// dilution headline is the opposite of a catalyst for a small-cap long); it is surfaced as
    /// an offering flag, like EDGAR's dilution filings on ross/news.
    Offering,
}

impl WireKind {
    pub fn as_str(self) -> &'static str {
        match self {
            WireKind::Catalyst => "catalyst",
            WireKind::Offering => "offering",
        }
    }
}

/// Headline phrases that make a release an offering, matched case-insensitively as substrings
/// of the headline only (a teaser can mention an "offering" of products). Small and explicit on
/// purpose: each one is a financing announcement in small-cap press releases, and each is
/// tested. Over-matching is the safe side: it only withholds a pass from the news pillar, it
/// never invents one.
pub const OFFERING_PHRASES: &[&str] = &[
    "public offering",
    "registered direct",
    "direct offering",
    "private placement",
    "at-the-market",
    "at the market offering",
    "atm offering",
    "atm program",
    "underwritten offering",
    "best efforts offering",
    "best-efforts offering",
    "proposed offering",
    "warrant inducement",
    "inducement of warrant",
    "exercise of warrants",
    "warrant exercise",
    "announces pricing",
    "pricing of",
    "prices offering",
    "equity line",
    "share subscription facility",
    "securities purchase agreement",
    "shelf registration",
];

pub fn classify(headline: &str) -> WireKind {
    let h = headline.to_lowercase();
    if OFFERING_PHRASES.iter().any(|p| h.contains(p)) {
        WireKind::Offering
    } else {
        WireKind::Catalyst
    }
}

/// Headline phrases of law-firm solicitations ("SHAREHOLDER ALERT: ... Announces Investigation
/// of ... (NASDAQ: ABCD)"). They name the ticker in exactly the strict form the extractor
/// accepts and are not the issuer's news; on the 2026-11-23 PR Newswire sample they were 2 of
/// the 3 releases that named a US listing. Matched case-insensitively on the headline.
pub const NOISE_HEADLINE: &[&str] = &[
    "shareholder alert",
    "investor alert",
    "shareholder rights",
    "class action",
    "securities fraud",
    "investors who lost",
    "lead plaintiff",
    "deadline alert",
    "reminds investors",
    "encourages investors",
    "investigation on behalf",
    "announces investigation",
    "investigation of",
    "lawsuit filed",
];

/// The same in the teaser: only phrases a solicitation uses and an issuer's own release does
/// not.
pub const NOISE_SUMMARY: &[&str] = &[
    "law firm",
    "securities class action",
    "class action lawsuit",
    "on behalf of investors",
    "on behalf of shareholders",
];

/// Whether a release is a solicitation about the issuer rather than news from it: archived,
/// never delivered.
pub fn is_noise(headline: &str, summary: &str) -> bool {
    let (h, s) = (headline.to_lowercase(), summary.to_lowercase());
    NOISE_HEADLINE.iter().any(|p| h.contains(p)) || NOISE_SUMMARY.iter().any(|p| s.contains(p))
}

/// The words of a headline for comparing releases: lower case, alphanumeric runs only.
pub fn words(title: &str) -> BTreeSet<String> {
    title
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Jaccard similarity at or above which two headlines for the same symbol are one release.
/// Wires reformat a headline slightly (case, a dropped "Inc.", punctuation), never reword it.
pub const SAME_TITLE: f64 = 0.8;

pub fn similar(a: &BTreeSet<String>, b: &BTreeSet<String>) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let inter = a.intersection(b).count() as f64;
    let union = a.union(b).count() as f64;
    inter / union >= SAME_TITLE
}
