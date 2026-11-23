//! The 1h chart's earlier sessions, read back from the archive.
//!
//! The engine keeps one trading date of candles (`EngineConfig::drop_prior_dates_at_open`), which
//! on every other timeframe is a full chart and on 1h is 7–16 candles: too few to read (owner,
//! 2026-11-22, "the chart should show multiple days"). So a 1h `chart.snapshot` gets the previous
//! [`DAYS`] archived trading dates' hourly candles in front of the engine's own.
//!
//! - **Source.** Only the archive this market already reads its history from (crate::history):
//!   each date as [`Archive::day_data`] says (the import when it has finished the date, else the
//!   live recording). Nothing is fetched from a provider, ever.
//! - **How.** Each (symbol, date) file is replayed through a scratch engine of 15s + 5m + 1h (the
//!   1h series is derived from the 5m one, chart_engine::Timeframe::derived; 15s keeps the settle
//!   window where the live engine has it), in file order, then
//!   the clock is moved past the date, as the import check replays a day
//!   (market_providers::import::check). Corrections, cancels and the condition policy are the
//!   engine's, so a candle here is the candle the live chart showed for that hour, up to the
//!   arrival-order differences inside the settle window.
//! - **When.** Off the core lock and off the async workers: the socket task warms this cache on a
//!   blocking thread ([`warm`]) before it takes the 1h snapshot, and the snapshot (under the core
//!   lock) only copies what the cache holds ([`prepend`]). A date is computed once per process; a
//!   provisional date is recomputed once the import has replaced it.
//! - **Bounded.** At most [`RECORDS_MAX`] archived events are replayed per warm (newest date
//!   first, so a very busy symbol shows fewer earlier days, never a slow chart), and the cache
//!   holds at most [`CACHE_DAYS`] (symbol, date) entries, oldest dropped first: an entry is the
//!   date's 7–17 candles, so the cache stays a few MB.
//! - **What it does not do.** Indicators, levels and zones of the 1h chart still come from the
//!   engine's own (today's) candles; the earlier days are candles only. Live increments never
//!   touch them (they are finished days).

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};

use chart_engine::wire::{CandleWire, ChartSnapshot};
use chart_engine::{ChartEngine, EngineConfig, Timeframe};
use chrono::NaiveDate;
use market_archive::{Archive, ArchiveError, DayData};
use market_events::session::{SessionId, classify, trading_date};
use market_events::{MarketEvent, UnixNanos};

use crate::market::Market;

/// Earlier trading dates a 1h snapshot shows.
pub const DAYS: usize = 10;
/// Archived events one warm replays at most, across its dates.
pub const RECORDS_MAX: u64 = 2_000_000;
/// (symbol, date) entries the cache keeps.
pub const CACHE_DAYS: usize = 20_000;

#[derive(Clone)]
struct Entry {
    /// The import had finished the date when this was computed: never recomputed.
    final_: bool,
    candles: Arc<Vec<CandleWire>>,
}

#[derive(Default)]
struct Cache {
    /// (archive root, symbol) → date → its 1h candles.
    days: HashMap<(PathBuf, String), BTreeMap<NaiveDate, Entry>>,
    /// Insertion order, for the bound.
    order: VecDeque<(PathBuf, String, NaiveDate)>,
    /// Symbols being warmed right now: a second request does not replay the same files again.
    busy: HashSet<(PathBuf, String)>,
}

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(Mutex::default);

fn archive_of(market: &Market) -> Option<&Archive> {
    market.0.history.get().map(|h| h.archive())
}

/// Computes the earlier dates of `symbol` the cache lacks. Blocking (file reads and replays):
/// call it on a blocking thread, never under the core lock. Does nothing for a market without an
/// archive history (replay, fixture) or a symbol already being warmed.
pub fn warm(market: &Market, symbol: &str) {
    let Some(archive) = archive_of(market) else {
        return;
    };
    let today = trading_date(crate::history::wall_now());
    warm_from(archive, symbol, today);
}

