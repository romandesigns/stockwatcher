//! Who hears about a fired alert: the "no spam" rules of the Strategy board, pure and tested.
//!
//! - **In app: always.** Every fire goes to every open socket and into Activity. (An alert whose
//!   "In app" chip is off still lands in Activity; it just shows no toast.)
//! - **Phone**, in this order (the first that applies holds it back, with that reason):
//!   1. the alert's "Phone" chip is off → `off` (not a hold-back: the person chose it);
//!   2. no phone registered for push → `no_device`;
//!   3. quiet hours (optional, ET wall time) → `quiet_hours`;
//!   4. the same stock got a phone notification less than [`SYMBOL_GAP_MS`] ago → `symbol_limit`,
//!      and it is counted into the next one for that stock ("+2 more");
//!   5. the daily budget (5/10/20/50, default 20, per ET calendar day) is spent → `budget`.
//!
//! Nothing is ever dropped silently: each hold-back becomes the event's `reason`, which the
//! Activity list shows.

use std::collections::HashMap;

use chrono::NaiveDate;

use super::model::Prefs;

/// At most one phone notification per stock per 5 minutes.
pub const SYMBOL_GAP_MS: i64 = 5 * 60_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phone {
    /// Hand it to the push service; `extra` = fires of this stock grouped into it.
    Send {
        extra: u32,
    },
    Off,
    Held(PhoneHold),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhoneHold {
    NoDevice,
    QuietHours,
    SymbolLimit,
    Budget,
}

impl PhoneHold {
    pub fn code(self) -> &'static str {
        match self {
            PhoneHold::NoDevice => "no_device",
            PhoneHold::QuietHours => "quiet_hours",
            PhoneHold::SymbolLimit => "symbol_limit",
            PhoneHold::Budget => "budget",
        }
    }
}

/// The phone side's memory: today's count, each stock's last notification, grouped fires.
#[derive(Debug, Default)]
pub struct Delivery {
    day: Option<NaiveDate>,
    sent_today: u32,
    last_by_symbol: HashMap<String, i64>,
    grouped: HashMap<String, u32>,
}

impl Delivery {
    /// Restores today's count (from Activity) after a restart, so a redeploy does not reset the budget.
    pub fn restore(
        &mut self,
        day: NaiveDate,
        sent_today: u32,
        last_by_symbol: HashMap<String, i64>,
    ) {
        self.day = Some(day);
        self.sent_today = sent_today;
        self.last_by_symbol = last_by_symbol;
    }

    pub fn sent_today(&self, today: NaiveDate) -> u32 {
        if self.day == Some(today) {
            self.sent_today
        } else {
            0
        }
    }

    /// The decision for one fire. `today` and `et_minute` are the ET calendar day and minute of
    /// the day at `now`; `devices` how many phones are registered.
    #[allow(clippy::too_many_arguments)]
    pub fn decide(
        &mut self,
        symbol: &str,
        wants_phone: bool,
        prefs: &Prefs,
        devices: usize,
        now: i64,
        today: NaiveDate,
        et_minute: u32,
    ) -> Phone {
        if self.day != Some(today) {
            self.day = Some(today);
            self.sent_today = 0;
        }
        if !wants_phone {
            return Phone::Off;
        }
        if devices == 0 {
            return Phone::Held(PhoneHold::NoDevice);
        }
        if prefs.quiet.is_some_and(|q| q.contains(et_minute)) {
            return Phone::Held(PhoneHold::QuietHours);
        }
        if self
            .last_by_symbol
            .get(symbol)
            .is_some_and(|t| now - t < SYMBOL_GAP_MS)
        {
            *self.grouped.entry(symbol.to_owned()).or_default() += 1;
            return Phone::Held(PhoneHold::SymbolLimit);
        }
        if self.sent_today >= prefs.phone_budget {
            return Phone::Held(PhoneHold::Budget);
        }
        self.sent_today += 1;
        self.last_by_symbol.insert(symbol.to_owned(), now);
        Phone::Send {
            extra: self.grouped.remove(symbol).unwrap_or(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alerts::model::QuietHours;

    fn day(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, d).unwrap()
    }

    #[test]
    fn budget_symbol_limit_quiet_hours() {
        let mut d = Delivery::default();
        let prefs = Prefs {
            phone_budget: 5,
            quiet: None,
            ..Prefs::default()
        };
        let at = |min: i64| min * 60_000;
        assert_eq!(d.decide("A", false, &prefs, 1, 0, day(15), 600), Phone::Off);
        assert_eq!(
            d.decide("A", true, &prefs, 0, 0, day(15), 600),
            Phone::Held(PhoneHold::NoDevice)
        );
        assert_eq!(
            d.decide("A", true, &prefs, 1, at(0), day(15), 600),
            Phone::Send { extra: 0 }
        );
        // The same stock within 5 minutes: held, and grouped into its next notification.
        assert_eq!(
            d.decide("A", true, &prefs, 1, at(2), day(15), 600),
            Phone::Held(PhoneHold::SymbolLimit)
        );
        assert_eq!(
            d.decide("A", true, &prefs, 1, at(3), day(15), 600),
            Phone::Held(PhoneHold::SymbolLimit)
        );
        assert_eq!(
            d.decide("A", true, &prefs, 1, at(6), day(15), 600),
            Phone::Send { extra: 2 }
        );
        // Other stocks use up the budget of 5.
        for s in ["B", "C", "D"] {
            assert!(matches!(
                d.decide(s, true, &prefs, 1, at(7), day(15), 600),
                Phone::Send { .. }
            ));
        }
        assert_eq!(d.sent_today(day(15)), 5);
        assert_eq!(
            d.decide("E", true, &prefs, 1, at(8), day(15), 600),
            Phone::Held(PhoneHold::Budget)
        );
        // A new ET day starts a new budget.
        assert!(matches!(
            d.decide("E", true, &prefs, 1, at(9), day(16), 600),
            Phone::Send { .. }
        ));
        assert_eq!(d.sent_today(day(15)), 0);
        // Quiet hours (22:00–07:00 ET).
        let quiet = Prefs {
            phone_budget: 20,
            quiet: Some(QuietHours {
                start_min: 22 * 60,
                end_min: 7 * 60,
            }),
            ..Prefs::default()
        };
        assert_eq!(
            d.decide("Q", true, &quiet, 1, at(100), day(16), 23 * 60),
            Phone::Held(PhoneHold::QuietHours)
        );
        assert!(matches!(
            d.decide("Q", true, &quiet, 1, at(101), day(16), 8 * 60),
            Phone::Send { .. }
        ));
    }
}
