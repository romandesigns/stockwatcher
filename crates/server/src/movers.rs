//! The dashboard's movers (owner, 2026-10-30): **Top Gainers**, and **Hot stocks** split into
//! **Bull** and **Bear** ("make the hot stocks panel into two sections, bull and bear ... make
//! use of rust live and realtime sockets capabilities, we don't want to create any sort of
//! bottleneck"), ranked here once per tick for every client and pushed over `/ws`.
//!
//! The lists used to be ranked in the browser from the whole quote board, which kept a
//! `quotes` subscription carrying thousands of rows open on every dashboard. Now the server ranks
//! its own board once a tick ([`rank`]) and each dashboard receives at most 75 rows, and after the
//! snapshot only the rows that changed.
//!
//! # The lists (the definitions packages/shared/src/tickers/movers.ts had)
//!
//! Candidates: every quote-board row with a price inside [`BAND`] ($0.10–$25) and at least
//! [`MIN_PRINTS`] counted prints. Change % is against the previous close when the closes file has
//! one, else against the board's reference (the regular open, else the day's first price): the
//! search's and the chart header's rule (`quoteChangePct`). Dollar volume is last price × day
//! volume (the board keeps no turnover or VWAP; for a $0.10–$25 ranking the last price is close
//! enough).
//!
//! - `gainers`: candidates up on the day with at least [`GAINERS_MIN_DOLLAR_VOLUME`] traded, by
//!   change % (ties: dollar volume, then symbol). "Up on the day" is new: the browser ranking
//!   let a flat or falling name fill the bottom of a thin list (premarket), which read as a bug.
//! - `bull`: candidates up on the day (change > 0), by dollar volume (ties: symbol).
//! - `bear`: candidates down on the day (change < 0), by dollar volume (ties: symbol).
//!
//! The top [`LIMIT`] of each, so a client re-sorting a list (price, change, ticker) has a real set
//! to sort. A symbol can sit in `gainers` and `bull` at once; it has one row.
//!
//! # Wire (after `welcome`; additive, an older client never subscribes)
//!
//! - client `movers.subscribe` -> server `movers.snapshot {t, gainers, bull, bear, rows}` at once:
//!   the three lists as symbol arrays in rank order, and one row per symbol in any of them, each
//!   with its WHOLE day line. `t` is the market clock (epoch ms) of the frame.
//! - then `movers.update {t, rows, gainers?, bull?, bear?, gone?}` when anything changed, at most
//!   once per second of wall time: a list only when its order or membership changed (whole, it
//!   replaces the client's); `rows` only the rows whose fields changed, that entered, or whose day
//!   line gained points; `gone` the symbols no longer in any list (drop the row and its line).
//! - A row: `symbol`, `price` (micros, the board's last), `pc` (previous close, micros, when
//!   known), `ref` (the fallback reference, micros), `chg` (change %, 3 decimals), `dv` (dollar
//!   volume, whole dollars), `volume`, `trades`, `halted` (only when true), plus the day line
//!   fields exactly as `ross.*` rows carry them (`crate::day_line`: whole in a snapshot and for
//!   an entering row, a slice in an update; a client that cannot place a slice re-subscribes).
//! - client `movers.unsubscribe` stops the updates; a seek, a day switch or a lag sends a fresh
//!   snapshot, as for every channel.
//!
//! # Cost and memory
//!
//! [`rank`] is one pass over the board's priced rows (no allocation per row, no symbol clone),
//! then a partial selection of the top [`LIMIT`] per list: measured on a 13,000-symbol board in
//! the test `tick_cost_on_a_13k_board` (`--nocapture` prints it). It runs on the quotes task
//! whether or not anyone listens, so the baseline a new subscriber's snapshot is cut from is
//! never stale; the day lines are read only while someone is subscribed, under the core lock at
//! serialization time, as `crate::day_line` explains.
//!
//! Everything kept is bounded by the lists, never by the market: [`Board::last`] holds at most
//! 3 × [`LIMIT`] rows and [`Board::lines`] at most one frame's rows (`decorate` replaces it with
//! the frame's members every time). A symbol that leaves keeps nothing here; if it comes back it
//! is entering again and gets its whole line.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use market_events::PriceBand;
use serde_json::{Map, Value, json};

