//! Research trade-condition policy, version [`POLICY_ID`] (`conditions-research-v1`).
//!
//! Production (`market_events::conditions`) encodes Alpaca's **minute-bar** column and treats
//! codes it does not know as regular sales. Research cannot afford that default: a feature that
//! silently counts a print whose meaning nobody checked is a feature nobody can defend. This
//! module audits every code per tape against the SIP specifications and Alpaca's table and:
//!
//! - follows the audited rule for every **resolved** code (which, for every resolved code, is the
//!   same answer production gives; see [`audit_rows`] and `docs/research/foundation/conditions.md`);
//! - **excludes the price and volume contribution** of a print carrying any **unresolved** code
//!   (reserved / placeholder / TBD in the spec, a code the tape's SIP does not define, an
//!   unaudited tape), keeping the raw event and flagging it ([`ResearchEffect::unresolved`]).
//!
//! The three things a print can update are kept apart, as both SIPs do: the last (open/close),
//! the high/low and the volume. In v1 every resolved code has `last == high_low`, because the
//! intraday rule follows Alpaca's minute column where the SIPs' consolidated columns differ only by
//! day-level "first or only last sale of the day" footnotes (see the audit notes).
//!
//! # Sources (read 2026-10-18)
//!
//! - CTS: "Consolidated Tape System (CTS) Pillar Multicast Output Binary Specification",
//!   v2.11b (2026-01-29), Field Descriptions, *Sale Condition* (p. 82 table: consolidated LAST,
//!   HIGH/LOW, VOLUME update; p. 83 notes #2-#4 and the multiple-condition rule; p. 84 category
//!   layout). <https://www.ctaplan.com/publicdocs/ctaplan/CTS_Pillar_Output_Specification.pdf>
//! - UTP: "UTP Data Feed Services Specification" v4.1 (September 2026), §3.14.1 Sale Condition
//!   Modifiers (p. 47), §3.14.2 UTP Sale Condition Matrix (p. 48, footnotes p. 49), §3.14.5 notes
//!   (pp. 51-52). <https://utpplan.com/DOC/UtpBinaryOutputSpec.pdf>
//! - Alpaca Market Data FAQ, "How are bars aggregated?" (minute `M` column).
//!   <https://docs.alpaca.markets/us/docs/market-data-faq>
//!
//! Changing any rule here is a new policy version: bump [`POLICY_ID`], never edit v1 in place,
//! because stored features name the policy they were computed with.

use market_events::{Tape, Trade, condition_rule};
use serde::{Deserialize, Serialize};

/// The policy identifier recorded with every research feature computed under these rules.
pub const POLICY_ID: &str = "conditions-research-v1";

/// What a print may update.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Updates {
    /// Open/close (the "last sale").
    pub last: bool,
    pub high_low: bool,
    /// Volume and trade count.
    pub volume: bool,
}

impl Updates {
    pub const FULL: Updates = Updates {
        last: true,
        high_low: true,
        volume: true,
    };
    pub const VOLUME_ONLY: Updates = Updates {
        last: false,
        high_low: false,
        volume: true,
    };
    pub const NONE: Updates = Updates {
        last: false,
        high_low: false,
        volume: false,
    };

    /// Field-wise AND: the strictest rule wins (CTS p. 83 "the `NO' takes precedence"; UTP p. 49
    /// "a 'no' update for any one of the statistics will take precedence").
    pub fn and(self, o: Updates) -> Updates {
        Updates {
            last: self.last && o.last,
            high_low: self.high_low && o.high_low,
            volume: self.volume && o.volume,
        }
    }

    /// Whether the print's price may be used at all (as a last price or an extreme).
    pub fn price(self) -> bool {
        self.last || self.high_low
    }
}

/// Which SIP's code table applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TapeFamily {
    /// Tapes A and B (CTA / CTS).
    Cta,
    /// Tape C (UTP / UTDF).
    Utp,
    /// Tape O (OTC / FINRA TDDS): not audited in v1.
    Otc,
    Unknown,
}

impl TapeFamily {
    pub fn of(tape: Tape) -> TapeFamily {
        match tape {
            Tape::A | Tape::B => TapeFamily::Cta,
            Tape::C => TapeFamily::Utp,
            Tape::O => TapeFamily::Otc,
            Tape::Unknown => TapeFamily::Unknown,
        }
    }
}

/// Why a code is unresolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnresolvedReason {
    /// The spec marks the code "Reserved" (CTS `N`, `8`; UTP `N`): no defined meaning, volume TBD.
    Reserved,
    /// The spec marks the code a placeholder with every column TBD (UTP `E`, `8`).
    Placeholder,
    /// The tape's SIP does not define this code at all (e.g. `A` on tape A).
    NotDefinedForTape,
    /// Tape O: FINRA's TDDS table was not audited for v1.
    TapeNotAudited,
    /// The tape is unknown and the code means different things on CTA and UTP tapes.
    AmbiguousTape,
}

