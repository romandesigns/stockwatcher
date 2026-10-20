//! The live market's history: the archive, read back, under the live tail.
//!
//! One store serves everything (owner, 2026-10-18): the live recorder writes what the stream
//! delivers into the archive (`market_archive::live`, a provisional day per trading date), the
//! nightly import replaces it with the complete day, and this module reads either back so a
//! restart (every deploy) keeps the day's live charts, and a closed market shows the last
//! session instead of nothing.
//!
//! # Which days ([`plan`])
//!
//! Today (the trading date of now, `market_events::session::trading_date`) when the archive has
//! anything for it, plus the previous trading day while today is still before 09:30 ET (an
//! engine that had not restarted would still show yesterday's session then; it keeps one trading
//! date of candles). When today has nothing yet (a weekend, a holiday, the hours before the
//! overnight session), the most recent day the archive has: the dashboard then shows the last
//! session, labelled with its date (`session` in `/api/symbols`), and rolls into the new session
//! when its first trades arrive. Each day is read as the archive says ([`Archive::day_data`]):
//! the import when it has finished the day, else the live recording.
//!
//! # Lazy, per symbol
//!
//! Replaying a whole-market day into the engine takes minutes (measured ~90k events/s, the
//! engine being the floor), so the market does not wait for it:
//!
//! - **At start** it lists the symbols those days have and restores the quote board from the
//!   recording's saved rows (`quotes.json`, which the quotes task rewrites every
//!   [`QUOTES_CHECKPOINT_TICKS`] seconds): candidates, prices, change and volume are there at
//!   once, and the status is `running` immediately. A day with no saved rows (an imported day)
//!   lists its rows as its symbols load.
//! - **A symbol is loaded** when a chart asks for it (its load jumps the queue; the chart gets an
//!   empty snapshot, then the real one on the `Loaded` broadcast) and, with
//!   `LIVE_HISTORY_PRELOAD` (default on), every other one in the background, busiest first, on
//!   one thread.
//! - **Until it is loaded** its live events are not applied to the engine (they would come
//!   before its history and make it all "late"): the market counts them, per trading day, and
//!   from the moment a load starts holds them (with `LIVE_RECORD=0` it holds them from the start,
//!   since no recording will have them); the board takes them with the print's own verdict
//!   ([`Counted::from_effect`]); the paper broker gets them as always.
//! - **Loading** reads the symbol's file of every day in order. For the day being recorded it
//!   first asks the recorder for a barrier (every event appended before it is on disk, below the
//!   length it reports) and keeps exactly the records before the ones the market is holding: the
//!   barrier's count says how many records this process wrote, the market's count how many of
//!   them it saw before the load began. The history goes into an engine of its own with the marks
//!   the live ingest used (`floor_1s` of each event's receive time, which the recorder stores as
//!   the delivery time), then the held events, and the symbol's state moves into the market's
//!   engine ([`ChartEngine::adopt_symbol`]: tested equal to feeding everything to one engine).
//!   Its quote row is replaced by the one that history built by the engine's verdicts.
//!
//! What a load cannot have: events the recorder dropped (its queue overflowed; counted and
//! logged), and the **restart gap** (nothing was connected; not backfilled, the archive's import
//! fills it at night).

use crate::market::{Core, Market, OutKind, Outbound, TAPE_HISTORY};
use crate::pressure::PressureBoard;
use crate::quotes::{Counted, QuoteBoard};
use chart_engine::ChartEngine;
use chart_engine::wire::ChartUpdate;
use chrono::NaiveDate;
use market_archive::{Archive, DayData};
use market_events::session::{classify, previous_trading_day, trading_date};
use market_events::{MarketEvent, Price, SessionKind, UnixNanos};
use market_providers::Recorder;
use market_providers::recorder::receive_time;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

/// The quote rows go to the recording every this many quote ticks (seconds).
pub const QUOTES_CHECKPOINT_TICKS: u64 = 10;
/// How far back a closed market looks for its last session (trading days).
const LOOKBACK_DAYS: usize = 30;

pub(crate) fn wall_now() -> UnixNanos {
    UnixNanos::from_utc(chrono::Utc::now()).unwrap_or_default()
}

