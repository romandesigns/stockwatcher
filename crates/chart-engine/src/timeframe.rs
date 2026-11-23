use market_events::UnixNanos;
use market_events::time::NANOS_PER_SEC;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

const HOUR_NS: i64 = 3600 * NANOS_PER_SEC;

/// A candle timeframe. Intervals are half-open `[start, start + len)` and aligned to the Unix
/// epoch in UTC. Every US session boundary (04:00, 09:30, 13:00, 16:00, 17:00, 20:00 ET) is a
/// multiple of five minutes and ET offsets are whole hours, so no interval of 10s..5m ever
/// straddles a session boundary.
///
/// **`1h` is the exception, handled here rather than by its callers.** Its intervals are aligned
/// to the clock hour, which is the ET hour on both sides of a DST change (the offset is a whole
/// number of hours), but the 09:00 hour holds the 09:30 open. An interval may never mix two
/// sessions (the RTH filter, the opening range and the profiles all key a candle by one session),
/// so the 1h interval is clipped to its session: 09:00–09:30 (pre) and 09:30–10:00 (regular), then
/// 10:00–11:00 and so on. [`start_of`] and [`end_of`] return the clipped bounds; [`nanos`] stays
/// the nominal length.
///
/// [`start_of`]: Timeframe::start_of
/// [`end_of`]: Timeframe::end_of
/// [`nanos`]: Timeframe::nanos
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Timeframe {
    #[serde(rename = "10s")]
    S10,
    #[serde(rename = "15s")]
    S15,
    #[serde(rename = "30s")]
    S30,
    #[serde(rename = "1m")]
    M1,
    #[serde(rename = "5m")]
    M5,
    #[serde(rename = "1h")]
    H1,
}

impl Timeframe {
    pub const ALL: [Timeframe; 6] = [
        Timeframe::S10,
        Timeframe::S15,
        Timeframe::S30,
        Timeframe::M1,
        Timeframe::M5,
        Timeframe::H1,
    ];

    pub const fn secs(self) -> i64 {
        match self {
            Timeframe::S10 => 10,
            Timeframe::S15 => 15,
            Timeframe::S30 => 30,
            Timeframe::M1 => 60,
            Timeframe::M5 => 300,
            Timeframe::H1 => 3600,
        }
    }

    pub const fn nanos(self) -> i64 {
        self.secs() * NANOS_PER_SEC
    }

    pub const fn label(self) -> &'static str {
        match self {
            Timeframe::S10 => "10s",
            Timeframe::S15 => "15s",
            Timeframe::S30 => "30s",
            Timeframe::M1 => "1m",
            Timeframe::M5 => "5m",
            Timeframe::H1 => "1h",
        }
    }

    /// Display-only timeframes the engine never measures its policies on. `10s` was added for
    /// the charts (2026-11-22); the settle window ("late" trades) and the base candles behind
    /// `Revision::day_high`/`last` stay on the 15 s grid they were built and tested on, so adding
    /// it changed no accepted print and nothing a detector reads.
    pub const fn display_only(self) -> bool {
        matches!(self, Timeframe::S10)
    }

    /// Built from a finer timeframe's candles rather than from trades. `1h` is: from trades its
    /// exact recompute (an out-of-order print, a correction, a cancel) would need a whole hour of
    /// trade records per symbol, 6–12x the default 10 min retention on a whole-market tape. Its
    /// candles are instead the exact merge of the next finer timeframe's buckets (5 m by
    /// default), which the engine keeps anyway: OHLC, volume and count all merge exactly.
    pub const fn derived(self) -> bool {
        matches!(self, Timeframe::H1)
    }

    /// Start of the interval containing `ts`. A trade at exactly `10:00:00.000` belongs to the
    /// `10:00:00` interval; one at `09:59:59.999_999_999` to the previous one. For `1h`, clipped
    /// to the start of `ts`'s session (see the type docs).
    pub fn start_of(self, ts: UnixNanos) -> UnixNanos {
        let start = ts.0.div_euclid(self.nanos()) * self.nanos();
        if let Timeframe::H1 = self
            && let Some((s, _)) = clipping_session(ts)
        {
            return UnixNanos(start.max(s));
        }
        UnixNanos(start)
    }

    /// Exclusive end of the interval containing `ts`. For `1h`, clipped to the end of `ts`'s
    /// session.
    pub fn end_of(self, ts: UnixNanos) -> UnixNanos {
        let end = ts.0.div_euclid(self.nanos()) * self.nanos() + self.nanos();
        if let Timeframe::H1 = self
            && let Some((_, e)) = clipping_session(ts)
        {
            return UnixNanos(end.min(e));
        }
        UnixNanos(end)
    }
}

/// UTC bounds of `ts`'s session when that session starts or ends inside `ts`'s clock hour (the
/// only case a 1h interval is clipped), else `None`. One ET conversion per call; the engine asks
/// once per new 1h bucket, not per print.
fn clipping_session(ts: UnixNanos) -> Option<(i64, i64)> {
    let hour = ts.0.div_euclid(HOUR_NS) * HOUR_NS;
    let (s, e) = market_events::classify(ts)?.bounds()?;
    if s.0 <= hour && e.0 >= hour + HOUR_NS {
        return None;
    }
    Some((s.0, e.0))
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
            .ok_or_else(|| {
                format!("unknown timeframe {s:?} (expected 10s, 15s, 30s, 1m, 5m or 1h)")
            })
    }
}
