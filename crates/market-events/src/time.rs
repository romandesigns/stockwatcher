//! Event time.
//!
//! All event times are `i64` **nanoseconds since the Unix epoch, UTC**. Nanoseconds because the
//! SIP feeds (and Alpaca's RFC 3339 timestamps) carry nanosecond precision, and two prints in the
//! same microsecond are common on busy names; truncating would manufacture ties. `i64` nanoseconds
//! cover 1677..2262. Clients receive epoch **milliseconds** (see `chart-engine`'s wire types).
//!
//! Nothing here reads the process's local timezone: Eastern Time conversions go through
//! `chrono-tz`'s `America/New_York` database in [`crate::session`]. (On the owner's Windows machine
//! Git Bash ignores `TZ=America/New_York`, so the shell's zone must never matter.)

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

pub const NANOS_PER_SEC: i64 = 1_000_000_000;
pub const NANOS_PER_MILLI: i64 = 1_000_000;

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct UnixNanos(pub i64);

impl UnixNanos {
    pub const fn from_secs(s: i64) -> Self {
        UnixNanos(s * NANOS_PER_SEC)
    }

    pub const fn from_millis(ms: i64) -> Self {
        UnixNanos(ms * NANOS_PER_MILLI)
    }

    pub const fn nanos(self) -> i64 {
        self.0
    }

    /// Epoch milliseconds, floored (so `…:59.999_999` stays in the earlier millisecond).
    pub const fn millis(self) -> i64 {
        self.0.div_euclid(NANOS_PER_MILLI)
    }

    pub fn to_utc(self) -> DateTime<Utc> {
        Utc.timestamp_nanos(self.0)
    }

    pub fn from_utc(dt: DateTime<Utc>) -> Option<Self> {
        dt.timestamp_nanos_opt().map(UnixNanos)
    }

    /// A UTC wall-clock instant; handy for fixtures. `None` for an invalid date.
    pub fn utc(date: NaiveDate, h: u32, m: u32, s: u32, nanos: u32) -> Option<Self> {
        let ndt = date.and_hms_nano_opt(h, m, s, nanos)?;
        Self::from_utc(Utc.from_utc_datetime(&ndt))
    }

    pub const fn add_nanos(self, n: i64) -> Self {
        UnixNanos(self.0 + n)
    }
}

impl fmt::Display for UnixNanos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            self.to_utc()
                .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn millis_floor() {
        assert_eq!(UnixNanos(1_999_999).millis(), 1);
        assert_eq!(UnixNanos(-1).millis(), -1);
    }

    #[test]
    fn utc_constructor() {
        let d = NaiveDate::from_ymd_opt(2026, 10, 15).unwrap();
        let t = UnixNanos::utc(d, 13, 30, 0, 0).unwrap();
        assert_eq!(t.to_string(), "2026-10-15T13:30:00.000000000Z");
    }
}