/// The research verdict for one code on one tape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Verdict {
    Resolved { updates: Updates },
    Unresolved { reason: UnresolvedReason },
}

use Updates as U;

const fn r(u: Updates) -> Verdict {
    Verdict::Resolved { updates: u }
}

const fn unres(reason: UnresolvedReason) -> Verdict {
    Verdict::Unresolved { reason }
}

/// Tapes A/B: CTS p. 82, intraday reading (see the audit for each deviation from the
/// consolidated columns).
fn cta(code: &str) -> Verdict {
    use UnresolvedReason::*;
    match code {
        // "Space" = Regular Sale. `@` was the CTS regular-sale code until CTS replaced it with
        // Space (CTS version history); Alpaca lists `@` for tapes C/O only. Both mean regular.
        "" | "@" => r(U::FULL),
        "B" | "C" | "H" | "I" | "R" | "U" | "V" | "7" => r(U::VOLUME_ONLY),
        "E" | "F" | "K" | "L" | "O" | "X" | "5" | "6" => r(U::FULL),
        // Extended Hours Trade: CTS consolidated NO/NO, volume YES; Alpaca's minute bars update
        // everything. v1 follows the minute bars (see the audit note "T").
        "T" => r(U::FULL),
        // P, Z, 4: CTS LAST "#2" (only qualifying last of the day), HIGH/LOW YES. Alpaca minute
        // bars: no price. Intraday: no price (stricter than the day-level high/low).
        "P" | "Z" | "4" => r(U::VOLUME_ONLY),
        // Official close/open and the corrected consolidated close: never an intraday execution.
        "M" | "Q" | "9" => r(U::NONE),
        "N" | "8" => unres(Reserved),
        _ => unres(NotDefinedForTape),
    }
}

/// Tape C: UTP §3.14.2 matrix, intraday reading.
fn utp(code: &str) -> Verdict {
    use UnresolvedReason::*;
    match code {
        // `@` Regular Sale; a space-filled (unused) byte position carries no condition.
        "" | "@" => r(U::FULL),
        "A" | "B" | "D" | "F" | "K" | "L" | "O" | "X" | "Y" | "5" | "6" => r(U::FULL),
        // S Split Trade and 1 Stopped Stock (Regular Trade): Yes/Yes/Yes in the matrix; absent from
        // Alpaca's table, so production reports them as unrecognized but counts them fully.
        "S" | "1" => r(U::FULL),
        "C" | "H" | "I" | "R" | "U" | "V" | "W" | "7" => r(U::VOLUME_ONLY),
        "T" => r(U::FULL),
        // G, P, Z, 4: high/low Yes, last only if first/only last-sale-eligible trade of the day
        // (footnote 1). Alpaca minute bars: no price. Intraday: no price.
        "G" | "P" | "Z" | "4" => r(U::VOLUME_ONLY),
        "M" | "Q" | "9" => r(U::NONE),
        "N" => unres(Reserved),
        "E" | "8" => unres(Placeholder),
        _ => unres(NotDefinedForTape),
    }
}

/// The v1 verdict for one condition code on one tape. Codes are matched exactly after trimming
/// surrounding whitespace (so `" "` is the empty code).
pub fn code_verdict(code: &str, tape: Tape) -> Verdict {
    let code = code.trim();
    match TapeFamily::of(tape) {
        TapeFamily::Cta => cta(code),
        TapeFamily::Utp => utp(code),
        TapeFamily::Otc => unres(UnresolvedReason::TapeNotAudited),
        TapeFamily::Unknown => match (cta(code), utp(code)) {
            (a @ Verdict::Resolved { .. }, b) if a == b => a,
            // Unresolved on both tapes: the CTA reason (they agree except for `8`, which is
            // reserved on CTA and a placeholder on UTP).
            (Verdict::Unresolved { reason }, Verdict::Unresolved { .. }) => unres(reason),
            _ => unres(UnresolvedReason::AmbiguousTape),
        },
    }
}

/// How the historical provider's later verdict on a print (Alpaca `u`) is used.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateMarkerMode {
    /// Ignore `u`: the print counts as delivered. For **point-in-time features**, because live
    /// consumers saw the print and only later (at an unknown time, which history does not record)
    /// its cancel. Using `u` here would leak hindsight into a feature.
    AsDelivered,
    /// `canceled` / `cancelled` / `incorrect` prints count for nothing (as production's
    /// `Trade::effect`). For **labels and clean reference bars**, which may use hindsight by
    /// definition and must declare it.
    Hindsight,
}