fn warm_from(archive: &Archive, symbol: &str, today: NaiveDate) {
    let key = (archive.root().to_path_buf(), symbol.to_string());
    // Imported dates and dates only recorded live (an import that has not run yet).
    let mut dates: Vec<NaiveDate> = archive
        .days()
        .unwrap_or_default()
        .into_iter()
        .chain(archive.live_days().unwrap_or_default())
        .filter(|d| *d < today && market_events::session::is_trading_day(*d))
        .collect();
    dates.sort_unstable();
    dates.dedup();
    // The newest DAYS dates with market data. Before the open the engine still holds the
    // previous date itself; [`prepend`] then skips it, as it takes only dates before the
    // engine's first.
    let dates: Vec<(NaiveDate, bool)> = dates
        .into_iter()
        .rev()
        .filter_map(|d| archive.day_data(d).map(|k| (d, k == DayData::Final)))
        .take(DAYS)
        .collect();
    let todo: Vec<(NaiveDate, bool)> = {
        let mut c = CACHE.lock().expect("prior sessions cache");
        if !c.busy.insert(key.clone()) {
            return;
        }
        let have = c.days.get(&key);
        // Missing, or a recording the import has replaced since.
        dates
            .into_iter()
            .filter(|(d, fin)| {
                have.and_then(|m| m.get(d))
                    .is_none_or(|e| *fin && !e.final_)
            })
            .collect()
    };
    let mut budget = RECORDS_MAX;
    let mut done = Vec::new();
    for (day, fin) in todo {
        if budget == 0 {
            break;
        }
        match day_candles(archive, day, symbol, fin, &mut budget) {
            Some(candles) => done.push((day, fin, candles)),
            None => break,
        }
    }
    let mut c = CACHE.lock().expect("prior sessions cache");
    c.busy.remove(&key);
    for (day, fin, candles) in done {
        let m = c.days.entry(key.clone()).or_default();
        if m.insert(
            day,
            Entry {
                final_: fin,
                candles: Arc::new(candles),
            },
        )
        .is_none()
        {
            c.order.push_back((key.0.clone(), key.1.clone(), day));
        }
    }
    while c.order.len() > CACHE_DAYS {
        let Some((root, sym, day)) = c.order.pop_front() else {
            break;
        };
        let k = (root, sym);
        if let Some(m) = c.days.get_mut(&k) {
            m.remove(&day);
            if m.is_empty() {
                c.days.remove(&k);
            }
        }
    }
}

/// One date's 1h candles of `symbol` from the archive (module docs), or `None` when replaying it
/// would pass the event budget (the date is left out rather than shown half-built). A symbol
/// with no file that date has no candles; an unreadable or corrupt file gives what decoded.
fn day_candles(
    archive: &Archive,
    day: NaiveDate,
    symbol: &str,
    fin: bool,
    budget: &mut u64,
) -> Option<Vec<CandleWire>> {
    let reader = if fin {
        archive.reader(day, symbol)
    } else {
        archive.live_reader(day, symbol, None)
    };
    let reader = match reader {
        Ok(r) => r,
        Err(ArchiveError::NotFound(_)) => return Some(Vec::new()),
        Err(e) => {
            tracing::warn!(%day, symbol, error = %e, "1h history: unreadable, left out");
            return Some(Vec::new());
        }
    };
    // 15 s is there for the policy, not its candles: the settle window is measured on the
    // finest timeframe, and without it a 5 m grid would accept late prints the live chart
    // excluded (caught by the test below against the production configuration).
    let mut engine = ChartEngine::new(EngineConfig {
        timeframes: vec![Timeframe::S15, Timeframe::M5, Timeframe::H1],
        drop_prior_dates_at_open: false,
        ..EngineConfig::default()
    })
    .expect("15s + 5m + 1h is a valid configuration");
    let mut last: Option<i64> = None;
    for rec in reader {
        let Ok(rec) = rec else {
            tracing::warn!(%day, symbol, "1h history: truncated record, the rest of the date left out");
            break;
        };
        if *budget == 0 {
            return None;
        }
        *budget -= 1;
        let e: &MarketEvent = &rec.event;
        engine.apply(e);
        last = Some(last.map_or(e.event_ts().0, |l| l.max(e.event_ts().0)));
    }
    if let Some(l) = last {
        engine.advance_clock(UnixNanos(l + Timeframe::H1.nanos()));
    }
    let candles = engine
        .snapshot(symbol, Timeframe::H1, None)
        .map(|s| s.candles)
        .unwrap_or_default()
        .into_iter()
        .filter(|c| date_of(c.t) == Some(day))
        .collect();
    Some(candles)
}

fn date_of(t_ms: i64) -> Option<NaiveDate> {
    classify(UnixNanos::from_millis(t_ms)).map(|s| s.date)
}

/// Puts the cached earlier dates of `snap`'s symbol in front of its candles (the [`DAYS`] newest
/// dates before its first candle's, or before today's when it has none) and adds their sessions
/// to its schedule. Cache reads only: cheap under the core lock. A no-op for any timeframe but
/// 1h and for a market without an archive history.
pub fn prepend(market: &Market, snap: &mut ChartSnapshot) {
    if snap.timeframe != Timeframe::H1 {
        return;
    }
    let Some(archive) = archive_of(market) else {
        return;
    };
    let before = snap
        .candles
        .first()
        .and_then(|c| date_of(c.t))
        .unwrap_or_else(|| trading_date(crate::history::wall_now()));
    prepend_from(archive, snap, before);
}

