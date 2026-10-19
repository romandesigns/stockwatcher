//! The live recorder: every event the live provider receives goes into the archive's live
//! recording of its trading day (`market_archive::live`), so the day's history survives a
//! restart (the server loads it back, `crates/server/src/history.rs`) and can be replayed until
//! the nightly import replaces it.
//!
//! # What is written
//!
//! Each event **after** receive-time stamping, with `receive_ts` set to its delivery time (the
//! running maximum the engine clock follows), in arrival order: exactly the engine's input, so a
//! symbol's day read back and fed with the same marks gives the same chart.
//!
//! # Write-behind, never in the way
//!
//! [`Recorder::append`] is a `try_send` into a bounded queue ([`RecorderConfig::queue`]) that one
//! dedicated OS thread drains; it never blocks and never awaits. When the thread falls behind
//! (a stalled disk) the queue fills and further events are **dropped from the recording and
//! counted**, never from the market: the charts are unaffected now, only a later reload would
//! miss them. The thread logs the count while it lasts. A file error, the `ARCHIVE_MIN_FREE_MB`
//! floor, or a day that the import has started (`Archive::accepts_live`: live never writes over
//! imported data) stops recording that day, logged.
//!
//! Durability: every [`FLUSH_EVERY`] the thread writes the frames that are due (a symbol's
//! pending records once they reach 64 KiB or are 10 s old: `market_archive::live`, "Frames", with
//! the measurement behind it), and every [`SYNC_EVERY`] checkpoints the day's manifest. The
//! ~7,000 symbol files are not fsynced (`LiveWriter::commit` says why: a process that stops or
//! dies loses nothing that was written; only a power loss can lose the last seconds the kernel
//! held). A clean stop ([`Recorder::close`], which the server calls on shutdown, so every
//! deploy) writes everything; a process killed outright loses up to 10 s of a quiet symbol.
//!
//! # Days
//!
//! The day of an event is the trading date of its delivery time
//! (`market_events::session::trading_date`), and never goes back, so a recording moves forward a
//! day at a time. When it moves on, the old day is **sealed** on a background thread (every file
//! checked and recounted; replay offers it from then on), and recordings beyond
//! [`RecorderConfig::keep_days`] that no import ever replaced are retired, never the day being
//! written. On start, earlier days left unsealed by a stopped process are sealed the same way.
//!
//! # Barrier
//!
//! [`Recorder::barrier`] passes through the same queue as the events, so when the thread answers,
//! every event appended before the call is in the symbol's file below the length it reports.
//! The server uses it to cut a symbol's history exactly where its own live tail begins.

use chrono::NaiveDate;
use market_archive::{Archive, ArchiveConfig, ArchiveError, LiveWriter};
use market_events::session::trading_date;
use market_events::{MarketEvent, UnixNanos};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::time::{Duration, Instant};

/// How often the thread writes the frames that are due.
pub const FLUSH_EVERY: Duration = Duration::from_secs(1);
/// How often the day's manifest is checkpointed.
pub const SYNC_EVERY: Duration = Duration::from_secs(15);

/// `LIVE_RECORD*` (see `config.rs`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecorderConfig {
    /// The archive the recording goes into (`ARCHIVE_ROOT`, `ARCHIVE_MIN_FREE_MB`).
    pub archive: ArchiveConfig,
    /// `LIVE_RECORD_DAYS`, default 5: live recordings kept when no import replaced them.
    pub keep_days: usize,
    /// Events the queue holds before it drops. 65,536 is a few seconds of the whole market at
    /// the open, ~20 MB.
    pub queue: usize,
    /// The trading date start-up housekeeping treats as today (recordings before it are sealed);
    /// `None`: the trading date of now. Tests whose events are not from today set it.
    pub today: Option<NaiveDate>,
}

impl RecorderConfig {
    pub fn new(archive: ArchiveConfig) -> Self {
        RecorderConfig {
            archive,
            keep_days: 5,
            queue: 65_536,
            today: None,
        }
    }
}