/// A print's research effect under v1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResearchEffect {
    pub updates: Updates,
    /// Codes that restricted at least one field (in delivery order).
    pub restricting: Vec<String>,
    /// Unresolved codes (in delivery order). Non-empty implies [`Updates::NONE`].
    pub unresolved: Vec<String>,
    /// The print was excluded because of a provider `u` marker under [`UpdateMarkerMode::Hindsight`].
    pub withdrawn: bool,
}

impl ResearchEffect {
    pub fn is_flagged(&self) -> bool {
        !self.unresolved.is_empty()
    }
}

/// Combines a print's conditions: any unresolved code excludes the print's price **and** volume;
/// otherwise each field is the AND over the codes. An empty list is a regular sale.
pub fn research_effect<S: AsRef<str>>(conditions: &[S], tape: Tape) -> ResearchEffect {
    let mut updates = Updates::FULL;
    let mut restricting = Vec::new();
    let mut unresolved = Vec::new();
    if TapeFamily::of(tape) == TapeFamily::Otc {
        // The whole tape is unaudited, so even a print with no codes is flagged.
        unresolved.push("tape:O".to_string());
    }
    for c in conditions {
        let code = c.as_ref().trim();
        match code_verdict(code, tape) {
            Verdict::Resolved { updates: u } => {
                if u != Updates::FULL {
                    restricting.push(code.to_string());
                }
                updates = updates.and(u);
            }
            Verdict::Unresolved { .. } => unresolved.push(code.to_string()),
        }
    }
    if !unresolved.is_empty() {
        updates = Updates::NONE;
    }
    ResearchEffect {
        updates,
        restricting,
        unresolved,
        withdrawn: false,
    }
}

/// [`research_effect`] for a trade, applying its `u` marker per `mode`.
pub fn trade_research_effect(t: &Trade, mode: UpdateMarkerMode) -> ResearchEffect {
    let mut e = research_effect(&t.conditions, t.tape);
    if mode == UpdateMarkerMode::Hindsight
        && matches!(
            t.update.as_deref().map(str::trim),
            Some("canceled" | "cancelled" | "incorrect")
        )
    {
        e.updates = Updates::NONE;
        e.withdrawn = true;
    }
    e
}

/// Production's answer for one code, as [`Updates`] (production collapses last and high/low into
/// "OHLC"). `None` = production does not know the code (it then counts it as a regular sale).
pub fn production_updates(code: &str, tape: Tape) -> Option<Updates> {
    condition_rule(code, tape).map(|(rule, _)| match rule {
        market_events::Rule::Full => Updates::FULL,
        market_events::Rule::VolumeOnly => Updates::VOLUME_ONLY,
        market_events::Rule::NoUpdate => Updates::NONE,
    })
}

// ---------------------------------------------------------------------------------------------
// The audit table: per tape family and code, the spec's columns, Alpaca's minute column,
// production and v1, so the document is generated from (and tested against) the code.
// ---------------------------------------------------------------------------------------------

/// One cell of a SIP spec column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Spec {
    Yes,
    No,
    /// Conditional, with the spec's note/footnote label.
    Cond(&'static str),
    Tbd,
    /// The code is not in this SIP's table.
    Undefined,
}

impl Spec {
    fn text(self) -> String {
        match self {
            Spec::Yes => "Y".into(),
            Spec::No => "N".into(),
            Spec::Cond(n) => format!("cond {n}"),
            Spec::Tbd => "TBD".into(),
            Spec::Undefined => "-".into(),
        }
    }
}

/// Audit classification of production against the audited rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    Matches,
    /// Production updates more than the audited rule allows.
    DiffersProductionLooser,
    /// Production updates less than the audited rule allows.
    DiffersProductionStricter,
    Unresolved,
}

#[derive(Clone, Debug, Serialize)]
pub struct AuditRow {
    pub family: TapeFamily,
    pub code: &'static str,
    pub description: &'static str,
    /// SIP consolidated columns: last, high/low, volume.
    pub spec: [Spec; 3],
    /// Alpaca minute column (open/close, high/low, volume), `None` when Alpaca does not list
    /// the code for this tape.
    pub alpaca_minute: Option<[bool; 3]>,
    pub production: Option<Updates>,
    pub verdict: Verdict,
    pub class: Class,
    pub note: &'static str,
}

use Spec::{Cond, No, Tbd, Undefined as Und, Yes};

type Raw = (
    &'static str,
    &'static str,
    [Spec; 3],
    Option<[bool; 3]>,
    &'static str,
);

const G: bool = true;
const R: bool = false;

