//! Key levels and opening ranges read off the engine's candles: what the clients' chart-core
//! `keyLevels` and `openingRanges` (packages/chart-core/src/levels.ts) computed from the candles
//! they held, computed here once and pushed (`levels.v1`, crates/server/src/chart_context.rs).
//!
//! # The rules (levels.ts, kept)
//!
//! - **The trading day** is the session date of the latest candle (the overnight session from
//!   20:00 ET belongs to the next date, as `tradingDayOf` has it).
//! - **Premarket high/low**: that day's pre-session candles. **High/low of day** and **the open**
//!   (first regular candle's open): its regular-session candles. A level with no candle behind
//!   it is `None`, never guessed.
//! - **Previous close fallback**: the close of the latest regular candle of an EARLIER date the
//!   series still holds. The server's official close (the closes file) wins over it when known,
//!   as `Series.prevClose` did on the client; that choice is the server's, not this module's.
//! - **Opening range**: per trading date, the first regular CANDLE of the series' own timeframe
//!   (a gap at 09:30 on a thin stock moves it to the first interval that traded), low to high,
//!   from its start to the regular close, `forming` while that candle is not final.
//!
//! # Intended differences from the client code
//!
//! The client classified candles with its display schedule (`sessionOf`: a fixed 09:30–16:00
//! regular session every weekday). This reads each bucket's session from the engine's NYSE
//! calendar (market-events `session`), so:
//!
//! - **Early-close days** (13:00): the 13:00–16:00 candles are after-hours. The client counted
//!   them in the high/low of day; this does not. The opening range ends at 13:00 on EVERY early
//!   close day; the client knew 13:00 only for the server's current session and drew older early
//!   closes to 16:00.
//! - Holidays and weekends have no session here, so no candles; nothing changes for a level.
//!
//! On a full trading day both give the same values bit for bit (the same f64s from the same wire
//! candles): tests/levels_parity.rs checks it against fixtures the TS code generated.
//!
//! # Cost
//!
//! [`key_levels`] reads the candles newest first and stops at the previous date's last regular
//! candle, so it touches about one day of buckets; [`opening_ranges`] reads the whole retained
//! series. Neither runs per trade for every symbol: the server calls them only for charts a
//! client watches, and only when [`KeyLevels::may_change`] / [`ranges_may_change`] say an
//! increment can move a value (a new extreme, the bucket that set one, a new day, a removal).

use chrono::NaiveDate;
use market_events::session::{SessionId, SessionKind};

use crate::wire::CandleWire;

/// The key levels of a symbol's latest trading day ([`key_levels`]).
#[derive(Clone, Debug, PartialEq)]
pub struct KeyLevels {
    /// The latest candle's trading date.
    pub day: NaiveDate,
    pub pm_high: Option<f64>,
    pub pm_low: Option<f64>,
    pub hod: Option<f64>,
    pub lod: Option<f64>,
    pub open: Option<f64>,
    /// The latest regular close of an earlier date in the series (the fallback; module docs).
    pub prev_close: Option<f64>,
    /// Start (ms) of the candle each value above came from, `i64::MIN` for none, in the order
    /// pm_high, pm_low, hod, lod, open, prev_close: a revision of one of THESE candles may move
    /// its value back, which a new trade elsewhere never does ([`KeyLevels::may_change`]).
    set_by: [i64; 6],
}

impl KeyLevels {
    /// Whether a candle increment (`session`, `c`) can change these levels, so a caller may skip
    /// recomputing for every other one (most trades: inside the day's range, not the open).
    /// Conservative: `true` means "recompute", which then decides. A removed or gapped candle
    /// always needs a recompute; that is the caller's.
    pub fn may_change(&self, session: SessionId, c: &CandleWire) -> bool {
        if session.date != self.day || self.set_by.contains(&c.t) {
            return true;
        }
        match session.kind {
            SessionKind::Pre => {
                self.pm_high.is_none_or(|h| c.h > h) || self.pm_low.is_none_or(|l| c.l < l)
            }
            SessionKind::Regular => {
                self.open.is_none()
                    || c.t < self.set_by[4]
                    || self.hod.is_none_or(|h| c.h > h)
                    || self.lod.is_none_or(|l| c.l < l)
            }
            SessionKind::After | SessionKind::Overnight => false,
        }
    }
}

