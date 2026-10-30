//! Which orders Alpaca takes when. In regular hours (9:30–16:00 ET on trading days) every type
//! goes as is. Outside them Alpaca accepts only `limit` orders with `extended_hours: true` and
//! `time_in_force: day` (pre-market, after-hours and the overnight session); it rejects every other
//! type, and brackets never run in extended hours. We refuse those ourselves, before sending,
//! with a reason the user can act on.
//!
//! "Regular hours now" comes from Alpaca's own `/v2/clock` (it knows holidays and half days),
//! cached until its next open/close. Without a clock reading the fallback is the plain ET rule:
//! weekdays 9:30–16:00.

use chrono::{Datelike, TimeZone, Timelike, Weekday};
use paper_broker::OrderType;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Session {
    Regular,
    /// Pre-market, after-hours, overnight, weekends and holidays.
    Extended,
}

/// Why an order cannot go out in this session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionRefusal {
    NotLimit,
    Bracket,
}

impl SessionRefusal {
    pub fn code(self) -> &'static str {
        match self {
            SessionRefusal::NotLimit => "extended_hours_limit_only",
            SessionRefusal::Bracket => "extended_hours_no_bracket",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            SessionRefusal::NotLimit => {
                "Outside regular hours (9:30-16:00 ET) Alpaca accepts only limit orders; market, stop and stop-limit orders are refused"
            }
            SessionRefusal::Bracket => {
                "Bracket orders run in regular hours only (9:30-16:00 ET) at Alpaca"
            }
        }
    }
}

/// `extended_hours` for the order, or why it cannot be sent now.
pub fn route(
    session: Session,
    order_type: OrderType,
    bracket: bool,
) -> Result<bool, SessionRefusal> {
    match session {
        Session::Regular => Ok(false),
        Session::Extended if bracket => Err(SessionRefusal::Bracket),
        Session::Extended if order_type != OrderType::Limit => Err(SessionRefusal::NotLimit),
        Session::Extended => Ok(true),
    }
}

/// The fallback when Alpaca's clock is unavailable: weekdays 9:30 to 16:00 New York time.
pub fn session_by_wall_clock(now_ms: i64) -> Session {
    let Some(t) = chrono_tz::America::New_York
        .timestamp_millis_opt(now_ms)
        .single()
    else {
        return Session::Extended;
    };
    let weekday = !matches!(t.weekday(), Weekday::Sat | Weekday::Sun);
    let mins = t.hour() * 60 + t.minute();
    if weekday && (9 * 60 + 30..16 * 60).contains(&mins) {
        Session::Regular
    } else {
        Session::Extended
    }
}

/// A `/v2/clock` reading: the session it reported and until when that holds (its next close
/// while open, its next open while closed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockReading {
    pub session: Session,
    pub valid_until_ms: i64,
}

impl ClockReading {
    pub fn from_alpaca(c: &crate::alpaca::AlpacaClock) -> Self {
        let until = if c.is_open {
            crate::alpaca::ms(c.next_close.as_deref())
        } else {
            crate::alpaca::ms(c.next_open.as_deref())
        };
        ClockReading {
            session: if c.is_open {
                Session::Regular
            } else {
                Session::Extended
            },
            valid_until_ms: until,
        }
    }

    /// The session at `now_ms`, if this reading still covers it.
    pub fn at(&self, now_ms: i64) -> Option<Session> {
        (now_ms < self.valid_until_ms).then_some(self.session)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alpaca::AlpacaClock;

    fn et(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
        chrono_tz::America::New_York
            .with_ymd_and_hms(y, mo, d, h, mi, 0)
            .single()
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn regular_hours_take_every_type_as_is() {
        for t in [
            OrderType::Market,
            OrderType::Limit,
            OrderType::Stop,
            OrderType::StopLimit,
        ] {
            assert_eq!(route(Session::Regular, t, false), Ok(false));
            assert_eq!(route(Session::Regular, t, true), Ok(false));
        }
    }

    #[test]
    fn extended_hours_take_only_plain_limits_with_the_flag() {
        assert_eq!(route(Session::Extended, OrderType::Limit, false), Ok(true));
        for t in [OrderType::Market, OrderType::Stop, OrderType::StopLimit] {
            assert_eq!(
                route(Session::Extended, t, false),
                Err(SessionRefusal::NotLimit)
            );
        }
        assert_eq!(
            route(Session::Extended, OrderType::Limit, true),
            Err(SessionRefusal::Bracket)
        );
    }

    #[test]
    fn the_wall_clock_fallback_is_weekdays_930_to_1600_new_york() {
        // Wednesday 2026-10-28.
        assert_eq!(
            session_by_wall_clock(et(2026, 10, 28, 9, 29)),
            Session::Extended
        );
        assert_eq!(
            session_by_wall_clock(et(2026, 10, 28, 9, 30)),
            Session::Regular
        );
        assert_eq!(
            session_by_wall_clock(et(2026, 10, 28, 15, 59)),
            Session::Regular
        );
        assert_eq!(
            session_by_wall_clock(et(2026, 10, 28, 16, 0)),
            Session::Extended
        );
        assert_eq!(
            session_by_wall_clock(et(2026, 10, 28, 4, 30)),
            Session::Extended
        );
        // Saturday.
        assert_eq!(
            session_by_wall_clock(et(2026, 10, 31, 11, 0)),
            Session::Extended
        );
    }

    #[test]
    fn a_clock_reading_holds_until_its_next_transition() {
        let c = AlpacaClock {
            is_open: true,
            next_open: Some("2026-10-29T09:30:00-04:00".into()),
            next_close: Some("2026-10-28T16:00:00-04:00".into()),
        };
        let r = ClockReading::from_alpaca(&c);
        assert_eq!(r.at(et(2026, 10, 28, 15, 0)), Some(Session::Regular));
        assert_eq!(r.at(et(2026, 10, 28, 16, 0)), None);
    }
}