/// CTS p. 82 (LAST, HIGH/LOW, VOLUME, consolidated) and Alpaca's AB rows.
const CTA_ROWS: &[Raw] = &[
    (" ", "Regular Sale", [Yes, Yes, Yes], Some([G, G, G]), ""),
    (
        "@",
        "Regular Sale (legacy CTS code)",
        [Und, Und, Und],
        None,
        "CTS replaced `@` with Space for Regular Sale (version history); treated as Space",
    ),
    (
        "B",
        "Average Price Trade",
        [No, No, Yes],
        Some([R, R, G]),
        "",
    ),
    (
        "C",
        "Cash Trade (Same Day Clearing)",
        [No, No, Yes],
        Some([R, R, G]),
        "",
    ),
    (
        "E",
        "Automatic Execution",
        [Yes, Yes, Yes],
        Some([G, G, G]),
        "",
    ),
    (
        "F",
        "Inter-market Sweep Order",
        [Yes, Yes, Yes],
        Some([G, G, G]),
        "",
    ),
    (
        "H",
        "Price Variation Trade",
        [No, No, Yes],
        Some([R, R, G]),
        "",
    ),
    ("I", "Odd Lot Trade", [No, No, Yes], Some([R, R, G]), ""),
    (
        "K",
        "Rule 127 (NYSE) / Rule 155 (NYSE American)",
        [Yes, Yes, Yes],
        Some([G, G, G]),
        "",
    ),
    (
        "L",
        "Sold Last (Late Reporting)",
        [Cond("#3"), Yes, Yes],
        Some([G, G, G]),
        "CTS last only if only qualifying last / same participant / listing market (a day-level \
         last-sale rule); intraday follows Alpaca: full",
    ),
    (
        "M",
        "Market Center Official Close",
        [No, No, No],
        Some([R, R, R]),
        "",
    ),
    (
        "N",
        "Reserved (was Next Day; reserved since CTS 2.10b)",
        [No, No, Tbd],
        Some([R, R, G]),
        "reserved, volume TBD: unresolved; production counts volume",
    ),
    (
        "O",
        "Market Center Opening Trade",
        [Yes, Yes, Yes],
        Some([G, G, G]),
        "",
    ),
    (
        "P",
        "Prior Reference Price",
        [Cond("#2"), Yes, Yes],
        Some([R, R, G]),
        "CTS high/low YES is day-level (the price belongs to an earlier time); intraday: no price, \
         as Alpaca's minute bars",
    ),
    (
        "Q",
        "Market Center Official Open",
        [No, No, No],
        Some([R, R, R]),
        "",
    ),
    ("R", "Seller", [No, No, Yes], Some([R, R, G]), ""),
    (
        "T",
        "Extended Hours Trade",
        [No, No, Yes],
        Some([G, G, G]),
        "SIP consolidated last/high/low exclude extended hours; Alpaca minute bars (and production) \
         use them, or pre-market would have no candles. Intentional: intraday features need them",
    ),
    (
        "U",
        "Extended Hours Sold (Out Of Sequence)",
        [No, No, Yes],
        Some([R, R, G]),
        "",
    ),
    ("V", "Contingent Trade", [No, No, Yes], Some([R, R, G]), ""),
    (
        "X",
        "Cross / Periodic Auction Trade",
        [Yes, Yes, Yes],
        Some([G, G, G]),
        "",
    ),
    (
        "Z",
        "Sold (Out Of Sequence)",
        [Cond("#2"), Yes, Yes],
        Some([R, R, G]),
        "as P: intraday no price",
    ),
    (
        "4",
        "Derivatively Priced",
        [Cond("#2"), Yes, Yes],
        Some([R, R, G]),
        "as P: intraday no price",
    ),
    (
        "5",
        "Market Center Reopening Trade",
        [Yes, Yes, Yes],
        Some([G, G, G]),
        "",
    ),
    (
        "6",
        "Market Center Closing Trade",
        [Yes, Yes, Yes],
        Some([G, G, G]),
        "",
    ),
    (
        "7",
        "Qualified Contingent Trade",
        [No, No, Yes],
        Some([R, R, G]),
        "",
    ),
    (
        "8",
        "Reserved",
        [No, No, Tbd],
        None,
        "reserved: unresolved; production does not know it and counts it fully",
    ),
    (
        "9",
        "Corrected Consolidated Close (per Listing Market)",
        [Yes, Yes, No],
        Some([R, R, R]),
        "a post-close correction of the day's close, zero volume; not an intraday execution",
    ),
    // UTP-only codes seen on a CTA tape: the CTS defines none of them.
    (
        "A",
        "(UTP Acquisition)",
        [Und, Und, Und],
        None,
        "not a CTS code; production applies the UTP meaning (full)",
    ),
    (
        "D",
        "(UTP Distribution)",
        [Und, Und, Und],
        None,
        "not a CTS code; production applies the UTP meaning (full)",
    ),
    (
        "G",
        "(UTP Bunched Sold Trade)",
        [Und, Und, Und],
        None,
        "not a CTS code; production applies the UTP meaning (volume only)",
    ),
    (
        "S",
        "(UTP Split Trade)",
        [Und, Und, Und],
        None,
        "not a CTS code; production: unrecognized, full",
    ),
    (
        "W",
        "(UTP Average Price Trade)",
        [Und, Und, Und],
        None,
        "not a CTS code; production applies the UTP meaning (volume only)",
    ),
    (
        "Y",
        "(UTP Yellow Flag Regular Trade)",
        [Und, Und, Und],
        None,
        "not a CTS code; production applies the UTP meaning (full)",
    ),
    (
        "1",
        "(UTP Stopped Stock)",
        [Und, Und, Und],
        None,
        "not a CTS code; production: unrecognized, full",
    ),
];