/// The archive history of a live market.
pub(crate) struct History {
    archive: Archive,
    recorder: Option<Recorder>,
    /// The days loaded from, oldest first ([`plan`]).
    days: Vec<NaiveDate>,
    /// Today's trading date at start: recordings from here on are this run's and are read too.
    started_on: NaiveDate,
    /// Loads a chart asked for (they jump the background queue).
    urgent: Mutex<Sender<String>>,
    /// Symbols at start, and the background load's progress.
    total: usize,
    started: Instant,
}

/// A symbol whose archived history is not in the engine yet.
pub(crate) struct Unloaded {
    /// This run's events of the symbol the market saw while not `holding`, per trading day: the
    /// recording has them, and a load reads exactly that many of this run's records.
    seen: BTreeMap<NaiveDate, u64>,
    /// Events kept in memory, applied after the history.
    held: Vec<MarketEvent>,
    /// Events are kept in `held` rather than counted: from the start of a load, or from the start
    /// of the run when nothing records them (`LIVE_RECORD=0`), since the archive will not have them.
    holding: bool,
    /// A load is running.
    loading: bool,
    /// Already asked for urgently.
    requested: bool,
}

/// The days the history is read from at `now` (module docs), oldest first.
pub(crate) fn plan(archive: &Archive, now: UnixNanos) -> Vec<NaiveDate> {
    let today = trading_date(now);
    if has_market_data(archive, today) {
        let before_open = classify(now).is_some_and(|s| {
            s.date == today && matches!(s.kind, SessionKind::Overnight | SessionKind::Pre)
        });
        let prev = previous_trading_day(today);
        if before_open && has_market_data(archive, prev) {
            return vec![prev, today];
        }
        return vec![today];
    }
    let mut d = today;
    for _ in 0..LOOKBACK_DAYS {
        d = previous_trading_day(d);
        if has_market_data(archive, d) {
            return vec![d];
        }
    }
    Vec::new()
}

/// A day the live market may show: the live recording (whatever the stream delivered), or an
/// imported day that is market data. A **fixture** day (`SIM_AUTOFIXTURE`, `swarchive fixture`:
/// synthetic, the replay's practice data) never is: the live page never shows the fixture.
fn has_market_data(archive: &Archive, day: NaiveDate) -> bool {
    match archive.day_data(day) {
        Some(DayData::Provisional { .. }) => true,
        Some(DayData::Final) => archive
            .manifest(day)
            .ok()
            .flatten()
            .is_some_and(|m| !m.fixture),
        None => false,
    }
}

/// The symbols a day has (its import's or its recording's manifest).
fn day_symbols(archive: &Archive, day: NaiveDate) -> Vec<(String, u64)> {
    let m = match archive.day_data(day) {
        Some(DayData::Final) => archive.manifest(day),
        Some(DayData::Provisional { .. }) => archive.live_manifest(day),
        None => return Vec::new(),
    };
    match m {
        Ok(Some(m)) => m
            .symbols
            .into_values()
            .map(|e| (e.symbol, e.trades))
            .collect(),
        Ok(None) => Vec::new(),
        Err(e) => {
            tracing::warn!(%day, error = %e, "live history: manifest unreadable");
            Vec::new()
        }
    }
}

