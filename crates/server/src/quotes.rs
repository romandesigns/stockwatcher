//! The quote board: one row per symbol (last price, open reference, day volume), kept current by
//! every trade the ingest task applies, and diffed once per second for `quotes.update`.
//!
//! It lives in the market's `Core`, under the same lock as the chart engine, so it is fed from the
//! one `FeedItem` stream whatever the provider is (archive replay or live). A rebuild (seek, or the
//! initial catch-up) replaces `Core` and with it the board, and the rebuild's re-read of the day
//! refills it, exactly as it refills the candles.
//!
//! # Cost
//!
//! [`QuoteBoard::apply_trade`] runs for every print (~100k/s at the open in a fast replay), with the
//! core lock held. It does one hash lookup by `&str` and a few integer updates; it allocates only
//! the first time a symbol trades (its row and its `Arc<str>` name) and when the dirty list grows
//! past a capacity it never shrinks from. It never serializes: JSON is built once per second, from
//! a copy taken under the lock, by the caller.
//!
//! # Rules (the candles' rules, so the search never disagrees with the chart)
//!
//! The board does not re-derive what a print counts for: the caller passes the chart engine's own
//! verdict, the tape row's `counted` ([`Counted::from_tape`]). So everything the candles exclude,
//! the board excludes too: restricted conditions ([`market_events::Trade::effect`]), withdrawn
//! prints, duplicates, prints outside every session and prints later than the settle window.
//! (Measured on the fixture day: re-deriving from the condition table alone overcounted SMPL's
//! volume by 3.7% at 10:40: prints the engine excludes, such as late arrivals.)
//!
//! - **Last** moves only on `ohlc_and_volume` prints (an odd lot or an average-price report does
//!   not move it), and only forward in execution time: an out-of-order print the engine still
//!   accepted belongs to its own earlier interval and does not become the last price.
//! - **Volume** and **trades** count `ohlc_and_volume` and `volume_only` prints (an odd lot
//!   counts). `trades` is the number of those prints.
//! - **Open** is the first OHLC-qualifying price at or after the regular open (09:30 ET), i.e. of
//!   a print classified `regular` or `after` on the trading date. Until one exists, the reference
//!   for change % is the day's **first** OHLC-qualifying price (premarket). This mirrors the web
//!   chart header (`sessionOpen` in apps/web/src/App.tsx: change vs the first candle of the anchor
//!   session), which before 09:30 anchors on the premarket.
//! - A print of a later trading date (live mode crossing midnight, or a BOATS overnight print,
//!   which belongs to the next date) starts that symbol's day over. A print outside every session
//!   keeps the current day.
//!
//! Live corrections and cancels (`MarketEvent::Correction` / `Cancel`) are not applied: the board
//! is a running tally and keeps no per-print records to undo, so after one its volume can differ
//! from the candles by the corrected size until the next seek or restart. Archived Alpaca history
//! has none of them (it marks withdrawn prints on the print itself, which the engine's verdict
//! already covers); the chart engine stays the authority for candles.

use chrono::NaiveDate;
use market_events::{Price, PriceBand, SessionKind, Trade, UnixNanos};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Debug)]
struct Entry {
    symbol: Arc<str>,
    day: Option<NaiveDate>,
    last: Option<Price>,
    /// Execution time of `last`.
    last_ts: UnixNanos,
    first: Option<Price>,
    open: Option<Price>,
    volume: u64,
    trades: u64,
    /// Changed since the last [`QuoteBoard::take_changes`]; its index is in `dirty`.
    dirty: bool,
    /// Inside the band at the last take, i.e. the clients' view has this row.
    listed: bool,
}

/// One row copied out of the board (cheap: the symbol is shared).
#[derive(Clone, Debug, PartialEq)]
pub struct QuoteRow {
    pub symbol: Arc<str>,
    pub last: Price,
    /// The change-% reference: the regular open, else the day's first price (see module docs).
    pub reference: Price,
    /// True for the regular-session open, false while `reference` is the premarket first price.
    pub regular_open: bool,
    pub volume: u64,
    pub trades: u64,
    /// The row was not in the clients' view before this take (it just entered the band), so it
    /// must carry its name and exchange, which updates otherwise leave out.
    pub entering: bool,
}

/// What changed since the previous take.
#[derive(Debug, Default)]
pub struct QuoteChanges {
    /// Changed rows inside the band.
    pub rows: Vec<QuoteRow>,
    /// Symbols the clients had that are now outside the band.
    pub gone: Vec<Arc<str>>,
}