/// When an event counts as delivered: its receive time, else its event time (replay's rule).
pub fn receive_time(e: &MarketEvent) -> UnixNanos {
    let r = match e {
        MarketEvent::Trade(t) => t.receive_ts,
        MarketEvent::Quote(q) => q.receive_ts,
        MarketEvent::Correction(c) => c.receive_ts,
        MarketEvent::Cancel(c) => c.receive_ts,
        MarketEvent::Status(s) => s.receive_ts,
    };
    r.unwrap_or_else(|| e.event_ts())
}

/// What a barrier reports for a symbol of the day being recorded: records this process wrote
/// to its file, and the file length every one of them lies below.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Written {
    pub records: u64,
    pub len: u64,
}

enum Msg {
    Event(MarketEvent),
    Barrier {
        day: NaiveDate,
        symbol: String,
        reply: std::sync::mpsc::Sender<Option<Written>>,
    },
    /// Write everything and checkpoint, then stop (a clean shutdown).
    Close(std::sync::mpsc::Sender<()>),
}

#[derive(Default)]
struct Stats {
    dropped: AtomicU64,
}

/// The handle the live provider (append) and the server (barrier) hold. Cheap to clone; the
/// thread finishes the day's files and exits when the last clone is dropped.
#[derive(Clone)]
pub struct Recorder {
    tx: SyncSender<Msg>,
    stats: Arc<Stats>,
}

impl Recorder {
    /// Seals earlier unsealed days and prunes (in the background), then starts the writer thread.
    /// Nothing is created on disk until the first event.
    pub fn start(cfg: &RecorderConfig) -> Recorder {
        Self::start_gated(cfg, None, cfg.today)
    }

    /// `gate`: the thread waits for it before writing anything (tests: a writer that has fallen
    /// behind). `today`: the day the start-up sealing treats as current (tests).
    fn start_gated(
        cfg: &RecorderConfig,
        gate: Option<Receiver<()>>,
        today: Option<NaiveDate>,
    ) -> Recorder {
        let (tx, rx) = sync_channel(cfg.queue.max(1));
        let stats = Arc::new(Stats::default());
        let archive = Archive::new(cfg.archive.clone());
        let today = today.unwrap_or_else(|| {
            trading_date(UnixNanos::from_utc(chrono::Utc::now()).unwrap_or_default())
        });
        let sealer = archive.clone();
        let keep = cfg.keep_days.max(2);
        let _ = std::thread::Builder::new()
            .name("live-record-seal".into())
            .spawn(move || seal_and_prune(&sealer, today, keep));
        let thread = Thread {
            archive,
            keep,
            floor: cfg.archive.min_free_bytes,
            stats: stats.clone(),
            writer: None,
            refused: None,
            reported_drops: 0,
        };
        let spawned = std::thread::Builder::new()
            .name("live-record".into())
            .spawn(move || {
                if let Some(g) = gate {
                    let _ = g.recv();
                }
                thread.run(rx)
            });
        if let Err(e) = spawned {
            // The receiver went with the closure: every append now drops and counts.
            tracing::error!(error = %e, "live recorder: thread did not start; nothing will be recorded");
        }
        Recorder { tx, stats }
    }

