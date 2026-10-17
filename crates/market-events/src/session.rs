//! US equity session calendar in `America/New_York`.
//!
//! # Sessions (Eastern Time, half-open)
//!
//! | Session     | Normal day        | Early-close day   |
//! |-------------|-------------------|-------------------|
//! | `overnight` | 20:00 (prev. calendar day) – 04:00 | same |
//! | `pre`       | 04:00 – 09:30     | 04:00 – 09:30     |
//! | `regular`   | 09:30 – 16:00     | 09:30 – 13:00     |
//! | `after`     | 16:00 – 20:00     | 13:00 – 17:00     |
//!
//! On an early-close day 17:00–20:00 belongs to **no** session (NYSE's 2026 calendar: the late
//! sessions of NYSE Arca/American/National/Texas close at 17:00 on those days).
//!
//! # Session ids and the overnight rule
//!
//! A session id is `"<trading date>/<kind>"`, e.g. `"2026-10-15/regular"`. **The overnight session
//! belongs to the NEXT trading day**: the window from 20:00 ET on calendar day D-1 to 04:00 ET on
//! day D is `"D/overnight"`, and it exists only when D is a trading day. So Sunday 20:00 opens
//! Monday's overnight session; Friday 20:00 is not a session (Saturday is not a trading day); the
//! evening before a holiday is not a session. This mirrors how Blue Ocean (BOATS) prints are
//! booked to the following trade date. **Assumption to verify against live BOATS data**: that Blue
//! Ocean follows the NYSE holiday table exactly (it is an ATS with its own schedule).
//!
//! # DST
//!
//! Every conversion goes through `chrono-tz`'s `America/New_York` rules. All session boundaries
//! fall at 04:00 or later, and US DST changes happen at 02:00 on a Sunday, so no boundary is ever
//! ambiguous or skipped; the UTC offset of each boundary simply follows the date (EDT -4h / EST -5h).
//!
//! # Holidays and early closes
//!
//! Source: NYSE "Holidays and trading hours" (nyse.com/markets/hours-calendars), read 2026-10-16
//! for 2026–2027. 2025 is NYSE's published 2025 calendar, including the 2025-01-09 closure for the
//! National Day of Mourning for President Carter. Dates outside 2025–2027 are not covered:
//! [`calendar_covers`] tells callers, and uncovered weekdays are treated as full trading days.

use crate::time::UnixNanos;
use chrono::{Datelike, Duration, NaiveDate, NaiveTime, TimeZone, Timelike, Weekday};
use chrono_tz::America::New_York;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// Session kinds, in chronological order within a trading date (the derived `Ord` relies on it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionKind {
    Overnight,
    Pre,
    Regular,
    After,
}

impl SessionKind {
    pub const ALL: [SessionKind; 4] = [
        SessionKind::Overnight,
        SessionKind::Pre,
        SessionKind::Regular,
        SessionKind::After,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            SessionKind::Overnight => "overnight",
            SessionKind::Pre => "pre",
            SessionKind::Regular => "regular",
            SessionKind::After => "after",
        }
    }
}

impl fmt::Display for SessionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `"<trading date>/<kind>"`. Ordered chronologically (date, then kind).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId {
    pub date: NaiveDate,
    pub kind: SessionKind,
}

impl SessionId {
    pub fn new(date: NaiveDate, kind: SessionKind) -> Self {
        SessionId { date, kind }
    }

    /// UTC bounds `[start, end)`, or `None` if this session does not exist on that date.
    pub fn bounds(&self) -> Option<(UnixNanos, UnixNanos)> {
        session_bounds(*self)
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.date.format("%Y-%m-%d"), self.kind)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionIdParseError(pub String);

impl fmt::Display for SessionIdParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid session id {:?}", self.0)
    }
}

impl std::error::Error for SessionIdParseError {}

