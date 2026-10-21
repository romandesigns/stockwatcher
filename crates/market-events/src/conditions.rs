//! Trade-condition policy: which prints update a candle's OHLC and/or its volume.
//!
//! # Source
//!
//! Alpaca Market Data FAQ, "How are bars aggregated?"
//! (<https://docs.alpaca.markets/us/docs/market-data-faq#how-are-bars-aggregated>, read 2026-10-16).
//! The table there gives, per CTA (tapes A/B) and UTP (tape C) sale condition, whether a trade
//! updates high/low, open/close and volume of Alpaca's **minute** bars. This module encodes the
//! minute-bar column, because 15s/30s/1m/5m candles are all intraday bars and must match Alpaca's
//! own 1-minute bars for replay to be comparable with reference charts.
//!
//! - "A trade can have more than one condition. In this case the strictest rule is applied."
//!   Strictness: [`Rule::NoUpdate`] > [`Rule::VolumeOnly`] > [`Rule::Full`].
//! - Alpaca emits no bar when the interval's price fields would be empty; correspondingly the chart
//!   engine draws no candle for an interval holding only volume-only prints (it marks a gap).
//!
//! | Rule | Codes |
//! |---|---|
//! | Full (OHLC + volume) | regular sale (blank / `@`), `A` `D` `Y` (UTP), `B` on tape C (bunched), `E` `F` `K` `L` `O` `X` `5` `6`, `T` (form T) |
//! | Volume only | `B` on tapes A/B (average price), `C` `G` `H` `I` (odd lot) `N` `P` `R` `U` `V` `W` `Z` `4` `7` |
//! | Nothing | `M` (official close), `Q` (official open), `9` (corrected consolidated close) |
//!
//! # Decisions not covered by the source
//!
//! - **Unlisted codes** (e.g. `S`, `1`, `8`): Alpaca's table is silent. They are treated as
//!   non-restricting (a regular sale) and reported in [`Effect::unrecognized`] so the data-quality
//!   counters surface them. *Owner decision pending* if a stricter default is wanted.
//! - **`B` on an unknown tape** (e.g. a BOATS print whose tape is not reported): treated as the
//!   stricter CTA meaning, average price (volume only).
//! - Codes are matched exactly (case-sensitive, surrounding whitespace trimmed); `" "` and `""`
//!   are the CTA regular sale.

use serde::{Deserialize, Serialize};

/// This table's identity, for records that have to name the rules a print was judged by (the
/// server's admission journal). A changed table is a new identity, never an edit of this one:
/// prints admitted under the old rules counted differently and the two cannot be compared.
pub const POLICY: &str = "conditions-alpaca-minute-bars-v1";

/// SIP tape. `A`/`B` are CTA (NYSE / regional), `C` is UTP (Nasdaq), `O` is OTC in Alpaca's docs.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum Tape {
    A,
    B,
    C,
    O,
    #[default]
    #[serde(rename = "unknown", other)]
    Unknown,
}

impl Tape {
    /// Parses Alpaca's `"z"` field.
    pub fn from_code(code: &str) -> Tape {
        match code.trim() {
            "A" => Tape::A,
            "B" => Tape::B,
            "C" => Tape::C,
            "O" => Tape::O,
            _ => Tape::Unknown,
        }
    }
}

/// What a condition allows a print to update, ordered by strictness.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rule {
    Full,
    VolumeOnly,
    NoUpdate,
}

