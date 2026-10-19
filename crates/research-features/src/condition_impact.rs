//! Quantifies what the research condition policy changes on real data: per (tape, code), how many
//! prints and shares carry it, how many v1 excludes as unresolved, and how many prints v1 and
//! production treat differently. Pure: feed it trades, read the report.

use crate::conditions::{POLICY_ID, Updates, production_updates, research_effect};
use market_events::{Tape, Trade, trade_effect};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct CodeStats {
    /// Prints carrying the code.
    pub prints: u64,
    pub shares: u64,
    /// Prints carrying the code that v1 excluded because of an unresolved code (this one or
    /// another on the same print).
    pub excluded_prints: u64,
    pub excluded_shares: u64,
    /// Whether production knows the code on this tape.
    pub production_known: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ImpactReport {
    pub policy: String,
    pub prints: u64,
    pub shares: u64,
    /// Prints v1 flagged (any unresolved code).
    pub flagged_prints: u64,
    pub flagged_shares: u64,
    /// Prints whose price eligibility (last) differs between production and v1.
    pub price_disagreements: u64,
    /// Prints whose volume eligibility differs between production and v1.
    pub volume_disagreements: u64,
    /// Keyed `"<tape>:<code>"`, code `" "`-trimmed (`""` = an empty code string).
    pub codes: BTreeMap<String, CodeStats>,
    /// Keyed by symbol: flagged prints per symbol (only symbols with any).
    pub flagged_by_symbol: BTreeMap<String, u64>,
}

fn tape_key(t: Tape) -> &'static str {
    match t {
        Tape::A => "A",
        Tape::B => "B",
        Tape::C => "C",
        Tape::O => "O",
        Tape::Unknown => "unknown",
    }
}

impl ImpactReport {
    pub fn new() -> Self {
        ImpactReport {
            policy: POLICY_ID.to_string(),
            ..Default::default()
        }
    }

    /// Counts one print. Provider `u` markers are ignored: this measures the condition table only.
    pub fn add(&mut self, t: &Trade) {
        let research = research_effect(&t.conditions, t.tape);
        let prod = trade_effect(&t.conditions, t.tape);
        let prod_u = Updates {
            last: prod.updates_ohlc,
            high_low: prod.updates_ohlc,
            volume: prod.updates_volume,
        };
        self.prints += 1;
        self.shares += t.size;
        let flagged = research.is_flagged();
        if flagged {
            self.flagged_prints += 1;
            self.flagged_shares += t.size;
            *self.flagged_by_symbol.entry(t.symbol.clone()).or_default() += 1;
        }
        if prod_u.last != research.updates.last {
            self.price_disagreements += 1;
        }
        if prod_u.volume != research.updates.volume {
            self.volume_disagreements += 1;
        }
        let mut seen: Vec<&str> = Vec::new();
        for c in &t.conditions {
            let code = c.trim();
            if seen.contains(&code) {
                continue;
            }
            seen.push(code);
            let s = self
                .codes
                .entry(format!("{}:{code}", tape_key(t.tape)))
                .or_default();
            s.prints += 1;
            s.shares += t.size;
            s.production_known = production_updates(code, t.tape).is_some();
            if flagged {
                s.excluded_prints += 1;
                s.excluded_shares += t.size;
            }
        }
    }

    pub fn merge(&mut self, o: &ImpactReport) {
        self.prints += o.prints;
        self.shares += o.shares;
        self.flagged_prints += o.flagged_prints;
        self.flagged_shares += o.flagged_shares;
        self.price_disagreements += o.price_disagreements;
        self.volume_disagreements += o.volume_disagreements;
        for (k, v) in &o.codes {
            let s = self.codes.entry(k.clone()).or_default();
            s.prints += v.prints;
            s.shares += v.shares;
            s.excluded_prints += v.excluded_prints;
            s.excluded_shares += v.excluded_shares;
            s.production_known = v.production_known;
        }
        for (k, v) in &o.flagged_by_symbol {
            *self.flagged_by_symbol.entry(k.clone()).or_default() += v;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use market_events::{Price, UnixNanos};

    fn t(c: &[&str], tape: Tape, size: u64) -> Trade {
        Trade::new("X", UnixNanos(0), Price::from_cents(100), size, 1)
            .with_tape(tape)
            .with_conditions(c.iter().copied())
    }

    #[test]
    fn counts_flags_and_disagreements() {
        let mut r = ImpactReport::new();
        r.add(&t(&["@"], Tape::C, 100));
        r.add(&t(&["@", "E"], Tape::C, 50)); // unresolved on C: production full, v1 nothing
        r.add(&t(&["@", "I"], Tape::C, 7));
        r.add(&t(&["N"], Tape::A, 10)); // production volume only, v1 nothing
        assert_eq!(r.prints, 4);
        assert_eq!(r.shares, 167);
        assert_eq!(r.flagged_prints, 2);
        assert_eq!(r.flagged_shares, 60);
        assert_eq!(r.price_disagreements, 1);
        assert_eq!(r.volume_disagreements, 2);
        assert_eq!(r.codes["C:E"].excluded_prints, 1);
        assert!(r.codes["C:E"].production_known); // production knows E (as CTA's meaning)
        assert_eq!(r.codes["C:@"].prints, 3);
        assert_eq!(r.codes["C:@"].excluded_prints, 1);
        assert_eq!(r.flagged_by_symbol["X"], 2);
        let mut m = ImpactReport::new();
        m.merge(&r);
        m.merge(&r);
        assert_eq!(m.prints, 8);
        assert_eq!(m.codes["A:N"].excluded_shares, 20);
    }
}