use crate::quotes::{QuoteBoard, ScanRow};

/// The price band every list keeps ($0.10–$25, the Ross Panel's band).
pub const BAND: PriceBand = PriceBand::DEFAULT;
/// Fewer prints than this is not a market yet: one stray print must not top a list.
pub const MIN_PRINTS: u64 = 100;
/// Top Gainers also needs real money behind the move.
pub const GAINERS_MIN_DOLLAR_VOLUME: f64 = 250_000.0;
/// Rows kept per list.
pub const LIMIT: usize = 25;
/// The shortest gap between two rankings ([`Throttle`]). The quotes task already ticks once per
/// wall-second boundary; this is the backstop that keeps a second caller (or a tick that lands
/// early after an overrun) from doubling the frame rate. Under a second so the phase-locked tick's
/// jitter never skips a second.
pub const MIN_GAP: Duration = Duration::from_millis(900);

/// The three lists, in wire order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum List {
    Gainers,
    Bull,
    Bear,
}

impl List {
    pub const ALL: [List; 3] = [List::Gainers, List::Bull, List::Bear];

    pub fn key(self) -> &'static str {
        match self {
            List::Gainers => "gainers",
            List::Bull => "bull",
            List::Bear => "bear",
        }
    }
}

/// One row as published (module docs, "Wire").
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub symbol: Arc<str>,
    /// Micros.
    pub price: i64,
    pub prev_close: Option<i64>,
    pub reference: i64,
    /// Percent, rounded to 3 decimals (so a sub-rounding wobble is not a change).
    pub change_pct: f64,
    /// Whole dollars.
    pub dollar_volume: u64,
    pub volume: u64,
    pub trades: u64,
    pub halted: bool,
    /// How `prev_close` was arrived at (split-adjusted, or withheld by the split guard).
    pub basis: crate::quotes::PcBasis,
}

impl Row {
    pub fn json(&self) -> Value {
        let mut m = Map::new();
        m.insert("symbol".into(), json!(&*self.symbol));
        m.insert("price".into(), json!(self.price));
        if let Some(pc) = self.prev_close {
            m.insert("pc".into(), json!(pc));
        }
        m.insert("ref".into(), json!(self.reference));
        m.insert("chg".into(), json!(self.change_pct));
        m.insert("dv".into(), json!(self.dollar_volume));
        m.insert("volume".into(), json!(self.volume));
        m.insert("trades".into(), json!(self.trades));
        if let Some(tag) = self.basis.tag() {
            m.insert("pcb".into(), json!(tag));
        }
        if self.halted {
            m.insert("halted".into(), json!(true));
        }
        Value::Object(m)
    }
}

/// One tick's ranking: the lists (symbols in rank order) and a row per member.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ranked {
    pub lists: [Vec<Arc<str>>; 3],
    pub rows: HashMap<Arc<str>, Row>,
}

impl Ranked {
    pub fn list(&self, l: List) -> &[Arc<str>] {
        &self.lists[l as usize]
    }

    /// The rows sorted by symbol (a stable frame order).
    fn sorted_rows(&self) -> Vec<&Row> {
        let mut rows: Vec<&Row> = self.rows.values().collect();
        rows.sort_by(|a, b| a.symbol.cmp(&b.symbol));
        rows
    }
}

/// A candidate with what the lists sort by.
#[derive(Clone, Copy)]
struct Cand<'a> {
    row: ScanRow<'a>,
    chg: f64,
    dv: f64,
}

/// Change % by the search's rule (module docs), rounded to 3 decimals.
pub(crate) fn change_pct(r: &ScanRow<'_>) -> f64 {
    let base = r
        .prev_close
        .filter(|p| p.micros() > 0)
        .unwrap_or(r.reference)
        .micros();
    if base == 0 {
        return 0.0;
    }
    let pct = (r.last.micros() - base) as f64 / base as f64 * 100.0;
    (pct * 1000.0).round() / 1000.0
}

