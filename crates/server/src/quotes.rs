//! The quote board: one row per symbol (last price, previous close, open reference, day volume),
//! kept current by every trade, correction and cancel the ingest task applies, and diffed once per
//! second for `quotes.update`.
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
//! - **Previous close** (`pc` on the wire) is the official close of the latest trading date
//!   before the row's own trading date that `daily-closes.json` has for the symbol
//!   ([`market_archive::PrevCloses::before`]). It is what brokers quote "today's change" against,
//!   so clients use it whenever it is present. It is looked up when the row's trading date is set
//!   or moves on (once per symbol per day, never per print), so a replay of D and live mode on D
//!   use the same rule, and a live session crossing midnight picks up the new day's reference.
//! - **Open** is the fallback reference, for a symbol the closes file does not cover: the first
//!   OHLC-qualifying price at or after the regular open (09:30 ET), i.e. of a print classified
//!   `regular` or `after` on the trading date. Until one exists, it is the day's **first**
//!   OHLC-qualifying price (premarket, flagged `pre`). This mirrors the chart header's own fallback
//!   (`changeReference` in packages/chart-core: the first candle of the anchor session).
//! - A print of a later trading date (live mode crossing midnight, or a BOATS overnight print,
//!   which belongs to the next date) starts that symbol's day over. A print outside every session
//!   keeps the current day.
//!
//! # Corrections and cancels
//!
//! The board keeps no per-print records, so it cannot undo a print by itself. It does not need to:
//! the engine keeps them for its retention window, and for every correction or cancel it applies it
//! reports what it did ([`chart_engine::Revision`] from `ChartEngine::apply_revising`): the print
//! it removed and what that had counted for, the replacement (a correction, re-run through the
//! condition policy) and the trading date's newest price print afterwards. [`QuoteBoard::apply_revision`]
//! subtracts and adds exactly that, so volume and trades stay equal to the candles' totals and
//! **last** equals the latest candle's close (the server test asserts both, after a seek too). A
//! correction or cancel the engine did not apply (target outside retention, or unknown) changes no
//! candle and is not reported, so the board ignores it as well.
//!
//! Not revised: **open** / the premarket **first** price. A cancel of the very print that set
//! them is rare, and following it exactly would need the day's earliest prints, which the engine
//! no longer retains by then (10 minutes). With a previous close known, they are not the
//! reference anyway.

use chart_engine::Revision;
use chrono::NaiveDate;
use market_archive::PrevCloses;
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
    /// The previous close for `day` (module docs) on `day`'s share basis, and whether a known
    /// split had to be applied for that; fixed until `day` moves on or the closes reload.
    prev_close: Option<(Price, bool)>,
    /// `prev_close` was (re)looked up and the clients' row may not have it yet: the next update
    /// that sends the row carries `pc` even though the row is not entering (see [`QuoteRow`]).
    prev_close_changed: bool,
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
    /// The previous trading day's official close, when the closes file has one (module docs).
    ///
    /// `None` also when the day plainly trades on another share basis than that close
    /// ([`PcBasis::Guard`]): then the change is measured against `reference`, never against a
    /// close the stock no longer trades near.
    pub prev_close: Option<Price>,
    /// How `prev_close` relates to the closes file ([`PcBasis`]).
    pub basis: PcBasis,
    /// `prev_close` changed since the clients last got this row (its trading date moved on), so an
    /// update must carry it or the client would keep measuring against the old day's close. It
    /// never changes to `None`: a later date sees every close an earlier one did. Always false in
    /// a snapshot and for an entering row (those carry `pc` anyway).
    pub prev_close_changed: bool,
    pub volume: u64,
    pub trades: u64,
    /// The row was not in the clients' view before this take (it just entered the band), so it
    /// must carry its name and exchange, which updates otherwise leave out.
    pub entering: bool,
}