impl Market {
    /// Sets up the history (module docs): plan, symbol list, quote rows, loader thread. Called
    /// once, before the first event is applied.
    pub(crate) fn init_history(
        &self,
        archive: Archive,
        recorder: Option<Recorder>,
        now: UnixNanos,
        preload: bool,
    ) {
        let started = Instant::now();
        let days = plan(&archive, now);
        let mut trades: HashMap<String, u64> = HashMap::new();
        for d in &days {
            for (s, n) in day_symbols(&archive, *d) {
                *trades.entry(s).or_default() += n;
            }
        }
        let mut rows = 0usize;
        let order: Vec<String>;
        {
            let mut core = self.0.core.lock().expect("core lock");
            for d in &days {
                match archive.read_live_quotes(*d) {
                    Ok(Some(saved)) => {
                        rows += saved.len();
                        core.quotes.restore(&saved);
                        for r in &saved {
                            if let Some(p) = r.last {
                                core.last_price.insert(r.symbol.clone(), p);
                            }
                            core.latest_trade_day = core.latest_trade_day.max(Some(r.day));
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!(day = %d, error = %e, "live history: saved quote rows unreadable")
                    }
                }
            }
            for s in trades.keys() {
                core.unloaded.insert(
                    s.clone(),
                    Unloaded {
                        seen: BTreeMap::new(),
                        held: Vec::new(),
                        holding: recorder.is_none(),
                        loading: false,
                        requested: false,
                    },
                );
            }
            // Busiest first: by the saved day volume, then by the manifests' trade counts.
            let mut v: Vec<(u64, u64, String)> = trades
                .iter()
                .map(|(s, n)| {
                    let vol = core.quotes.get(s).map_or(0, |r| r.volume);
                    (vol, *n, s.clone())
                })
                .collect();
            v.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
            order = v.into_iter().map(|(_, _, s)| s).collect();
            core.seq += 1;
        }
        let (urgent, rx) = channel();
        let total = order.len();
        tracing::info!(
            days = ?days,
            symbols = total,
            quote_rows = rows,
            ms = started.elapsed().as_millis() as u64,
            preload,
            "live history: the market is up; symbols load from the archive on demand{}",
            if preload { " and in the background" } else { "" }
        );
        let _ = self.0.history.set(History {
            archive,
            recorder,
            days,
            started_on: trading_date(now),
            urgent: Mutex::new(urgent),
            total,
            started: Instant::now(),
        });
        if total > 0 {
            let weak = Arc::downgrade(&self.0);
            let queue: VecDeque<String> = if preload {
                order.into()
            } else {
                VecDeque::new()
            };
            let _ = std::thread::Builder::new()
                .name("live-history".into())
                .spawn(move || loader(weak, rx, queue));
        }
    }

    /// Loads one symbol's history into the engine (module docs). Returns the events read, or
    /// `None` when there was nothing to load (already loaded, or a load is running).
    pub fn load_history(&self, symbol: &str) -> Option<u64> {
        let h = self.0.history.get()?;
        let seen = {
            let mut core = self.0.core.lock().expect("core lock");
            let u = core.unloaded.get_mut(symbol)?;
            if u.loading {
                return None;
            }
            u.loading = true;
            u.holding = true;
            u.seen.clone()
        };
        let mut days = h.days.clone();
        if let Ok(live) = h.archive.live_days() {
            days.extend(live.into_iter().filter(|d| *d >= h.started_on));
        }
        days.sort();
        days.dedup();
        let mut b = Builder::new(self.current_closes());
        let mut read = 0u64;
        for day in days {
            for e in read_day(h, day, symbol, seen.get(&day).copied().unwrap_or(0)) {
                read += 1;
                b.feed(&e);
            }
        }
        let seq = {
            let mut core = self.0.core.lock().expect("core lock");
            let held = core
                .unloaded
                .remove(symbol)
                .map(|u| u.held)
                .unwrap_or_default();
            for e in &held {
                b.feed(e);
            }
            let Builder {
                mut engine,
                board,
                pressure,
                tape,
                last_price,
                ..
            } = b;
            core.engine.adopt_symbol(&mut engine, symbol);
            core.quotes.replace_from(symbol, &board);
            core.pressure.replace_from(symbol, &pressure);
            if !tape.is_empty() {
                core.tape.insert(symbol.to_string(), tape);
            }
            if let Some(p) = last_price {
                core.last_price.insert(symbol.to_string(), p);
            }
            core.seq += 1;
            core.seq
        };
        let _ = self.0.tx.send(Arc::new(Outbound {
            seq,
            kind: OutKind::Loaded(symbol.to_string()),
        }));
        Some(read)
    }

    /// A clean stop of the live recording (server shutdown): the quote rows saved, every pending
    /// record written, within `grace`.
    pub async fn close_recording(&self, grace: std::time::Duration) {
        let Some(h) = self.0.history.get() else {
            return;
        };
        let rows = self.0.core.lock().expect("core lock").quotes.export();
        let archive = h.archive.clone();
        let recorder = h.recorder.clone();
        let done = tokio::task::spawn_blocking(move || {
            save_quotes(&archive, rows);
            recorder.is_none_or(|r| r.close(grace))
        })
        .await
        .unwrap_or(false);
        if !done {
            tracing::warn!(
                "live recorder: not finished within the shutdown grace; the last seconds may be missing from the recording"
            );
        }
    }

    /// Writes the quote rows into the recording of each day they belong to (the next restart's
    /// summary). A blocking write, off the async workers; skipped without a history, and for
    /// days with no recording or already imported.
    pub fn checkpoint_quotes(&self) {
        let Some(h) = self.0.history.get() else {
            return;
        };
        let rows = self.0.core.lock().expect("core lock").quotes.export();
        let archive = h.archive.clone();
        tokio::task::spawn_blocking(move || save_quotes(&archive, rows));
    }
}

fn save_quotes(archive: &Archive, rows: Vec<market_archive::LiveQuote>) {
    let mut by_day: BTreeMap<NaiveDate, Vec<market_archive::LiveQuote>> = BTreeMap::new();
    for r in rows {
        by_day.entry(r.day).or_default().push(r);
    }
    for (day, rows) in by_day {
        if let Err(e) = archive.write_live_quotes(day, &rows) {
            tracing::warn!(%day, error = %e, "live history: could not save the quote rows");
        }
    }
}

/// Every event of `symbol` on `day`, in arrival order: the import's file, or the recording's;
/// for the day being recorded, cut where the market's held events begin (module docs).
fn read_day(h: &History, day: NaiveDate, symbol: &str, seen: u64) -> Vec<MarketEvent> {
    let a = &h.archive;
    let collect = |r: Result<market_archive::SymbolReader, market_archive::ArchiveError>| {
        match r {
            Ok(r) => r.map_while(|x| x.ok().map(|v| v.event)).collect(),
            // No file: the symbol did not trade that day.
            Err(market_archive::ArchiveError::NotFound(_)) => Vec::new(),
            Err(e) => {
                tracing::warn!(%day, symbol, error = %e, "live history: unreadable");
                Vec::new()
            }
        }
    };
    if a.is_final(day) {
        return collect(a.reader(day, symbol));
    }
    match h.recorder.as_ref().and_then(|r| r.barrier(day, symbol)) {
        Some(w) => {
            let mut events: Vec<MarketEvent> = collect(a.live_reader(day, symbol, Some(w.len)));
            let earlier = (events.len() as u64).saturating_sub(w.records);
            let keep = earlier + seen;
            if keep > events.len() as u64 {
                // The recorder dropped some of this run's events (its queue overflowed).
                tracing::warn!(%day, symbol, "live history: the recording misses events the market saw (recorder drops)");
            }
            events.truncate(keep as usize);
            events
        }
        None => collect(a.live_reader(day, symbol, None)),
    }
}

/// One symbol's history replayed on its own, as the ingest would have.
struct Builder {
    engine: ChartEngine,
    board: QuoteBoard,
    pressure: PressureBoard,
    tape: VecDeque<String>,
    last_price: Option<Price>,
    mark: Option<i64>,
}

impl Builder {
    fn new(closes: Arc<market_archive::PrevCloses>) -> Self {
        Builder {
            engine: ChartEngine::default(),
            board: QuoteBoard::with_closes(closes),
            pressure: PressureBoard::new(),
            tape: VecDeque::new(),
            last_price: None,
            mark: None,
        }
    }