impl QuoteChanges {
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty() && self.gone.is_empty()
    }
}

/// What the chart engine counted a print for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Counted {
    pub ohlc: bool,
    pub volume: bool,
}

impl Counted {
    /// From a tape row's `counted`: `ohlc_and_volume`, `volume_only` or `excluded:<reason>`
    /// (crates/chart-engine/src/wire.rs `TapeTrade::counted`).
    pub fn from_tape(counted: &str) -> Self {
        match counted {
            "ohlc_and_volume" => Counted {
                ohlc: true,
                volume: true,
            },
            "volume_only" => Counted {
                ohlc: false,
                volume: true,
            },
            _ => Counted {
                ohlc: false,
                volume: false,
            },
        }
    }
}

#[derive(Debug, Default)]
pub struct QuoteBoard {
    index: HashMap<Arc<str>, usize>,
    entries: Vec<Entry>,
    dirty: Vec<usize>,
}

impl QuoteBoard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of symbols that have traded.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Applies one print with the engine's verdict on it.
    pub fn apply_trade(&mut self, t: &Trade, counted: Counted) {
        if !counted.ohlc && !counted.volume {
            return;
        }
        let i = match self.index.get(t.symbol.as_str()) {
            Some(&i) => i,
            None => {
                let symbol: Arc<str> = Arc::from(t.symbol.as_str());
                let i = self.entries.len();
                self.entries.push(Entry {
                    symbol: symbol.clone(),
                    day: None,
                    last: None,
                    last_ts: UnixNanos(i64::MIN),
                    first: None,
                    open: None,
                    volume: 0,
                    trades: 0,
                    dirty: false,
                    listed: false,
                });
                self.index.insert(symbol, i);
                i
            }
        };
        let e = &mut self.entries[i];
        if let Some(s) = t.session {
            if e.day.is_some_and(|d| s.date > d) {
                e.last = None;
                e.last_ts = UnixNanos(i64::MIN);
                e.first = None;
                e.open = None;
                e.volume = 0;
                e.trades = 0;
            }
            if e.day.is_none_or(|d| s.date > d) {
                e.day = Some(s.date);
            }
        }
        if counted.ohlc {
            if t.event_ts >= e.last_ts {
                e.last = Some(t.price);
                e.last_ts = t.event_ts;
            }
            e.first.get_or_insert(t.price);
            if t.session
                .is_some_and(|s| matches!(s.kind, SessionKind::Regular | SessionKind::After))
            {
                e.open.get_or_insert(t.price);
            }
        }
        if counted.volume {
            e.volume += t.size;
            e.trades += 1;
        }
        if !e.dirty {
            e.dirty = true;
            self.dirty.push(i);
        }
    }

    fn row(e: &Entry, entering: bool) -> Option<QuoteRow> {
        let last = e.last?;
        Some(QuoteRow {
            symbol: e.symbol.clone(),
            last,
            reference: e.open.or(e.first).unwrap_or(last),
            regular_open: e.open.is_some(),
            volume: e.volume,
            trades: e.trades,
            entering,
        })
    }

    /// Every row inside `band`, sorted by symbol (a symbol with volume but no price-qualifying
    /// print yet has no price, so it is not a candidate and not listed).
    pub fn snapshot(&self, band: PriceBand) -> Vec<QuoteRow> {
        let mut rows: Vec<QuoteRow> = self
            .entries
            .iter()
            .filter(|e| e.last.is_some_and(|p| band.contains(p)))
            .filter_map(|e| Self::row(e, true))
            .collect();
        rows.sort_by(|a, b| a.symbol.cmp(&b.symbol));
        rows
    }

    /// The rows changed since the previous take, and clears the dirty set. A changed row inside
    /// the band is sent; one that was listed and has left the band is `gone`; one outside the band
    /// that was not listed is nobody's business. Called even when nobody listens, so `listed`
    /// always describes what the last broadcast would have shown.
    pub fn take_changes(&mut self, band: PriceBand) -> QuoteChanges {
        let mut out = QuoteChanges::default();
        for i in self.dirty.drain(..) {
            let e = &mut self.entries[i];
            e.dirty = false;
            let inside = e.last.is_some_and(|p| band.contains(p));
            if inside {
                if let Some(r) = Self::row(e, !e.listed) {
                    out.rows.push(r);
                }
            } else if e.listed {
                out.gone.push(e.symbol.clone());
            }
            e.listed = inside;
        }
        out.rows.sort_by(|a, b| a.symbol.cmp(&b.symbol));
        out.gone.sort();
        out
    }

    /// After a rebuild every client takes a fresh snapshot, so the changes the rebuild piled up
    /// are already in it: drop them, and record the band membership those snapshots show.
    pub fn mark_all_sent(&mut self, band: PriceBand) {
        for i in self.dirty.drain(..) {
            self.entries[i].dirty = false;
        }
        for e in &mut self.entries {
            e.listed = e.last.is_some_and(|p| band.contains(p));
        }
    }

    /// One symbol's row, whatever the band (tests and diagnostics).
    pub fn get(&self, symbol: &str) -> Option<QuoteRow> {
        self.index
            .get(symbol)
            .and_then(|&i| Self::row(&self.entries[i], false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use market_events::UnixNanos;
    use market_events::session::et;

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 16).unwrap()
    }

    fn at(h: u32, m: u32) -> UnixNanos {
        et(day(), h, m, 0, 0).unwrap()
    }

    fn trade(sym: &str, ts: UnixNanos, cents: i64, size: u64) -> Trade {
        Trade::new(sym, ts, Price::from_cents(cents), size, 1)
    }

    /// The engine's verdict for a print it accepted: its condition effect.
    fn apply(b: &mut QuoteBoard, t: Trade) {
        let eff = t.effect();
        b.apply_trade(
            &t,
            Counted {
                ohlc: eff.updates_ohlc,
                volume: eff.updates_volume,
            },
        );
    }

    #[test]
    fn the_engine_verdict_decides() {
        let both = Counted {
            ohlc: true,
            volume: true,
        };
        let volume = Counted {
            ohlc: false,
            volume: true,
        };
        let none = Counted {
            ohlc: false,
            volume: false,
        };
        assert_eq!(Counted::from_tape("ohlc_and_volume"), both);
        assert_eq!(Counted::from_tape("volume_only"), volume);
        for excluded in [
            "excluded:late",
            "excluded:duplicate",
            "excluded:condition_W",
        ] {
            assert_eq!(Counted::from_tape(excluded), none);
        }
        let mut b = QuoteBoard::new();
        b.apply_trade(&trade("AAA", at(10, 0), 500, 100), none);
        assert!(b.is_empty(), "an excluded print does not even create a row");
    }

    #[test]
    fn an_out_of_order_print_does_not_become_the_last_price() {
        let mut b = QuoteBoard::new();
        apply(&mut b, trade("AAA", at(10, 1), 510, 100));
        apply(&mut b, trade("AAA", at(10, 0), 490, 100));
        let r = b.get("AAA").unwrap();
        assert_eq!(r.last, Price::from_cents(510));
        assert_eq!(r.volume, 200, "but it still counts toward volume");
    }

    #[test]
    fn odd_lots_add_volume_but_do_not_move_last() {
        let mut b = QuoteBoard::new();
        apply(&mut b, trade("AAA", at(10, 0), 500, 100));
        // "I" = odd lot: volume yes, OHLC no (the candles' rule).
        apply(
            &mut b,
            trade("AAA", at(10, 1), 900, 37).with_conditions(["@", "I"]),
        );
        let r = b.get("AAA").unwrap();
        assert_eq!(r.last, Price::from_cents(500));
        assert_eq!(r.volume, 137);
        assert_eq!(r.trades, 2);
    }

    #[test]
    fn withdrawn_prints_count_for_nothing() {
        let mut b = QuoteBoard::new();
        apply(&mut b, trade("AAA", at(10, 0), 500, 100));
        apply(
            &mut b,
            trade("AAA", at(10, 1), 900, 50).with_update("canceled"),
        );
        let r = b.get("AAA").unwrap();
        assert_eq!(
            (r.last, r.volume, r.trades),
            (Price::from_cents(500), 100, 1)
        );
        assert_eq!(b.take_changes(PriceBand::DEFAULT).rows.len(), 1);
        apply(
            &mut b,
            trade("AAA", at(10, 2), 900, 50).with_update("incorrect"),
        );
        assert!(
            b.take_changes(PriceBand::DEFAULT).is_empty(),
            "a print that changes nothing does not dirty the row"
        );
    }

    #[test]
    fn volume_only_prints_leave_a_symbol_without_a_price_unlisted() {
        let mut b = QuoteBoard::new();
        apply(
            &mut b,
            trade("AAA", at(10, 0), 500, 10).with_conditions(["@", "I"]),
        );
        assert!(b.get("AAA").is_none(), "no price yet");
        assert!(b.snapshot(PriceBand::DEFAULT).is_empty());
        assert!(b.take_changes(PriceBand::DEFAULT).is_empty());
    }

    #[test]
    fn the_reference_is_the_premarket_first_price_until_the_regular_open() {
        let mut b = QuoteBoard::new();
        apply(&mut b, trade("AAA", at(8, 0), 400, 100));
        apply(&mut b, trade("AAA", at(9, 0), 450, 100));
        let r = b.get("AAA").unwrap();
        assert_eq!(r.reference, Price::from_cents(400));
        assert!(!r.regular_open);
        apply(&mut b, trade("AAA", at(9, 30), 500, 100));
        apply(&mut b, trade("AAA", at(9, 31), 550, 100));
        let r = b.get("AAA").unwrap();
        assert_eq!(
            r.reference,
            Price::from_cents(500),
            "first print at/after 09:30"
        );
        assert!(r.regular_open);
        assert_eq!(r.last, Price::from_cents(550));
        assert_eq!(r.volume, 400, "day volume includes the premarket");
    }

    #[test]
    fn a_symbol_first_trading_after_hours_takes_that_as_its_open() {
        let mut b = QuoteBoard::new();
        apply(&mut b, trade("AAA", at(16, 30), 300, 100));
        let r = b.get("AAA").unwrap();
        assert!(r.regular_open);
        assert_eq!(r.reference, Price::from_cents(300));
    }

    #[test]
    fn a_later_trading_date_starts_the_day_over() {
        let mut b = QuoteBoard::new();
        apply(&mut b, trade("AAA", at(10, 0), 500, 100));
        let next = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
        apply(&mut b, trade("AAA", et(next, 8, 0, 0, 0).unwrap(), 600, 7));
        let r = b.get("AAA").unwrap();
        assert_eq!((r.volume, r.trades), (7, 1));
        assert_eq!(r.reference, Price::from_cents(600));
        assert!(!r.regular_open);
    }

    #[test]
    fn take_changes_sends_only_what_changed_and_names_only_entering_rows() {
        let mut b = QuoteBoard::new();
        let band = PriceBand::DEFAULT;
        apply(&mut b, trade("AAA", at(10, 0), 500, 100));
        apply(&mut b, trade("BBB", at(10, 0), 700, 100));
        let c = b.take_changes(band);
        let syms: Vec<&str> = c.rows.iter().map(|r| &*r.symbol).collect();
        assert_eq!(syms, ["AAA", "BBB"]);
        assert!(c.rows.iter().all(|r| r.entering));
        assert!(b.take_changes(band).is_empty(), "nothing changed since");
        apply(&mut b, trade("BBB", at(10, 1), 710, 100));
        apply(&mut b, trade("BBB", at(10, 2), 720, 100));
        let c = b.take_changes(band);
        assert_eq!(c.rows.len(), 1, "a row changed twice is sent once");
        assert_eq!(&*c.rows[0].symbol, "BBB");
        assert_eq!(c.rows[0].last, Price::from_cents(720));
        assert!(!c.rows[0].entering);
    }

    #[test]
    fn leaving_the_band_is_gone_and_coming_back_is_entering() {
        let mut b = QuoteBoard::new();
        let band = PriceBand::DEFAULT;
        apply(&mut b, trade("AAA", at(10, 0), 2400, 100));
        b.take_changes(band);
        apply(&mut b, trade("AAA", at(10, 1), 2600, 100));
        let c = b.take_changes(band);
        assert!(c.rows.is_empty());
        assert_eq!(c.gone.iter().map(|s| &**s).collect::<Vec<_>>(), ["AAA"]);
        assert!(b.snapshot(band).is_empty());
        apply(&mut b, trade("AAA", at(10, 2), 2700, 100));
        assert!(
            b.take_changes(band).is_empty(),
            "still outside and already gone: nothing to say"
        );
        apply(&mut b, trade("AAA", at(10, 3), 2450, 100));
        let c = b.take_changes(band);
        assert!(c.rows[0].entering);
    }

    #[test]
    fn mark_all_sent_drops_the_rebuild_backlog() {
        let mut b = QuoteBoard::new();
        let band = PriceBand::DEFAULT;
        apply(&mut b, trade("AAA", at(10, 0), 500, 100));
        apply(&mut b, trade("ZZZ", at(10, 0), 9900, 100));
        b.mark_all_sent(band);
        assert!(b.take_changes(band).is_empty());
        apply(&mut b, trade("AAA", at(10, 1), 510, 100));
        let c = b.take_changes(band);
        assert!(!c.rows[0].entering, "the snapshot after the rebuild had it");
        assert_eq!(b.snapshot(band).len(), 1, "ZZZ is out of band");
    }
}