/// The key levels from a series' candles given NEWEST FIRST, each with its session (module
/// docs); `None` when there is no candle. Stops at the previous date's last regular candle.
pub fn key_levels(
    newest_first: impl Iterator<Item = (SessionId, CandleWire)>,
) -> Option<KeyLevels> {
    let mut out: Option<KeyLevels> = None;
    for (session, c) in newest_first {
        let k = out.get_or_insert(KeyLevels {
            day: session.date,
            pm_high: None,
            pm_low: None,
            hod: None,
            lod: None,
            open: None,
            prev_close: None,
            set_by: [i64::MIN; 6],
        });
        if session.date < k.day {
            // The latest regular candle before the day is its fallback previous close.
            if session.kind == SessionKind::Regular {
                k.prev_close = Some(c.c);
                k.set_by[5] = c.t;
                break;
            }
            continue;
        }
        match session.kind {
            SessionKind::Pre => {
                // `>` / `<`: on a tie the newer candle keeps the mark; either is right, since a
                // revision of the marked one recomputes over all of them.
                if k.pm_high.is_none_or(|h| c.h > h) {
                    k.pm_high = Some(c.h);
                    k.set_by[0] = c.t;
                }
                if k.pm_low.is_none_or(|l| c.l < l) {
                    k.pm_low = Some(c.l);
                    k.set_by[1] = c.t;
                }
            }
            SessionKind::Regular => {
                if k.hod.is_none_or(|h| c.h > h) {
                    k.hod = Some(c.h);
                    k.set_by[2] = c.t;
                }
                if k.lod.is_none_or(|l| c.l < l) {
                    k.lod = Some(c.l);
                    k.set_by[3] = c.t;
                }
                // Read backwards, the last regular candle seen is the day's first.
                k.open = Some(c.o);
                k.set_by[4] = c.t;
            }
            SessionKind::After | SessionKind::Overnight => {}
        }
    }
    out
}

/// One trading day's opening range ([`opening_ranges`]); the wire shape of levels.ts
/// `OpeningRange`.
#[derive(Clone, Debug, PartialEq)]
pub struct OpeningRange {
    /// Trading date, `YYYY-MM-DD`.
    pub day: NaiveDate,
    /// Start of the first regular candle, epoch ms.
    pub t: i64,
    /// The regular session's end on that date, epoch ms (13:00 ET on an early close).
    pub end: i64,
    pub high: f64,
    pub low: f64,
    /// The first candle is not final yet: high and low can still move.
    pub forming: bool,
}

/// One opening range per trading date with a regular candle, oldest first, from a series'
/// candles given OLDEST FIRST with their sessions (module docs).
pub fn opening_ranges(
    oldest_first: impl Iterator<Item = (SessionId, CandleWire)>,
) -> Vec<OpeningRange> {
    let mut out: Vec<OpeningRange> = Vec::new();
    for (session, c) in oldest_first {
        if session.kind != SessionKind::Regular || out.last().is_some_and(|r| r.day == session.date)
        {
            continue;
        }
        let Some((_, end)) = session.bounds() else {
            continue;
        };
        out.push(OpeningRange {
            day: session.date,
            t: c.t,
            end: end.millis(),
            high: c.h,
            low: c.l,
            forming: !c.is_final,
        });
    }
    out
}

/// Whether a candle increment at `t` in `session` can change `ranges` ([`opening_ranges`] of the
/// same series): only a regular candle of a date that has no range yet, or at or before that
/// date's first candle. A removal always can; that is the caller's.
pub fn ranges_may_change(ranges: &[OpeningRange], session: SessionId, t: i64) -> bool {
    session.kind == SessionKind::Regular
        && ranges
            .iter()
            .rev()
            .find(|r| r.day == session.date)
            .is_none_or(|r| t <= r.t)
}