    fn feed(&mut self, e: &MarketEvent) {
        let m = receive_time(e).0.div_euclid(1_000_000_000) * 1_000_000_000;
        if self.mark.is_none_or(|l| m > l) {
            self.mark = Some(m);
            // The clock moves the LULD bands too (doubling windows, the close): the Halt
            // watcher needs them, the rest of the increments are re-snapshotted.
            for u in self.engine.advance_clock(UnixNanos(m)) {
                if let ChartUpdate::Luld(l) = &u {
                    self.pressure.apply_luld(&l.symbol, &l.point);
                }
            }
        }
        if let MarketEvent::Trade(t) = e
            && t.effect().updates_ohlc
        {
            self.last_price = Some(t.price);
        }
        let (ups, rev) = self.engine.apply_revising(e);
        if let MarketEvent::Trade(t) = e
            && let Some(ChartUpdate::Tape(row)) = ups.first()
        {
            let counted = Counted::from_tape(&row.counted);
            self.board.apply_trade(t, counted);
            self.pressure.apply_trade(t, counted);
        }
        if let Some(rev) = &rev {
            self.board.apply_revision(rev);
        }
        for u in &ups {
            if let ChartUpdate::Halt(h) = u {
                self.pressure.apply_halt(&h.symbol, &h.halt);
            }
            if let ChartUpdate::Luld(l) = u {
                self.pressure.apply_luld(&l.symbol, &l.point);
            }
            if let ChartUpdate::Tape(_) = u {
                if self.tape.len() == TAPE_HISTORY {
                    self.tape.pop_front();
                }
                self.tape
                    .push_back(serde_json::to_string(u).expect("wire types serialize"));
            }
        }
    }
}

/// Holds back an event of a symbol not loaded yet (module docs). Returns false for every other
/// event, which the ingest applies as usual.
pub(crate) fn hold_unloaded(core: &mut Core, e: &MarketEvent) -> bool {
    let Some(symbol) = e.symbol() else {
        return false;
    };
    let Core {
        unloaded,
        quotes,
        pressure,
        ..
    } = core;
    let Some(u) = unloaded.get_mut(symbol) else {
        return false;
    };
    if u.holding {
        u.held.push(e.clone());
    } else {
        *u.seen.entry(trading_date(receive_time(e))).or_default() += 1;
    }
    if let MarketEvent::Trade(t) = e {
        // Provisional, like the quote row: the load replaces both with what the history built.
        quotes.apply_trade(t, Counted::from_effect(t));
        pressure.apply_trade(t, Counted::from_effect(t));
    }
    true
}

/// Puts `symbol` at the front of the load queue when its history is not loaded yet.
pub(crate) fn request_load(market: &Market, core: &mut Core, symbol: &str) {
    let Some(u) = core.unloaded.get_mut(symbol) else {
        return;
    };
    if u.requested || u.loading {
        return;
    }
    u.requested = true;
    if let Some(h) = market.0.history.get() {
        let _ = h
            .urgent
            .lock()
            .expect("urgent lock")
            .send(symbol.to_string());
    }
}

/// `session` in `/api/symbols` and `/api/replay/state` (live): the trading day shown.
pub(crate) fn session_json(market: &Market) -> Value {
    let Some(h) = market.0.history.get() else {
        return Value::Null;
    };
    let latest = market.0.core.lock().expect("core lock").latest_trade_day;
    let Some(day) = latest.max(h.days.last().copied()) else {
        return Value::Null;
    };
    json!({
        "day": day.to_string(),
        "current": day >= trading_date(wall_now()),
        "provisional": h.archive.day_data(day) != Some(DayData::Final),
    })
}

/// `history` in `/api/replay/state` (live): what was loaded from the archive.
pub(crate) fn history_json(market: &Market) -> Value {
    let Some(h) = market.0.history.get() else {
        return Value::Null;
    };
    let pending = market.0.core.lock().expect("core lock").unloaded.len();
    json!({
        "days": h.days.iter().map(|d| d.to_string()).collect::<Vec<_>>(),
        "symbols": h.total,
        "pending": pending,
    })
}

/// The loader thread: urgent requests first, then (with preload) the background queue.
fn loader(weak: Weak<crate::market::Inner>, urgent: Receiver<String>, mut queue: VecDeque<String>) {
    let mut done_logged = queue.is_empty();
    loop {
        let next = match urgent.try_recv() {
            Ok(s) => Some(s),
            Err(TryRecvError::Disconnected) => return,
            Err(TryRecvError::Empty) => queue.pop_front(),
        };
        let symbol = match next {
            Some(s) => s,
            None => {
                if !done_logged {
                    done_logged = true;
                    if let Some(inner) = weak.upgrade()
                        && let Some(h) = inner.history.get()
                    {
                        tracing::info!(
                            symbols = h.total,
                            secs = h.started.elapsed().as_secs_f64(),
                            "live history: every symbol loaded"
                        );
                    }
                }
                match urgent.recv() {
                    Ok(s) => s,
                    Err(_) => return,
                }
            }
        };
        let Some(inner) = weak.upgrade() else { return };
        Market(inner).load_history(&symbol);
    }
}