/// The indices of the best [`LIMIT`] candidates that `keep`, best first by `cmp`.
fn top(
    cands: &[Cand<'_>],
    keep: impl Fn(&Cand<'_>) -> bool,
    cmp: impl Fn(&Cand<'_>, &Cand<'_>) -> std::cmp::Ordering,
) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..cands.len()).filter(|&i| keep(&cands[i])).collect();
    let by = |a: &usize, b: &usize| cmp(&cands[*a], &cands[*b]);
    if idx.len() > LIMIT {
        idx.select_nth_unstable_by(LIMIT - 1, by);
        idx.truncate(LIMIT);
    }
    idx.sort_by(by);
    idx
}

/// Ranks the board (module docs). `halted` says whether a symbol is halted right now.
pub fn rank(quotes: &QuoteBoard, halted: impl Fn(&str) -> bool) -> Ranked {
    let cands: Vec<Cand<'_>> = quotes
        .scan()
        .filter(|r| BAND.contains(r.last) && r.trades >= MIN_PRINTS)
        .map(|row| Cand {
            row,
            chg: change_pct(&row),
            dv: row.last.to_f64() * row.volume as f64,
        })
        .collect();
    let by_symbol = |a: &Cand<'_>, b: &Cand<'_>| a.row.symbol.cmp(b.row.symbol);
    let by_dv = |a: &Cand<'_>, b: &Cand<'_>| b.dv.total_cmp(&a.dv).then_with(|| by_symbol(a, b));
    let gainers = top(
        &cands,
        |c| c.chg > 0.0 && c.dv >= GAINERS_MIN_DOLLAR_VOLUME,
        |a, b| b.chg.total_cmp(&a.chg).then_with(|| by_dv(a, b)),
    );
    let bull = top(&cands, |c| c.chg > 0.0, by_dv);
    let bear = top(&cands, |c| c.chg < 0.0, by_dv);
    let mut out = Ranked::default();
    for (slot, picked) in [gainers, bull, bear].into_iter().enumerate() {
        for i in picked {
            let c = &cands[i];
            let symbol = c.row.symbol.clone();
            out.lists[slot].push(symbol.clone());
            out.rows.entry(symbol.clone()).or_insert_with(|| Row {
                halted: halted(&symbol),
                symbol,
                price: c.row.last.micros(),
                prev_close: c.row.prev_close.map(|p| p.micros()),
                reference: c.row.reference.micros(),
                change_pct: c.chg,
                dollar_volume: c.dv.round() as u64,
                volume: c.row.volume,
                trades: c.row.trades,
                basis: c.row.basis,
            });
        }
    }
    out
}

/// What an update says beyond the previous ranking.
#[derive(Debug, Default, PartialEq)]
pub struct Diff {
    /// The lists whose order or membership changed (whole).
    pub lists: [Option<Vec<Arc<str>>>; 3],
    /// Members whose row changed or that entered, by symbol.
    pub rows: Vec<Arc<str>>,
    /// Symbols that were members and are in no list now, by symbol.
    pub gone: Vec<Arc<str>>,
}

impl Diff {
    pub fn is_empty(&self) -> bool {
        self.lists.iter().all(Option::is_none) && self.rows.is_empty() && self.gone.is_empty()
    }
}

pub fn diff(prev: &Ranked, next: &Ranked) -> Diff {
    let mut d = Diff::default();
    for l in List::ALL {
        if prev.list(l) != next.list(l) {
            d.lists[l as usize] = Some(next.list(l).to_vec());
        }
    }
    d.rows = next
        .rows
        .iter()
        .filter(|(s, r)| prev.rows.get(*s) != Some(*r))
        .map(|(s, _)| s.clone())
        .collect();
    d.rows.sort();
    d.gone = prev
        .rows
        .keys()
        .filter(|s| !next.rows.contains_key(*s))
        .cloned()
        .collect();
    d.gone.sort();
    d
}

