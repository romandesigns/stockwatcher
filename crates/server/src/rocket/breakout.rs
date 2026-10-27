//! Exact, source-backed momentum highs. Frozen model bars remain f32.
//!
//! A row starts an empty accumulator at its actual creation decision, not its event label.
//! Only later admissions whose execution is strictly after that decision may contribute.
//! Completed keys are consumed once; a late revision cannot create a retrospective notice.

use std::collections::BTreeMap;

use market_events::Trade;
use serde::{Deserialize, Serialize};

use super::features::BAR_NS;

pub const POLICY_ID: &str = "rocket-momentum-15s-exact-after-v1";
pub const RULE: &str = "post_flag_exact_high_v1";

/// Immutable measurements for a confirmed 15-second momentum breakout.
/// Nanosecond clocks and admission counters are strings on client transports.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BreakoutNotice {
    pub v: u8,
    pub rule: String,
    pub policy_id: String,
    pub timeframe_secs: u32,
    pub event_time_basis: String,
    pub volume_scope: String,
    pub source: String,
    pub exact_state: String,
    pub bar_start_ms: i64,
    pub bar_end_ms: i64,
    pub event_at_ms: i64,
    pub evaluated_at_ms: i64,
    pub flag_at_ms: i64,
    pub flag_known_at_ms: i64,
    pub high_exec_ms: i64,
    pub reference_price_micros: i64,
    pub high_price_micros: i64,
    pub close_price_micros: Option<i64>,
    pub high_receive_ms: Option<i64>,
    pub flag_known_ns: String,
    pub evaluated_ns: String,
    pub high_exec_ns: String,
    pub high_receive_ns: Option<String>,
    pub flag_creation_admission: String,
    pub decision_admission: String,
    pub high_admission: String,
    pub reference_known_at_ms: i64,
    pub reference_known_ns: String,
    pub reference_basis: String,
    pub reference_bar_start_ms: Option<i64>,
    pub reference_bar_end_ms: Option<i64>,
    pub volume: f64,
    pub pace_slot: f64,
    pub required_multiple: f64,
    pub volume_ratio: Option<f64>,
    pub zero_base_fallback: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Print {
    pub micros: i64,
    pub exec_ns: i64,
    pub receive_ns: Option<i64>,
    pub admission: u64,
}

