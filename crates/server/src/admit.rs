//! Writing the server's admission journal (`market_archive::admissions`).
//!
//! # Why
//!
//! A restart restores a symbol from its recorded day, and the restore has to be able to say
//! whether it replayed **the same inputs in the same order** the run admitted them. A recording
//! alone cannot answer that: the recorder has a queue of its own and drops under pressure, its
//! file order is the recorder's rather than the market core's, a redelivery is indistinguishable
//! from its original in the file, and an imported day carries no delivery times from this run at
//! all. Without an answer, "the restored day equals the live day" is an assumption, and every
//! window measured over the restored prefix inherits it.
//!
//! So the market writes down what it admitted, as it admits it. The scope is the inputs to a
//! symbol's **trade-derived facts** — prints, corrections, cancels and the clock advances that
//! decide which barrier folds a print in — because that is all a restore adopts. Pillar verdicts,
//! membership and setups are not restored from a recording at all, so the evidence and halts
//! behind them are deliberately not journalled.
//!
//! # Where the work happens
//!
//! [`AdmissionSink::record`] is called under the market's core lock, so it is one atomic, one
//! payload hash and a bounded `try_send`: never a wait, never disk. A full queue is a counted
//! drop — and because the sequence is assigned here rather than by the writer, a dropped record
//! leaves a hole in the sequence that the reader sees, so the journal reports itself incomplete
//! without needing this process's counters to be believed.
//!
//! The writer is a plain thread doing blocking appends into this boot's own segment, checkpointed
//! as it goes and sealed at a clean stop. It is never on an async worker and never holds a market
//! lock.
//!
//! # Build identity
//!
//! The header carries two things. [`crate::COMMIT`] is the commit a release build was made from,
//! and `unknown` for a local one. [`code_id`] is a fingerprint of the running executable, taken
//! once at startup off any lock.
//!
//! Both are needed. The commit alone cannot tell two binaries built from one commit apart, and
//! this audit is the reason that matters: several implementations were developed against the same
//! HEAD, each admitting prints under different code while reporting the same commit. The
//! executable fingerprint alone is no better — it identifies a binary but ties it to no source
//! anyone can read. An identity missing either one is never exact.

use chrono::NaiveDate;
use market_archive::admissions::{
    AdmissionRecord, AdmissionWriter, JournalIdentity, Kind, payload_hash,
};
use market_events::session::trading_date;
use market_events::{MarketEvent, UnixNanos};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::time::Duration;

/// Records the queue holds before `record` starts dropping them (and counting the drops).
///
/// Sized for a busy open: the writer empties it several times a second, so filling it takes a
/// stall of the writer thread rather than a burst of prints.
pub(crate) const QUEUE_CAPACITY: usize = 65_536;

/// How long the writer waits for a record before flushing what it has.
const FLUSH_EVERY: Duration = Duration::from_millis(250);

/// Records appended in one batch at most, so one `write_all` stays small.
const BATCH_MAX: usize = 4_096;

/// What the journal writer has done, and what it could not.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AdmissionReport {
    /// Inputs this process assigned a journal sequence to. An offer the close fence turned away
    /// never takes one and is counted in `fenced` instead, so `written + dropped == admitted`
    /// while `fenced` is the journal's own incompleteness.
    pub admitted: u64,
    /// Records the queue refused. Each leaves a hole in the journal's sequence, which is how a
    /// later restore learns the journal is not a whole account of the day.
    pub dropped: u64,
    /// Records the writer appended.
    pub written: u64,
    /// Append or open failures (a full disk, a removed directory).
    pub errors: u64,
    pub queue_capacity: usize,
    /// Trading dates whose sequence run was evicted (more than four in one boot). Their records
    /// restart at 1 and the journal reports itself disordered.
    pub dates_evicted: u64,
    /// Admissions offered after the journal was closed. Not in the file, so the segments are
    /// left unsealed rather than sealed over them.
    pub fenced: u64,
}

impl AdmissionReport {
    /// Whether a restore of this run's own day could be joined to every admission it made.
    pub fn complete(&self) -> bool {
        self.dropped == 0
            && self.errors == 0
            && self.dates_evicted == 0
            && self.fenced == 0
            && self.written == self.admitted
    }
}

/// Counters and the close flag, shared by the market and the writer thread.
///
/// Deliberately **not** inside [`AdmissionSink`]: the sink owns the queue's sender, so a writer
/// holding the sink would keep the channel alive and could never see it disconnect. Its seal
/// branch would then be unreachable and no segment would ever be closed cleanly — which is the
/// difference between a restart that can prove its predecessor's prefix and one that cannot.
#[derive(Debug, Default)]
pub(crate) struct Counters {
    dropped: AtomicU64,
    written: AtomicU64,
    errors: AtomicU64,
    /// Set by [`AdmissionSink::close`]: the writer drains what is queued, seals and exits.
    closing: std::sync::atomic::AtomicBool,
    /// Admissions offered after the close fence. They are not in the journal, so the segment is
    /// **not** sealed: a seal says nothing followed, and these did.
    fenced: AtomicU64,
}

/// Sequence numbers, one run per journal segment.
///
/// A segment is a boot's records **for one trading date**, and a reader checks it for holes: a
/// record the queue dropped leaves a gap, which is how a journal reports itself incomplete
/// without anyone having to believe this process's counters. So the sequence has to be per date.
/// One run for the whole boot would number the first date 1..N and the second N+1.., and the
/// second segment would read as though N records had been dropped — a normal date roll looking
/// exactly like queue loss, and a late print of the previous date splitting both.
///
/// A handful of slots, because a boot sees one date and its successor, plus whatever arrives late
/// for the one before. A fifth is pathological and is counted rather than silently reusing a
/// sequence run.
#[derive(Debug, Default)]
struct SeqByDate {
    slots: [Option<(NaiveDate, u64)>; 4],
    /// Dates whose sequence run was evicted to make room. Their later records restart at 1 and
    /// the journal reports itself disordered, which is the honest outcome.
    evicted: u64,
}

impl SeqByDate {
    fn next(&mut self, day: NaiveDate) -> u64 {
        if let Some(slot) = self
            .slots
            .iter_mut()
            .find(|s| s.is_some_and(|(d, _)| d == day))
        {
            let (_, n) = slot.as_mut().expect("just matched");
            *n += 1;
            return *n;
        }
        if let Some(free) = self.slots.iter_mut().find(|s| s.is_none()) {
            *free = Some((day, 1));
            return 1;
        }
        // Full: take the oldest date's slot, counted.
        self.evicted += 1;
        let oldest = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.map(|(d, _)| (d, i)))
            .min()
            .map(|(_, i)| i)
            .unwrap_or(0);
        self.slots[oldest] = Some((day, 1));
        1
    }
}