/// Why a trade has its effect.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum EffectReason {
    /// No restricting condition.
    Regular,
    /// Volume only, and the odd-lot flag `I` is the *only* restricting condition: the price is a
    /// real execution at the market, just under 100 shares.
    OddLot,
    /// The strictest restricting condition (the first such code in the trade's list; among
    /// volume-only codes a non-odd-lot code is preferred, because it is the one that says the price
    /// is not a market price).
    Condition {
        code: String,
        rule: Rule,
        description: String,
    },
    /// The provider later withdrew the print (historical `u` = `canceled` / `incorrect`); see
    /// `Trade::effect`. Never produced by [`trade_effect`], which sees conditions only.
    Withdrawn { update: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Effect {
    pub updates_ohlc: bool,
    pub updates_volume: bool,
    pub reason: EffectReason,
    /// Codes not in Alpaca's table; treated as non-restricting.
    pub unrecognized: Vec<String>,
}

impl Effect {
    pub fn rule(&self) -> Rule {
        match (self.updates_ohlc, self.updates_volume) {
            (true, _) => Rule::Full,
            (false, true) => Rule::VolumeOnly,
            (false, false) => Rule::NoUpdate,
        }
    }

    /// Short machine reason, e.g. `"odd_lot"`, `"condition_W"`; `None` for a regular sale.
    pub fn reason_code(&self) -> Option<String> {
        match &self.reason {
            EffectReason::Regular => None,
            EffectReason::OddLot => Some("odd_lot".to_string()),
            EffectReason::Condition { code, .. } => Some(format!("condition_{code}")),
            EffectReason::Withdrawn { update } => Some(format!("withdrawn_{update}")),
        }
    }
}

/// The minute-bar rule for one code on one tape, with Alpaca's description.
/// `None` means the code is not in Alpaca's table.
pub fn condition_rule(code: &str, tape: Tape) -> Option<(Rule, &'static str)> {
    use Rule::*;
    let r = match code.trim() {
        "" | "@" => (Full, "Regular Sale"),
        "A" => (Full, "Acquisition"),
        "B" if tape == Tape::C => (Full, "Bunched Trade"),
        "B" => (VolumeOnly, "Average Price Trade"),
        "C" => (VolumeOnly, "Cash Sale"),
        "D" => (Full, "Distribution"),
        "E" => (Full, "Automatic Execution"),
        "F" => (Full, "Intermarket Sweep"),
        "G" => (VolumeOnly, "Bunched Sold Trade"),
        "H" => (VolumeOnly, "Price Variation Trade"),
        "I" => (VolumeOnly, "Odd Lot Trade"),
        "K" => (Full, "Rule 127 or Rule 155"),
        "L" => (Full, "Sold Last"),
        "M" => (NoUpdate, "Market Center Official Close"),
        "N" => (VolumeOnly, "Next Day"),
        "O" => (Full, "Market Center Opening Trade"),
        "P" => (VolumeOnly, "Prior Reference Price"),
        "Q" => (NoUpdate, "Market Center Official Open"),
        "R" => (VolumeOnly, "Seller"),
        "T" => (Full, "Extended Hours Trade"),
        "U" => (VolumeOnly, "Extended Trading Hours"),
        "V" => (VolumeOnly, "Contingent Trade"),
        "W" => (VolumeOnly, "Average Price Trade"),
        "X" => (Full, "Cross Trade"),
        "Y" => (Full, "Yellow Flag Regular Trade"),
        "Z" => (VolumeOnly, "Sold Out Of Sequence"),
        "4" => (VolumeOnly, "Derivatively Priced"),
        "5" => (Full, "Market Center Reopening Trade"),
        "6" => (Full, "Market Center Closing Trade"),
        "7" => (VolumeOnly, "Qualified Contingent Trade"),
        "9" => (NoUpdate, "Corrected Consolidated Close"),
        _ => return None,
    };
    Some(r)
}

/// Combines a trade's conditions with the strictest-rule-wins policy.
pub fn trade_effect<S: AsRef<str>>(conditions: &[S], tape: Tape) -> Effect {
    let mut strictest = Rule::Full;
    let mut chosen: Option<(String, Rule, &'static str)> = None;
    let mut unrecognized = Vec::new();
    for c in conditions {
        let code = c.as_ref().trim();
        let Some((rule, desc)) = condition_rule(code, tape) else {
            unrecognized.push(code.to_string());
            continue;
        };
        if rule == Rule::Full {
            continue;
        }
        let replace = match &chosen {
            None => true,
            Some((prev_code, prev_rule, _)) => {
                rule > *prev_rule || (rule == *prev_rule && prev_code == "I" && code != "I")
            }
        };
        if replace {
            chosen = Some((code.to_string(), rule, desc));
        }
        strictest = strictest.max(rule);
    }
    let reason = match chosen {
        None => EffectReason::Regular,
        Some((code, Rule::VolumeOnly, _)) if code == "I" => EffectReason::OddLot,
        Some((code, rule, desc)) => EffectReason::Condition {
            code,
            rule,
            description: desc.to_string(),
        },
    };
    Effect {
        updates_ohlc: strictest == Rule::Full,
        updates_volume: strictest != Rule::NoUpdate,
        reason,
        unrecognized,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eff(c: &[&str], tape: Tape) -> (bool, bool, Option<String>) {
        let e = trade_effect(c, tape);
        (e.updates_ohlc, e.updates_volume, e.reason_code())
    }

    #[test]
    fn regular_sales_update_everything() {
        assert_eq!(eff(&[], Tape::A), (true, true, None));
        assert_eq!(eff(&[" "], Tape::A), (true, true, None));
        assert_eq!(eff(&["@"], Tape::C), (true, true, None));
        assert_eq!(eff(&["@", "F"], Tape::C), (true, true, None));
        assert_eq!(eff(&["@", "T"], Tape::C), (true, true, None)); // form T updates minute bars
        for code in ["A", "D", "Y", "E", "K", "L", "O", "X", "5", "6"] {
            assert_eq!(eff(&[code], Tape::C), (true, true, None), "{code}");
        }
    }

    #[test]
    fn nothing_codes() {
        for code in ["M", "Q", "9"] {
            assert_eq!(
                eff(&[code], Tape::A),
                (false, false, Some(format!("condition_{code}"))),
                "{code}"
            );
        }
    }

    #[test]
    fn volume_only_codes() {
        for code in ["C", "G", "H", "N", "P", "R", "U", "V", "W", "Z", "4", "7"] {
            assert_eq!(
                eff(&["@", code], Tape::C),
                (false, true, Some(format!("condition_{code}"))),
                "{code}"
            );
        }
        assert_eq!(
            eff(&["@", "I"], Tape::C),
            (false, true, Some("odd_lot".into()))
        );
    }

    #[test]
    fn b_depends_on_tape() {
        assert_eq!(
            eff(&["B"], Tape::A),
            (false, true, Some("condition_B".into()))
        );
        assert_eq!(
            eff(&["B"], Tape::B),
            (false, true, Some("condition_B".into()))
        );
        assert_eq!(eff(&["B"], Tape::C), (true, true, None));
        assert_eq!(
            eff(&["B"], Tape::Unknown),
            (false, true, Some("condition_B".into()))
        );
    }

    #[test]
    fn strictest_rule_wins() {
        // Alpaca's own example: "@", "4", "I" on Nasdaq updates volume only.
        assert_eq!(
            eff(&["@", "4", "I"], Tape::C),
            (false, true, Some("condition_4".into()))
        );
        // Order does not matter for the non-odd-lot preference.
        assert_eq!(
            eff(&["I", "4"], Tape::C),
            (false, true, Some("condition_4".into()))
        );
        assert_eq!(
            eff(&["I", "M"], Tape::A),
            (false, false, Some("condition_M".into()))
        );
        assert_eq!(
            eff(&["Z", "9"], Tape::A),
            (false, false, Some("condition_9".into()))
        );
    }

    #[test]
    fn unrecognized_codes_are_reported_not_restricting() {
        let e = trade_effect(&["@", "S", "1"], Tape::C);
        assert!(e.updates_ohlc && e.updates_volume);
        assert_eq!(e.unrecognized, vec!["S".to_string(), "1".to_string()]);
    }

    #[test]
    fn tape_codes() {
        assert_eq!(Tape::from_code("C"), Tape::C);
        assert_eq!(Tape::from_code("x"), Tape::Unknown);
        assert_eq!(
            serde_json::to_string(&Tape::Unknown).unwrap(),
            "\"unknown\""
        );
        assert_eq!(
            serde_json::from_str::<Tape>("\"Q\"").unwrap(),
            Tape::Unknown
        );
    }
}
