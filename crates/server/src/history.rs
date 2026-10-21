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
use market_archive::admissions::Kind;
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
    /// The admission journal per trading date, parsed once and shared by every symbol's load.
    ///
    /// The journal is one sequence for the whole market, so parsing it inside each symbol's load
    /// would re-read and re-index the whole day's admissions per symbol — thousands of symbols
    /// times a day's prints. It is read once per date here, and re-read only when the file set
    /// has grown ([`JournalCache`]).
    journals: Mutex<HashMap<NaiveDate, JournalCache>>,
    /// Symbols at start, and the background load's progress.
    total: usize,
    started: Instant,
}

/// A symbol whose archived history is not in the engine yet.
pub(crate) struct Unloaded {
    /// This run's events of the symbol the market saw while not `holding`, per trading day: the
    /// recording has them, and a load reads exactly that many of this run's records.
    seen: BTreeMap<NaiveDate, u64>,
    /// Events kept in memory, applied after the history — bounded by [`HELD_MAX`].
    ///
    /// It was an unbounded `Vec`, which is two problems in one. A symbol whose load is slow (a
    /// large archive, a queue behind it) holds the whole live tail in memory with nothing to stop
    /// it; and the entire tail was then fed through the [`Builder`] **under the core lock**, so a
    /// long tail paused every dashboard in the process while it replayed.
    held: VecDeque<MarketEvent>,
    /// Events dropped because `held` was full. Each is volume this symbol's restore will not
    /// have, so the symbol's coverage says so rather than the figure being quietly short.
    overflow: u64,
    /// Events the ingest has offered to hold since the hold began, dropped ones included.
    ///
    /// The catch-up cursor, and monotone: a load drains `held` off the core lock, so more events
    /// can arrive while it is replaying, and it compares this count with what it has consumed to
    /// know whether the tail moved under it instead of assuming it did not. A deferred load puts
    /// its events back without changing this, because they are the same occurrences returning.
    pushed: u64,
    /// Events are kept in `held` rather than counted: from the start of a load, or from the start
    /// of the run when nothing records them (`LIVE_RECORD=0`), since the archive will not have them.
    holding: bool,
    /// A load is running.
    loading: bool,
    /// Already asked for urgently.
    requested: bool,
}

impl Unloaded {
    /// Takes everything held now, leaving the cursor where it is.
    fn take_held(&mut self) -> Vec<MarketEvent> {
        self.held.drain(..).collect()
    }

    /// Puts drained events back at the front, in order, after a load gave up.
    ///
    /// A deferred load discards its replay, so every event it had drained has to go back or it
    /// is simply lost: the archive cutoff (`seen`) still points where it did, so a retry would
    /// re-read the same recorded prefix and never see these again. `pushed` is **not** touched —
    /// these are the same occurrences coming back, not new ones, and decrementing it would make
    /// the cursor disagree with what the ingest actually offered.
    fn requeue(&mut self, batches: Vec<Vec<MarketEvent>>) {
        let mut back: VecDeque<MarketEvent> = batches.into_iter().flatten().collect();
        back.append(&mut self.held);
        // Still bounded: the oldest go first, counted, exactly as a live push would do.
        while back.len() > HELD_MAX {
            back.pop_front();
            self.overflow += 1;
        }
        self.held = back;
    }
}

/// Live events a symbol may hold while its history loads.
///
/// A hard count, beside the time a load takes: a tail is not something to assume the size of. At
/// roughly 10,000 prints a second for the busiest symbol of an open, this is a few seconds of the
/// heaviest traffic the app has seen, which is longer than a load of one symbol takes. Past it
/// the oldest held events are dropped and counted, because the newest are the ones the chart and
/// the scanner are about to show.
pub const HELD_MAX: usize = 50_000;

/// Rounds of off-lock catch-up before a load gives up and defers.
///
/// Each round replays what arrived during the previous one, so the rounds shrink quickly; a
/// symbol that is still gaining events faster than they can be replayed after this many is one
/// the loader should come back to rather than hold the core lock for.
const CATCHUP_ROUNDS: usize = 8;

/// Events a load will replay **under** the core lock to close the last gap atomically.
///
/// The adoption has to be atomic with respect to the ingest — the symbol leaves `unloaded` and
/// its state lands in the engine in one critical section — so the final round's leftovers are
/// replayed there. Small by construction: it is whatever arrived during one replay round.
const ATOMIC_MAX: usize = 512;

/// A parsed journal and the file set it was parsed from.
struct JournalCache {
    /// What the segments looked like: (segment count, total bytes). A new boot adds a segment and
    /// the running one grows, so a change in either means the parse is stale.
    fingerprint: (usize, u64),
    journal: Arc<market_archive::admissions::Admissions>,
}

/// Admission records one date's journal may hold before a load stops reading it.
///
/// The journal of a busy day is large, and a restore reads it in full. Past this the parse stops
/// and the day's order is reported unproven rather than the process spending unbounded memory on
/// a proof — the figures still come back, labelled. Roughly a day of the heaviest traffic the app
/// has recorded, several times over.
pub const JOURNAL_RECORDS_MAX: usize = 4_000_000;

impl History {
    /// This date's admission journal, parsed once and shared.
    ///
    /// `Err` is a reason the journal cannot be used at all, in the same vocabulary the archive's
    /// reader uses, so a caller labels coverage the same way whatever went wrong.
    fn journal(
        &self,
        day: NaiveDate,
    ) -> Result<Arc<market_archive::admissions::Admissions>, &'static str> {
        let dir = self.archive.live_dir(day);
        let print = fingerprint(&dir);
        let mut cache = self.journals.lock().expect("journal cache lock");
        if let Some(c) = cache.get(&day)
            && c.fingerprint == print
        {
            return Ok(c.journal.clone());
        }
        let want = crate::admit::identity("live", "recorded_live");
        let got = market_archive::admissions::read(&dir, day, &want);
        if got.records.len() > JOURNAL_RECORDS_MAX {
            // Not cached: a day this size should not be held in memory for the rest of the run.
            cache.remove(&day);
            tracing::warn!(
                %day,
                records = got.records.len(),
                max = JOURNAL_RECORDS_MAX,
                "admission journal: too large to use for a restore; this day's order is unproven"
            );
            return Err("journal_too_large");
        }
        let journal = Arc::new(got);
        // One entry per date the history reads, which is one or two.
        cache.insert(
            day,
            JournalCache {
                fingerprint: print,
                journal: journal.clone(),
            },
        );
        Ok(journal)
    }
}