fn prepend_from(archive: &Archive, snap: &mut ChartSnapshot, before: NaiveDate) {
    let key = (archive.root().to_path_buf(), snap.symbol.clone());
    let earlier: Vec<(NaiveDate, Arc<Vec<CandleWire>>)> = {
        let c = CACHE.lock().expect("prior sessions cache");
        let Some(m) = c.days.get(&key) else { return };
        m.range(..before)
            .rev()
            .take(DAYS)
            .map(|(d, e)| (*d, e.candles.clone()))
            .collect()
    };
    if earlier.is_empty() {
        return;
    }
    let mut candles: Vec<CandleWire> = earlier
        .iter()
        .rev()
        .flat_map(|(_, cs)| cs.iter().cloned())
        .collect();
    candles.append(&mut snap.candles);
    snap.candles = candles;
    // Their sessions too: the clients shade and filter by the schedule.
    let mut sessions =
        chart_engine::session_schedule(earlier.iter().map(|(d, _)| *d), None::<SessionId>);
    let have: HashSet<String> = snap.sessions.iter().map(|s| s.id.clone()).collect();
    sessions.retain(|s| !have.contains(&s.id));
    sessions.append(&mut snap.sessions);
    sessions.sort_by_key(|s| s.start);
    snap.sessions = sessions;
}

#[cfg(test)]
mod tests {
    use super::*;
    use market_archive::ArchiveConfig;
    use market_archive::fixture::{DEFAULT_SEED, write_fixture_day};

    fn d(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    fn snapshot(candles: Vec<CandleWire>) -> ChartSnapshot {
        ChartSnapshot {
            kind: "chart.snapshot".into(),
            symbol: "SMPL".into(),
            timeframe: Timeframe::H1,
            session: None,
            candles,
            gaps: Vec::new(),
            profile: None,
            quality: Default::default(),
            clock: None,
            halts: Vec::new(),
            luld: Vec::new(),
            prev_close: None,
            sessions: Vec::new(),
        }
    }

    /// The day's 1h candles from the production engine fed the same file.
    fn engine_hours(archive: &Archive, day: NaiveDate) -> Vec<CandleWire> {
        let mut e = ChartEngine::default();
        let mut last = 0;
        for r in archive.reader(day, "SMPL").unwrap() {
            let ev = r.unwrap().event;
            last = last.max(ev.event_ts().0);
            e.apply(&ev);
        }
        e.advance_clock(UnixNanos(last + Timeframe::H1.nanos()));
        e.snapshot("SMPL", Timeframe::H1, None).unwrap().candles
    }

    #[test]
    fn earlier_dates_come_from_the_archive_in_front_of_the_engines_candles() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = Archive::new(ArchiveConfig {
            root: tmp.path().join("archive"),
            retention_days: 0,
            min_free_bytes: 0,
        });
        let syms = vec!["SMPL".to_string()];
        for day in ["2026-10-13", "2026-10-14", "2026-10-15"] {
            write_fixture_day(&archive, d(day), &syms, DEFAULT_SEED).unwrap();
        }
        // FXLO only: no SMPL file that date, so no SMPL candles from it.
        write_fixture_day(
            &archive,
            d("2026-10-16"),
            &["FXLO".to_string()],
            DEFAULT_SEED,
        )
        .unwrap();
        warm_from(&archive, "SMPL", d("2026-10-19"));
        let today = engine_hours(&archive, d("2026-10-15"));
        // The engine holds 10-15 (say): the two dates before it go in front.
        let mut snap = snapshot(today.clone());
        prepend_from(&archive, &mut snap, d("2026-10-15"));
        let mut want = engine_hours(&archive, d("2026-10-13"));
        want.extend(engine_hours(&archive, d("2026-10-14")));
        assert!(!want.is_empty());
        want.extend(today.clone());
        assert_eq!(
            snap.candles, want,
            "the candles the engine builds from those files"
        );
        assert!(snap.candles.windows(2).all(|w| w[0].t < w[1].t));
        assert!(snap.candles.iter().all(|c| c.t % 1_800_000 == 0));
        // Their sessions are in the schedule, ascending, once each.
        assert!(snap.sessions.iter().any(|s| s.id == "2026-10-13/regular"));
        assert!(snap.sessions.windows(2).all(|w| w[0].start < w[1].start));
        // Before 10-14: only 10-13.
        let mut early = snapshot(Vec::new());
        prepend_from(&archive, &mut early, d("2026-10-14"));
        assert_eq!(early.candles, engine_hours(&archive, d("2026-10-13")));
        // A symbol nobody warmed: untouched.
        let mut other = snapshot(today.clone());
        other.symbol = "FXLO".into();
        prepend_from(&archive, &mut other, d("2026-10-15"));
        assert_eq!(other.candles, today);
    }

    #[test]
    fn the_event_budget_leaves_out_whole_dates() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = Archive::new(ArchiveConfig {
            root: tmp.path().join("archive"),
            retention_days: 0,
            min_free_bytes: 0,
        });
        let syms = vec!["SMPL".to_string()];
        write_fixture_day(&archive, d("2026-10-15"), &syms, DEFAULT_SEED).unwrap();
        let mut budget = 10;
        assert!(day_candles(&archive, d("2026-10-15"), "SMPL", true, &mut budget).is_none());
        let mut budget = RECORDS_MAX;
        let got = day_candles(&archive, d("2026-10-15"), "SMPL", true, &mut budget).unwrap();
        assert_eq!(got, engine_hours(&archive, d("2026-10-15")));
    }
}