/// UTP §3.14.2 (consolidated Update Last, Update High/Low, Update Volume) and Alpaca's C rows.
/// Note the UTP matrix prints High/Low before Last; the array order here is last, high/low, volume.
const UTP_ROWS: &[Raw] = &[
    ("@", "Regular Sale", [Yes, Yes, Yes], Some([G, G, G]), ""),
    (
        " ",
        "(space-filled byte: no condition)",
        [Und, Und, Und],
        None,
        "unused byte positions are space-filled (§3.14.3); no condition",
    ),
    ("A", "Acquisition", [Yes, Yes, Yes], Some([G, G, G]), ""),
    ("B", "Bunched Trade", [Yes, Yes, Yes], Some([G, G, G]), ""),
    ("C", "Cash Sale", [No, No, Yes], Some([R, R, G]), ""),
    ("D", "Distribution", [Yes, Yes, Yes], Some([G, G, G]), ""),
    (
        "E",
        "Placeholder",
        [Tbd, Tbd, Tbd],
        None,
        "placeholder, all TBD: unresolved; production applies the CTA meaning (full)",
    ),
    (
        "F",
        "Intermarket Sweep",
        [Yes, Yes, Yes],
        Some([G, G, G]),
        "",
    ),
    (
        "G",
        "Bunched Sold Trade",
        [Cond("fn 1"), Yes, Yes],
        Some([R, R, G]),
        "high/low Yes and last only if first/only last-sale trade of the day are day-level; \
         intraday no price, as Alpaca's minute bars",
    ),
    (
        "H",
        "Price Variation Trade",
        [No, No, Yes],
        Some([R, R, G]),
        "",
    ),
    ("I", "Odd Lot Trade", [No, No, Yes], Some([R, R, G]), ""),
    (
        "K",
        "Rule 155 Trade (AMEX)",
        [Yes, Yes, Yes],
        Some([G, G, G]),
        "",
    ),
    (
        "L",
        "Sold Last",
        [Cond("fn 2"), Yes, Yes],
        Some([G, G, G]),
        "consolidated last only before 16:00:10 (End of Last Sale Eligibility); intraday: full",
    ),
    (
        "M",
        "Market Center Official Close",
        [No, No, No],
        Some([R, R, R]),
        "",
    ),
    (
        "N",
        "Reserved",
        [No, No, Tbd],
        Some([R, R, G]),
        "reserved, volume TBD: unresolved; production counts volume",
    ),
    ("O", "Opening Prints", [Yes, Yes, Yes], Some([G, G, G]), ""),
    (
        "P",
        "Prior Reference Price",
        [Cond("fn 1"), Yes, Yes],
        Some([R, R, G]),
        "as G: intraday no price",
    ),
    (
        "Q",
        "Market Center Official Open",
        [No, No, No],
        Some([R, R, R]),
        "",
    ),
    ("R", "Seller", [No, No, Yes], Some([R, R, G]), ""),
    (
        "S",
        "Split Trade",
        [Yes, Yes, Yes],
        None,
        "not in Alpaca's table; production reports it unrecognized but counts it fully, as the \
         matrix says",
    ),
    (
        "T",
        "Form T",
        [No, No, Yes],
        Some([G, G, G]),
        "as CTA T: intentional, intraday features need extended-hours prices",
    ),
    (
        "U",
        "Extended trading hours (Sold Out of Sequence)",
        [No, No, Yes],
        Some([R, R, G]),
        "",
    ),
    ("V", "Contingent Trade", [No, No, Yes], Some([R, R, G]), ""),
    (
        "W",
        "Average Price Trade",
        [No, No, Yes],
        Some([R, R, G]),
        "",
    ),
    (
        "X",
        "Cross/Periodic Auction Trade",
        [Yes, Yes, Yes],
        Some([G, G, G]),
        "",
    ),
    (
        "Y",
        "Yellow Flag Regular Trade",
        [Yes, Yes, Yes],
        Some([G, G, G]),
        "",
    ),
    (
        "Z",
        "Sold (out of sequence)",
        [Cond("fn 1"), Yes, Yes],
        Some([R, R, G]),
        "as G: intraday no price",
    ),
    (
        "1",
        "Stopped Stock (Regular Trade)",
        [Yes, Yes, Yes],
        None,
        "not in Alpaca's table; production: unrecognized, full, as the matrix says",
    ),
    (
        "4",
        "Derivatively priced",
        [Cond("fn 1"), Yes, Yes],
        Some([R, R, G]),
        "as G: intraday no price",
    ),
    (
        "5",
        "Re-Opening Prints",
        [Yes, Yes, Yes],
        Some([G, G, G]),
        "",
    ),
    ("6", "Closing Prints", [Yes, Yes, Yes], Some([G, G, G]), ""),
    (
        "7",
        "Qualified Contingent Trade (QCT)",
        [No, No, Yes],
        Some([R, R, G]),
        "",
    ),
    (
        "8",
        "Placeholder For 611 Exempt",
        [Tbd, Tbd, Tbd],
        None,
        "placeholder, all TBD: unresolved; production: unrecognized, full",
    ),
    (
        "9",
        "Corrected Consolidated Close (per listing market)",
        [Yes, Yes, No],
        Some([R, R, R]),
        "a post-close correction, zero volume (§3.14.5 note 8); not an intraday execution",
    ),
];