/// (segment count, total bytes) of a day's journal segments: cheap, and enough to notice a new
/// boot's segment or the running one growing.
fn fingerprint(dir: &std::path::Path) -> (usize, u64) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return (0, 0);
    };
    let mut n = 0;
    let mut bytes = 0;
    for e in rd.filter_map(|e| e.ok()) {
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with(market_archive::admissions::SEGMENT_PREFIX) {
            n += 1;
            bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    (n, bytes)
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
        // The admission journal, for the restarts after this one: with a recording to restore
        // from, this run writes down what it admits so a later process can prove it replayed the
        // same inputs in the same order (`crate::admit`). Without a recorder there is nothing to
        // restore from, so there is nothing to prove and no journal is written.
        if recorder.is_some() {
            let (sink, rx, counters) =
                crate::admit::AdmissionSink::new(crate::admit::QUEUE_CAPACITY);
            if self.0.admissions.set(Arc::new(sink)).is_ok() {
                let a = archive.clone();
                // The writer takes only the counters, never the sink: holding the sink would keep
                // the queue's sender alive, so the thread could never see the channel close and
                // would never seal its segments.
                crate::admit::spawn_writer(
                    counters,
                    rx,
                    move |day| a.live_dir(day),
                    crate::admit::identity("live", "recorded_live"),
                );
            }
        }
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
                        held: VecDeque::new(),
                        overflow: 0,
                        pushed: 0,
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
            journals: Mutex::new(HashMap::new()),
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
    /// `None` when there was nothing to load (already loaded, a load is running, or the catch-up
    /// could not close its gap and the loader should come back).
    ///
    /// # Off the core lock
    ///
    /// The archive read, the replay of the day and the replay of the held tail all happen with no
    /// lock held. Only the last step takes the core lock: a short critical section that replays
    /// whatever arrived during the final round and swaps the symbol's state in. The tail used to
    /// be replayed **inside** that section, which paused every dashboard in the process for as
    /// long as the tail was — unbounded, by a symbol with a slow load.
    ///
    /// Catching up off-lock means events keep arriving while the replay runs, so the tail is
    /// drained in rounds, each replaying what the previous one let in. The rounds shrink quickly;
    /// a symbol still outpacing them after [`CATCHUP_ROUNDS`] is deferred rather than held under
    /// the lock, and so is one whose final gap is larger than [`ATOMIC_MAX`].
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
        // The scanner's restore engine, built under this board's own policies and run identity.
        // Taken here, with the lock held only long enough to clone three policies.
        let restore = {
            let core = self.0.core.lock().expect("core lock");
            core.context.as_ref().map(|cx| cx.restore_engine())
        };
        let mut b = Builder::new(self.current_closes());
        if let Some(mut re) = restore {
            // The evidence this symbol's decisions join, as the market's loader has it. Only the
            // symbol's own records: a restore of one symbol has no business seeding others.
            if let Some(ev) = self.0.evidence.get() {
                // A short read of the log, not held across the replay.
                let batch = ev.log.lock().expect("evidence log lock").for_symbol(symbol);
                crate::evidence::seed_engine(&mut re, batch);
            }
            b = b.with_context(re);
        }
        let mut read = 0u64;
        let mut incomplete: Option<&'static str> = None;
        let mut unproven: Option<&'static str> = None;
        for day in days {
            let got = read_day(h, day, symbol, seen.get(&day).copied().unwrap_or(0));
            incomplete = incomplete.or(got.incomplete);
            let plan = script(h, day, symbol, got.events);
            unproven = unproven.or(plan.unproven);
            for st in &plan.steps {
                if matches!(st, Step::Event { .. }) {
                    read += 1;
                }
                b.step(st);
            }
        }

        // Catch up on the live tail off the lock, in rounds, until nothing new has arrived.
        //
        // Every batch is kept as well as replayed, because a deferred load throws its replay away
        // and the events have to go back into the hold: the archive cutoff still points where it
        // did, so a retry re-reads the same recorded prefix and would never see them again.
        let mut drained: Vec<Vec<MarketEvent>> = Vec::new();
        let mut consumed = 0u64;
        let mut overflow = 0u64;
        let mut caught_up = false;
        for _ in 0..CATCHUP_ROUNDS {
            let (batch, cursor, over) = {
                let mut core = self.0.core.lock().expect("core lock");
                let Some(u) = core.unloaded.get_mut(symbol) else {
                    // Loaded by someone else while this ran: give the drained events back to
                    // nobody — there is no hold any more — and stop.
                    return None;
                };
                (u.take_held(), u.pushed, u.overflow)
            };
            overflow = over;
            // The cursor is what the ingest has offered; what this load has consumed plus what
            // was dropped at the bound must account for all of it, or the tail moved under the
            // replay and another round is due.
            consumed += batch.len() as u64;
            let behind = cursor.saturating_sub(consumed + overflow);
            for e in &batch {
                b.feed(e);
            }
            drained.push(batch);
            if behind == 0 {
                caught_up = true;
                break;
            }
        }
        if !caught_up {
            // Still gaining: put the symbol back for the loader rather than close the gap under
            // the core lock. The replay is discarded, so the drained events go back into the hold
            // in order and the retry starts again from the archive.
            let mut core = self.0.core.lock().expect("core lock");
            if let Some(u) = core.unloaded.get_mut(symbol) {
                u.requeue(drained);
                u.loading = false;
                u.requested = false;
            }
            tracing::info!(
                symbol,
                rounds = CATCHUP_ROUNDS,
                "live history: the live tail outpaced the catch-up; deferring the load"
            );
            return None;
        }

        let seq = {
            let mut core = self.0.core.lock().expect("core lock");
            // The last gap: whatever arrived between the final round and this lock. Bounded,
            // because it is one round's worth; anything larger is deferred rather than replayed
            // here, since this section blocks the whole market.
            let (last, cursor, over) = match core.unloaded.get_mut(symbol) {
                Some(u) => (u.take_held(), u.pushed, u.overflow),
                None => (Vec::new(), 0, overflow),
            };
            if last.len() > ATOMIC_MAX {
                if let Some(u) = core.unloaded.get_mut(symbol) {
                    // Everything this load drained goes back, this round's leftovers last, with
                    // the cursor left alone: these are the same occurrences returning.
                    drained.push(last);
                    u.requeue(drained);
                    u.loading = false;
                    u.requested = false;
                }
                tracing::info!(
                    symbol,
                    offered = cursor,
                    "live history: the final gap was too large to close under the core lock; deferring"
                );
                return None;
            }
            for e in &last {
                b.feed(e);
            }
            consumed += last.len() as u64;
            overflow = over.max(overflow);
            if consumed + overflow != cursor && cursor > 0 {
                // The cursor is what the ingest pushed; what this load consumed plus what was
                // dropped has to account for all of it. A mismatch means the tail moved in a way
                // the rounds did not see, so the day is reported short rather than assumed whole.
                tracing::warn!(
                    symbol,
                    consumed,
                    overflow,
                    cursor,
                    "live history: the held tail did not reconcile; the restored day is short"
                );
                incomplete = incomplete.or(Some("held_tail_unreconciled"));
            }
            core.unloaded.remove(symbol);
            let Builder {
                mut engine,
                board,
                pressure,
                tape,
                last_price,
                context,
                restored_through,
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
            // The scanner's facts. What may be claimed for them, in order of severity: a short
            // read or a dropped tail means the day itself is incomplete; an unproved order means
            // the figures are sound but the barrier each print folded into is not established, so
            // windows over the restored prefix are not reportable.
            if let (Some(mut re), Some(cx)) = (context, core.context.as_mut()) {
                let coverage = match (incomplete, overflow > 0) {
                    (Some(why), _) => Some((stock_context::Coverage::Partial, why)),
                    (None, true) => Some((stock_context::Coverage::Partial, "held_tail_overflow")),
                    (None, false) => unproven.map(|why| (stock_context::Coverage::Unverified, why)),
                };
                let prefix = unproven.and(restored_through);
                match cx.adopt_facts(&mut re, symbol, coverage, prefix) {
                    stock_context::Adoption::Refused(why) => tracing::warn!(
                        symbol,
                        reason = why,
                        "live history: the scanner refused the restored facts"
                    ),
                    outcome => tracing::debug!(symbol, ?outcome, "live history: scanner facts adopted"),
                }
            }
            core.seq += 1;
            core.seq
        };
        if overflow > 0 {
            tracing::warn!(
                symbol,
                dropped = overflow,
                "live history: the held tail overflowed; this symbol's day is short by the dropped events"
            );
        }
        if let Some(why) = incomplete {
            tracing::warn!(symbol, reason = why, "live history: the recording is incomplete");
        }
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
        // Seal the admission journal first: the next process reads it to decide whether it may
        // take this run's prefix as proven, and an unsealed segment leaves that prefix's tail
        // unknown for no reason when the stop was clean.
        if let Some(j) = self.0.admissions.get() {
            j.close();
        }
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

/// One step of a day's replay, in the order the run admitted it.
#[derive(Debug)]
enum Step {
    /// A recorded event, with the verdict the run applied to it.
    ///
    /// `admitted_at` is what the run's own clock read at the admission. It is carried as
    /// provenance and **not** used as the engine's mark: the live scanner folds a print by its
    /// delivery time (`receive_ts`), so a replay that marked prints by the server clock instead
    /// would reproduce something the live run never computed. What the journal contributes is
    /// that this is the same set of deliveries in the same order, with the same verdicts.
    Event {
        event: MarketEvent,
        /// The verdict the chart engine gave it, as recorded. `None` when the journal could not
        /// prove the order and the replay has to re-derive it.
        verdict: Option<(bool, bool)>,
        admitted_at: Option<i64>,
    },
    /// A clock advance the run issued. The scanner's barriers are decided by these, so a replay
    /// that invents its own from the prints' timestamps decides different barriers — which is
    /// exactly what the first draft did.
    Clock(i64),
}

/// A day's replay plan for one symbol.
struct Script {
    steps: Vec<Step>,
    /// Why the plan is not the run's own order, when it is not: a `journal_*` reason from
    /// [`market_archive::admissions`], or `journal_join_mismatch` when the recording and the
    /// journal do not describe the same deliveries of this symbol.
    ///
    /// `None` means every delivery the recording holds joined exactly one admission of this
    /// symbol inside the journal's proven prefix, and the clock advances between them are the
    /// run's own.
    unproven: Option<&'static str>,
}

/// Builds one symbol's replay plan: the recording's events placed in the run's admission order,
/// with the run's verdicts and clock advances between them.
///
/// # Why this is per symbol over a journal that is not
///
/// The journal is one sequence for the whole market, and a load reads one symbol's file. So the
/// join is over this symbol's own admissions only — a second symbol's records are not missing
/// from the recording, they are simply irrelevant to it, and counting them as a mismatch would
/// make every multi-symbol day unprovable. Clock records are kept whatever symbol is being
/// restored, because they are what decides the barriers.
fn script(h: &History, day: NaiveDate, symbol: &str, events: Vec<MarketEvent>) -> Script {
    let plain = |events: Vec<MarketEvent>, why: Option<&'static str>| Script {
        steps: events
            .into_iter()
            .map(|event| Step::Event {
                event,
                verdict: None,
                admitted_at: None,
            })
            .collect(),
        unproven: why,
    };
    let journal = match h.journal(day) {
        Ok(j) => j,
        Err(why) => return plain(events, Some(why)),
    };
    // No proof at all for the day: the recording's own order is all there is.
    if let Some(u) = &journal.unusable {
        let why = match u {
            market_archive::admissions::Unusable::None => "no_admission_journal",
            market_archive::admissions::Unusable::Reason(r) => r,
        };
        return plain(events, Some(why));
    }
    // A proof of a *prefix*, possibly with an unverified tail after it. The prefix is still worth
    // having: the events it covers are replayed in the run's own order with the run's verdicts,
    // and whatever lies past it is replayed in delivery order and labelled. Falling back to the
    // plain path for the whole day because of a tail would throw away the part that is proved.
    let tail_reason = match &journal.tail {
        market_archive::admissions::Tail::Unverified { reason, .. } => Some(*reason),
        market_archive::admissions::Tail::None => None,
    };

    // This symbol's admissions and the clock advances, in (epoch, sequence) order — sequences
    // restart per boot, so a position is the pair and never the sequence alone.
    let mut mine: Vec<&market_archive::admissions::Admitted> = journal
        .proven_records()
        .iter()
        .filter(|a| a.rec.y.as_deref() == Some(symbol) || a.rec.w == Kind::Clock)
        .collect();
    mine.sort_by_key(|a| (a.epoch, a.rec.s));

    // Join the recording's deliveries to this symbol's admissions, one occurrence each.
    let mut by_payload: HashMap<u64, VecDeque<usize>> = HashMap::new();
    for (i, a) in mine.iter().enumerate() {
        if a.rec.w != Kind::Clock {
            by_payload.entry(a.rec.h).or_default().push_back(i);
        }
    }
    let wanted: usize = by_payload.values().map(|v| v.len()).sum();
    let mut joined: HashMap<usize, (MarketEvent, Option<(bool, bool)>)> = HashMap::new();
    let mut leftover: Vec<MarketEvent> = Vec::new();
    for e in events {
        let hash = crate::admit::occurrence_hash(&e);
        match by_payload.get_mut(&hash).and_then(|v| v.pop_front()) {
            Some(i) => {
                let rec = &mine[i].rec;
                // The journal's own note of the delivery time has to agree with the recording's,
                // or the two are not describing the same delivery whatever the payload says.
                if rec.r.is_some() && rec.r != Some(receive_time(&e).0) {
                    leftover.push(e);
                    continue;
                }
                let verdict = match (rec.o, rec.vo) {
                    (Some(o), Some(v)) => Some((o, v)),
                    _ => None,
                };
                joined.insert(i, (e, verdict));
            }
            // Not in the proven prefix: either it is in the unverified tail, or the run never
            // admitted it. A readable print is a fact either way, so it is replayed rather than
            // dropped — in delivery order, after the proven prefix, and labelled.
            None => leftover.push(e),
        }
    }
    // Admissions inside the proof that the recording has nothing for: the recorder's queue
    // dropped them. The live numerator had them and this restore cannot.
    let missing = wanted - joined.len();

    let mut steps = Vec::with_capacity(mine.len() + leftover.len());
    for (i, a) in mine.iter().enumerate() {
        if a.rec.w == Kind::Clock {
            if let Some(to) = a.rec.e {
                steps.push(Step::Clock(to));
            }
            continue;
        }
        if let Some((event, verdict)) = joined.remove(&i) {
            steps.push(Step::Event {
                event,
                verdict,
                admitted_at: Some(a.rec.a),
            });
        }
    }
    leftover.sort_by_key(|e| receive_time(e).0);
    for event in leftover.drain(..) {
        steps.push(Step::Event {
            event,
            verdict: None,
            admitted_at: None,
        });
    }
    // In order of what it tells a reader: a recorder drop means the recording is short of what
    // the run admitted, a tail means the order past a point is unknown, and events outside the
    // proof mean the same for them.
    let unproven = if missing > 0 {
        Some("recorder_drops")
    } else if steps.len() > mine.len() {
        tail_reason.or(Some("journal_unproven_tail"))
    } else {
        tail_reason
    };
    Script { steps, unproven }
}

/// What one day's read produced, and whether it is all of it.
struct DayRead {
    events: Vec<MarketEvent>,
    /// Why the read is short of the day, when it is: `archive_unreadable`,
    /// `archive_truncated`, `recorder_drops`. `None` means the file was read to its end.
    ///
    /// An unreadable or truncated file used to read as an empty day — the same value as "the
    /// symbol did not trade" — so a storage fault restored a symbol with no volume and nothing
    /// said why. The two are not the same statement and are not reported as one.
    incomplete: Option<&'static str>,
}

/// Every event of `symbol` on `day`, in arrival order: the import's file, or the recording's;
/// for the day being recorded, cut where the market's held events begin (module docs).
fn read_day(h: &History, day: NaiveDate, symbol: &str, seen: u64) -> DayRead {
    let a = &h.archive;
    let collect = |r: Result<market_archive::SymbolReader, market_archive::ArchiveError>| match r {
        Ok(r) => {
            let mut events = Vec::new();
            let mut incomplete = None;
            for x in r {
                match x {
                    Ok(v) => events.push(v.event),
                    Err(e) => {
                        // A record that will not decode: everything after it is unknown, so the
                        // read stops and says it is short rather than silently ending here.
                        tracing::warn!(%day, symbol, error = %e, "live history: truncated or corrupt record");
                        incomplete = Some("archive_truncated");
                        break;
                    }
                }
            }
            DayRead { events, incomplete }
        }
        // No file: the symbol did not trade that day. That is a complete read of nothing.
        Err(market_archive::ArchiveError::NotFound(_)) => DayRead {
            events: Vec::new(),
            incomplete: None,
        },
        Err(e) => {
            tracing::warn!(%day, symbol, error = %e, "live history: unreadable");
            DayRead {
                events: Vec::new(),
                incomplete: Some("archive_unreadable"),
            }
        }
    };
    if a.is_final(day) {
        return collect(a.reader(day, symbol));
    }
    match h.recorder.as_ref().and_then(|r| r.barrier(day, symbol)) {
        Some(w) => {
            let mut got = collect(a.live_reader(day, symbol, Some(w.len)));
            let earlier = (got.events.len() as u64).saturating_sub(w.records);
            let keep = earlier + seen;
            if keep > got.events.len() as u64 {
                // The recorder dropped some of this run's events (its queue overflowed). The
                // live numerator was fine; the recording this restore reads is short.
                tracing::warn!(%day, symbol, "live history: the recording misses events the market saw (recorder drops)");
                got.incomplete = got.incomplete.or(Some("recorder_drops"));
            }
            got.events.truncate(keep as usize);
            got
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
    /// The scanner's own engine for this symbol's day, when the context is on.
    ///
    /// A second engine, not the market's: feeding the recorded prints into the live one would
    /// classify every one of them as late against a clock that has already advanced with the
    /// tail, and would decide none of the day's barriers. What it produces is adopted as **facts**
    /// ([`stock_context::Engine::adopt_facts`]) — never as decisions, which this run did not make.
    context: Option<stock_context::Engine>,
    /// The latest delivery mark replayed into `context`: how far the restored prefix reaches.
    restored_through: Option<i64>,
    /// The latest server admission time of a replayed step, from the journal. Provenance: what
    /// the run's own clock read, as distinct from when the collector delivered the print.
    admitted_through: Option<i64>,
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
            context: None,
            restored_through: None,
            admitted_through: None,
        }
    }

    /// The same builder restoring the scanner's state too, on `engine`.
    fn with_context(mut self, engine: stock_context::Engine) -> Self {
        self.context = Some(engine);
        self
    }

    /// Runs one step of a replay plan ([`Step`]).
    ///
    /// With a proven plan the clock advances are the run's own and the verdicts are the ones it
    /// applied. Without one the marks are derived from the deliveries, as the first draft did
    /// throughout — which decides different barriers from the live run, and is why an unproven
    /// restore reports its windows as not reportable rather than as equal.
    fn step(&mut self, step: &Step) {
        match step {
            Step::Clock(to) => self.advance(*to),
            Step::Event {
                event,
                verdict,
                admitted_at,
            } => {
                if verdict.is_none() {
                    // No proven plan: the clock follows the deliveries, to the second, as the
                    // live ingest's own idle ticks would have.
                    let m = receive_time(event).0.div_euclid(1_000_000_000) * 1_000_000_000;
                    self.advance(m);
                }
                if let Some(a) = admitted_at {
                    self.admitted_through = Some(self.admitted_through.map_or(*a, |t| t.max(*a)));
                }
                self.apply(event, *verdict);
            }
        }
    }

    fn advance(&mut self, m: i64) {
        if self.mark.is_some_and(|l| m <= l) {
            return;
        }
        self.mark = Some(m);
        // The clock moves the LULD bands too (doubling windows, the close): the Halt watcher
        // needs them, the rest of the increments are re-snapshotted.
        for u in self.engine.advance_clock(UnixNanos(m)) {
            if let ChartUpdate::Luld(l) = &u {
                self.pressure.apply_luld(&l.symbol, &l.point);
            }
        }
        if let Some(cx) = self.context.as_mut() {
            cx.advance_to(UnixNanos(m));
        }
    }

    fn feed(&mut self, e: &MarketEvent) {
        self.step(&Step::Event {
            event: e.clone(),
            verdict: None,
            admitted_at: None,
        });
    }

    fn apply(&mut self, e: &MarketEvent, saved: Option<(bool, bool)>) {
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
        // The scanner sees the same event with the same verdict the chart engine gave it, in the
        // same order: the point of the restore is to end up where the live run was.
        if let Some(cx) = self.context.as_mut() {
            // The mark is the delivery time, as the live ingest uses: the engine folds a print by
            // when the collector delivered it, so that is what a faithful replay must use.
            let admit = receive_time(e);
            self.restored_through = Some(self.restored_through.map_or(admit.0, |t| t.max(admit.0)));
            match e {
                MarketEvent::Trade(t) => {
                    // The verdict the run recorded, when the journal proved the order; the chart
                    // engine's own otherwise.
                    let (ohlc, volume) = match saved {
                        Some(v) => v,
                        None => {
                            let counted = match ups.first() {
                                Some(ChartUpdate::Tape(row)) => Counted::from_tape(&row.counted),
                                _ => Counted::from_effect(t),
                            };
                            (counted.ohlc, counted.volume)
                        }
                    };
                    cx.apply(t, stock_context::Verdict { ohlc, volume }, admit);
                }
                MarketEvent::Correction(_) | MarketEvent::Cancel(_) => {
                    crate::context::apply_revision_to(cx, e, rev.as_ref(), admit);
                }
                _ => {}
            }
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
        u.pushed += 1;
        if u.held.len() >= HELD_MAX {
            // Drop the oldest: the newest are what the chart and the scanner are about to show,
            // and the loss is counted so the symbol's coverage can say the day is short.
            u.held.pop_front();
            u.overflow += 1;
        }
        u.held.push_back(e.clone());
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

#[cfg(test)]
mod tests {
    use super::*;
    use market_events::{Price, Trade};

    fn unloaded() -> Unloaded {
        Unloaded {
            seen: BTreeMap::new(),
            held: VecDeque::new(),
            overflow: 0,
            pushed: 0,
            holding: true,
            loading: false,
            requested: false,
        }
    }

    fn print(seq: u64) -> MarketEvent {
        let mut t = Trade::new(
            "ABCD",
            UnixNanos(1_000_000_000 * seq as i64),
            Price::from_cents(500),
            100,
            seq,
        );
        t.receive_ts = Some(UnixNanos(1_000_000_000 * seq as i64 + 1_000_000));
        MarketEvent::Trade(t)
    }

    /// The hold is bounded by a count, and what it drops is counted rather than quietly missing.
    ///
    /// It was an unbounded `Vec`: a symbol whose load was slow held the whole live tail in memory
    /// with nothing to stop it.
    #[test]
    fn the_hold_is_bounded_and_its_drops_are_counted() {
        let mut u = unloaded();
        for k in 0..(HELD_MAX as u64 + 250) {
            u.pushed += 1;
            if u.held.len() >= HELD_MAX {
                u.held.pop_front();
                u.overflow += 1;
            }
            u.held.push_back(print(k));
        }
        assert_eq!(u.held.len(), HELD_MAX, "the hold never grows past its bound");
        assert_eq!(u.overflow, 250, "and the loss is a number, not an absence");
        assert_eq!(
            u.pushed,
            HELD_MAX as u64 + 250,
            "the cursor counts what was offered, dropped included"
        );
        // The newest are kept: they are what the chart and the scanner are about to show.
        assert_eq!(
            u.held.back().map(|e| e.event_ts()),
            Some(print(HELD_MAX as u64 + 249).event_ts())
        );
    }

    /// A deferred load gives every drained event back, in order, and leaves the cursor alone.
    ///
    /// The first draft dropped them: the replay was discarded while the archive cutoff still
    /// pointed where it had, so a retry re-read the same recorded prefix and those live events
    /// were simply gone from the day. It also decremented the cursor while putting events back,
    /// which made the next attempt's reconciliation disagree with what the ingest had offered.
    #[test]
    fn a_deferred_load_returns_what_it_drained_and_keeps_the_cursor() {
        let mut u = unloaded();
        for k in 0..30u64 {
            u.pushed += 1;
            u.held.push_back(print(k));
        }

        // Attempt one: drains two rounds, then gives up.
        let first = u.take_held();
        for k in 30..34u64 {
            u.pushed += 1;
            u.held.push_back(print(k));
        }
        let second = u.take_held();
        assert_eq!((first.len(), second.len()), (30, 4));
        u.requeue(vec![first, second]);
        assert_eq!(u.held.len(), 34, "everything is back");
        assert_eq!(u.overflow, 0);
        assert_eq!(u.pushed, 34, "and the cursor still counts each one once");

        // In order, oldest first, so the replay sees the stream as it arrived.
        let order: Vec<i64> = u.held.iter().map(|e| e.event_ts().0).collect();
        let mut sorted = order.clone();
        sorted.sort_unstable();
        assert_eq!(order, sorted, "the hold is still in arrival order");

        // Attempt two: the whole tail is there, and the accounting closes.
        let all = u.take_held();
        assert_eq!(all.len(), 34);
        assert_eq!(
            all.len() as u64 + u.overflow,
            u.pushed,
            "consumed plus dropped accounts for everything offered"
        );
    }

    /// A requeue past the bound drops the oldest and counts them, rather than growing the hold.
    #[test]
    fn a_requeue_past_the_bound_counts_what_it_drops() {
        let mut u = unloaded();
        let drained: Vec<MarketEvent> = (0..HELD_MAX as u64).map(print).collect();
        u.pushed = HELD_MAX as u64 + 10;
        for k in 0..10u64 {
            u.held.push_back(print(HELD_MAX as u64 + k));
        }
        u.requeue(vec![drained]);
        assert_eq!(u.held.len(), HELD_MAX);
        assert_eq!(u.overflow, 10, "the oldest ten went, counted");
        assert_eq!(
            u.held.len() as u64 + u.overflow,
            u.pushed,
            "and the accounting still closes"
        );
    }

// --- the proven path, at the script/Builder seam ---------------------------------------------

/// A journal this process can name itself as the author of.
///
/// Production cannot: a local build has no commit baked in, so its journals are never a proof —
/// which is right, and leaves the branch that *uses* one with no coverage. The seam is compiled
/// only under `cfg(test)` (`crate::admit::test_seam`), so no release build carries it.
fn named_build() -> market_archive::admissions::JournalIdentity {
    crate::admit::test_seam::force("testcommit01", "testcode00000001");
    crate::admit::identity("live", "recorded_live")
}

/// A history over `root` that reads the archive and records nothing.
fn history_over(root: &std::path::Path, on: NaiveDate) -> History {
    let (urgent, _rx) = channel();
    History {
        archive: market_archive::Archive::new(market_archive::ArchiveConfig {
            root: root.to_path_buf(),
            retention_days: 0,
            min_free_bytes: 0,
        }),
        recorder: None,
        days: vec![on],
        started_on: on,
        urgent: Mutex::new(urgent),
        journals: Mutex::new(HashMap::new()),
        total: 0,
        started: Instant::now(),
    }
}

fn fixture_day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 16).expect("valid")
}

fn fixture_at(h: u32, m: u32, s: u32) -> i64 {
    market_events::session::et(fixture_day(), h, m, s, 0)
        .expect("valid")
        .nanos()
}

/// A print of `symbol`, delivered 50 ms after it executed.
fn delivered(symbol: &str, seq: u64, exec: i64, size: u64) -> MarketEvent {
    let mut t = market_events::Trade::new(
        symbol,
        UnixNanos(exec),
        Price::from_cents(500),
        size,
        seq,
    );
    t.receive_ts = Some(UnixNanos(exec + 50_000_000));
    MarketEvent::Trade(t)
}

/// The admission record for one delivery, as the market would have written it: the delivery time
/// and the server's own clock kept apart, and the verdict that was applied.
fn admitted(
    s: u64,
    e: &MarketEvent,
    verdict: Option<(bool, bool)>,
) -> market_archive::admissions::AdmissionRecord {
    market_archive::admissions::AdmissionRecord {
        s,
        // Later than the delivery, as a real admission is: queues and a lock lie between them.
        a: receive_time(e).0 + 7_000_000,
        w: market_archive::admissions::Kind::Trade,
        y: e.symbol().map(|x| x.to_string()),
        h: crate::admit::occurrence_hash(e),
        i: None,
        e: Some(e.event_ts().0),
        r: Some(receive_time(e).0),
        o: verdict.map(|(o, _)| o),
        vo: verdict.map(|(_, v)| v),
    }
}

/// An idle clock advance, as the ingest issues between prints.
fn clock_to(s: u64, to: i64) -> market_archive::admissions::AdmissionRecord {
    market_archive::admissions::AdmissionRecord {
        s,
        a: to + 1_000_000,
        w: market_archive::admissions::Kind::Clock,
        y: None,
        h: market_archive::admissions::payload_hash(&to.to_le_bytes()),
        i: None,
        e: Some(to),
        r: None,
        o: None,
        vo: None,
    }
}

/// The restore replays a proven journal in the run's own order, with its verdicts and its clock
/// advances — over a journal that holds two symbols and two boots.
///
/// The first draft of this seam only sorted the recording by sequence: the marks, the barriers and
/// the verdicts were all still re-derived from the deliveries, so even a successful join replayed
/// something the run had not done. And the join was over the whole journal, so a second symbol's
/// admissions read as deliveries this symbol's recording was missing — which made every
/// multi-symbol day unprovable.
#[test]
fn a_proven_journal_supplies_the_order_the_verdicts_and_the_clock() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().to_path_buf();
    let id = named_build();
    let h = history_over(&root, fixture_day());
    let dir = h.archive.live_dir(fixture_day());

    // Two symbols, interleaved, with an idle clock advance between the two prints of ABCD.
    let a1 = delivered("ABCD", 1, fixture_at(9, 30, 0), 100);
    let other = delivered("WXYZ", 2, fixture_at(9, 30, 5), 700);
    let a2 = delivered("ABCD", 3, fixture_at(9, 30, 20), 400);

    // Boot one: the first two admissions, sealed.
    let mut w =
        market_archive::admissions::AdmissionWriter::open(&dir, fixture_day(), 1, id.clone())
            .expect("open");
    w.append(&[
        admitted(1, &a1, Some((true, true))),
        admitted(2, &other, Some((true, true))),
        clock_to(3, fixture_at(9, 30, 15)),
    ])
    .expect("append");
    w.seal(fixture_at(9, 30, 16)).expect("seal");
    drop(w);

    // Boot two: one more admission, checkpointed, then a record past the proof.
    let mut w2 =
        market_archive::admissions::AdmissionWriter::open(&dir, fixture_day(), 2, id.clone())
            .expect("open");
    w2.append(&[admitted(1, &a2, Some((true, false)))])
        .expect("append");
    w2.checkpoint(fixture_at(9, 30, 21)).expect("checkpoint");
    drop(w2);

    // The recording holds the two ABCD deliveries; the journal holds those plus WXYZ and a clock.
    let plan = script(
        &h,
        fixture_day(),
        "ABCD",
        vec![a1.clone(), a2.clone()],
    );
    assert_eq!(
        plan.unproven, None,
        "another symbol's admissions are irrelevant to this one, not missing from it"
    );

    // The plan is the run's own order: the first print, the clock advance the run issued, then
    // the second print. WXYZ is in the journal and not in this plan.
    let shape: Vec<&str> = plan
        .steps
        .iter()
        .map(|s| match s {
            Step::Clock(_) => "clock",
            Step::Event { .. } => "event",
        })
        .collect();
    assert_eq!(shape, vec!["event", "clock", "event"], "{shape:?}");
    match &plan.steps[1] {
        Step::Clock(to) => assert_eq!(
            *to,
            fixture_at(9, 30, 15),
            "the clock position is the run's, not one derived from the prints"
        ),
        other => panic!("expected the clock advance, got {other:?}"),
    }
    // The verdicts are the ones the run applied, including the one that counted no volume.
    let verdicts: Vec<Option<(bool, bool)>> = plan
        .steps
        .iter()
        .filter_map(|s| match s {
            Step::Event { verdict, .. } => Some(*verdict),
            Step::Clock(_) => None,
        })
        .collect();
    assert_eq!(
        verdicts,
        vec![Some((true, true)), Some((true, false))],
        "taken from the journal rather than re-derived"
    );
    // And the server's admission time travels as provenance, never as the engine's mark.
    match &plan.steps[2] {
        Step::Event {
            admitted_at,
            event,
            ..
        } => {
            assert_eq!(*admitted_at, Some(receive_time(&a2).0 + 7_000_000));
            assert_ne!(
                *admitted_at,
                Some(receive_time(event).0),
                "the two clocks are not the same and are not conflated"
            );
        }
        other => panic!("expected the second print, got {other:?}"),
    }

    // Running the plan through the real Builder applies the journal's verdicts: the second print
    // counted no volume, so only the first print's 100 shares are in the day.
    let mut b = Builder::new(Arc::new(market_archive::PrevCloses::default()))
        .with_context(context_engine());
    for st in &plan.steps {
        b.step(st);
    }
    let cx = b.context.as_mut().expect("a context engine");
    cx.advance_to(UnixNanos(fixture_at(9, 31, 0)));
    let d = cx.current().next().expect("a decision").clone();
    assert_eq!(
        (d.volume.shares, d.volume.trades),
        (100, 1),
        "the second print carried the run's own volume-excluding verdict, so neither its shares          nor its trade are in the day: a re-derivation from the print's conditions would have          counted both"
    );
}

/// A delivery the journal has no admission for is still replayed — it is a readable fact — and the
/// symbol says the order past the proof is unestablished rather than dropping the print.
#[test]
fn a_delivery_outside_the_proof_is_replayed_and_labelled() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().to_path_buf();
    let id = named_build();
    let h = history_over(&root, fixture_day());
    let dir = h.archive.live_dir(fixture_day());

    let first = delivered("ABCD", 1, fixture_at(9, 30, 0), 100);
    let past_the_proof = delivered("ABCD", 2, fixture_at(9, 31, 0), 250);
    let mut w =
        market_archive::admissions::AdmissionWriter::open(&dir, fixture_day(), 1, id.clone())
            .expect("open");
    w.append(&[admitted(1, &first, Some((true, true)))])
        .expect("append");
    w.checkpoint(fixture_at(9, 30, 1)).expect("checkpoint");
    // Admitted, and past the checkpoint: inside the file but outside the proof.
    w.append(&[admitted(2, &past_the_proof, Some((true, true)))])
        .expect("append");
    w.flush().expect("flush");
    drop(w);

    let plan = script(
        &h,
        fixture_day(),
        "ABCD",
        vec![first.clone(), past_the_proof.clone()],
    );
    assert_eq!(
        plan.unproven,
        Some("journal_unproven_tail"),
        "the tail is named, and the prefix before it is still used"
    );
    assert_eq!(plan.steps.len(), 2, "neither print was discarded");

    let mut b = Builder::new(Arc::new(market_archive::PrevCloses::default()))
        .with_context(context_engine());
    for st in &plan.steps {
        b.step(st);
    }
    let cx = b.context.as_mut().expect("a context engine");
    cx.advance_to(UnixNanos(fixture_at(9, 32, 0)));
    let d = cx.current().next().expect("a decision").clone();
    assert_eq!(
        (d.volume.shares, d.volume.trades),
        (350, 2),
        "the facts are all there; what is unproven is the order, not the arithmetic"
    );
}

/// A recording the journal proves there was more of reports the recorder's loss, not a whole day.
#[test]
fn an_admission_with_no_delivery_is_a_recorder_drop() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().to_path_buf();
    let id = named_build();
    let h = history_over(&root, fixture_day());
    let dir = h.archive.live_dir(fixture_day());

    let kept = delivered("ABCD", 1, fixture_at(9, 30, 0), 100);
    let lost = delivered("ABCD", 2, fixture_at(9, 30, 10), 900);
    let mut w =
        market_archive::admissions::AdmissionWriter::open(&dir, fixture_day(), 1, id.clone())
            .expect("open");
    w.append(&[
        admitted(1, &kept, Some((true, true))),
        admitted(2, &lost, Some((true, true))),
    ])
    .expect("append");
    w.seal(fixture_at(9, 30, 11)).expect("seal");
    drop(w);

    // The recording has only one of them: the recorder's queue dropped the other.
    let plan = script(&h, fixture_day(), "ABCD", vec![kept]);
    assert_eq!(
        plan.unproven,
        Some("recorder_drops"),
        "the live numerator had it and this restore cannot: {:?}",
        plan.unproven
    );
    assert_eq!(plan.steps.len(), 1);
}

/// Two deliveries of one print join one admission each, in order.
#[test]
fn a_redelivery_joins_its_own_admission() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().to_path_buf();
    let id = named_build();
    let h = history_over(&root, fixture_day());
    let dir = h.archive.live_dir(fixture_day());

    let p = delivered("ABCD", 1, fixture_at(9, 30, 0), 100);
    let mut w =
        market_archive::admissions::AdmissionWriter::open(&dir, fixture_day(), 1, id.clone())
            .expect("open");
    // The same payload admitted twice: a reconnect replay. The second was refused by the ledger,
    // which the recorded verdict does not say and does not need to — the engine decides that.
    w.append(&[
        admitted(1, &p, Some((true, true))),
        admitted(2, &p, Some((true, true))),
    ])
    .expect("append");
    w.seal(fixture_at(9, 30, 5)).expect("seal");
    drop(w);

    let plan = script(&h, fixture_day(), "ABCD", vec![p.clone(), p.clone()]);
    assert_eq!(plan.unproven, None, "two deliveries, two admissions");
    assert_eq!(plan.steps.len(), 2);

    // Replayed through the engine, the redelivery is refused on identity: the day is 100 shares,
    // not 200. That is the ledger's job, and the restore does not undo it.
    let mut b = Builder::new(Arc::new(market_archive::PrevCloses::default()))
        .with_context(context_engine());
    for st in &plan.steps {
        b.step(st);
    }
    let cx = b.context.as_mut().expect("a context engine");
    cx.advance_to(UnixNanos(fixture_at(9, 31, 0)));
    let d = cx.current().next().expect("a decision").clone();
    assert_eq!(
        (d.volume.shares, d.volume.trades),
        (100, 1),
        "the redelivery is still a duplicate"
    );
}

/// A context engine with this server's own policies, for the seam tests.
fn context_engine() -> stock_context::Engine {
    stock_context::Engine::new(
        stock_context::Policy::current(),
        stock_context::RunInfo {
            run_id: "live".into(),
            source: "live".into(),
            fidelity: stock_context::Fidelity::RecordedLive,
            numerator_coverage: stock_context::Coverage::Unverified,
        },
    )
    .with_panel_policy(stock_context::PanelPolicy::v1())
    .with_setup_policy(stock_context::SetupPolicy::v0())
}

}