impl FromStr for SessionId {
    type Err = SessionIdParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || SessionIdParseError(s.to_string());
        let (d, k) = s.split_once('/').ok_or_else(err)?;
        let date = NaiveDate::parse_from_str(d, "%Y-%m-%d").map_err(|_| err())?;
        let kind = match k {
            "overnight" => SessionKind::Overnight,
            "pre" => SessionKind::Pre,
            "regular" => SessionKind::Regular,
            "after" => SessionKind::After,
            _ => return Err(err()),
        };
        Ok(SessionId { date, kind })
    }
}

impl Serialize for SessionId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for SessionId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// What kind of day a calendar date is for US equities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DayKind {
    Weekend,
    Holiday(&'static str),
    /// Regular session closes at 13:00 ET.
    EarlyClose(&'static str),
    Full,
}

const HOLIDAYS: &[(i32, u32, u32, &str)] = &[
    (2025, 1, 1, "New Year's Day"),
    (2025, 1, 9, "National Day of Mourning (President Carter)"),
    (2025, 1, 20, "Martin Luther King, Jr. Day"),
    (2025, 2, 17, "Washington's Birthday"),
    (2025, 4, 18, "Good Friday"),
    (2025, 5, 26, "Memorial Day"),
    (2025, 6, 19, "Juneteenth"),
    (2025, 7, 4, "Independence Day"),
    (2025, 9, 1, "Labor Day"),
    (2025, 11, 27, "Thanksgiving Day"),
    (2025, 12, 25, "Christmas Day"),
    (2026, 1, 1, "New Year's Day"),
    (2026, 1, 19, "Martin Luther King, Jr. Day"),
    (2026, 2, 16, "Washington's Birthday"),
    (2026, 4, 3, "Good Friday"),
    (2026, 5, 25, "Memorial Day"),
    (2026, 6, 19, "Juneteenth"),
    (2026, 7, 3, "Independence Day (observed)"),
    (2026, 9, 7, "Labor Day"),
    (2026, 11, 26, "Thanksgiving Day"),
    (2026, 12, 25, "Christmas Day"),
    (2027, 1, 1, "New Year's Day"),
    (2027, 1, 18, "Martin Luther King, Jr. Day"),
    (2027, 2, 15, "Washington's Birthday"),
    (2027, 3, 26, "Good Friday"),
    (2027, 5, 31, "Memorial Day"),
    (2027, 6, 18, "Juneteenth (observed)"),
    (2027, 7, 5, "Independence Day (observed)"),
    (2027, 9, 6, "Labor Day"),
    (2027, 11, 25, "Thanksgiving Day"),
    (2027, 12, 24, "Christmas Day (observed)"),
];

const EARLY_CLOSES: &[(i32, u32, u32, &str)] = &[
    (2025, 7, 3, "Independence Day eve"),
    (2025, 11, 28, "Day after Thanksgiving"),
    (2025, 12, 24, "Christmas Eve"),
    (2026, 11, 27, "Day after Thanksgiving"),
    (2026, 12, 24, "Christmas Eve"),
    (2027, 11, 26, "Day after Thanksgiving"),
];

/// Years the holiday/early-close table covers.
pub const CALENDAR_YEARS: std::ops::RangeInclusive<i32> = 2025..=2027;

/// Whether `date` falls inside the holiday table. Outside it, weekdays are assumed full days.
pub fn calendar_covers(date: NaiveDate) -> bool {
    CALENDAR_YEARS.contains(&date.year())
}

fn lookup(table: &[(i32, u32, u32, &'static str)], date: NaiveDate) -> Option<&'static str> {
    table
        .iter()
        .find(|(y, m, d, _)| date.year() == *y && date.month() == *m && date.day() == *d)
        .map(|(_, _, _, name)| *name)
}

pub fn day_kind(date: NaiveDate) -> DayKind {
    if matches!(date.weekday(), Weekday::Sat | Weekday::Sun) {
        return DayKind::Weekend;
    }
    if let Some(name) = lookup(HOLIDAYS, date) {
        return DayKind::Holiday(name);
    }
    if let Some(name) = lookup(EARLY_CLOSES, date) {
        return DayKind::EarlyClose(name);
    }
    DayKind::Full
}

pub fn is_trading_day(date: NaiveDate) -> bool {
    matches!(day_kind(date), DayKind::Full | DayKind::EarlyClose(_))
}

fn hm(h: u32, m: u32) -> NaiveTime {
    NaiveTime::from_hms_opt(h, m, 0).expect("valid time")
}

/// (regular close, after-hours end) in ET for a trading day.
fn close_times(date: NaiveDate) -> (NaiveTime, NaiveTime) {
    match day_kind(date) {
        DayKind::EarlyClose(_) => (hm(13, 0), hm(17, 0)),
        _ => (hm(16, 0), hm(20, 0)),
    }
}

/// The UTC instant of an ET wall-clock time. `None` only for a local time skipped by a DST jump
/// (02:00–02:59 on spring-forward Sunday); an ambiguous fall-back time resolves to the earlier one.
pub fn et_instant(date: NaiveDate, time: NaiveTime) -> Option<UnixNanos> {
    let local = New_York
        .from_local_datetime(&date.and_time(time))
        .earliest()?;
    local.timestamp_nanos_opt().map(UnixNanos)
}

/// ET wall clock with milliseconds, for fixtures: `et(date, 9, 30, 0, 0)`.
pub fn et(date: NaiveDate, h: u32, m: u32, s: u32, ms: u32) -> Option<UnixNanos> {
    et_instant(date, NaiveTime::from_hms_milli_opt(h, m, s, ms)?)
}

/// UTC bounds `[start, end)` of a session, or `None` if that session does not exist.
pub fn session_bounds(id: SessionId) -> Option<(UnixNanos, UnixNanos)> {
    if !is_trading_day(id.date) {
        return None;
    }
    let (close, after_end) = close_times(id.date);
    let (sd, st, ed, end) = match id.kind {
        SessionKind::Overnight => (id.date - Duration::days(1), hm(20, 0), id.date, hm(4, 0)),
        SessionKind::Pre => (id.date, hm(4, 0), id.date, hm(9, 30)),
        SessionKind::Regular => (id.date, hm(9, 30), id.date, close),
        SessionKind::After => (id.date, close, id.date, after_end),
    };
    Some((et_instant(sd, st)?, et_instant(ed, end)?))
}

/// All sessions of a trading date, in order. Empty for a non-trading date.
pub fn sessions_for(date: NaiveDate) -> Vec<(SessionId, UnixNanos, UnixNanos)> {
    SessionKind::ALL
        .iter()
        .filter_map(|k| {
            let id = SessionId::new(date, *k);
            session_bounds(id).map(|(s, e)| (id, s, e))
        })
        .collect()
}

/// Classify a UTC instant into its session, or `None` when no session is open (weekends,
/// holidays, Friday/pre-holiday evenings, 17:00–20:00 on early-close days).
pub fn classify(ts: UnixNanos) -> Option<SessionId> {
    let local = ts.to_utc().with_timezone(&New_York);
    let date = local.date_naive();
    let t = local.time();
    // Seconds-of-day comparisons avoid any sub-second rounding at boundaries.
    let secs = t.num_seconds_from_midnight();
    let at = |h: u32, m: u32| h * 3600 + m * 60;
    if secs >= at(20, 0) {
        let next = date + Duration::days(1);
        return is_trading_day(next).then_some(SessionId::new(next, SessionKind::Overnight));
    }
    if !is_trading_day(date) {
        return None;
    }
    let (close, after_end) = close_times(date);
    let kind = if secs < at(4, 0) {
        SessionKind::Overnight
    } else if secs < at(9, 30) {
        SessionKind::Pre
    } else if secs < close.num_seconds_from_midnight() {
        SessionKind::Regular
    } else if secs < after_end.num_seconds_from_midnight() {
        SessionKind::After
    } else {
        return None;
    };
    Some(SessionId::new(date, kind))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn utc(y: i32, m: u32, day: u32, h: u32, mi: u32, s: u32, ms: u32) -> UnixNanos {
        UnixNanos::utc(d(y, m, day), h, mi, s, ms * 1_000_000).unwrap()
    }

    fn sid(s: &str) -> Option<SessionId> {
        Some(s.parse().unwrap())
    }

    #[test]
    fn regular_day_boundaries_edt() {
        // 2026-10-15 (Thu) is EDT: ET = UTC-4.
        assert_eq!(
            classify(utc(2026, 10, 15, 7, 59, 59, 999)),
            sid("2026-10-15/overnight")
        );
        assert_eq!(
            classify(utc(2026, 10, 15, 8, 0, 0, 0)),
            sid("2026-10-15/pre")
        );
        assert_eq!(
            classify(utc(2026, 10, 15, 13, 29, 59, 999)),
            sid("2026-10-15/pre")
        );
        assert_eq!(
            classify(utc(2026, 10, 15, 13, 30, 0, 0)),
            sid("2026-10-15/regular")
        );
        assert_eq!(
            classify(utc(2026, 10, 15, 19, 59, 59, 999)),
            sid("2026-10-15/regular")
        );
        assert_eq!(
            classify(utc(2026, 10, 15, 20, 0, 0, 0)),
            sid("2026-10-15/after")
        );
        assert_eq!(
            classify(utc(2026, 10, 15, 23, 59, 59, 999)),
            sid("2026-10-15/after")
        );
        // 20:00 ET Thursday opens Friday's overnight session.
        assert_eq!(
            classify(utc(2026, 10, 16, 0, 0, 0, 0)),
            sid("2026-10-16/overnight")
        );
    }

    #[test]
    fn sub_millisecond_boundary() {
        let open = et(d(2026, 10, 15), 9, 30, 0, 0).unwrap();
        assert_eq!(classify(open.add_nanos(-1)), sid("2026-10-15/pre"));
        assert_eq!(classify(open), sid("2026-10-15/regular"));
    }

    #[test]
    fn weekend_and_overnight_rule() {
        // Friday 2026-10-16 20:00 ET: Saturday is not a trading day.
        assert_eq!(classify(utc(2026, 10, 17, 0, 0, 0, 0)), None);
        assert_eq!(classify(utc(2026, 10, 17, 15, 0, 0, 0)), None);
        // Sunday 2026-10-18 19:59:59.999 ET: closed; 20:00 ET: Monday's overnight.
        assert_eq!(classify(utc(2026, 10, 18, 23, 59, 59, 999)), None);
        assert_eq!(
            classify(utc(2026, 10, 19, 0, 0, 0, 0)),
            sid("2026-10-19/overnight")
        );
        assert_eq!(
            classify(utc(2026, 10, 19, 7, 59, 59, 999)),
            sid("2026-10-19/overnight")
        );
    }

    #[test]
    fn spring_forward_2026() {
        // DST starts Sunday 2026-03-08 02:00 ET.
        // Friday 03-06 is EST (UTC-5): open at 14:30Z.
        assert_eq!(
            et(d(2026, 3, 6), 9, 30, 0, 0),
            Some(utc(2026, 3, 6, 14, 30, 0, 0))
        );
        assert_eq!(
            classify(utc(2026, 3, 6, 14, 29, 59, 999)),
            sid("2026-03-06/pre")
        );
        assert_eq!(
            classify(utc(2026, 3, 6, 14, 30, 0, 0)),
            sid("2026-03-06/regular")
        );
        // Monday 03-09 is EDT (UTC-4): open at 13:30Z.
        assert_eq!(
            classify(utc(2026, 3, 9, 13, 29, 59, 999)),
            sid("2026-03-09/pre")
        );
        assert_eq!(
            classify(utc(2026, 3, 9, 13, 30, 0, 0)),
            sid("2026-03-09/regular")
        );
        assert_eq!(
            classify(utc(2026, 3, 9, 14, 30, 0, 0)),
            sid("2026-03-09/regular")
        );
        // Sunday 03-08 20:00 EDT = Monday 00:00Z opens Monday's overnight; 19:59 EDT is closed.
        assert_eq!(classify(utc(2026, 3, 8, 23, 59, 0, 0)), None);
        assert_eq!(
            classify(utc(2026, 3, 9, 0, 0, 0, 0)),
            sid("2026-03-09/overnight")
        );
        // The DST Sunday itself has no session at 02:30 local (which does not exist) or noon.
        assert_eq!(classify(utc(2026, 3, 8, 7, 30, 0, 0)), None);
        assert_eq!(classify(utc(2026, 3, 8, 16, 0, 0, 0)), None);
        let (s, e) = session_bounds(SessionId::new(d(2026, 3, 9), SessionKind::Overnight)).unwrap();
        assert_eq!(
            (s, e),
            (utc(2026, 3, 9, 0, 0, 0, 0), utc(2026, 3, 9, 8, 0, 0, 0))
        );
    }

    #[test]
    fn fall_back_2026() {
        // DST ends Sunday 2026-11-01 02:00 ET.
        assert_eq!(
            classify(utc(2026, 10, 30, 13, 30, 0, 0)),
            sid("2026-10-30/regular")
        );
        assert_eq!(
            classify(utc(2026, 10, 30, 19, 59, 59, 999)),
            sid("2026-10-30/regular")
        );
        assert_eq!(
            classify(utc(2026, 10, 30, 20, 0, 0, 0)),
            sid("2026-10-30/after")
        );
        // Monday 11-02 is EST: 13:30Z is still pre-market, 14:30Z opens, 21:00Z closes.
        assert_eq!(
            classify(utc(2026, 11, 2, 13, 30, 0, 0)),
            sid("2026-11-02/pre")
        );
        assert_eq!(
            classify(utc(2026, 11, 2, 14, 30, 0, 0)),
            sid("2026-11-02/regular")
        );
        assert_eq!(
            classify(utc(2026, 11, 2, 20, 59, 59, 999)),
            sid("2026-11-02/regular")
        );
        assert_eq!(
            classify(utc(2026, 11, 2, 21, 0, 0, 0)),
            sid("2026-11-02/after")
        );
        // Sunday 11-01 20:00 EST = Monday 01:00Z.
        assert_eq!(classify(utc(2026, 11, 2, 0, 59, 59, 999)), None);
        assert_eq!(
            classify(utc(2026, 11, 2, 1, 0, 0, 0)),
            sid("2026-11-02/overnight")
        );
        // The repeated 01:00–02:00 hour on the Sunday is closed either way.
        assert_eq!(classify(utc(2026, 11, 1, 5, 30, 0, 0)), None);
        assert_eq!(classify(utc(2026, 11, 1, 6, 30, 0, 0)), None);
    }

    #[test]
    fn early_close_day() {
        // 2026-11-27 (Fri, day after Thanksgiving): regular 09:30–13:00, after 13:00–17:00 (EST).
        let day = d(2026, 11, 27);
        assert_eq!(day_kind(day), DayKind::EarlyClose("Day after Thanksgiving"));
        assert_eq!(
            classify(utc(2026, 11, 27, 17, 59, 59, 999)),
            sid("2026-11-27/regular")
        );
        assert_eq!(
            classify(utc(2026, 11, 27, 18, 0, 0, 0)),
            sid("2026-11-27/after")
        );
        assert_eq!(
            classify(utc(2026, 11, 27, 21, 59, 59, 999)),
            sid("2026-11-27/after")
        );
        assert_eq!(classify(utc(2026, 11, 27, 22, 0, 0, 0)), None); // 17:00 ET
        assert_eq!(classify(utc(2026, 11, 28, 0, 59, 0, 0)), None); // 19:59 ET
        assert_eq!(classify(utc(2026, 11, 28, 1, 0, 0, 0)), None); // 20:00 ET, Saturday next
        let (s, e) = session_bounds(SessionId::new(day, SessionKind::Regular)).unwrap();
        assert_eq!(
            (s, e),
            (
                utc(2026, 11, 27, 14, 30, 0, 0),
                utc(2026, 11, 27, 18, 0, 0, 0)
            )
        );
        // Christmas Eve 2026 (Thu, EST): 20:00 ET belongs to no session (Christmas is closed).
        assert_eq!(
            classify(utc(2026, 12, 24, 18, 0, 0, 0)),
            sid("2026-12-24/after")
        );
        assert_eq!(classify(utc(2026, 12, 25, 1, 0, 0, 0)), None);
        // Thanksgiving Thursday 20:00 ET opens Friday's (early-close day's) overnight session.
        assert_eq!(
            classify(utc(2026, 11, 27, 1, 0, 0, 0)),
            sid("2026-11-27/overnight")
        );
    }

    #[test]
    fn holidays() {
        assert_eq!(
            day_kind(d(2025, 1, 9)),
            DayKind::Holiday("National Day of Mourning (President Carter)")
        );
        assert_eq!(day_kind(d(2026, 4, 3)), DayKind::Holiday("Good Friday"));
        assert_eq!(day_kind(d(2027, 3, 26)), DayKind::Holiday("Good Friday"));
        assert_eq!(
            day_kind(d(2026, 7, 3)),
            DayKind::Holiday("Independence Day (observed)")
        );
        assert_eq!(
            day_kind(d(2027, 12, 24)),
            DayKind::Holiday("Christmas Day (observed)")
        );
        assert_eq!(day_kind(d(2026, 7, 2)), DayKind::Full); // no early close before an observed holiday
        assert_eq!(day_kind(d(2026, 10, 17)), DayKind::Weekend);
        assert!(!is_trading_day(d(2026, 11, 26)));
        assert!(is_trading_day(d(2026, 11, 27)));
        // Holiday: no session at all, and the previous evening is not overnight.
        assert_eq!(classify(utc(2026, 11, 26, 15, 0, 0, 0)), None);
        assert_eq!(classify(utc(2026, 11, 26, 1, 0, 0, 0)), None); // Wed 20:00 EST
        assert!(sessions_for(d(2026, 11, 26)).is_empty());
        assert_eq!(sessions_for(d(2026, 11, 27)).len(), 4);
        assert!(calendar_covers(d(2027, 12, 31)));
        assert!(!calendar_covers(d(2028, 1, 3)));
    }

    #[test]
    fn session_id_text_and_serde() {
        let id: SessionId = "2026-10-15/regular".parse().unwrap();
        assert_eq!(id.to_string(), "2026-10-15/regular");
        assert!("2026-10-15/lunch".parse::<SessionId>().is_err());
        assert!("2026-13-01/pre".parse::<SessionId>().is_err());
        let a: SessionId = "2026-10-15/overnight".parse().unwrap();
        let b: SessionId = "2026-10-15/pre".parse().unwrap();
        let c: SessionId = "2026-10-14/after".parse().unwrap();
        assert!(c < a && a < b && b < id);
    }

    #[test]
    fn bounds_round_trip_through_classify() {
        for date in [
            d(2026, 3, 9),
            d(2026, 10, 15),
            d(2026, 11, 2),
            d(2026, 11, 27),
            d(2025, 7, 3),
        ] {
            for (id, s, e) in sessions_for(date) {
                assert_eq!(classify(s), Some(id), "{id} start");
                assert_eq!(classify(e.add_nanos(-1)), Some(id), "{id} end-1ns");
                assert_ne!(classify(e), Some(id), "{id} end");
            }
        }
    }
}