/// The bounded queue the market admits into.
pub(crate) struct AdmissionSink {
    tx: SyncSender<AdmissionRecord>,
    /// Sequences per trading date, and the total for reporting.
    seq: std::sync::Mutex<SeqByDate>,
    admitted: AtomicU64,
    counters: Arc<Counters>,
}

impl AdmissionSink {
    /// The sink, the receiving end and the shared counters for [`spawn_writer`].
    pub(crate) fn new(
        capacity: usize,
    ) -> (AdmissionSink, Receiver<AdmissionRecord>, Arc<Counters>) {
        let (tx, rx) = sync_channel(capacity.max(1));
        let counters = Arc::new(Counters::default());
        (
            AdmissionSink {
                tx,
                seq: std::sync::Mutex::new(SeqByDate::default()),
                admitted: AtomicU64::new(0),
                counters: counters.clone(),
            },
            rx,
            counters,
        )
    }

    /// Closes the journal: no further admission is accepted, and what is queued is written and
    /// checkpointed.
    ///
    /// It does **not** seal. A seal says nothing followed the records before it, and at the point
    /// a shutdown asks for one that is not something this process can know: the ingest and clock
    /// tasks are still running and still admitting, so an offer can pass the fence check, or
    /// reach the queue, after the writer has looked. A seal already on disk cannot be taken back,
    /// so resting one on a counter read would be guessing — and the one thing a seal must never be
    /// is a guess.
    ///
    /// What a clean close leaves is therefore a **checkpointed prefix with an unverified tail**,
    /// which is exactly what it can support: the next process restores from the prefix and treats
    /// the tail as unknown. The seal is written only where it is provable — when the channel
    /// disconnects, because then the only sender is gone and nothing can arrive at all.
    pub(crate) fn close(&self) {
        self.counters.closing.store(true, Ordering::Relaxed);
    }

    /// Records one admitted event. Safe under the core lock: one atomic, one hash and a bounded
    /// `try_send`.
    ///
    /// `at` is this process's own clock at the admission, which is **not** the mark the engine
    /// folds the print by — that is the print's delivery time, carried separately as `r`. Both are
    /// kept, and neither is ever relabelled the other.
    pub(crate) fn record(&self, e: &MarketEvent, verdict: Option<(bool, bool)>, at: i64) -> u64 {
        if self.fenced() {
            return 0;
        }
        let kind = match e {
            MarketEvent::Trade(_) => Kind::Trade,
            MarketEvent::Correction(_) => Kind::Correction,
            MarketEvent::Cancel(_) => Kind::Cancel,
            // Quotes and status changes are not inputs to the facts; the caller does not offer
            // them, and a stray one is recorded as a clock-kind marker rather than mislabelled.
            _ => Kind::Clock,
        };
        let (id, exec, recv) = match e {
            MarketEvent::Trade(t) => (
                Some(trade_key(&t.id())),
                Some(t.event_ts.nanos()),
                t.receive_ts.map(|r| r.nanos()),
            ),
            // The *target* print's identity, which is what the ledger resolves against.
            MarketEvent::Correction(c) => (
                Some(trade_key(&c.original_trade_id())),
                Some(c.event_ts.nanos()),
                c.receive_ts.map(|r| r.nanos()),
            ),
            MarketEvent::Cancel(c) => (
                Some(trade_key(&c.target_trade_id())),
                Some(c.event_ts.nanos()),
                c.receive_ts.map(|r| r.nanos()),
            ),
            _ => (None, Some(e.event_ts().nanos()), None),
        };
        let s = self.take_seq(recv.or(exec).unwrap_or(at));
        let r = AdmissionRecord {
            s,
            a: at,
            w: kind,
            y: e.symbol().map(|s| s.to_string()),
            h: occurrence_hash(e),
            i: id,
            e: exec,
            r: recv,
            o: verdict.map(|(o, _)| o),
            vo: verdict.map(|(_, v)| v),
        };
        if self.tx.try_send(r).is_err() {
            self.counters.dropped.fetch_add(1, Ordering::Relaxed);
        }
        s
    }