/// At most one ranking per [`MIN_GAP`] of wall time.
#[derive(Debug, Default)]
pub struct Throttle {
    last: Option<Instant>,
}

impl Throttle {
    /// Whether a ranking may run at `now`; if so, `now` becomes the last one.
    pub fn ready(&mut self, now: Instant) -> bool {
        if self
            .last
            .is_some_and(|l| now.saturating_duration_since(l) < MIN_GAP)
        {
            return false;
        }
        self.last = Some(now);
        true
    }
}

/// The market's side of the channel: the last ranking (what snapshots are cut from and what the
/// next update is a diff against), the day lines the last update carried, and the frames'
/// sequence.
#[derive(Debug, Default)]
pub struct Board {
    pub(crate) last: Ranked,
    pub(crate) lines: crate::day_line::Published,
    /// Bumped whenever the ranking changes and read by every snapshot: an update with a
    /// generation at or below a connection's snapshot is already in it. Not the engine seq,
    /// which stands still while a replay is paused.
    pub(crate) generation: u64,
    pub(crate) throttle: Throttle,
}

/// Symbols as a JSON array.
fn strs(l: &[Arc<str>]) -> Value {
    Value::Array(l.iter().map(|s| Value::String(s.to_string())).collect())
}

/// A whole `movers.snapshot` body from `r` (day lines are added by the caller).
pub fn snapshot_frame(t: Option<i64>, r: &Ranked) -> Value {
    let mut v = json!({"type": "movers.snapshot", "t": t});
    let m = v.as_object_mut().expect("object");
    for l in List::ALL {
        m.insert(l.key().into(), strs(r.list(l)));
    }
    m.insert(
        "rows".into(),
        Value::Array(r.sorted_rows().into_iter().map(Row::json).collect()),
    );
    v
}

/// Every member's row of `r`, for [`crate::day_line::decorate`] to put the lines on before
/// [`update_frame`] keeps the ones to send.
pub fn member_rows(r: &Ranked) -> Value {
    json!({"rows": r.sorted_rows().into_iter().map(Row::json).collect::<Vec<_>>()})
}