/// How a row's previous close was arrived at (owner, 2026-10-30: reverse splits read as
/// +3,900 % gainers against the raw closes file).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PcBasis {
    /// The file's close as is (or no close at all).
    #[default]
    Raw,
    /// A split the corporate-actions ledger knows took effect after the close: the close was put
    /// on today's basis (`market_archive::PrevCloses::lookup`).
    Split,
    /// No split is known, but the day's FIRST print is a typical split factor away from the
    /// close ([`split_guard`]): the close is withheld and change % falls back to the open / first
    /// price. Marked so a client can say why.
    Guard,
}

impl PcBasis {
    /// The wire tag (`pcb`), absent for `Raw`.
    pub fn tag(self) -> Option<&'static str> {
        match self {
            PcBasis::Raw => None,
            PcBasis::Split => Some("split"),
            PcBasis::Guard => Some("guard"),
        }
    }
}

/// The day opened this many times above (or below) the previous close, or more: no stock does
/// that on news at its first print; a share-basis change does.
pub const SPLIT_GUARD_ALWAYS: f64 = 10.0;
/// From this factor up to [`SPLIT_GUARD_ALWAYS`], a gap within [`SPLIT_GUARD_TOLERANCE`] of a
/// whole number (1-for-3 … 1-for-9 and their forward twins) reads as a split. Below it (2×) a
/// real gap is likelier than a 1-for-2, so the raw close stands.
pub const SPLIT_GUARD_MIN: f64 = 3.0;
pub const SPLIT_GUARD_TOLERANCE: f64 = 0.04;

/// Whether a day whose FIRST counted print is `first` plainly trades on another share basis than
/// `prev_close` (no split known for it). The first print, not the last: a genuine runner starts
/// near its close and climbs during the day (that is the "supporting intraday move"), while a
/// split stock is already at the new basis when it first trades. Measured either way, so a
/// forward split (1/4 of the close) is caught like a reverse one.
///
/// A real +900 % gap at the day's first print would be withheld too; it then shows its move from
/// that first print, flagged, which is the honest reading when nothing says which it was.
pub fn split_guard(first: Price, prev_close: Price) -> bool {
    let (f, p) = (first.micros() as f64, prev_close.micros() as f64);
    if f <= 0.0 || p <= 0.0 {
        return false;
    }
    let factor = if f >= p { f / p } else { p / f };
    if factor >= SPLIT_GUARD_ALWAYS {
        return true;
    }
    factor >= SPLIT_GUARD_MIN && (factor - factor.round()).abs() / factor <= SPLIT_GUARD_TOLERANCE
}

/// A closes lookup as the board keeps it.
fn pc_ref(c: market_archive::CloseRef) -> (Price, bool) {
    (c.price, c.split_adjusted)
}

/// The previous close a row may use, its basis, given the day's first print.
fn effective_pc(prev: Option<(Price, bool)>, first: Option<Price>) -> (Option<Price>, PcBasis) {
    match prev {
        None => (None, PcBasis::Raw),
        Some((p, true)) => (Some(p), PcBasis::Split),
        Some((p, false)) if first.is_some_and(|f| split_guard(f, p)) => (None, PcBasis::Guard),
        Some((p, false)) => (Some(p), PcBasis::Raw),
    }
}

/// One priced row borrowed from the board ([`QuoteBoard::scan`]): the fields of a [`QuoteRow`]
/// a ranking needs, without cloning the symbol.
#[derive(Clone, Copy, Debug)]
pub struct ScanRow<'a> {
    pub symbol: &'a Arc<str>,
    pub last: Price,
    pub reference: Price,
    /// As [`QuoteRow::prev_close`]: split-adjusted, or withheld by the guard.
    pub prev_close: Option<Price>,
    pub basis: PcBasis,
    pub volume: u64,
    pub trades: u64,
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

    /// The print's own verdict without the engine: its conditions ([`Trade::effect`]) and a
    /// session. Used only for a symbol whose history the live market has not loaded into the
    /// engine yet (`crate::history`): it misses the engine-only exclusions (late, duplicate),
    /// and the row is replaced by the engine's exact one when the symbol loads.
    pub fn from_effect(t: &Trade) -> Self {
        let e = t.effect();
        let in_session = t.session.is_some();
        Counted {
            ohlc: e.updates_ohlc && in_session,
            volume: e.updates_volume && in_session,
        }
    }
}