fn classify(production: Option<Updates>, verdict: Verdict) -> Class {
    // Production's effective behaviour: an unknown code counts as a regular sale.
    let prod = production.unwrap_or(Updates::FULL);
    match verdict {
        Verdict::Unresolved { .. } => Class::Unresolved,
        Verdict::Resolved { updates } if updates == prod => Class::Matches,
        Verdict::Resolved { updates } => {
            let looser = (prod.last && !updates.last)
                || (prod.high_low && !updates.high_low)
                || (prod.volume && !updates.volume);
            if looser {
                Class::DiffersProductionLooser
            } else {
                Class::DiffersProductionStricter
            }
        }
    }
}

/// Every audited (tape family, code) row, CTA first, then UTP.
pub fn audit_rows() -> Vec<AuditRow> {
    let mut out = Vec::new();
    for (family, rows, tape) in [
        (TapeFamily::Cta, CTA_ROWS, Tape::A),
        (TapeFamily::Utp, UTP_ROWS, Tape::C),
    ] {
        for &(code, description, spec, alpaca_minute, note) in rows {
            let production = production_updates(code, tape);
            let verdict = code_verdict(code, tape);
            out.push(AuditRow {
                family,
                code,
                description,
                spec,
                alpaca_minute,
                production,
                verdict,
                class: classify(production, verdict),
                note,
            });
        }
    }
    out
}

fn updates_text(u: Updates) -> &'static str {
    match (u.last, u.high_low, u.volume) {
        (true, true, true) => "full",
        (false, false, true) => "volume only",
        (false, false, false) => "nothing",
        _ => "mixed",
    }
}