/// The `movers.update` for `d`, from the members' decorated rows: a row goes out when the diff
/// names it or its day-line slice has points. `None` when there is nothing to say.
pub fn update_frame(t: Option<i64>, d: &Diff, decorated: Value) -> Option<Value> {
    let named: HashSet<&str> = d.rows.iter().map(|s| &**s).collect();
    let rows: Vec<Value> = match decorated {
        Value::Object(mut m) => match m.remove("rows") {
            Some(Value::Array(rows)) => rows
                .into_iter()
                .filter(|r| {
                    r["symbol"].as_str().is_some_and(|s| named.contains(s))
                        || r["day_line"].as_array().is_some_and(|l| !l.is_empty())
                })
                .collect(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    };
    if d.is_empty() && rows.is_empty() {
        return None;
    }
    let mut v = json!({"type": "movers.update", "t": t, "rows": rows});
    let m = v.as_object_mut().expect("object");
    for l in List::ALL {
        if let Some(list) = &d.lists[l as usize] {
            m.insert(l.key().into(), strs(list));
        }
    }
    if !d.gone.is_empty() {
        m.insert("gone".into(), strs(&d.gone));
    }
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::day_line::{Published, Source};
    use crate::quotes::Counted;
    use chart_engine::ChartEngine;
    use chrono::NaiveDate;
    use market_archive::PrevCloses;
    use market_events::session::et;
    use market_events::{MarketEvent, Price, Source as Feed, Tape, Trade, UnixNanos};

    fn at(h: u32, m: u32, s: u32) -> UnixNanos {
        et(NaiveDate::from_ymd_opt(2026, 10, 15).unwrap(), h, m, s, 0).unwrap()
    }

    const BOTH: Counted = Counted {
        ohlc: true,
        volume: true,
    };

    /// A board row: opens at `open_cents` (premarket first print) and trades `prints` prints of
    /// `size` shares, the last at `last_cents`.
    fn put(
        b: &mut QuoteBoard,
        sym: &str,
        open_cents: i64,
        last_cents: i64,
        prints: u64,
        size: u64,
    ) {
        b.apply_trade(
            &Trade::new(sym, at(8, 0, 0), Price::from_cents(open_cents), size, 1),
            BOTH,
        );
        for i in 1..prints {
            b.apply_trade(
                &Trade::new(
                    sym,
                    at(8, 0, 1 + (i % 50) as u32),
                    Price::from_cents(last_cents),
                    size,
                    1 + i,
                ),
                BOTH,
            );
        }
    }

    fn syms(l: &[Arc<str>]) -> Vec<&str> {
        l.iter().map(|s| &**s).collect()
    }

    #[test]
    fn lists_keep_the_band_the_print_floor_and_rank_by_their_key() {
        let mut b = QuoteBoard::new();
        // Gainers: change %, $250K floor. UP50 +50% on $300K, UP10 +10% on $1M, THIN +90% on $20K.
        put(&mut b, "UP50", 200, 300, 100, 1_000); // 100 × 1,000 × $3 = $300K
        put(&mut b, "UP10", 1_000, 1_100, 100, 1_000); // $1.1M
        put(&mut b, "THIN", 100, 190, 100, 100); // $19K: up, but not a gainer
        put(&mut b, "FEW", 100, 500, 99, 100_000); // 99 prints: nowhere
        put(&mut b, "PRICY", 2_400, 2_600, 200, 10_000); // $26: outside the band
        put(&mut b, "DOWN", 1_000, 800, 100, 10_000); // −20% on $8M
        put(&mut b, "DIP", 500, 490, 100, 1_000); // −2% on $490K
        put(&mut b, "FLAT", 500, 500, 100, 1_000_000); // 0%: neither bull nor bear
        let r = rank(&b, |_| false);
        assert_eq!(syms(r.list(List::Gainers)), ["UP50", "UP10", "FLAT"][..2]);
        assert_eq!(syms(r.list(List::Bull)), ["UP10", "UP50", "THIN"]);
        assert_eq!(syms(r.list(List::Bear)), ["DOWN", "DIP"]);
        let up = &r.rows[&Arc::from("UP50")];
        assert_eq!(up.price, 3_000_000);
        assert_eq!(up.change_pct, 50.0);
        assert_eq!(up.dollar_volume, 300_000 - 2 * 1_000 + 2_000);
        assert_eq!(up.trades, 100);
        assert!(!r.rows.contains_key("FEW") && !r.rows.contains_key("PRICY"));
        assert!(!r.rows.contains_key("FLAT"), "FLAT is in no list: no row");
    }

    #[test]
    fn the_previous_close_is_the_reference_when_known_and_ties_break_stably() {
        let mut closes = market_archive::DailyCloses::new();
        closes
            .entry(NaiveDate::from_ymd_opt(2026, 10, 14).unwrap())
            .or_default()
            .insert("PCUP".into(), 2.0);
        let mut b = QuoteBoard::with_closes(Arc::new(PrevCloses::new(&closes).0));
        // Opened at $5 and fell to $4, but closed yesterday at $2: +100%, a bull.
        put(&mut b, "PCUP", 500, 400, 100, 1_000);
        // Two equal rows rank by symbol.
        put(&mut b, "BBB", 100, 200, 100, 2_000);
        put(&mut b, "AAA", 100, 200, 100, 2_000);
        let r = rank(&b, |s| s == "AAA");
        assert_eq!(r.rows[&Arc::from("PCUP")].change_pct, 100.0);
        assert_eq!(r.rows[&Arc::from("PCUP")].prev_close, Some(2_000_000));
        assert_eq!(
            syms(r.list(List::Bull)),
            ["AAA", "BBB", "PCUP"],
            "equal dollar volume: by symbol"
        );
        assert!(r.rows[&Arc::from("AAA")].halted);
        assert_eq!(r.rows[&Arc::from("AAA")].json()["halted"], true);
        assert!(r.rows[&Arc::from("BBB")].json().get("halted").is_none());
    }

    #[test]
    fn each_list_keeps_its_top_twenty_five() {
        let mut b = QuoteBoard::new();
        for i in 0..60i64 {
            put(
                &mut b,
                &format!("U{i:02}"),
                100,
                101 + i,
                100,
                10_000 + i as u64,
            );
            put(
                &mut b,
                &format!("D{i:02}"),
                2_000,
                1_000 - i,
                100,
                10_000 + i as u64,
            );
        }
        let r = rank(&b, |_| false);
        for l in List::ALL {
            assert_eq!(r.list(l).len(), LIMIT, "{l:?}");
        }
        // Top change and top dollar volume both go to the last ones added.
        assert_eq!(&*r.list(List::Gainers)[0], "U59");
        assert_eq!(&*r.list(List::Gainers)[LIMIT - 1], "U35");
        assert_eq!(&*r.list(List::Bull)[0], "U59");
        assert_eq!(
            &*r.list(List::Bear)[0],
            "D00",
            "$10 × volume beats $9.41 × a bit more"
        );
        assert!(r.rows.len() <= 3 * LIMIT);
    }

    #[test]
    fn the_diff_names_changed_and_entering_rows_changed_lists_and_gone_symbols() {
        let mut b = QuoteBoard::new();
        put(&mut b, "AAA", 100, 200, 100, 10_000);
        put(&mut b, "BBB", 100, 150, 100, 10_000);
        let first = rank(&b, |_| false);
        let d = diff(&Ranked::default(), &first);
        assert_eq!(syms(&d.rows), ["AAA", "BBB"], "everything enters");
        assert!(d.lists[List::Bull as usize].is_some() && d.lists[List::Bear as usize].is_none());

        assert!(
            diff(&first, &rank(&b, |_| false)).is_empty(),
            "nothing changed"
        );

        // BBB trades higher (its row changes and it overtakes AAA on dollar volume); CCC enters.
        b.apply_trade(
            &Trade::new("BBB", at(9, 0, 0), Price::from_cents(160), 500_000, 999),
            BOTH,
        );
        put(&mut b, "CCC", 1_000, 900, 100, 1_000);
        let second = rank(&b, |_| false);
        let d = diff(&first, &second);
        assert_eq!(syms(&d.rows), ["BBB", "CCC"]);
        assert_eq!(
            syms(d.lists[List::Bull as usize].as_ref().unwrap()),
            ["BBB", "AAA"]
        );
        assert_eq!(
            syms(d.lists[List::Bear as usize].as_ref().unwrap()),
            ["CCC"]
        );
        assert!(d.gone.is_empty());

        // CCC climbs back to flat: in no list, so gone.
        b.apply_trade(
            &Trade::new("CCC", at(9, 1, 0), Price::from_cents(1_000), 100, 1_000),
            BOTH,
        );
        let third = rank(&b, |_| false);
        let d = diff(&second, &third);
        assert_eq!(syms(&d.gone), ["CCC"]);
        assert_eq!(d.lists[List::Bear as usize], Some(vec![]));
        assert!(
            !third.rows.contains_key("CCC"),
            "nothing kept for a symbol that left"
        );
    }

    #[test]
    fn the_throttle_lets_one_ranking_through_per_gap() {
        let mut t = Throttle::default();
        let t0 = Instant::now();
        assert!(t.ready(t0));
        assert!(!t.ready(t0));
        assert!(!t.ready(t0 + Duration::from_millis(500)));
        assert!(!t.ready(t0 + MIN_GAP - Duration::from_millis(1)));
        assert!(t.ready(t0 + MIN_GAP));
        // Phase-locked ticks a second apart (with jitter either way) always pass.
        let mut t = Throttle::default();
        for (i, jitter) in [0i64, 40, -40, 25, -60, 0].into_iter().enumerate() {
            let at = t0 + Duration::from_millis((1_000 * i as i64 + 100 + jitter) as u64);
            assert!(t.ready(at), "tick {i}");
        }
        // A clock that steps back does not wedge it (saturating), it just waits the gap.
        let mut t = Throttle::default();
        assert!(t.ready(t0 + Duration::from_secs(5)));
        assert!(!t.ready(t0));
    }

    fn engine_trade(e: &mut ChartEngine, sym: &str, t: UnixNanos, cents: i64, seq: u64) {
        e.apply(&MarketEvent::Trade(
            Trade::new(sym, t, Price::from_cents(cents), 100, seq)
                .with_exchange("V")
                .with_tape(Tape::C)
                .with_source(Feed::Fixture),
        ));
    }

    /// The update keeps a member's line continuous across frames (each slice starts where the
    /// last frame's line ended), sends a member whose fields did not change only when its line
    /// gained points, and gives a symbol that left and came back its whole line again.
    #[test]
    fn day_line_slices_stay_continuous_and_a_returning_symbol_gets_its_whole_line() {
        let mut e = ChartEngine::default();
        let closes = PrevCloses::default();
        let mut b = QuoteBoard::new();
        put(&mut b, "AAA", 100, 200, 100, 10_000);
        put(&mut b, "BBB", 100, 150, 100, 10_000);
        engine_trade(&mut e, "AAA", at(4, 0, 5), 100, 1);
        engine_trade(&mut e, "BBB", at(4, 0, 5), 100, 2);
        e.advance_clock(at(4, 1, 1));
        let mut lines = Published::default();
        let mut client: HashMap<String, Vec<i64>> = HashMap::new();
        let step = |client: &mut HashMap<String, Vec<i64>>,
                    e: &ChartEngine,
                    prev: &Ranked,
                    next: &Ranked,
                    lines: &mut Published| {
            let d = diff(prev, next);
            let mut all = member_rows(next);
            crate::day_line::decorate(
                &mut all,
                Source {
                    engine: e,
                    closes: &closes,
                },
                Some(lines),
            );
            let frame = update_frame(Some(0), &d, all);
            if let Some(f) = &frame {
                for g in f["gone"].as_array().into_iter().flatten() {
                    client.remove(g.as_str().unwrap());
                }
                for r in f["rows"].as_array().unwrap() {
                    let s = r["symbol"].as_str().unwrap().to_owned();
                    let from = r["day_line_from"].as_u64().unwrap() as usize;
                    let held = client.entry(s).or_default();
                    assert!(held.len() >= from, "a slice the client can place");
                    held.truncate(from);
                    held.extend(
                        r["day_line"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|v| v.as_i64().unwrap()),
                    );
                }
            }
            frame
        };
        let r1 = rank(&b, |_| false);
        let f = step(&mut client, &e, &Ranked::default(), &r1, &mut lines).unwrap();
        assert_eq!(f["rows"].as_array().unwrap().len(), 2);
        assert_eq!(f["rows"][0]["day_line_from"], 0);
        assert_eq!(client["AAA"], vec![1_000_000, 1_000_000]);

        // Nothing changed at all: no frame.
        assert!(step(&mut client, &e, &r1, &r1, &mut lines).is_none());

        // A new completed interval: both rows go out with one point each, fields unchanged.
        engine_trade(&mut e, "AAA", at(4, 1, 10), 120, 3);
        e.advance_clock(at(4, 1, 31));
        let f = step(&mut client, &e, &r1, &r1, &mut lines).unwrap();
        let rows = f["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["day_line_from"], 2);
        assert_eq!(rows[0]["day_line"], json!([1_200_000]));
        assert!(
            f.get("bull").is_none() && f.get("gone").is_none(),
            "no list changed"
        );
        assert_eq!(client["AAA"], vec![1_000_000, 1_000_000, 1_200_000]);

        // BBB leaves (flat on the day), then comes back: whole line again.
        b.apply_trade(
            &Trade::new("BBB", at(9, 0, 0), Price::from_cents(100), 100, 50),
            BOTH,
        );
        let r2 = rank(&b, |_| false);
        let f = step(&mut client, &e, &r1, &r2, &mut lines).unwrap();
        assert_eq!(f["gone"], json!(["BBB"]));
        assert_eq!(lines.len(), 1, "only the members' lines are kept");
        b.apply_trade(
            &Trade::new("BBB", at(9, 0, 1), Price::from_cents(130), 100, 51),
            BOTH,
        );
        let r3 = rank(&b, |_| false);
        let f = step(&mut client, &e, &r2, &r3, &mut lines).unwrap();
        let bbb = f["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["symbol"] == "BBB")
            .unwrap()
            .clone();
        assert_eq!(bbb["day_line_from"], 0);
        assert_eq!(client["BBB"].len(), 3);
        assert_eq!(client["AAA"].len(), 3);
    }

    /// The load check the owner asked for ("we don't want to create any sort of bottleneck"):
    /// one tick on a 13,000-symbol board, every row priced and most of them candidates, and the
    /// day lines of 75 members over a full 04:00–20:00 day. Run with `--nocapture` to print.
    #[test]
    fn tick_cost_on_a_13k_board() {
        let mut b = QuoteBoard::new();
        for i in 0..13_000u64 {
            let sym = format!("S{i:05}");
            let open = 50 + (i % 2_000) as i64;
            let last = open + (i as i64 * 7919 % 400) - 200;
            b.apply_trade(
                &Trade::new(&sym, at(8, 0, 0), Price::from_cents(open), 100, 1),
                BOTH,
            );
            let t = Trade::new(
                &sym,
                at(8, 0, 1),
                Price::from_cents(last.max(1)),
                1_000 + i * 37 % 50_000,
                2,
            );
            b.apply_trade(&t, BOTH);
            // The print floor: only the trade count matters, so add it without 100 calls.
            for k in 0..(if i % 5 == 0 { 10 } else { 120 }) {
                b.apply_trade(
                    &Trade::new(&sym, at(8, 0, 2), Price::from_cents(last.max(1)), 1, 3 + k),
                    Counted {
                        ohlc: false,
                        volume: true,
                    },
                );
            }
        }
        // Warm, then time ranking + diff.
        let mut prev = rank(&b, |_| false);
        let n = 20;
        let t0 = Instant::now();
        for _ in 0..n {
            let next = rank(&b, |_| false);
            let _ = diff(&prev, &next);
            prev = next;
        }
        let per_rank = t0.elapsed() / n;

        // The serialization side: 75 members' full-day lines (1,920 points each).
        let mut e = ChartEngine::default();
        let members: Vec<Arc<str>> = prev.rows.keys().cloned().collect();
        let mut seq = 0;
        let mut ts = at(4, 0, 0).0;
        while ts < at(19, 59, 30).0 {
            for (k, s) in members.iter().enumerate() {
                seq += 1;
                engine_trade(
                    &mut e,
                    s,
                    UnixNanos(ts),
                    100 + ((ts / 1_000_000_000 + k as i64) % 97),
                    seq,
                );
            }
            ts += 30_000_000_000;
        }
        e.advance_clock(at(20, 0, 0));
        let closes = PrevCloses::default();
        let src = Source {
            engine: &e,
            closes: &closes,
        };
        let mut lines = Published::default();
        let mut all = member_rows(&prev);
        crate::day_line::decorate(&mut all, src, Some(&mut lines));
        let t1 = Instant::now();
        for _ in 0..n {
            let mut all = member_rows(&prev);
            crate::day_line::decorate(&mut all, src, Some(&mut lines));
            let _ = update_frame(Some(0), &Diff::default(), all);
        }
        let per_lines = t1.elapsed() / n;
        eprintln!(
            "movers tick on 13,000 symbols: rank + diff {per_rank:?}; {} members' day lines + update {per_lines:?}",
            prev.rows.len()
        );
        assert!(prev.rows.len() > 25 && prev.rows.len() <= 75);
        // Generous bounds (debug builds, a busy CI box): the point is milliseconds, not seconds.
        assert!(per_rank < Duration::from_millis(100), "{per_rank:?}");
        assert!(per_lines < Duration::from_millis(250), "{per_lines:?}");
    }
}