impl Print {
    pub fn from_trade(t: &Trade, admission: u64) -> Self {
        Self {
            micros: t.price.micros(),
            exec_ns: t.event_ts.0,
            receive_ns: t.receive_ts.map(|t| t.0),
            admission,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Fresh {
    pub key: u32,
    pub reference: i64,
    pub reference_known_ns: i64,
    pub reference_bar_key: Option<u32>,
    pub high: Print,
}

/// Candidate-bar measurements, separate from the clock and admission decision that reads them.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BarContext {
    pub key: u32,
    pub end_ns: i64,
    pub close_price_micros: Option<i64>,
    pub volume: f64,
    pub pace_slot: f64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct DecisionContext {
    pub evaluated_ns: i64,
    pub flag_at_ns: i64,
    pub admission: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Exact {
    pub flag_known_ns: i64,
    pub creation_admission: u64,
    pub admission_cutoff: u64,
    pub checked_key: u32,
    pub creation_key: u32,
    pub flag_price_micros: i64,
    pub high_price_micros: i64,
    pub reference_known_ns: i64,
    pub reference_bar_key: Option<u32>,
    pub pending: BTreeMap<u32, Print>,
    pub complete: bool,
    pub creation_had_ahead: bool,
    pub restored: bool,
    #[serde(skip)]
    pub rebuild_ready: bool,
    pub unknown_reason: Option<String>,
    #[serde(skip)]
    pub fresh: Option<Fresh>,
    /// Latest admitted OHLC source for provisional display, never inferred from a quote.
    #[serde(skip)]
    pub live: Option<(u32, Print)>,
}

impl Exact {
    pub fn new(known: i64, admission: u64, checked: u32, price: i64, ahead: bool) -> Self {
        Self {
            flag_known_ns: known,
            creation_admission: admission,
            admission_cutoff: admission,
            checked_key: checked,
            creation_key: checked.saturating_add(1),
            flag_price_micros: price,
            high_price_micros: price,
            reference_known_ns: known,
            reference_bar_key: None,
            pending: BTreeMap::new(),
            complete: true,
            creation_had_ahead: ahead,
            restored: false,
            rebuild_ready: false,
            unknown_reason: None,
            fresh: None,
            live: None,
        }
    }

    pub fn observe(&mut self, key: u32, print: Print) {
        self.admission_cutoff = self.admission_cutoff.max(print.admission);
        let clock_pending =
            self.unknown_reason.as_deref() == Some("future_barrier_ahead_of_decision_clock");
        self.live = (self.complete
            && key > self.checked_key
            && print.admission > self.creation_admission
            && print.exec_ns > self.flag_known_ns)
            .then_some((key, print));
        if (!self.complete && !clock_pending)
            || print.admission <= self.creation_admission
            || print.exec_ns <= self.flag_known_ns
        {
            return;
        }
        // A malformed future tape cannot grow per-row state without bound. Normal feed
        // skew occupies one or two keys; an overflow withholds strict claims explicitly.
        if !self.pending.contains_key(&key) && self.pending.len() >= super::features::RING {
            self.complete = false;
            self.unknown_reason = Some("pending_bar_capacity_exceeded".into());
            self.pending.clear();
            return;
        }
        let high = self.pending.entry(key).or_insert(print);
        if print.micros > high.micros {
            *high = print;
        }
    }

    pub fn fold(&mut self, through: u32, evaluated: i64) -> Vec<u32> {
        self.fresh = None;
        if !self.complete {
            return Vec::new();
        }
        let mut highs = Vec::new();
        while let Some((&key, _)) = self.pending.first_key_value() {
            if key > through {
                break;
            }
            let high = self.pending.remove(&key).expect("first pending key");
            if self.complete && high.micros > self.high_price_micros {
                if key > self.checked_key {
                    self.fresh = Some(Fresh {
                        key,
                        reference: self.high_price_micros,
                        reference_known_ns: self.reference_known_ns,
                        reference_bar_key: self.reference_bar_key,
                        high,
                    });
                    highs.push(key);
                }
                self.high_price_micros = high.micros;
                self.reference_known_ns = evaluated;
                self.reference_bar_key = Some(key);
            }
        }
        self.checked_key = self.checked_key.max(through);
        if self.live.is_some_and(|(key, _)| key <= self.checked_key) {
            self.live = None;
        }
        highs
    }

    pub fn notice(&self, bar: BarContext, decision: DecisionContext) -> Option<BreakoutNotice> {
        let BarContext {
            key,
            end_ns: end,
            close_price_micros: close,
            volume,
            pace_slot: pace,
        } = bar;
        let DecisionContext {
            evaluated_ns: evaluated,
            flag_at_ns: flag_at,
            admission,
        } = decision;
        let f = self.fresh.filter(|f| self.complete && f.key == key)?;
        if evaluated < end || f.high.exec_ns < end - BAR_NS || f.high.exec_ns >= end {
            return None;
        }
        let reference_end = f
            .reference_bar_key
            .map(|k| end - i64::from(key - k) * BAR_NS);
        Some(BreakoutNotice {
            v: 1,
            rule: RULE.into(),
            policy_id: POLICY_ID.into(),
            timeframe_secs: 15,
            event_time_basis: "bar_end".into(),
            volume_scope: "whole_15s_bar".into(),
            source: "accepted_original_ohlc".into(),
            exact_state: if self.restored {
                "restored"
            } else {
                "complete"
            }
            .into(),
            bar_start_ms: (end - BAR_NS) / 1_000_000,
            bar_end_ms: end / 1_000_000,
            event_at_ms: end / 1_000_000,
            evaluated_at_ms: evaluated / 1_000_000,
            flag_at_ms: flag_at / 1_000_000,
            flag_known_at_ms: self.flag_known_ns / 1_000_000,
            high_exec_ms: f.high.exec_ns / 1_000_000,
            reference_price_micros: f.reference,
            high_price_micros: f.high.micros,
            close_price_micros: close,
            high_receive_ms: f.high.receive_ns.map(|n| n / 1_000_000),
            flag_known_ns: self.flag_known_ns.to_string(),
            evaluated_ns: evaluated.to_string(),
            high_exec_ns: f.high.exec_ns.to_string(),
            high_receive_ns: f.high.receive_ns.map(|n| n.to_string()),
            flag_creation_admission: self.creation_admission.to_string(),
            decision_admission: admission.to_string(),
            high_admission: f.high.admission.to_string(),
            reference_known_at_ms: f.reference_known_ns / 1_000_000,
            reference_known_ns: f.reference_known_ns.to_string(),
            reference_basis: if f.reference_bar_key.is_some() {
                "eligible_bar_high"
            } else {
                "flag_price"
            }
            .into(),
            reference_bar_start_ms: reference_end.map(|t| (t - BAR_NS) / 1_000_000),
            reference_bar_end_ms: reference_end.map(|t| t / 1_000_000),
            volume,
            pace_slot: pace,
            required_multiple: super::track::BREAK_PACE,
            volume_ratio: (pace > 0.0).then(|| volume / pace),
            zero_base_fallback: pace <= 0.0,
        })
    }
}

/// Only for paths explicitly proven to carry Price::to_f64(), never f32 model prices or gains.
pub(crate) fn original_micros(price: f64) -> Option<i64> {
    if !price.is_finite() || price <= 0.0 || price > 1_000_000.0 {
        return None;
    }
    let micros = (price * 1_000_000.0).round() as i64;
    (micros as f64 / 1_000_000.0 == price).then_some(micros)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn print(micros: i64, exec_ns: i64, admission: u64) -> Print {
        Print {
            micros,
            exec_ns,
            receive_ns: None,
            admission,
        }
    }

    #[test]
    fn exact_source_prices_distinguish_equal_and_every_micro_increment() {
        for price in [
            100_000, 110_000, 200_000, 300_000, 500_000, 1_250_000, 1_830_000, 1_840_000,
            8_010_000, 16_000_000, 16_000_001, 16_000_002, 24_990_000, 24_999_999, 25_000_000,
        ] {
            for delta in [-10_000, -100, -1, 0, 1, 100, 10_000] {
                let mut e = Exact::new(10, 7, 100, price, false);
                e.observe(101, print(price + delta, 11, 8));
                let highs = e.fold(101, BAR_NS);
                assert_eq!(highs == [101], delta > 0, "{price} {delta}");
                assert_eq!(e.high_price_micros, price.max(price + delta));
            }
        }
    }

    #[test]
    fn execution_tie_and_precreation_admission_do_not_enter_the_suffix() {
        let mut e = Exact::new(10, 7, 100, 1_250_000, false);
        e.observe(101, print(1_900_000, 11, 7)); // future-stamped, already admitted at creation
        e.observe(101, print(1_800_000, 9, 8)); // late preflag execution
        e.observe(101, print(1_700_000, 10, 9)); // exact known-at tie
        e.observe(101, print(1_350_000, 11, 10));
        e.observe(101, print(1_250_000, 12, 11)); // retreat leaves the eligible high intact
        assert_eq!(e.fold(101, BAR_NS), [101]);
        assert_eq!(e.high_price_micros, 1_350_000);
        assert_eq!(e.fresh.unwrap().high.admission, 10);
    }

    #[test]
    fn tied_precreation_and_postcreation_high_retains_the_postcreation_witness() {
        let mut e = Exact::new(10, 7, 100, 1_250_000, false);
        e.observe(101, print(1_350_000, 9, 7));
        e.observe(101, print(1_350_000, 11, 8));
        assert_eq!(e.fold(101, BAR_NS), [101]);
        assert_eq!(e.fresh.unwrap().high.exec_ns, 11);
    }

    #[test]
    fn accepted_late_revision_improves_floor_without_retroactive_fresh_high() {
        let mut e = Exact::new(10, 7, 100, 1_250_000, false);
        e.fold(101, BAR_NS);
        e.observe(101, print(1_400_000, 11, 8));
        e.observe(102, print(1_350_000, BAR_NS + 1, 9));
        assert!(e.fold(102, 2 * BAR_NS).is_empty());
        assert_eq!(e.high_price_micros, 1_400_000);
        assert!(e.fresh.is_none());
        e.observe(103, print(1_400_001, 2 * BAR_NS + 1, 10));
        assert_eq!(e.fold(103, 3 * BAR_NS), [103]);
        assert_eq!(e.fresh.unwrap().reference, 1_400_000);
    }

    #[test]
    fn immutable_evidence_freezes_previous_floor_actual_high_and_all_clocks() {
        let mut e = Exact::new(10, 7, 100, 1_830_000, false);
        let mut high = print(1_830_001, 11, 8);
        high.receive_ns = Some(13);
        e.observe(101, high);
        e.fold(101, BAR_NS + 250_000_000);
        let b = e
            .notice(
                BarContext {
                    key: 101,
                    end_ns: BAR_NS,
                    close_price_micros: Some(1_830_000),
                    volume: 200.0,
                    pace_slot: 100.0,
                },
                DecisionContext {
                    evaluated_ns: BAR_NS + 250_000_000,
                    flag_at_ns: 0,
                    admission: 9,
                },
            )
            .unwrap();
        assert_eq!(b.reference_price_micros, 1_830_000);
        assert_eq!(b.high_price_micros, 1_830_001);
        assert_eq!(b.close_price_micros, Some(1_830_000));
        assert_eq!(b.high_exec_ns, "11");
        assert_eq!(b.high_receive_ns.as_deref(), Some("13"));
        assert_eq!(b.flag_known_ns, "10");
        assert_eq!(b.flag_creation_admission, "7");
        assert_eq!(b.high_admission, "8");
        assert_eq!(b.decision_admission, "9");
        assert_eq!(b.volume_ratio, Some(2.0));
        assert!(!b.zero_base_fallback);
        assert_eq!(b.reference_basis, "flag_price");
        e.observe(102, print(1_840_000, BAR_NS + 1, 10));
        e.fold(102, 2 * BAR_NS);
        let b = e
            .notice(
                BarContext {
                    key: 102,
                    end_ns: 2 * BAR_NS,
                    close_price_micros: None,
                    volume: 1.0,
                    pace_slot: 0.0,
                },
                DecisionContext {
                    evaluated_ns: 2 * BAR_NS,
                    flag_at_ns: 0,
                    admission: 10,
                },
            )
            .unwrap();
        assert_eq!(b.reference_price_micros, 1_830_001);
        assert_eq!(b.reference_known_at_ms, 15_250);
        assert_eq!(b.reference_bar_start_ms, Some(0));
        assert_eq!(b.reference_bar_end_ms, Some(15_000));
        assert_eq!(b.high_receive_ns, None);
        assert!(b.zero_base_fallback);
        assert_eq!(b.volume_ratio, None);
    }

    #[test]
    fn original_float_conversion_is_not_a_recovery_path_for_narrowed_prices() {
        assert_eq!(original_micros(1.83), Some(1_830_000));
        assert_eq!(original_micros(16.000001), Some(16_000_001));
        assert_eq!(original_micros(f64::from(1.83f32)), None);
        assert_eq!(original_micros(f64::NAN), None);
    }
}