/// The audit as a Markdown table (pasted into `docs/research/foundation/conditions.md`).
pub fn audit_markdown() -> String {
    let mut s = String::from(
        "| Tape | Code | Description | SIP last | SIP high/low | SIP volume | Alpaca 1m (o/c, h/l, v) | Production | Research v1 | Class | Note |\n|---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    for row in audit_rows() {
        let tape = match row.family {
            TapeFamily::Cta => "A/B",
            TapeFamily::Utp => "C",
            _ => "?",
        };
        let alpaca = row.alpaca_minute.map_or("not listed".to_string(), |a| {
            a.iter()
                .map(|&b| if b { "G" } else { "R" })
                .collect::<Vec<_>>()
                .join(" ")
        });
        let prod = match row.production {
            Some(u) => updates_text(u).to_string(),
            None => "unrecognized → full".to_string(),
        };
        let research = match row.verdict {
            Verdict::Resolved { updates } => updates_text(updates).to_string(),
            Verdict::Unresolved { reason } => format!("**excluded** ({reason:?})"),
        };
        let class = match row.class {
            Class::Matches => "matches",
            Class::DiffersProductionLooser => "differs: production looser",
            Class::DiffersProductionStricter => "differs: production stricter",
            Class::Unresolved => "**unresolved**",
        };
        let code = if row.code == " " {
            "` `".to_string()
        } else {
            format!("`{}`", row.code)
        };
        s.push_str(&format!(
            "| {tape} | {code} | {} | {} | {} | {} | {alpaca} | {prod} | {research} | {class} | {} |\n",
            row.description,
            row.spec[0].text(),
            row.spec[1].text(),
            row.spec[2].text(),
            row.note
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upd(c: &[&str], tape: Tape) -> Updates {
        research_effect(c, tape).updates
    }

    #[test]
    fn policy_id_is_v1() {
        assert_eq!(POLICY_ID, "conditions-research-v1");
    }

    // One assertion per code per tape family: the v1 table, spelled out.
    #[test]
    fn cta_every_code() {
        for tape in [Tape::A, Tape::B] {
            for code in ["", " ", "@", "E", "F", "K", "L", "O", "X", "5", "6", "T"] {
                assert_eq!(upd(&[code], tape), Updates::FULL, "{code:?} {tape:?}");
            }
            for code in ["B", "C", "H", "I", "R", "U", "V", "7", "P", "Z", "4"] {
                assert_eq!(upd(&[code], tape), Updates::VOLUME_ONLY, "{code} {tape:?}");
            }
            for code in ["M", "Q", "9"] {
                assert_eq!(upd(&[code], tape), Updates::NONE, "{code} {tape:?}");
                assert!(!research_effect(&[code], tape).is_flagged());
            }
            for (code, reason) in [
                ("N", UnresolvedReason::Reserved),
                ("8", UnresolvedReason::Reserved),
                ("A", UnresolvedReason::NotDefinedForTape),
                ("D", UnresolvedReason::NotDefinedForTape),
                ("G", UnresolvedReason::NotDefinedForTape),
                ("S", UnresolvedReason::NotDefinedForTape),
                ("W", UnresolvedReason::NotDefinedForTape),
                ("Y", UnresolvedReason::NotDefinedForTape),
                ("1", UnresolvedReason::NotDefinedForTape),
                ("2", UnresolvedReason::NotDefinedForTape),
                ("3", UnresolvedReason::NotDefinedForTape),
                ("J", UnresolvedReason::NotDefinedForTape),
                ("b", UnresolvedReason::NotDefinedForTape),
            ] {
                assert_eq!(
                    code_verdict(code, tape),
                    Verdict::Unresolved { reason },
                    "{code} {tape:?}"
                );
                let e = research_effect(&[code], tape);
                assert_eq!(e.updates, Updates::NONE);
                assert_eq!(e.unresolved, vec![code.to_string()]);
            }
        }
    }

    #[test]
    fn utp_every_code() {
        let tape = Tape::C;
        for code in [
            "", " ", "@", "A", "B", "D", "F", "K", "L", "O", "X", "Y", "5", "6", "S", "1", "T",
        ] {
            assert_eq!(upd(&[code], tape), Updates::FULL, "{code:?}");
        }
        for code in ["C", "H", "I", "R", "U", "V", "W", "7", "G", "P", "Z", "4"] {
            assert_eq!(upd(&[code], tape), Updates::VOLUME_ONLY, "{code}");
        }
        for code in ["M", "Q", "9"] {
            assert_eq!(upd(&[code], tape), Updates::NONE, "{code}");
        }
        for (code, reason) in [
            ("N", UnresolvedReason::Reserved),
            ("E", UnresolvedReason::Placeholder),
            ("8", UnresolvedReason::Placeholder),
            ("2", UnresolvedReason::NotDefinedForTape),
            ("3", UnresolvedReason::NotDefinedForTape),
            ("J", UnresolvedReason::NotDefinedForTape),
        ] {
            assert_eq!(
                code_verdict(code, tape),
                Verdict::Unresolved { reason },
                "{code}"
            );
        }
    }

    #[test]
    fn otc_tape_is_unaudited() {
        assert_eq!(
            code_verdict("@", Tape::O),
            Verdict::Unresolved {
                reason: UnresolvedReason::TapeNotAudited
            }
        );
        // Every tape O print is excluded and flagged, with or without codes.
        assert_eq!(upd(&[], Tape::O), Updates::NONE);
        assert_eq!(
            research_effect(&[] as &[&str], Tape::O).unresolved,
            vec!["tape:O"]
        );
        assert_eq!(upd(&["@"], Tape::O), Updates::NONE);
    }

    #[test]
    fn unknown_tape_resolves_only_codes_that_agree() {
        for code in ["@", "F", "O", "X", "5", "6", "K", "L", "T"] {
            assert_eq!(upd(&[code], Tape::Unknown), Updates::FULL, "{code}");
        }
        for code in ["C", "H", "I", "R", "U", "V", "7", "P", "Z", "4"] {
            assert_eq!(upd(&[code], Tape::Unknown), Updates::VOLUME_ONLY, "{code}");
        }
        for code in ["M", "Q", "9"] {
            assert_eq!(upd(&[code], Tape::Unknown), Updates::NONE, "{code}");
        }
        // B: average price (CTA) vs bunched (UTP); E: auto-execution vs placeholder; A/D/G/W/Y/S/1
        // defined on UTP only.
        for code in ["B", "E", "A", "D", "G", "W", "Y", "S", "1"] {
            assert_eq!(
                code_verdict(code, Tape::Unknown),
                Verdict::Unresolved {
                    reason: UnresolvedReason::AmbiguousTape
                },
                "{code}"
            );
        }
        assert_eq!(
            code_verdict("N", Tape::Unknown),
            Verdict::Unresolved {
                reason: UnresolvedReason::Reserved
            }
        );
        assert_eq!(
            code_verdict("J", Tape::Unknown),
            Verdict::Unresolved {
                reason: UnresolvedReason::NotDefinedForTape
            }
        );
    }

    #[test]
    fn strictest_wins_and_unresolved_excludes_everything() {
        assert_eq!(upd(&["@", "F", "T"], Tape::C), Updates::FULL);
        let e = research_effect(&["@", "4", "I"], Tape::C);
        assert_eq!(e.updates, Updates::VOLUME_ONLY);
        assert_eq!(e.restricting, vec!["4".to_string(), "I".to_string()]);
        assert_eq!(upd(&["I", "M"], Tape::A), Updates::NONE);
        // An unresolved code next to a regular one still excludes price and volume.
        let e = research_effect(&["@", "E"], Tape::C);
        assert_eq!(e.updates, Updates::NONE);
        assert!(e.is_flagged());
        assert_eq!(e.unresolved, vec!["E".to_string()]);
        // Whitespace is trimmed.
        assert_eq!(upd(&[" F "], Tape::A), Updates::FULL);
    }

    #[test]
    fn update_marker_modes() {
        use market_events::{Price, UnixNanos};
        let t = Trade::new("X", UnixNanos(0), Price::from_cents(100), 100, 1)
            .with_tape(Tape::C)
            .with_conditions(["@"])
            .with_update("canceled");
        let pit = trade_research_effect(&t, UpdateMarkerMode::AsDelivered);
        assert_eq!(pit.updates, Updates::FULL);
        assert!(!pit.withdrawn);
        let hs = trade_research_effect(&t, UpdateMarkerMode::Hindsight);
        assert_eq!(hs.updates, Updates::NONE);
        assert!(hs.withdrawn);
        let corrected = t.clone().with_update("corrected");
        assert_eq!(
            trade_research_effect(&corrected, UpdateMarkerMode::Hindsight).updates,
            Updates::FULL
        );
    }

    #[test]
    fn every_resolved_audit_row_matches_production_and_v1_agrees_with_the_table() {
        let rows = audit_rows();
        assert_eq!(rows.len(), CTA_ROWS.len() + UTP_ROWS.len());
        for row in &rows {
            // Every row the spec defines with no TBD and no reserved meaning is resolved.
            let spec_defined = row
                .spec
                .iter()
                .all(|s| !matches!(s, Spec::Tbd | Spec::Undefined));
            if spec_defined {
                assert!(
                    matches!(row.verdict, Verdict::Resolved { .. }),
                    "{:?} {}",
                    row.family,
                    row.code
                );
            }
            // The audit found no resolved code where production disagrees.
            assert_ne!(row.class, Class::DiffersProductionLooser, "{}", row.code);
            assert_ne!(row.class, Class::DiffersProductionStricter, "{}", row.code);
            // Where Alpaca lists the code and v1 resolves it, v1 equals Alpaca's minute column.
            if let (Some(a), Verdict::Resolved { updates }) = (row.alpaca_minute, row.verdict) {
                assert_eq!(
                    [updates.last, updates.high_low, updates.volume],
                    a,
                    "{:?} {}",
                    row.family,
                    row.code
                );
            }
        }
        let unresolved: Vec<_> = rows
            .iter()
            .filter(|r| r.class == Class::Unresolved)
            .map(|r| format!("{:?}:{}", r.family, r.code))
            .collect();
        assert_eq!(
            unresolved,
            vec![
                "Cta:N", "Cta:8", "Cta:A", "Cta:D", "Cta:G", "Cta:S", "Cta:W", "Cta:Y", "Cta:1",
                "Utp:E", "Utp:N", "Utp:8"
            ]
        );
    }

    #[test]
    fn markdown_has_a_row_per_audited_code() {
        let md = audit_markdown();
        assert_eq!(md.lines().count(), 2 + CTA_ROWS.len() + UTP_ROWS.len());
    }
}