    /// Queues one stamped event. Never blocks: a full queue drops it from the recording (counted).
    pub fn append(&self, e: &MarketEvent) {
        match self.tx.try_send(Msg::Event(e.clone())) {
            Ok(()) => {}
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                self.stats.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Waits (blocking: not on an async worker) until every event appended before the call is
    /// written, and reports what was written of `symbol` on `day`. `None` when `day` is not the
    /// day being recorded (its files are complete as they are) or the recorder is gone.
    pub fn barrier(&self, day: NaiveDate, symbol: &str) -> Option<Written> {
        let (reply, answer) = std::sync::mpsc::channel();
        self.tx
            .send(Msg::Barrier {
                day,
                symbol: symbol.to_string(),
                reply,
            })
            .ok()?;
        answer.recv_timeout(Duration::from_secs(30)).ok().flatten()
    }

    /// A clean stop: waits (blocking, at most `timeout`) until everything appended so far is
    /// written and checkpointed. Later appends are dropped. Returns whether it finished in time. The
    /// server calls it on shutdown, so a deploy loses no recorded event.
    pub fn close(&self, timeout: Duration) -> bool {
        let (reply, done) = std::sync::mpsc::channel();
        if self.tx.send(Msg::Close(reply)).is_err() {
            return true;
        }
        done.recv_timeout(timeout).is_ok()
    }

    /// Events dropped from the recording since start (the market still applied them).
    pub fn dropped(&self) -> u64 {
        self.stats.dropped.load(Ordering::Relaxed)
    }
}

/// Start-up housekeeping, off the writer thread: seal every recording before `today` that a
/// stopped process left unsealed, then retire recordings beyond `keep` (never today's).
fn seal_and_prune(archive: &Archive, today: NaiveDate, keep: usize) {
    let days = match archive.live_days() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "live recorder: cannot list recordings");
            return;
        }
    };
    for d in days.into_iter().filter(|d| *d < today) {
        let sealed = archive
            .live_manifest(d)
            .ok()
            .flatten()
            .and_then(|m| m.live)
            .is_some_and(|l| l.sealed);
        if !sealed && archive.accepts_live(d) {
            seal(archive, d);
        }
    }
    match archive.prune_live(keep, Some(today)) {
        Ok(p) if !p.is_empty() => {
            tracing::info!(days = ?p, "live recorder: retired old recordings no import replaced")
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "live recorder: prune failed"),
    }
}

fn seal(archive: &Archive, day: NaiveDate) {
    let started = Instant::now();
    match archive.seal_live(day) {
        Ok(m) => tracing::info!(
            %day,
            symbols = m.symbols.len(),
            events = m.total_events(),
            secs = started.elapsed().as_secs_f64(),
            "live recorder: day sealed (replay offers it as provisional until the import replaces it)"
        ),
        Err(e) => tracing::warn!(%day, error = %e, "live recorder: sealing failed"),
    }
}

struct Thread {
    archive: Archive,
    keep: usize,
    floor: u64,
    stats: Arc<Stats>,
    writer: Option<LiveWriter>,
    /// A day that refused recording (imported, file error, space floor): nothing more is written
    /// to it.
    refused: Option<NaiveDate>,
    reported_drops: u64,
}

