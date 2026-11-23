//! Strict ticker extraction from press-release text.
//!
//! Only an **exchange-qualified listing inside parentheses** counts: `(NASDAQ: ABCD)`,
//! `(Nasdaq: ABCD)`, `(NYSE American: ABC)`, `(NYSE: ABC)`, `(OTCQB: ABCD)`,
//! `(TSX-V: ABC; NASDAQ: ABCD)`, `(NASDAQ: ABCD, ABCDW)`. A bare capitalised word never does:
//! headlines are full of them (`CEO`, `FDA`, `AI`, `PR`), and every one is a real ticker
//! somewhere.
//!
//! The rules, in the order they apply (each one is tested in `super::tests`):
//!
//! 1. Only text between `(` and `)` is read. Inside, the group is split on `;` and `,` into
//!    pieces. A piece with a `:` names a venue (left of the first `:`) and one ticker (right);
//!    a piece without one is a further ticker for the previous piece's venue.
//! 2. The venue must be one of the names in [`venue_of`], compared case-insensitively with `-`
//!    read as a space. Anything else (`TSX`, `TSXV`, `CSE`, `ASX`, `HKEX stock code`, `LSE`,
//!    `the "Company"`) is a non-US venue or not a venue: its tickers are never mapped, and
//!    continuation pieces after it stay unmapped too.
//! 3. A ticker is 1-5 ASCII capitals, optionally followed by one class suffix `.X`/`.XX` or
//!    `-X`/`-XX` (written in the app's notation, `BRK.B`). Anything else in the piece, a space
//!    (`FHN PR E`), a slash (`MS/PL`), lower case, digits, makes the **whole piece** invalid.
//!    This is the lesson from research: loosely splitting class tickers once mapped the real
//!    tickers F, PR and PL to banks' preferred shares. An invalid piece is dropped, never
//!    salvaged.
//! 4. The caller maps a listing to a symbol only if the tradable universe (`assets.json`) has
//!    the ticker **on the same venue family** ([`Venue::matches`]): `(TSX: AB)` or
//!    `(NYSE: AB)` never maps to a NASDAQ `AB`.

/// A US venue a listing can name. Foreign and unknown venues are not represented: their
/// listings are never mapped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Venue {
    Nasdaq,
    Nyse,
    NyseAmerican,
    NyseArca,
    Cboe,
    Otc,
}

impl Venue {
    pub fn as_str(self) -> &'static str {
        match self {
            Venue::Nasdaq => "NASDAQ",
            Venue::Nyse => "NYSE",
            Venue::NyseAmerican => "NYSE American",
            Venue::NyseArca => "NYSE Arca",
            Venue::Cboe => "Cboe",
            Venue::Otc => "OTC",
        }
    }

    /// Whether an `assets.json` exchange (Alpaca's listing exchange: `NASDAQ`, `NYSE`, `AMEX`,
    /// `ARCA`, `BATS`, `OTC`) is this venue. Alpaca labels NYSE American `AMEX` and Cboe BZX
    /// `BATS`.
    pub fn matches(self, asset_exchange: &str) -> bool {
        let e = asset_exchange.trim().to_ascii_uppercase();
        match self {
            Venue::Nasdaq => e == "NASDAQ",
            Venue::Nyse => e == "NYSE",
            Venue::NyseAmerican => e == "AMEX" || e == "NYSEAMERICAN" || e == "NYSE AMERICAN",
            Venue::NyseArca => e == "ARCA" || e == "NYSEARCA",
            Venue::Cboe => e == "BATS" || e == "CBOE",
            Venue::Otc => e == "OTC",
        }
    }
}

/// The venue names accepted (rule 2). Kept explicit: a name added here is a decision.
pub fn venue_of(name: &str) -> Option<Venue> {
    let norm: String = name
        .trim()
        .replace('-', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_uppercase();
    Some(match norm.as_str() {
        "NASDAQ"
        | "NASDAQ GS"
        | "NASDAQ GM"
        | "NASDAQ CM"
        | "NASDAQGS"
        | "NASDAQGM"
        | "NASDAQCM"
        | "NASDAQ CAPITAL MARKET"
        | "NASDAQ GLOBAL MARKET"
        | "NASDAQ GLOBAL SELECT MARKET"
        | "NASDAQ GLOBAL SELECT" => Venue::Nasdaq,
        "NYSE" | "NEW YORK STOCK EXCHANGE" => Venue::Nyse,
        "NYSE AMERICAN" | "NYSEAMERICAN" | "NYSE AMER" | "NYSE MKT" | "NYSEMKT" | "AMEX" => {
            Venue::NyseAmerican
        }
        "NYSE ARCA" | "NYSEARCA" => Venue::NyseArca,
        "CBOE" | "CBOE BZX" | "BATS" => Venue::Cboe,
        "OTC" | "OTCQB" | "OTCQX" | "OTC QB" | "OTC QX" | "OTC PINK" | "OTCPINK" | "OTCPK"
        | "OTC MARKETS" | "OTCMKTS" => Venue::Otc,
        _ => return None,
    })
}

/// One exchange-qualified listing as written in the text, in the app's notation.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Listing {
    pub venue: Venue,
    pub ticker: String,
}

/// A ticker in the app's notation, or `None` when the piece is not exactly one ticker (rule 3).
pub fn ticker_of(piece: &str) -> Option<String> {
    let p = piece.trim();
    let (base, class) = match p.find(['.', '-']) {
        Some(i) => (&p[..i], Some(&p[i + 1..])),
        None => (p, None),
    };
    let caps = |s: &str, lo: usize, hi: usize| {
        (lo..=hi).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_uppercase())
    };
    if !caps(base, 1, 5) {
        return None;
    }
    match class {
        None => Some(base.to_owned()),
        Some(c) if caps(c, 1, 2) => Some(format!("{base}.{c}")),
        Some(_) => None,
    }
}

/// Every exchange-qualified US listing in `text`, in order of appearance, without repeats.
pub fn listings(text: &str) -> Vec<Listing> {
    let mut out: Vec<Listing> = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('(') {
        let after = &rest[open + 1..];
        let Some(close) = after.find([')', '(']) else {
            break;
        };
        let group = &after[..close];
        rest = &after[close..];
        if rest.starts_with(')') {
            rest = &rest[1..];
        }
        // `None`: no venue yet, or a venue that is not mapped (rule 2).
        let mut venue: Option<Venue> = None;
        let mut named = false;
        for piece in group.split([';', ',']) {
            let ticker = match piece.split_once(':') {
                Some((v, t)) => {
                    venue = venue_of(v);
                    named = true;
                    t
                }
                // A continuation only follows a named venue.
                None if named => piece,
                None => continue,
            };
            let (Some(v), Some(t)) = (venue, ticker_of(ticker)) else {
                continue;
            };
            let l = Listing {
                venue: v,
                ticker: t,
            };
            if !out.contains(&l) {
                out.push(l);
            }
        }
    }
    out
}