    /// Whether the journal is closed. An admission offered after that is counted and refused,
    /// which bounds how far past the shutdown the file has to reach — not because the refusal
    /// makes anything provable (a concurrent offer can always pass this check a moment before the
    /// flag is set) but because there is nothing useful to do with a record whose writer is
    /// finishing. Nothing that is written depends on this being race-free.
    fn fenced(&self) -> bool {
        if !self.counters.closing.load(Ordering::Relaxed) {
            return false;
        }
        self.counters.fenced.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// The next sequence for the segment this admission belongs to, by the same rule the writer
    /// files it under ([`journal_day`]). Taken here, at the admission, so a record the queue
    /// refuses leaves a visible hole in that segment.
    fn take_seq(&self, when: i64) -> u64 {
        let day = trading_date(UnixNanos(when));
        self.admitted.fetch_add(1, Ordering::Relaxed);
        self.seq.lock().expect("admission sequence lock").next(day)
    }

    /// Records a clock advance: it decides which barrier folds the prints admitted before it.
    pub(crate) fn record_clock(&self, to: UnixNanos, at: i64) -> u64 {
        if self.fenced() {
            return 0;
        }
        let s = self.take_seq(to.0);
        let r = AdmissionRecord {
            s,
            a: at,
            w: Kind::Clock,
            y: None,
            h: payload_hash(&to.0.to_le_bytes()),
            i: None,
            e: Some(to.0),
            r: None,
            o: None,
            vo: None,
        };
        if self.tx.try_send(r).is_err() {
            self.counters.dropped.fetch_add(1, Ordering::Relaxed);
        }
        s
    }

    pub(crate) fn report(&self) -> AdmissionReport {
        AdmissionReport {
            admitted: self.admitted.load(Ordering::Relaxed),
            dropped: self.counters.dropped.load(Ordering::Relaxed),
            written: self.counters.written.load(Ordering::Relaxed),
            errors: self.counters.errors.load(Ordering::Relaxed),
            queue_capacity: QUEUE_CAPACITY,
            dates_evicted: self.seq.lock().expect("admission sequence lock").evicted,
            fenced: self.counters.fenced.load(Ordering::Relaxed),
        }
    }
}

/// Whether an event is an input to a symbol's trade-derived facts, and so belongs in the journal.
///
/// Quotes and status changes are not: they move pillar verdicts, membership and setups, none of
/// which a restore takes from a recording. Clock advances are recorded separately
/// ([`AdmissionSink::record_clock`]), because they are not market events.
pub(crate) fn is_fact_input(e: &MarketEvent) -> bool {
    matches!(
        e,
        MarketEvent::Trade(_) | MarketEvent::Correction(_) | MarketEvent::Cancel(_)
    )
}

/// A print's provider identity as one string: source, exchange and sequence
/// (`market_events::TradeId`'s three parts, which have no textual form of their own).
fn trade_key(id: &market_events::TradeId) -> String {
    format!("{:?}/{}/{}", id.source, id.exchange, id.seq)
}

/// A canonical hash of one delivery, computed identically on the live path and on the restore's.
///
/// This is what makes a redelivery joinable: two deliveries of the same print hash the same, and
/// the journal holds one record per delivery, so the restore can consume them one at a time
/// instead of guessing which occurrence the run counted.
///
/// Only fields the archive round-trips are hashed, because the restore computes this over an event
/// decoded back out of the recording and the two have to agree
/// (`crates/server/tests/admission_journal.rs` pins that).
pub(crate) fn occurrence_hash(e: &MarketEvent) -> u64 {
    let mut b: Vec<u8> = Vec::with_capacity(96);
    let tag: u8 = match e {
        MarketEvent::Trade(_) => 1,
        MarketEvent::Correction(_) => 2,
        MarketEvent::Cancel(_) => 3,
        MarketEvent::Quote(_) => 4,
        MarketEvent::Status(_) => 5,
    };
    b.push(tag);
    b.extend_from_slice(e.symbol().unwrap_or("").as_bytes());
    b.push(0);
    b.extend_from_slice(&e.event_ts().0.to_le_bytes());
    match e {
        MarketEvent::Trade(t) => {
            b.extend_from_slice(trade_key(&t.id()).as_bytes());
            b.push(0);
            b.extend_from_slice(&t.price.micros().to_le_bytes());
            b.extend_from_slice(&t.size.to_le_bytes());
            b.extend_from_slice(&t.receive_ts.map(|r| r.0).unwrap_or(0).to_le_bytes());
            for c in &t.conditions {
                b.extend_from_slice(c.as_bytes());
                b.push(b',');
            }
        }
        MarketEvent::Correction(c) => {
            b.extend_from_slice(trade_key(&c.original_trade_id()).as_bytes());
            b.push(0);
            b.extend_from_slice(&c.original_price.micros().to_le_bytes());
            b.extend_from_slice(&c.original_size.to_le_bytes());
            b.extend_from_slice(&c.corrected_id.to_le_bytes());
            b.extend_from_slice(&c.corrected_price.micros().to_le_bytes());
            b.extend_from_slice(&c.corrected_size.to_le_bytes());
            b.extend_from_slice(&c.receive_ts.map(|r| r.0).unwrap_or(0).to_le_bytes());
        }
        MarketEvent::Cancel(c) => {
            b.extend_from_slice(trade_key(&c.target_trade_id()).as_bytes());
            b.push(0);
            b.extend_from_slice(&c.price.micros().to_le_bytes());
            b.extend_from_slice(&c.size.to_le_bytes());
            b.extend_from_slice(&c.receive_ts.map(|r| r.0).unwrap_or(0).to_le_bytes());
        }
        _ => {}
    }
    payload_hash(&b)
}

/// [`code_id`], for other records that need the same identity (`crate::shadow`). Warmed at
/// startup, off every lock.
pub(crate) fn code_id_for_records() -> &'static str {
    code_id()
}

/// A fingerprint of the running executable, computed once.
///
/// Read and hashed on the first call, which the market does at startup, off every lock; later
/// calls are free. Empty when the process cannot find or read its own executable, which the
/// journal reader treats as an identity that names no build rather than as a weaker match.
///
/// Why this exists on top of the commit: two binaries built from the same commit with different
/// working trees report the same [`crate::COMMIT`] and are different implementations. Hashing
/// what is actually running is the only thing that separates them.
pub(crate) fn code_id() -> &'static str {
    static ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    ID.get_or_init(|| {
        let Ok(path) = std::env::current_exe() else {
            tracing::warn!(
                "admission journal: cannot locate this executable; its journals will not be exact"
            );
            return String::new();
        };
        match hash_file(&path) {
            Some(h) => format!("{h:016x}"),
            None => {
                tracing::warn!(
                    path = %path.display(),
                    "admission journal: cannot read this executable; its journals will not be exact"
                );
                String::new()
            }
        }
    })
}

/// Hashes a file in chunks, so a large binary is not held in memory.
fn hash_file(path: &std::path::Path) -> Option<u64> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = [0u8; 64 * 1024];
    let mut h = market_archive::admissions::DIGEST_SEED;
    loop {
        match f.read(&mut buf) {
            Ok(0) => return Some(h),
            Ok(n) => h = market_archive::admissions::roll(h, &buf[..n]),
            Err(_) => return None,
        }
    }
}

/// What this process was built from and what is running: the commit (`unknown` for a local
/// build, which the reader never treats as exact) and the executable's own fingerprint.
///
/// In a release build this is the whole of it — there is no override and no code for one. The
/// test seam below is compiled only under `cfg(test)`, because a journal that can claim exactness
/// on request is not a proof of anything, and a `#[doc(hidden)]` function still ships.
fn build_identity() -> (String, String) {
    #[cfg(test)]
    if let Some((b, c)) = test_seam::forced() {
        return (b, c);
    }
    (crate::COMMIT.to_string(), code_id().to_string())
}

/// Lets a test name a build, so the journal's proven path can be exercised at all. Compiled out
/// of every build that is not running this crate's own tests.
#[cfg(test)]
pub(crate) mod test_seam {
    use std::sync::OnceLock;

    static FORCED: OnceLock<(String, String)> = OnceLock::new();

    /// Fixes the build identity for the rest of the process.
    pub(crate) fn force(build: &str, code: &str) {
        let _ = FORCED.set((build.to_string(), code.to_string()));
    }

    pub(crate) fn forced() -> Option<(String, String)> {
        FORCED.get().cloned()
    }
}