impl Thread {
    fn run(mut self, rx: Receiver<Msg>) {
        let mut last_flush = Instant::now();
        let mut last_sync = Instant::now();
        loop {
            match rx.recv_timeout(FLUSH_EVERY) {
                Ok(Msg::Event(e)) => self.write(&e),
                Ok(Msg::Barrier { day, symbol, reply }) => {
                    let _ = reply.send(self.barrier(day, &symbol));
                }
                Ok(Msg::Close(reply)) => {
                    self.finish();
                    tracing::info!("live recorder: closed (everything written)");
                    let _ = reply.send(());
                    return;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if last_flush.elapsed() >= FLUSH_EVERY {
                last_flush = Instant::now();
                let sync = last_sync.elapsed() >= SYNC_EVERY;
                if sync {
                    last_sync = last_flush;
                }
                self.flush(sync);
                self.report_drops();
            }
        }
        self.finish();
    }

    fn finish(&mut self) {
        if let Some(w) = self.writer.take() {
            let day = w.day();
            if let Err(e) = w.finish() {
                tracing::warn!(%day, error = %e, "live recorder: could not finish the day's files");
            }
        }
        self.report_drops();
    }

    fn write(&mut self, e: &MarketEvent) {
        let day = trading_date(receive_time(e));
        // Never back a day: a stamp on the far side of a boundary stays in the current one.
        let current = self.writer.as_ref().map(LiveWriter::day);
        let day = current.map_or(day, |c| c.max(day));
        if current != Some(day) {
            if self.refused.is_some_and(|r| r >= day) {
                return;
            }
            self.roll(day);
        }
        let Some(w) = self.writer.as_mut() else {
            return;
        };
        if let Err(err) = w.append(e) {
            self.fail(day, &err);
        }
    }

    /// Finishes the old day, seals it in the background, and opens the new one.
    fn roll(&mut self, day: NaiveDate) {
        if let Some(old) = self.writer.take() {
            let d = old.day();
            match old.finish() {
                Ok(()) => {
                    let a = self.archive.clone();
                    let _ = std::thread::Builder::new()
                        .name("live-record-seal".into())
                        .spawn(move || seal(&a, d));
                }
                Err(e) => {
                    tracing::warn!(day = %d, error = %e, "live recorder: could not finish the day's files")
                }
            }
        }
        match self.archive.prune_live(self.keep, Some(day)) {
            Ok(p) if !p.is_empty() => {
                tracing::info!(days = ?p, "live recorder: retired old recordings no import replaced")
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "live recorder: prune failed"),
        }
        let created_by = concat!("stockwatcher live recorder ", env!("CARGO_PKG_VERSION"));
        match LiveWriter::open(&self.archive, day, self.floor, created_by) {
            Ok(w) => {
                tracing::info!(%day, dir = %self.archive.live_dir(day).display(), "live recorder: recording");
                self.writer = Some(w);
            }
            Err(e) => self.fail(day, &e),
        }
    }

    fn fail(&mut self, day: NaiveDate, e: &ArchiveError) {
        match e {
            ArchiveError::DayIsFinal(_) => {
                tracing::info!(%day, "live recorder: the day is imported (or being imported); live recording of it stops (imported data wins)")
            }
            _ => {
                tracing::error!(%day, error = %e, "live recorder: recording stops until the next trading day (the market is unaffected)")
            }
        }
        self.writer = None;
        self.refused = Some(day);
    }

    fn barrier(&mut self, day: NaiveDate, symbol: &str) -> Option<Written> {
        let w = self.writer.as_mut().filter(|w| w.day() == day)?;
        match w.barrier(symbol) {
            Ok((records, len)) => Some(Written { records, len }),
            Err(e) => {
                self.fail(day, &e);
                None
            }
        }
    }

    fn flush(&mut self, sync: bool) {
        let Some(w) = self.writer.as_mut() else {
            return;
        };
        let day = w.day();
        let res = w
            .flush_due()
            .and_then(|()| if sync { w.commit() } else { Ok(()) });
        if let Err(e) = res {
            self.fail(day, &e);
        }
    }

    fn report_drops(&mut self) {
        let n = self.stats.dropped.load(Ordering::Relaxed);
        if n > self.reported_drops {
            tracing::warn!(
                dropped = n - self.reported_drops,
                total = n,
                "live recorder fell behind: events dropped from the recording (charts are unaffected; a reload would miss them)"
            );
            self.reported_drops = n;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use market_events::{Price, Trade};

    fn et(d: u32, h: u32, m: u32) -> UnixNanos {
        market_events::session::et(NaiveDate::from_ymd_opt(2026, 10, d).unwrap(), h, m, 0, 0)
            .unwrap()
    }

    fn date(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, d).unwrap()
    }

    fn cfg(root: &std::path::Path) -> RecorderConfig {
        RecorderConfig::new(ArchiveConfig {
            root: root.to_path_buf(),
            retention_days: 0,
            min_free_bytes: 0,
        })
    }

    fn trade(t: UnixNanos, seq: u64) -> MarketEvent {
        let mut tr = Trade::new("SMPL", t, Price::from_cents(500), 100, seq);
        tr.receive_ts = Some(t);
        MarketEvent::Trade(tr)
    }

    fn seqs(a: &Archive, day: NaiveDate) -> Vec<u64> {
        match a.live_reader(day, "SMPL", None) {
            Ok(r) => r
                .filter_map(|e| match e.ok()?.event {
                    MarketEvent::Trade(t) => Some(t.source_seq),
                    _ => None,
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    fn eventually(f: impl Fn() -> bool) {
        for _ in 0..500 {
            if f() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("condition never held");
    }

    #[test]
    fn the_recorder_moves_to_the_next_trading_day_seals_the_last_and_never_goes_back() {
        let tmp = tempfile::tempdir().unwrap();
        let r = Recorder::start_gated(&cfg(tmp.path()), None, Some(date(15)));
        r.append(&trade(et(15, 19, 59), 1));
        r.append(&trade(et(15, 20, 1), 2)); // Friday's overnight
        let mut late = trade(et(15, 19, 59), 3);
        if let MarketEvent::Trade(t) = &mut late {
            t.receive_ts = Some(et(15, 19, 59));
        }
        r.append(&late);
        // The barrier sees everything appended before it, unflushed or not.
        assert_eq!(
            r.barrier(date(16), "SMPL").map(|w| w.records),
            Some(2),
            "2 and 3 on Friday"
        );
        assert_eq!(
            r.barrier(date(15), "SMPL"),
            None,
            "Thursday is no longer recorded"
        );
        drop(r);
        let a = Archive::new(cfg(tmp.path()).archive);
        eventually(|| seqs(&a, date(16)) == [2, 3]);
        assert_eq!(seqs(&a, date(15)), [1]);
        eventually(|| {
            a.live_manifest(date(15))
                .ok()
                .flatten()
                .and_then(|m| m.live)
                .is_some_and(|l| l.sealed)
        });
    }

    /// The write-behind's contract: when the writer cannot keep up, `append` returns at once and
    /// the overflow is counted, not waited for.
    #[test]
    fn a_recorder_that_falls_behind_drops_and_counts_instead_of_blocking() {
        let tmp = tempfile::tempdir().unwrap();
        let mut c = cfg(tmp.path());
        c.queue = 8;
        let (open, gate) = std::sync::mpsc::channel();
        let r = Recorder::start_gated(&c, Some(gate), Some(date(15)));
        let started = Instant::now();
        for i in 0..1_000 {
            r.append(&trade(et(15, 10, 0), i));
        }
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "append never waits"
        );
        assert_eq!(r.dropped(), 1_000 - 8);
        open.send(()).unwrap();
        drop(r);
        let a = Archive::new(c.archive);
        eventually(|| seqs(&a, date(15)).len() == 8);
    }

    /// Frames wait for size or age, but a clean stop writes and syncs everything before it
    /// returns (the server's shutdown: a deploy loses nothing).
    #[test]
    fn a_clean_close_writes_everything_before_it_returns() {
        let tmp = tempfile::tempdir().unwrap();
        let r = Recorder::start_gated(&cfg(tmp.path()), None, Some(date(15)));
        for i in 0..50 {
            r.append(&trade(et(15, 10, 0), i));
        }
        let a = Archive::new(cfg(tmp.path()).archive);
        std::thread::sleep(Duration::from_millis(1_500));
        assert!(
            seqs(&a, date(15)).is_empty(),
            "a small, young frame is not due yet"
        );
        assert!(r.close(Duration::from_secs(10)));
        assert_eq!(seqs(&a, date(15)), (0..50).collect::<Vec<_>>());
        assert_eq!(
            a.live_manifest(date(15)).unwrap().unwrap().symbols["SMPL"].events,
            50
        );
        r.append(&trade(et(15, 10, 1), 99));
        assert_eq!(r.dropped(), 1, "nothing is recorded after the close");
    }

    #[test]
    fn a_day_the_import_has_started_is_not_recorded() {
        let tmp = tempfile::tempdir().unwrap();
        let a = Archive::new(cfg(tmp.path()).archive);
        a.mark_importing(date(15)).unwrap();
        let r = Recorder::start_gated(&cfg(tmp.path()), None, Some(date(15)));
        r.append(&trade(et(15, 10, 0), 1));
        assert_eq!(r.barrier(date(15), "SMPL"), None);
        drop(r);
        std::thread::sleep(Duration::from_millis(50));
        assert!(!a.live_dir(date(15)).exists());
    }
}
