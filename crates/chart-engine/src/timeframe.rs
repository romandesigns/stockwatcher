use market_events::UnixNanos;
use market_events::time::NANOS_PER_SEC;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// A candle timeframe. Intervals are half-open `[start, start + len)` and aligned to the Unix
/// epoch in UTC. Every US session boundary (04:00, 09:30, 13:00, 16:00, 17:00, 20:00 ET) is a
/// multiple of five minutes and ET offsets are whole hours, so no interval of any of these
/// timeframes ever straddles a session boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Timeframe {
    #[serde(rename = "15s")]
    S15,
    #[serde(rename = "30s")]
    S30,
    #[serde(rename = "1m")]
    M1,
    #[serde(rename = "5m")]
    M5,
}

impl Timeframe {
    pub const ALL: [Timeframe; 4] = [Timeframe::S15, Timeframe::S30, Timeframe::M1, Timeframe::M5];

    pub const fn secs(self) -> i64 {
        match self {
            Timeframe::S15 => 15,
            Timeframe::S30 => 30,
            Timeframe::M1 => 60,
            Timeframe::M5 => 300,
        }
    }

    pub const fn nanos(self) -> i64 {
        self.secs() * NANOS_PER_SEC
    }

    pub const fn label(self) -> &'static str {
        match self {
            Timeframe::S15 => "15s",
            Timeframe::S30 => "30s",
            Timeframe::M1 => "1m",
            Timeframe::M5 => "5m",
        }
    }

    /// Start of the interval containing `ts`. A trade at exactly `10:00:00.000` belongs to the
    /// `10:00:00` interval; one at `09:59:59.999_999_999` to the previous one.
    pub const fn start_of(self, ts: UnixNanos) -> UnixNanos {
        UnixNanos(ts.0.div_euclid(self.nanos()) * self.nanos())
    }

    /// Exclusive end of the interval containing `ts`.
    pub const fn end_of(self, ts: UnixNanos) -> UnixNanos {
        UnixNanos(self.start_of(ts).0 + self.nanos())
    }
}

impl fmt::Display for Timeframe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

impl FromStr for Timeframe {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Timeframe::ALL
            .into_iter()
            .find(|t| t.label() == s)
            .ok_or_else(|| format!("unknown timeframe {s:?} (expected 15s, 30s, 1m or 5m)"))
    }
}