/// This process's identity for a journal header: what it was built from, what is running, the
/// run's source and fidelity, and the exact policy bytes every admission was judged by.
pub(crate) fn identity(source: &str, fidelity: &str) -> JournalIdentity {
    let (build, code) = build_identity();
    JournalIdentity {
        build,
        code,
        source: source.to_string(),
        fidelity: fidelity.to_string(),
        pillars: stock_context::Policy::current().tag(),
        panel: stock_context::PanelPolicy::v1().tag(),
        setup: stock_context::DetectionPolicy::current().tag(),
        conditions: market_events::conditions::POLICY.to_string(),
    }
}

/// Starts the journal writer thread. `dir_of` gives the directory a day's segment belongs in (the
/// archive's `live/` directory for that day), so the writer never needs the archive's locks.
pub(crate) fn spawn_writer(
    counters: Arc<Counters>,
    rx: Receiver<AdmissionRecord>,
    dir_of: impl Fn(NaiveDate) -> PathBuf + Send + 'static,
    identity: JournalIdentity,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("admission-journal".into())
        .spawn(move || writer_loop(&counters, rx, dir_of, identity))
        .expect("spawn the admission journal writer")
}

/// Date writers kept open at once.
///
/// A trading date rolls at most once in a run, so two is a date and its successor. A third means
/// something is wrong with the clock, and the oldest is sealed and closed rather than the handles
/// accumulating for the life of the process.
const WRITERS_MAX: usize = 2;

fn writer_loop(
    counters: &Counters,
    rx: Receiver<AdmissionRecord>,
    dir_of: impl Fn(NaiveDate) -> PathBuf,
    identity: JournalIdentity,
) {
    let mut writers: BTreeMap<NaiveDate, Option<AdmissionWriter>> = BTreeMap::new();
    let mut batch: Vec<AdmissionRecord> = Vec::with_capacity(BATCH_MAX);
    loop {
        let gone = match rx.recv_timeout(FLUSH_EVERY) {
            Ok(r) => {
                batch.push(r);
                while batch.len() < BATCH_MAX {
                    match rx.try_recv() {
                        Ok(r) => batch.push(r),
                        Err(_) => break,
                    }
                }
                false
            }
            Err(RecvTimeoutError::Timeout) => false,
            // The sink is gone: the market is finished with the journal.
            Err(RecvTimeoutError::Disconnected) => true,
        };
        flush(counters, &mut writers, &mut batch, &dir_of, &identity);
        if !gone && !counters.closing.load(Ordering::Relaxed) {
            continue;
        }

        // Finishing. Write everything still queued first: a batch is bounded and a busy market
        // can have many waiting, so stopping after one of them would leave the rest unwritten.
        loop {
            while batch.len() < BATCH_MAX {
                match rx.try_recv() {
                    Ok(r) => batch.push(r),
                    Err(_) => break,
                }
            }
            if batch.is_empty() {
                break;
            }
            flush(counters, &mut writers, &mut batch, &dir_of, &identity);
        }
        if gone {
            // The sink itself is gone, so there is no sender left in the process and the queue is
            // empty. *That* is a proof that nothing followed, and the only thing this writer ever
            // seals on.
            for w in writers.values_mut().flatten() {
                let _ = w.seal();
            }
        } else {
            // A clean close while the market is still running: checkpointed, not sealed. The
            // ingest and clock tasks can still be admitting, and no reading of a counter here
            // could establish otherwise.
            for w in writers.values_mut().flatten() {
                let _ = w.checkpoint();
            }
            tracing::info!(
                fenced = counters.fenced.load(Ordering::Relaxed),
                "admission journal: closed on request; the prefix is checkpointed and the tail is left unverified, because a clean-stop seal cannot be proved while the market is still admitting"
            );
        }
        return;
    }
}

/// The trading date a record is filed under: the **event's**, never the admission clock's.
///
/// A restore looks for the journal of the day it is restoring, beside that day's recording, and
/// the recorder files an event by its delivery time. So the journal has to agree: filing by what
/// the server's clock happened to read would put a replayed or fixture day's admissions under
/// today's date, where nothing looks for them. In production the two coincide; a test is where
/// the difference shows, which is the point of having one.
fn journal_day(r: &AdmissionRecord) -> NaiveDate {
    trading_date(UnixNanos(r.r.or(r.e).unwrap_or(r.a)))
}

/// The writer's filing rule and the sink's sequence rule have to agree, or a segment's sequences
/// would not be its own run. Both read the delivery time, then the execution time, then the
/// admission clock — in that order, because that is the order of how well each identifies the
/// trading date the record belongs to.
#[cfg(test)]
pub(crate) fn journal_day_of(r: &AdmissionRecord) -> NaiveDate {
    journal_day(r)
}