#[derive(Debug, Default)]
pub struct QuoteBoard {
    index: HashMap<Arc<str>, usize>,
    entries: Vec<Entry>,
    dirty: Vec<usize>,
    /// Shared with the market; a rebuild's new board gets the market's current index, and a
    /// reload of the file reaches a running board through [`QuoteBoard::set_closes`].
    closes: Arc<PrevCloses>,
}

impl QuoteBoard {
    /// A board with no previous closes: every row falls back to the open.
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_closes(closes: Arc<PrevCloses>) -> Self {
        QuoteBoard {
            closes,
            ..Self::default()
        }
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
                    prev_close: None,
                    prev_close_changed: false,
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
                let pc = self.closes.lookup(&e.symbol, s.date).map(pc_ref);
                // Only a real change needs resending (a first day has no client row yet; the
                // entering row carries it).
                e.prev_close_changed |= pc != e.prev_close;
                e.prev_close = pc;
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

    /// Applies a correction or cancel the chart engine applied, with the engine's own account of
    /// it (module docs). Ignored for a symbol without a row, or for a print of another trading
    /// date than the row's (the row holds only its current day).
    pub fn apply_revision(&mut self, rev: &Revision) {
        let Some(&i) = self.index.get(rev.symbol.as_str()) else {
            return;
        };
        let e = &mut self.entries[i];
        if e.day != Some(rev.session.date) {
            return;
        }
        let before = (e.last, e.last_ts, e.volume, e.trades);
        if rev.removed.volume {
            e.volume = e.volume.saturating_sub(rev.removed.size);
            e.trades = e.trades.saturating_sub(1);
        }
        if let Some(a) = rev.added.filter(|a| a.volume) {
            e.volume += a.size;
            e.trades += 1;
        }
        if rev.removed.ohlc || rev.added.is_some_and(|a| a.ohlc) {
            // The engine's newest price print of the day, i.e. the chart's last close. None: no
            // candle of the day is left, so the row has no price and leaves the clients' view.
            match rev.last {
                Some((p, ts)) => {
                    e.last = Some(p);
                    e.last_ts = ts;
                }
                None => {
                    e.last = None;
                    e.last_ts = UnixNanos(i64::MIN);
                }
            }
        }
        if (e.last, e.last_ts, e.volume, e.trades) != before && !e.dirty {
            e.dirty = true;
            self.dirty.push(i);
        }
    }

    fn row(e: &Entry, entering: bool) -> Option<QuoteRow> {
        let last = e.last?;
        let (pc, basis) = effective_pc(e.prev_close, e.first);
        Some(QuoteRow {
            symbol: e.symbol.clone(),
            last,
            reference: e.open.or(e.first).unwrap_or(last),
            regular_open: e.open.is_some(),
            prev_close: pc,
            basis,
            prev_close_changed: e.prev_close_changed && !entering,
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
                e.prev_close_changed = false;
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
            e.prev_close_changed = false;
        }
    }

    /// Swaps in a reloaded `daily-closes.json` index (the archive maintainer rewrites it after
    /// every session; a long-running live session needs yesterday's closes without a restart).
    ///
    /// Every row's previous close is looked up again for its current trading date, and a row
    /// whose reference actually changed is marked dirty with `prev_close_changed`, so the next
    /// `quotes.update` carries the new `pc` exactly like a trading date moving on. In practice
    /// that only happens when the file gained a close it lacked (a first fill, a date the import
    /// had missed): the official close of a finished day does not change. The chart header looks
    /// its `prevClose` up in this same index ([`QuoteBoard::closes`]), so the two never disagree.
    pub fn set_closes(&mut self, closes: Arc<PrevCloses>) {
        self.closes = closes;
        for (i, e) in self.entries.iter_mut().enumerate() {
            let Some(day) = e.day else { continue };
            let pc = self.closes.lookup(&e.symbol, day).map(pc_ref);
            if pc != e.prev_close {
                e.prev_close = pc;
                e.prev_close_changed = true;
                if !e.dirty {
                    e.dirty = true;
                    self.dirty.push(i);
                }
            }
        }
    }

    /// `symbol`'s previous close for trading date `day` as the board's rows use it: split-adjusted
    /// when a split is known, and `None` when the row for that day is held by the guard
    /// ([`PcBasis::Guard`]). For a day the board has no row for, the closes lookup alone. The
    /// chart header's `prevClose` reads this, so it never disagrees with the search row.
    pub fn prev_close_on(&self, symbol: &str, day: NaiveDate) -> Option<Price> {
        let c = self.closes.lookup(symbol, day)?;
        let e = self.index.get(symbol).map(|&i| &self.entries[i]);
        match e {
            Some(e) if e.day == Some(day) => effective_pc(Some(pc_ref(c)), e.first).0,
            _ => Some(c.price),
        }
    }

    /// The closes index this board looks previous closes up in.
    pub fn closes(&self) -> &Arc<PrevCloses> {
        &self.closes
    }

    /// One symbol's row, whatever the band (tests and diagnostics).
    pub fn get(&self, symbol: &str) -> Option<QuoteRow> {
        self.index
            .get(symbol)
            .and_then(|&i| Self::row(&self.entries[i], false))
    }

    /// Execution time (epoch ns) of the print behind `symbol`'s last price, when it has one: the
    /// Favorites panel's stale marker (crate::favorites).
    pub fn last_trade_ns(&self, symbol: &str) -> Option<i64> {
        let e = &self.entries[*self.index.get(symbol)?];
        e.last.map(|_| e.last_ts.0)
    }

    /// Every row that has a price, without copying it: the movers ranking's once-a-tick pass
    /// (`crate::movers`). No allocation, no symbol clone; under the core lock, so it only reads.
    pub fn scan(&self) -> impl Iterator<Item = ScanRow<'_>> {
        self.entries.iter().filter_map(|e| {
            let last = e.last?;
            let (pc, basis) = effective_pc(e.prev_close, e.first);
            Some(ScanRow {
                symbol: &e.symbol,
                last,
                reference: e.open.or(e.first).unwrap_or(last),
                prev_close: pc,
                basis,
                volume: e.volume,
                trades: e.trades,
            })
        })
    }

    /// The index of `symbol`'s entry, created empty when missing.
    fn entry_index(&mut self, symbol: &str) -> usize {
        if let Some(&i) = self.index.get(symbol) {
            return i;
        }
        let symbol: Arc<str> = Arc::from(symbol);
        let i = self.entries.len();
        self.entries.push(Entry {
            symbol: symbol.clone(),
            day: None,
            last: None,
            last_ts: UnixNanos(i64::MIN),
            first: None,
            open: None,
            prev_close: None,
            prev_close_changed: false,
            volume: 0,
            trades: 0,
            dirty: false,
            listed: false,
        });
        self.index.insert(symbol, i);
        i
    }

    fn mark_dirty(&mut self, i: usize) {
        if !self.entries[i].dirty {
            self.entries[i].dirty = true;
            self.dirty.push(i);
        }
    }

    /// Every row with a trading date, as the live market saves them in the archive
    /// (`market_archive::LiveQuote`, the startup summary of a recorded day).
    pub fn export(&self) -> Vec<market_archive::LiveQuote> {
        self.entries
            .iter()
            .filter_map(|e| {
                Some(market_archive::LiveQuote {
                    symbol: e.symbol.to_string(),
                    day: e.day?,
                    last: e.last,
                    last_ts: e.last_ts.0,
                    first: e.first,
                    open: e.open,
                    volume: e.volume,
                    trades: e.trades,
                })
            })
            .collect()
    }

    /// Restores saved rows (a restart: the live market's history summary), each replacing the
    /// symbol's row unless the board already has a later trading date for it. The previous close
    /// is looked up for the row's date as when it traded; every restored row is dirty, so it goes
    /// out like any change.
    pub fn restore(&mut self, rows: &[market_archive::LiveQuote]) {
        for r in rows {
            let i = self.entry_index(&r.symbol);
            let e = &mut self.entries[i];
            if e.day.is_some_and(|d| d > r.day) {
                continue;
            }
            let pc = self.closes.lookup(&e.symbol, r.day).map(pc_ref);
            e.prev_close_changed |= pc != e.prev_close;
            e.prev_close = pc;
            e.day = Some(r.day);
            e.last = r.last;
            e.last_ts = UnixNanos(r.last_ts);
            e.first = r.first;
            e.open = r.open;
            e.volume = r.volume;
            e.trades = r.trades;
            self.mark_dirty(i);
        }
    }

    /// Replaces `symbol`'s row with `other`'s (a symbol whose history was just loaded into the
    /// engine: `other` is the board that history built, by the engine's own verdicts, replacing
    /// the provisional row kept while it was not loaded). Nothing happens when `other` has none.
    pub fn replace_from(&mut self, symbol: &str, other: &QuoteBoard) {
        let Some(src) = other.index.get(symbol).map(|&j| other.entries[j].clone()) else {
            return;
        };
        let i = self.entry_index(symbol);
        let e = &mut self.entries[i];
        let pc = src
            .day
            .and_then(|d| self.closes.lookup(&e.symbol, d).map(pc_ref));
        e.prev_close_changed |= pc != e.prev_close;
        e.prev_close = pc;
        e.day = src.day;
        e.last = src.last;
        e.last_ts = src.last_ts;
        e.first = src.first;
        e.open = src.open;
        e.volume = src.volume;
        e.trades = src.trades;
        self.mark_dirty(i);
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

    fn closes(rows: &[(&str, &str, f64)]) -> Arc<PrevCloses> {
        let mut m = market_archive::DailyCloses::new();
        for (d, sym, c) in rows {
            m.entry(NaiveDate::parse_from_str(d, "%Y-%m-%d").unwrap())
                .or_default()
                .insert(sym.to_string(), *c);
        }
        Arc::new(PrevCloses::new(&m).0)
    }

    #[test]
    fn rows_carry_the_previous_close_of_the_latest_earlier_day_or_none() {
        let mut b = QuoteBoard::with_closes(closes(&[
            ("2026-10-14", "AAA", 4.0),
            ("2026-10-15", "AAA", 4.5),
            // The day's own close is never its reference.
            ("2026-10-16", "AAA", 9.0),
        ]));
        apply(&mut b, trade("AAA", at(8, 0), 400, 100));
        apply(&mut b, trade("BBB", at(8, 0), 700, 100));
        let a = b.get("AAA").unwrap();
        assert_eq!(a.prev_close, Some(Price::from_cents(450)));
        assert!(!a.regular_open, "the open fallback is still kept beside it");
        assert_eq!(b.get("BBB").unwrap().prev_close, None, "not in the file");
        let snap = b.snapshot(PriceBand::DEFAULT);
        assert!(snap.iter().all(|r| r.entering && !r.prev_close_changed));
    }

    #[test]
    fn a_new_trading_date_looks_the_previous_close_up_again_and_resends_it_once() {
        let mut b = QuoteBoard::with_closes(closes(&[("2026-10-15", "AAA", 4.5)]));
        let band = PriceBand::DEFAULT;
        apply(&mut b, trade("AAA", at(15, 0), 500, 100));
        let c = b.take_changes(band);
        assert!(c.rows[0].entering && !c.rows[0].prev_close_changed);
        apply(&mut b, trade("AAA", at(15, 1), 510, 100));
        assert!(
            !b.take_changes(band).rows[0].prev_close_changed,
            "static for the day"
        );
        // Live mode crossing into Monday: Friday 10-16 has no close in the file, so the latest
        // earlier one (Thursday's) stays the reference and nothing needs resending.
        let mon = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
        apply(&mut b, trade("AAA", et(mon, 8, 0, 0, 0).unwrap(), 520, 100));
        let r = &b.take_changes(band).rows[0];
        assert_eq!(r.prev_close, Some(Price::from_cents(450)));
        assert!(!r.prev_close_changed);

        // A row whose new day gains a reference gets it in its next update, once.
        let mut b = QuoteBoard::with_closes(closes(&[("2026-10-15", "AAA", 4.5)]));
        let before = NaiveDate::from_ymd_opt(2026, 10, 15).unwrap();
        apply(
            &mut b,
            trade("AAA", et(before, 10, 0, 0, 0).unwrap(), 500, 100),
        );
        assert_eq!(
            b.get("AAA").unwrap().prev_close,
            None,
            "nothing before the 15th"
        );
        b.take_changes(band);
        apply(&mut b, trade("AAA", at(10, 0), 510, 100));
        let r = &b.take_changes(band).rows[0];
        assert_eq!(
            (r.prev_close, r.prev_close_changed),
            (Some(Price::from_cents(450)), true)
        );
        apply(&mut b, trade("AAA", at(10, 1), 520, 100));
        assert!(
            !b.take_changes(band).rows[0].prev_close_changed,
            "sent once"
        );
    }

    #[test]
    fn a_reloaded_closes_file_reaches_existing_rows_once_and_only_where_it_changed() {
        let band = PriceBand::DEFAULT;
        let mut b = QuoteBoard::with_closes(closes(&[("2026-10-15", "AAA", 4.5)]));
        apply(&mut b, trade("AAA", at(10, 0), 500, 100));
        apply(&mut b, trade("BBB", at(10, 0), 700, 100));
        b.take_changes(band);
        // The maintainer's rewrite: AAA unchanged, BBB gains its missing close.
        b.set_closes(closes(&[
            ("2026-10-15", "AAA", 4.5),
            ("2026-10-15", "BBB", 6.5),
        ]));
        let c = b.take_changes(band);
        assert_eq!(c.rows.len(), 1, "only the row whose reference moved");
        assert_eq!(&*c.rows[0].symbol, "BBB");
        assert_eq!(
            (c.rows[0].prev_close, c.rows[0].prev_close_changed),
            (Some(Price::from_cents(650)), true)
        );
        assert_eq!(b.closes().symbols(), 2);
        assert!(b.take_changes(band).rows.is_empty(), "sent once");
    }

    fn rev(
        sym: &str,
        removed: (i64, u64, bool, bool),
        added: Option<(i64, u64, bool, bool)>,
        last: Option<(i64, UnixNanos)>,
    ) -> Revision {
        let print = |(c, size, ohlc, volume): (i64, u64, bool, bool)| chart_engine::RevisedPrint {
            price: Price::from_cents(c),
            size,
            ohlc,
            volume,
        };
        Revision {
            symbol: sym.into(),
            ts: UnixNanos(0),
            session: market_events::SessionId {
                date: day(),
                kind: SessionKind::Regular,
            },
            removed: print(removed),
            added: added.map(print),
            last: last.map(|(c, ts)| (Price::from_cents(c), ts)),
            // The quote board does not keep a day high, so this fixture leaves it out; the
            // context engine's own revision tests cover it.
            day_high: None,
        }
    }

    #[test]
    fn a_cancel_takes_out_exactly_what_the_engine_had_counted() {
        let mut b = QuoteBoard::new();
        let band = PriceBand::DEFAULT;
        apply(&mut b, trade("AAA", at(10, 0), 500, 100));
        apply(&mut b, trade("AAA", at(10, 1), 520, 30));
        b.take_changes(band);
        // The newest price print is cancelled: last falls back to the engine's newest one.
        b.apply_revision(&rev(
            "AAA",
            (520, 30, true, true),
            None,
            Some((500, at(10, 0))),
        ));
        let r = b.get("AAA").unwrap();
        assert_eq!(
            (r.last, r.volume, r.trades),
            (Price::from_cents(500), 100, 1)
        );
        assert_eq!(b.take_changes(band).rows.len(), 1, "the change goes out");
        // An out-of-order print older than the restored last is still not the last price.
        apply(&mut b, trade("AAA", at(9, 59), 480, 10));
        assert_eq!(b.get("AAA").unwrap().last, Price::from_cents(500));
        // A cancel of a volume-only print moves volume, not last.
        b.apply_revision(&rev(
            "AAA",
            (480, 10, false, true),
            None,
            Some((999, at(11, 0))),
        ));
        let r = b.get("AAA").unwrap();
        assert_eq!((r.last, r.volume), (Price::from_cents(500), 100));
        // Cancelling the day's only price print: no price left, so the row leaves the view.
        b.take_changes(band);
        b.apply_revision(&rev("AAA", (500, 100, true, true), None, None));
        assert!(b.get("AAA").is_none());
        assert_eq!(b.take_changes(band).gone.len(), 1);
    }

    #[test]
    fn a_correction_swaps_the_print_and_other_days_or_symbols_are_ignored() {
        let mut b = QuoteBoard::new();
        let band = PriceBand::DEFAULT;
        apply(&mut b, trade("AAA", at(10, 0), 500, 100));
        apply(&mut b, trade("AAA", at(10, 1), 520, 30));
        b.take_changes(band);
        // Corrected to a bigger size and a new price, still the newest print.
        b.apply_revision(&rev(
            "AAA",
            (520, 30, true, true),
            Some((525, 50, true, true)),
            Some((525, at(10, 1))),
        ));
        let r = b.get("AAA").unwrap();
        assert_eq!(
            (r.last, r.volume, r.trades),
            (Price::from_cents(525), 150, 2)
        );
        // Corrected to an excluded condition: counts for nothing any more.
        b.apply_revision(&rev(
            "AAA",
            (525, 50, true, true),
            Some((525, 50, false, false)),
            Some((500, at(10, 0))),
        ));
        let r = b.get("AAA").unwrap();
        assert_eq!(
            (r.last, r.volume, r.trades),
            (Price::from_cents(500), 100, 1)
        );
        b.take_changes(band);
        b.apply_revision(&rev("ZZZ", (500, 100, true, true), None, None));
        let mut other_day = rev("AAA", (500, 100, true, true), None, None);
        other_day.session.date = NaiveDate::from_ymd_opt(2026, 10, 15).unwrap();
        b.apply_revision(&other_day);
        assert!(b.take_changes(band).is_empty(), "nothing of today changed");
        assert_eq!(b.get("AAA").unwrap().volume, 100);
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

    /// The owner's case (2026-10-30): ATYR closed at $0.128 and a 1-for-40 reverse split took
    /// effect, so today it trades at $5.17. Against the raw close that is +3,939 %.
    #[test]
    fn a_known_reverse_split_puts_the_previous_close_on_todays_basis() {
        let (idx, _) = PrevCloses::new(&{
            let mut m = market_archive::DailyCloses::new();
            m.entry(NaiveDate::from_ymd_opt(2026, 10, 15).unwrap())
                .or_default()
                .insert("ATYR".into(), 0.128);
            m
        });
        let idx = idx.with_splits([market_archive::SplitFact {
            symbol: "ATYR".into(),
            effective: day(),
            factor: 40.0,
        }]);
        let mut b = QuoteBoard::with_closes(Arc::new(idx));
        apply(&mut b, trade("ATYR", at(8, 0), 500, 100));
        apply(&mut b, trade("ATYR", at(10, 0), 517, 100));
        let r = b.get("ATYR").unwrap();
        assert_eq!(r.prev_close, Some(Price::from_cents(512)));
        assert_eq!(r.basis, PcBasis::Split);
        let pct = (r.last.to_f64() / r.prev_close.unwrap().to_f64() - 1.0) * 100.0;
        assert!((pct - 0.98).abs() < 0.01, "{pct}");
        assert_eq!(b.prev_close_on("ATYR", day()), Some(Price::from_cents(512)));
    }

    /// The same day with no split on record (the ledger had not seen it): the first print is
    /// ~39× the close, so the close is withheld and change % falls back to the day's first price,
    /// flagged `guard`. A real runner that starts near its close and climbs keeps its close.
    #[test]
    fn an_unknown_split_is_caught_by_the_guard_and_a_real_runner_is_not() {
        let mut b = QuoteBoard::with_closes(closes(&[
            ("2026-10-15", "ATYR", 0.128),
            ("2026-10-15", "RUN", 0.50),
            ("2026-10-15", "FWD", 80.0),
            ("2026-10-15", "GAP", 1.00),
        ]));
        apply(&mut b, trade("ATYR", at(8, 0), 500, 100));
        apply(&mut b, trade("ATYR", at(10, 0), 517, 100));
        let r = b.get("ATYR").unwrap();
        assert_eq!((r.prev_close, r.basis), (None, PcBasis::Guard));
        assert_eq!(
            r.reference,
            Price::from_cents(517),
            "the regular open, not the old close"
        );
        assert_eq!(
            b.prev_close_on("ATYR", day()),
            None,
            "the chart header agrees"
        );
        let s = b.scan().find(|s| &**s.symbol == "ATYR").unwrap();
        assert_eq!((s.prev_close, s.basis), (None, PcBasis::Guard));

        // +1,000 % over the day from a first print near the close: real, never guarded.
        apply(&mut b, trade("RUN", at(7, 0), 52, 100));
        apply(&mut b, trade("RUN", at(11, 0), 550, 100));
        let r = b.get("RUN").unwrap();
        assert_eq!(
            (r.prev_close, r.basis),
            (Some(Price::from_cents(50)), PcBasis::Raw)
        );

        // A 4-for-1 forward split: a quarter of the close at the first print.
        apply(&mut b, trade("FWD", at(9, 30), 2_010, 100));
        assert_eq!(b.get("FWD").unwrap().basis, PcBasis::Guard);

        // A 2.6× gap on news is not a split factor: the close stands.
        apply(&mut b, trade("GAP", at(4, 0), 260, 100));
        assert_eq!(b.get("GAP").unwrap().basis, PcBasis::Raw);
    }

    #[test]
    fn the_guard_reads_whole_split_factors_and_any_tenfold_gap() {
        let p = |c: i64| Price::from_cents(c);
        assert!(split_guard(p(517), Price::from_micros(128_000)), "1-for-40");
        assert!(split_guard(p(1_250), p(116)), "ENHA: 10.8×");
        assert!(
            split_guard(p(300), p(100)) && split_guard(p(510), p(100)),
            "1-for-3, 1-for-5"
        );
        assert!(split_guard(p(100), p(400)), "4-for-1 forward");
        assert!(!split_guard(p(250), p(100)), "2.5×: a gap, not a split");
        assert!(!split_guard(p(350), p(100)), "3.5×: between factors");
        assert!(!split_guard(p(200), p(100)), "below 3×: left alone");
        assert!(!split_guard(p(105), p(100)));
    }
}