/// Appends the batch, grouped by the trading date of each admission, and checkpoints.
fn flush(
    counters: &Counters,
    writers: &mut BTreeMap<NaiveDate, Option<AdmissionWriter>>,
    batch: &mut Vec<AdmissionRecord>,
    dir_of: &impl Fn(NaiveDate) -> PathBuf,
    identity: &JournalIdentity,
) {
    if batch.is_empty() {
        return;
    }
    let mut by_day: HashMap<NaiveDate, Vec<AdmissionRecord>> = HashMap::new();
    for r in batch.drain(..) {
        by_day.entry(journal_day(&r)).or_default().push(r);
    }
    for (day, rows) in by_day {
        // A new date beyond the bound seals and closes the oldest first, so handles do not
        // accumulate for the life of the process.
        while writers.len() >= WRITERS_MAX && !writers.contains_key(&day) {
            let Some(oldest) = writers.keys().next().copied() else {
                break;
            };
            if let Some(Some(mut w)) = writers.remove(&oldest) {
                let _ = w.seal();
            }
        }
        let started = rows.first().map(|r| r.a).unwrap_or_default();
        let w = writers.entry(day).or_insert_with(|| {
            match AdmissionWriter::open(&dir_of(day), day, started, identity.clone()) {
                Ok(w) => Some(w),
                Err(e) => {
                    tracing::warn!(%day, error = %e, "admission journal: cannot open; a restore of this day will not be able to prove its order");
                    None
                }
            }
        });
        let Some(w) = w else {
            counters
                .errors
                .fetch_add(rows.len() as u64, Ordering::Relaxed);
            continue;
        };
        match w.append(&rows) {
            Ok(()) => {
                counters
                    .written
                    .fetch_add(rows.len() as u64, Ordering::Relaxed);
                // Checkpoint on every flush rather than only every CHECKPOINT_EVERY records: the
                // proven prefix is what a restore may use, and a quiet market would otherwise
                // leave its whole day unproven.
                if let Err(e) = w.checkpoint() {
                    tracing::warn!(%day, error = %e, "admission journal: checkpoint failed");
                }
            }
            Err(e) => {
                counters
                    .errors
                    .fetch_add(rows.len() as u64, Ordering::Relaxed);
                tracing::warn!(%day, error = %e, "admission journal: append failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use market_archive::admissions::{self, UNKNOWN_BUILD};
    use market_events::{Price, Trade};

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 16).expect("valid")
    }

    fn at(h: u32, m: u32, s: u32) -> i64 {
        market_events::session::et(day(), h, m, s, 0)
            .expect("valid")
            .nanos()
    }

    fn trade(seq: u64, exec: i64, size: u64) -> MarketEvent {
        let mut t = Trade::new("ABCD", UnixNanos(exec), Price::from_cents(500), size, seq);
        t.receive_ts = Some(UnixNanos(exec + 50_000_000));
        MarketEvent::Trade(t)
    }

    /// A full queue drops and counts it, and because the sequence was already taken the journal
    /// has a hole where the dropped record would have been — rather than a shorter file that
    /// reads as a complete day.
    #[test]
    fn a_full_queue_leaves_a_hole_rather_than_a_shorter_journal() {
        let (sink, rx, _c) = AdmissionSink::new(2);
        for k in 0..6u64 {
            sink.record(
                &trade(k, at(9, 30, 0) + k as i64, 100),
                Some((true, true)),
                at(9, 30, 1),
            );
        }
        let r = sink.report();
        assert_eq!((r.admitted, r.dropped), (6, 4), "{r:?}");
        assert!(!r.complete(), "nothing here may claim a whole journal");
        let kept: Vec<u64> = rx.try_iter().map(|r| r.s).collect();
        assert_eq!(
            kept,
            vec![1, 2],
            "and the ones that got through kept their own numbers"
        );
    }

    /// Two deliveries of the same print hash the same, so the journal holds two joinable
    /// occurrences; a different print does not.
    #[test]
    fn a_redelivery_hashes_the_same_and_a_different_print_does_not() {
        let a = trade(7, at(9, 30, 0), 100);
        let again = trade(7, at(9, 30, 0), 100);
        assert_eq!(occurrence_hash(&a), occurrence_hash(&again));
        assert_ne!(
            occurrence_hash(&a),
            occurrence_hash(&trade(7, at(9, 30, 0), 200)),
            "a different size is a different payload"
        );
        assert_ne!(
            occurrence_hash(&a),
            occurrence_hash(&trade(8, at(9, 30, 0), 100)),
            "and so is a different id"
        );
    }

    /// The identity separates the source it was built from and the binary that ran, because the
    /// commit alone cannot tell two builds of one tree apart.
    #[test]
    fn the_same_commit_with_other_code_is_not_the_same_identity() {
        let mine = identity("live", "recorded_live");
        // The audit's own case: one HEAD, two implementations.
        let sibling = JournalIdentity {
            code: "0123456789abcdef".into(),
            ..mine.clone()
        };
        assert_eq!(mine.build, sibling.build, "the same commit");
        assert_ne!(mine, sibling, "and still not the same identity");

        assert!(
            !JournalIdentity {
                build: UNKNOWN_BUILD.into(),
                code: "0123456789abcdef".into(),
                ..mine.clone()
            }
            .names_a_build(),
            "a fingerprinted binary with no known source names no build"
        );
        assert!(
            !JournalIdentity {
                build: "3f9a1c2d".into(),
                code: String::new(),
                ..mine.clone()
            }
            .names_a_build(),
            "and a known commit with no fingerprint cannot tell two builds of it apart"
        );
        assert!(
            JournalIdentity {
                build: "3f9a1c2d".into(),
                code: "0123456789abcdef".into(),
                ..mine
            }
            .names_a_build()
        );
    }

    /// This process can fingerprint itself, and does it once.
    #[test]
    fn this_process_fingerprints_its_own_executable() {
        let a = code_id();
        assert_eq!(a.len(), 16, "a 64-bit fingerprint in hex: {a:?}");
        assert_eq!(a, code_id(), "computed once");
        assert_ne!(a, "0000000000000000");
    }

    /// With nowhere to write, the records are counted as errors rather than reported as written.
    #[test]
    fn a_journal_that_cannot_be_opened_is_counted_not_claimed() {
        let (sink, rx, counters) = AdmissionSink::new(QUEUE_CAPACITY);
        sink.record(
            &trade(1, at(9, 30, 0), 100),
            Some((true, true)),
            at(9, 30, 1),
        );
        let tmp = tempfile::tempdir().expect("tmp");
        let blocker = tmp.path().join("not-a-dir");
        std::fs::write(&blocker, b"x").expect("write");
        let mut writers = BTreeMap::new();
        let mut batch: Vec<AdmissionRecord> = rx.try_iter().collect();
        assert_eq!(batch.len(), 1);
        flush(
            &counters,
            &mut writers,
            &mut batch,
            &|_| blocker.join("live"),
            &identity("live", "recorded_live"),
        );
        let r = sink.report();
        assert_eq!((r.admitted, r.written, r.errors), (1, 0, 1), "{r:?}");
        assert!(!r.complete());
    }

    /// A record is filed under the **event's** trading date, never the admission clock's.
    ///
    /// They coincide in production and diverge everywhere else: a fixture or a replayed day is
    /// admitted at today's wall clock, and filing by that put its journal beside a recording of a
    /// different date, where no restore looks for it.
    #[test]
    fn a_record_is_filed_under_the_events_trading_date() {
        let (sink, rx, _c) = AdmissionSink::new(QUEUE_CAPACITY);
        // Admitted "now" (a much later wall clock), about a print delivered on the fixture day.
        sink.record(
            &trade(1, at(9, 30, 0), 100),
            Some((true, true)),
            at(9, 30, 0) + 400 * 24 * 3_600 * 1_000_000_000,
        );
        let r = rx.try_recv().expect("a record");
        assert_eq!(
            journal_day(&r),
            day(),
            "the print's day, not the clock's: {r:?}"
        );
    }

    /// A clean close checkpoints and does **not** seal, and the next boot still writes its own
    /// segment naming this one.
    ///
    /// A seal says nothing followed. At the moment a shutdown asks for one, the ingest and clock
    /// tasks are still admitting, so an offer can pass the fence a moment before the flag is set
    /// or reach the queue after the writer last looked — and a seal already on disk cannot be
    /// taken back. So the clean path leaves a checkpointed prefix with an unverified tail, which
    /// is what it can actually support.
    #[test]
    fn a_clean_close_checkpoints_without_claiming_nothing_followed() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("live");
        let id = identity("live", "recorded_live");

        // Boot one: three prints, then a clean close.
        let (sink, rx, counters) = AdmissionSink::new(QUEUE_CAPACITY);
        let d = dir.clone();
        let handle = spawn_writer(counters, rx, move |_| d.clone(), id.clone());
        for k in 0..3u64 {
            sink.record(
                &trade(k, at(9, 30, 0) + k as i64, 100),
                Some((true, true)),
                at(9, 30, 1) + k as i64,
            );
        }
        sink.close();
        handle.join().expect("the writer stops when asked");
        assert_eq!(sink.report().written, 3);

        let after_one = admissions::read(&dir, day(), &id);
        assert_eq!(after_one.headers.len(), 1, "one boot, one segment");
        let path = dir.join(format!(
            "{}1.{}",
            admissions::SEGMENT_PREFIX,
            admissions::SEGMENT_EXT
        ));
        let text = std::fs::read_to_string(&path).expect("the segment");
        assert!(
            text.contains("\"k\":\"ck\""),
            "the prefix is checkpointed: {text}"
        );
        assert!(
            !text.contains("\"k\":\"seal\""),
            "and not sealed, because a clean stop cannot prove nothing followed: {text}"
        );
        if id.names_a_build() {
            let p = after_one.proven.expect("a proof of the prefix");
            assert!(!p.sealed, "checkpointed, not sealed: {p:?}");
            assert_eq!(p.seq, 3, "and it covers everything that was written");
            assert_eq!(
                after_one.tail,
                admissions::Tail::Unverified {
                    records: 0,
                    reason: "journal_unproven_tail",
                },
                "no readable record lies past the checkpoint, but only a seal proves no tail"
            );
        } else {
            // A development build cannot name itself, so its journal is never a proof whatever
            // is on disk. The file assertions above are what this case checks.
            assert_eq!(
                after_one.unusable,
                Some(admissions::Unusable::Reason("journal_unknown_build"))
            );
        }

        // Boot two: its own segment, naming the first.
        let (sink2, rx2, counters2) = AdmissionSink::new(QUEUE_CAPACITY);
        let d = dir.clone();
        let handle2 = spawn_writer(counters2, rx2, move |_| d.clone(), id.clone());
        sink2.record(
            &trade(99, at(13, 0, 0), 500),
            Some((true, true)),
            at(13, 0, 1),
        );
        sink2.close();
        handle2.join().expect("the second writer stops");

        let both = admissions::read(&dir, day(), &id);
        assert_eq!(both.headers.len(), 2, "a segment per boot");
        assert_eq!(
            (both.headers[0].epoch, both.headers[1].epoch),
            (1, 2),
            "numbered in order"
        );
        let pred = both.headers[1]
            .predecessor
            .as_ref()
            .expect("the second names the first");
        assert_eq!((pred.epoch, pred.through_seq), (1, 3));
        assert!(
            !pred.sealed,
            "the predecessor was checkpointed rather than sealed, so the gap between the two \
             boots is declared rather than assumed away"
        );
        assert_eq!(
            both.records.len(),
            4,
            "both boots' admissions, in epoch order"
        );
        assert_eq!(
            both.records[3].rec.s, 1,
            "sequences restart per segment, so a position is (epoch, sequence)"
        );
    }

    /// Dropping the sink stops the writer too, and seals what it had: a process that exits without
    /// calling close still leaves a readable, sealed segment.
    #[test]
    fn dropping_the_sink_stops_and_seals_the_writer() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("live");
        let id = identity("live", "recorded_live");
        let (sink, rx, counters) = AdmissionSink::new(QUEUE_CAPACITY);
        let d = dir.clone();
        let handle = spawn_writer(counters, rx, move |_| d.clone(), id.clone());
        sink.record(
            &trade(1, at(9, 30, 0), 100),
            Some((true, true)),
            at(9, 30, 1),
        );
        drop(sink);
        handle.join().expect("the writer stops when its sink goes");
        let got = admissions::read(&dir, day(), &id);
        assert_eq!(got.headers.len(), 1);
        // The seal is there whatever the build identity says about using it.
        let text = std::fs::read_to_string(dir.join(format!(
            "{}1.{}",
            admissions::SEGMENT_PREFIX,
            admissions::SEGMENT_EXT
        )))
        .expect("the segment");
        assert!(
            text.contains("\"k\":\"seal\""),
            "the segment was sealed: {text}"
        );
    }

    /// A close writes everything queued before it seals, however much is waiting.
    ///
    /// The writer used to flush one bounded batch, see the close flag and seal — so on a busy
    /// market the rest of the queue was left unwritten behind a seal that says nothing followed.
    /// That is the one statement a seal must never get wrong, because the next process takes it
    /// as proof that the prefix is the whole of the run.
    #[test]
    fn a_close_drains_everything_queued_before_it_seals() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("live");
        let id = identity("live", "recorded_live");
        let (sink, rx, counters) = AdmissionSink::new(QUEUE_CAPACITY);

        // More than one batch, queued before the writer is even started, so the drain has to loop.
        let n = BATCH_MAX * 2 + 17;
        for k in 0..n as u64 {
            sink.record(
                &trade(k, at(9, 30, 0) + k as i64, 100),
                Some((true, true)),
                at(9, 30, 1) + k as i64,
            );
        }
        assert_eq!(sink.report().dropped, 0, "the queue held them all");

        let d = dir.clone();
        let handle = spawn_writer(counters, rx, move |_| d.clone(), id.clone());
        sink.close();
        handle.join().expect("the writer stops when asked");

        let rep = sink.report();
        assert_eq!(
            (rep.admitted, rep.written),
            (n as u64, n as u64),
            "every queued record reached the file before the writer stopped: {rep:?}"
        );
        assert_eq!(rep.fenced, 0);
        assert!(rep.complete(), "{rep:?}");

        let got = admissions::read(&dir, day(), &id);
        assert_eq!(got.records.len(), n, "and the file has all of them");
        assert_eq!(got.holes, 0);
        if id.names_a_build() {
            let p = got.proven.expect("a proof");
            assert_eq!(p.seq, n as u64, "checkpointed over all of it: {p:?}");
            assert!(!p.sealed, "a clean close does not seal");
            assert_eq!(
                got.tail,
                admissions::Tail::Unverified {
                    records: 0,
                    reason: "journal_unproven_tail",
                },
                "draining proves the prefix, while a clean close leaves the tail unsealed"
            );
        }
    }

    /// An admission offered after the close is refused and counted, and nothing on disk claims
    /// otherwise: the segment carries a checkpoint, never a seal.
    ///
    /// The fence is a bound on how far past a shutdown the file has to reach, not a proof. A
    /// concurrent offer can always pass it a moment before the flag is set, which is exactly why
    /// nothing written depends on it.
    #[test]
    fn an_admission_after_the_close_is_refused_and_nothing_is_sealed() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("live");
        let id = identity("live", "recorded_live");
        let (sink, rx, counters) = AdmissionSink::new(QUEUE_CAPACITY);
        sink.record(
            &trade(1, at(9, 30, 0), 100),
            Some((true, true)),
            at(9, 30, 1),
        );
        sink.close();
        // The ingest had one more in flight.
        assert_eq!(
            sink.record(
                &trade(2, at(9, 30, 2), 100),
                Some((true, true)),
                at(9, 30, 3)
            ),
            0,
            "refused rather than queued behind the seal"
        );
        let d = dir.clone();
        let handle = spawn_writer(counters, rx, move |_| d.clone(), id.clone());
        handle.join().expect("the writer stops");

        let rep = sink.report();
        assert_eq!(rep.fenced, 1, "{rep:?}");
        assert!(!rep.complete(), "nothing here may claim a whole journal");
        let got = admissions::read(&dir, day(), &id);
        assert_eq!(got.records.len(), 1, "only the one that got in");
        let text = std::fs::read_to_string(dir.join(format!(
            "{}1.{}",
            admissions::SEGMENT_PREFIX,
            admissions::SEGMENT_EXT
        )))
        .expect("the segment");
        assert!(
            !text.contains("\"k\":\"seal\""),
            "and it is not sealed, so a restore treats its tail as unverified: {text}"
        );
    }

    /// Dropping the sink is the one state a seal can rest on: the only sender in the process is
    /// gone, so nothing can arrive, and the queue was drained first.
    #[test]
    fn only_a_vanished_sink_justifies_a_seal() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("live");
        let id = identity("live", "recorded_live");
        let (sink, rx, counters) = AdmissionSink::new(QUEUE_CAPACITY);
        let d = dir.clone();
        let handle = spawn_writer(counters, rx, move |_| d.clone(), id.clone());
        for k in 0..(BATCH_MAX as u64 + 5) {
            sink.record(
                &trade(k, at(9, 30, 0) + k as i64, 100),
                Some((true, true)),
                at(9, 30, 1) + k as i64,
            );
        }
        // No close() at all: the sink simply goes away, as it does when the market is dropped.
        drop(sink);
        handle.join().expect("the writer stops when its sink goes");
        let text = std::fs::read_to_string(dir.join(format!(
            "{}1.{}",
            admissions::SEGMENT_PREFIX,
            admissions::SEGMENT_EXT
        )))
        .expect("the segment");
        assert!(
            text.contains("\"k\":\"seal\""),
            "with no sender left, nothing can follow and the seal is a fact"
        );
        let got = admissions::read(&dir, day(), &id);
        assert_eq!(got.records.len(), BATCH_MAX + 5, "drained before sealing");
        if id.names_a_build() {
            assert!(got.proven.expect("a proof").sealed);
            assert_eq!(got.tail, admissions::Tail::None);
        }
    }

    /// A normal date roll is not queue loss.
    ///
    /// The sequence used to be one run for the whole boot while the files are split per trading
    /// date, so the second date's segment began at N+1 and read as though N records had been
    /// dropped. A late print of the previous date, arriving after the roll, split both runs again.
    /// Each segment now carries its own run, so a hole means what it says.
    #[test]
    fn a_date_roll_is_not_queue_loss() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path().to_path_buf();
        let id = identity("live", "recorded_live");
        let (sink, rx, counters) = AdmissionSink::new(QUEUE_CAPACITY);
        let d1 = NaiveDate::from_ymd_opt(2026, 10, 15).expect("valid");
        let d2 = NaiveDate::from_ymd_opt(2026, 10, 16).expect("valid");
        let on = |d: NaiveDate, h: u32, m: u32, s: u32| {
            market_events::session::et(d, h, m, s, 0)
                .expect("valid")
                .nanos()
        };

        // Three prints on the first date, two on the second, then a late one belonging to the
        // first that arrives after the roll.
        let mut sent: Vec<MarketEvent> = Vec::new();
        for k in 0..3u64 {
            sent.push(trade(k, on(d1, 15, 0, k as u32), 100));
        }
        for k in 0..2u64 {
            sent.push(trade(10 + k, on(d2, 9, 30, k as u32), 200));
        }
        sent.push(trade(99, on(d1, 15, 30, 0), 300));
        for e in &sent {
            // Admitted "now", whenever that is: the filing follows the print, not the clock.
            sink.record(e, Some((true, true)), on(d2, 9, 31, 0));
        }

        let mut writers = BTreeMap::new();
        let mut batch: Vec<AdmissionRecord> = rx.try_iter().collect();
        assert_eq!(batch.len(), 6);
        // Each record is filed under its own print's date.
        let days: Vec<NaiveDate> = batch.iter().map(journal_day_of).collect();
        assert_eq!(days, vec![d1, d1, d1, d2, d2, d1], "{days:?}");
        // And the sequences are a run per date: 1,2,3 for the first and 1,2 for the second, with
        // the late print continuing the first date's run at 4.
        let seqs: Vec<u64> = batch.iter().map(|r| r.s).collect();
        assert_eq!(seqs, vec![1, 2, 3, 1, 2, 4], "{seqs:?}");

        let r = root.clone();
        flush(
            &counters,
            &mut writers,
            &mut batch,
            &|day| r.join(day.to_string()).join("live"),
            &id,
        );
        // Both dates read without a hole: a roll is a roll.
        for (d, want) in [(d1, 4usize), (d2, 2usize)] {
            let got = admissions::read(&r.join(d.to_string()).join("live"), d, &id);
            assert_eq!(got.holes, 0, "{d}: a date roll is not a dropped record");
            assert_eq!(got.records.len(), want, "{d}");
            if id.names_a_build() {
                assert_eq!(
                    got.tail,
                    admissions::Tail::Unverified {
                        records: 0,
                        reason: "journal_unproven_tail",
                    },
                    "{d}: the roll loses no readable records, but the checkpoint is not a seal"
                );
            }
        }
        let rep = sink.report();
        assert_eq!((rep.admitted, rep.dropped, rep.dates_evicted), (6, 0, 0));
        assert!(rep.complete(), "{rep:?}");
    }

    /// A record the queue really dropped still leaves a hole in its own date's run.
    #[test]
    fn a_dropped_record_still_leaves_a_hole_in_its_date() {
        let (sink, rx, _c) = AdmissionSink::new(2);
        let d = NaiveDate::from_ymd_opt(2026, 10, 16).expect("valid");
        let on = |k: u32| {
            market_events::session::et(d, 9, 30, k, 0)
                .expect("valid")
                .nanos()
        };
        for k in 0..5u64 {
            sink.record(&trade(k, on(k as u32), 100), Some((true, true)), on(10));
        }
        let kept: Vec<u64> = rx.try_iter().map(|r| r.s).collect();
        assert_eq!(kept, vec![1, 2], "the rest were refused");
        let rep = sink.report();
        assert_eq!((rep.admitted, rep.dropped), (5, 3), "{rep:?}");
        assert!(!rep.complete());
    }

    /// Only the inputs to the facts are journalled.
    #[test]
    fn quotes_and_status_are_not_fact_inputs() {
        assert!(is_fact_input(&trade(1, at(9, 30, 0), 100)));
        let q = market_events::Quote {
            symbol: "ABCD".into(),
            event_ts: UnixNanos(at(9, 30, 0)),
            receive_ts: None,
            bid_price: Price::from_cents(499),
            bid_size: 100,
            bid_exchange: "X".into(),
            ask_price: Price::from_cents(501),
            ask_size: 100,
            ask_exchange: "X".into(),
            conditions: Vec::new(),
            tape: Default::default(),
            source: market_events::Source::Fixture,
        };
        assert!(!is_fact_input(&MarketEvent::Quote(q)));
    }

    /// Date writers do not accumulate: a new date seals and closes the oldest.
    #[test]
    fn date_writers_are_bounded() {
        let tmp = tempfile::tempdir().expect("tmp");
        let id = identity("live", "recorded_live");
        let (sink, rx, counters) = AdmissionSink::new(QUEUE_CAPACITY);
        // Three dates, one record each.
        let days = [
            NaiveDate::from_ymd_opt(2026, 10, 14).expect("valid"),
            NaiveDate::from_ymd_opt(2026, 10, 15).expect("valid"),
            NaiveDate::from_ymd_opt(2026, 10, 16).expect("valid"),
        ];
        for (k, d) in days.iter().enumerate() {
            let exec = market_events::session::et(*d, 9, 30, 0, 0)
                .expect("valid")
                .nanos();
            sink.record(
                &trade(k as u64, exec, 100),
                Some((true, true)),
                exec + 1_000_000,
            );
        }
        let mut writers = BTreeMap::new();
        let root = tmp.path().to_path_buf();
        for _ in 0..3 {
            let mut batch: Vec<AdmissionRecord> = rx.try_iter().take(1).collect();
            if batch.is_empty() {
                break;
            }
            let r = root.clone();
            flush(
                &counters,
                &mut writers,
                &mut batch,
                &|day| r.join(day.to_string()).join("live"),
                &id,
            );
        }
        assert!(
            writers.len() <= WRITERS_MAX,
            "handles do not accumulate per date: {}",
            writers.len()
        );
        // Every date still got its own journal, and the ones that were closed were sealed.
        for d in days {
            let dir = root.join(d.to_string()).join("live");
            assert!(
                dir.join(format!(
                    "{}1.{}",
                    admissions::SEGMENT_PREFIX,
                    admissions::SEGMENT_EXT
                ))
                .exists(),
                "{d} has a journal"
            );
        }
    }

    /// Measurement, not a gate: bytes on disk per journalled SIP-shaped print, with the writer's
    /// checkpoint cadence at its extremes (one checkpoint per record, which is what a writer woken
    /// per print does at an ordinary rate, up to one per 4,096-record batch).
    ///
    /// `cargo test -p stockwatcher-server --lib admit::tests::measure_bytes_per_record -- --ignored --nocapture`
    #[test]
    #[ignore = "measurement"]
    fn measure_bytes_per_record() {
        let syms = ["AAPL", "TSLA", "SOFI", "NVDA", "GME", "ABCDE"];
        let exch = ["V", "P", "D", "Q", "K"];
        let n = 100_000u64;
        let (sink, rx, _c) = AdmissionSink::new(n as usize + 1);
        for k in 0..n {
            let exec = at(10, 0, 0) + (k as i64) * 1_700_000;
            let mut t = Trade::new(
                syms[(k % 6) as usize],
                UnixNanos(exec),
                Price::from_cents(1234 + (k % 50) as i64),
                1 + (k % 300),
                52_983_525_029_461 + k * 37,
            )
            .with_exchange(exch[(k % 5) as usize])
            .with_source(market_events::Source::AlpacaSip)
            .with_conditions(["@", "I"]);
            t.receive_ts = Some(UnixNanos(exec + 3_000_000));
            sink.record(&MarketEvent::Trade(t), Some((true, true)), exec + 4_000_000);
        }
        let all: Vec<AdmissionRecord> = rx.try_iter().collect();
        for batch in [1usize, 16, 4_096] {
            let tmp = tempfile::tempdir().expect("tmp");
            let mut w = AdmissionWriter::open(
                tmp.path(),
                day(),
                at(10, 0, 0),
                identity("live", "recorded_live"),
            )
            .expect("open");
            for chunk in all.chunks(batch) {
                w.append(chunk).expect("append");
                w.checkpoint().expect("checkpoint");
            }
            drop(w);
            let bytes: u64 = std::fs::read_dir(tmp.path())
                .expect("dir")
                .filter_map(|e| e.ok())
                .map(|e| std::fs::metadata(e.path()).map(|m| m.len()).unwrap_or(0))
                .sum();
            println!(
                "batch {batch}: {n} records, {bytes} B, {:.1} B/record",
                bytes as f64 / n as f64
            );
        }
    }
}
