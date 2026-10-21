//! Shadow measurement: when the four non-RVOL pillars were all passing, against when the strict
//! five-pillar panel first qualified a symbol. The definitions, frozen, and the capture behind
//! them.
//!
//! # What is measured, and why this one
//!
//! The agreed program needs its definitions written down **before** any forward observation
//! accumulates, because a metric chosen after seeing outcomes is not evidence.
//!
//! An earlier draft related the EXPERIMENTAL `setups-v0` arm to panel entry. That cannot measure
//! early discovery and the draft was wrong to try: a setup only arms for a Ross Panel member, and
//! membership requires strict five-pillar qualification first, so the setup is strictly
//! *downstream* of the thing it was supposed to anticipate. `setups-v0` stays experimental and
//! unchanged, and is not used here.
//!
//! What is related instead, both from labels the engine already produces under their current
//! unchanged policies:
//!
//! - a **shadow candidate**: the price, gain, float and news pillars all PASS at a decision. An
//!   observational label and nothing else. It is never called 5/5, never changes membership,
//!   never reaches the UI and never routes an order.
//! - the first **strict qualification**: all five pillars PASS
//!   (`stock_context::Qualification::Qualified`). Not panel retention, not a later re-entry.
//!
//! The two can occur at the same barrier. RVOL's ratio and its intraday diagnostics are recorded
//! as *features* of the session, never as gates or scores.
//!
//! # What this cannot establish
//!
//! A candidate is four of the five pillars the strict rule needs, so wherever strict holds the
//! candidate holds too, and the candidate can never occur *later*. The gap is therefore
//! non-negative by construction: it measures how long RVOL was the outstanding pillar, and
//! nothing about whether a candidate is a good stock to look at. No efficacy, accuracy or
//! "earlier is better" conclusion follows from it, and clearing the reporting floor
//! ([`MIN_SESSIONS`]) establishes nothing by itself.
//!
//! # Absence is not a negative
//!
//! The rules that keep this honest, frozen with the rest:
//!
//! - a session is **closed** when the **run's own clock** has passed that session's date at
//!   [`SESSION_CLOSES_AT_ET`]. Per date, never one flag for every date at once, and never from
//!   the wall clock: yesterday's closed sessions must not reopen at this morning's 09:00, and a
//!   process started at 19:59 must not count one barrier as a whole session.
//! - a session is **left-censored** when the **run** was not monitoring from the start of that
//!   date's window, or had a gap inside it. Not when the *symbol* happened to decide late: a
//!   symbol's first barrier is its first print, which for almost every symbol is well after
//!   04:00, so reading the symbol's own first decision as the monitoring start censored every
//!   session there was. What matters is whether the run was there to see, which is what the
//!   lifecycle records ([`Lifecycle`]).
//! - a session is a **negative** (`Neither`) only when the four candidate pillars were decided at
//!   *every* barrier observed. One decided barrier among many unknown ones is a gap, and a gap is
//!   [`SessionClass::Missing`].
//! - a candidate that never reached strict counts as `CandidateOnly` only when RVOL was decidable
//!   at *every* barrier from the candidate on. One decided barrier does not make the rest
//!   observable: if RVOL went unknown afterwards, the symbol might have qualified inside that
//!   interval and nothing would know.
//! - strict without a candidate, or a strict barrier before its candidate, contradicts the
//!   definition: [`SessionClass::Rejected`], counted and never pooled.
//!
//! Replays, fixtures and reconstructed history are kept out of the live cohort by the decision's
//! own provenance (`run_id`, `source`, `fidelity`), not by a caller's assertion.
//!
//! # Where the work happens
//!
//! [`ShadowSink::capture`] is called under the market's core lock, so it is one atomic and a
//! bounded `try_send`: never a wait, never disk, never a map walk. [`ShadowSink::absorb`] runs
//! with no lock held, folds at most [`ABSORB_MAX`] records per call — a drain that kept taking
//! whatever concurrent captures refilled would have no bound at all — and appends them to an
//! append-only log under a header naming the definitions, the schema and the build. A full queue
//! is a counted drop, a failed write a counted error, and a backlog a counted remainder; any of
//! them makes the report insufficient rather than quietly smaller.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use stock_context::{Decision, Fidelity, PillarId, Qualification, Tri};

/// The frozen identity of these definitions.
///
/// A change to what is measured, how a session enters the cohort, or how censoring works is a new
/// identity — never an edit of this one, because figures gathered under two definitions cannot be
/// pooled and a reader has no way to tell afterwards which rule produced which number.
pub const DEFINITIONS: &str = "shadow-candidate-vs-strict-v1";

/// The pillars a shadow candidate needs, all PASS. RVOL is deliberately not among them: it is the
/// pillar whose wait is being measured.
pub const CANDIDATE_PILLARS: [PillarId; 4] = [
    PillarId::Price,
    PillarId::Gain,
    PillarId::Float,
    PillarId::News,
];

/// Live, closed, uncensored sessions needed before any figure is reported.
///
/// A reporting floor, not a statistical power calculation: it stops a handful of sessions reading
/// as a measurement. Clearing it establishes nothing on its own.
pub const MIN_SESSIONS: usize = 200;

/// Symbol-sessions held before the least recently seen is dropped and counted.
pub const SESSIONS_TRACKED: usize = 8_192;

/// Runs the capture keeps monitoring for, and gaps per run. Both bounded: a replay seek makes a
/// run id per date, and a flapping feed makes a gap per flap, so neither grows with traffic but
/// neither is naturally finite either. What is dropped is counted, and a run whose gaps were
/// dropped can no longer claim to have covered anything.
pub const RUNS_TRACKED: usize = 64;
pub const GAPS_PER_RUN: usize = 512;

/// Events the queue holds before `capture` starts dropping them (and counting the drops).
pub const QUEUE_CAPACITY: usize = 8_192;

/// Records one [`ShadowSink::absorb`] folds at most. The queue refills while it drains, so
/// without a cap the call has no bound; what is left over is reported as a backlog.
pub const ABSORB_MAX: usize = 65_536;

/// The schema of the appended log.
pub const LOG_SCHEMA: u32 = 1;

/// When a session's window opens and closes, ET, on the **run's** clock. Frozen with the rest of
/// the definitions: 04:00 is the first bar a decision can cover, 20:00 the last.
pub const SESSION_OPENS_AT_ET: (u32, u32) = (4, 0);
pub const SESSION_CLOSES_AT_ET: (u32, u32) = (20, 0);

/// Added to the close, because a bar's decision is made at its **barrier**, not at its end.
///
/// The pillar policy's grid settles a bar `settle_ms` after it closes, so the canonical decision
/// covering the 20:00 bar has a cutoff of 20:00:02. Closing a session at 20:00 flat would settle
/// it before that decision had been processed, and a symbol that qualified in the last bar would
/// be counted a negative. So the endpoint is the last bar's barrier, which is the conservative
/// side: a session stays censored a moment longer rather than being judged a moment early.
///
/// Frozen here rather than read from the policy at runtime, like the rest of the definitions —
/// and tied to it by a test ([`tests::the_close_endpoint_is_the_last_barrier`]), so a policy that
/// changed its settle would fail that test instead of silently moving the frozen endpoint.
pub const SESSION_CLOSE_SETTLE_MS: i64 = 2_000;

/// A clock advance larger than this is a discontinuity in the run's own clock, and the interval
/// it skipped was not monitored. Frozen with the rest: five minutes is twenty bar grids, far more
/// than any ordinary scheduling delay and far less than an outage worth censoring for.
pub const GAP_SECS: i64 = 300;

/// What a session is counted as. Fixed by [`DEFINITIONS`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionClass {
    /// The candidate occurred and the strict rule followed (possibly at the same barrier).
    CandidateThenStrict,
    /// The candidate occurred, RVOL was decidable afterwards, and the strict rule never came.
    CandidateOnly,
    /// No candidate, and the four candidate pillars were decided at every barrier observed.
    Neither,
    /// Part of the window was not observed, or a candidate pillar was never decided, or RVOL
    /// never became decidable after a candidate. Not a negative: the question was not answered.
    Missing,
    /// The session's date had not closed on the run's clock, or the run was not there for the
    /// start of the window. Not a negative.
    Censored,
    /// Strict without a candidate, or strict before its candidate: the definition says neither
    /// can happen, so it is counted as a contradiction and never pooled.
    Rejected,
}

/// One decision, as the capture hands it off under the core lock.
///
/// Deliberately small and already reduced to the facts the definitions read, so building it is a
/// handful of field reads: nothing walks a map or touches a disk while the lock is held.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowEvent {
    pub run_id: String,
    pub source: String,
    pub fidelity: String,
    /// `pillars-v2@<hash>`: the rules the decision was made under.
    pub policy: String,
    pub trading_date: chrono::NaiveDate,
    pub symbol: String,
    /// The decision's barrier, on the run's clock.
    pub cutoff: i64,
    pub candidate: bool,
    pub strict: bool,
    /// All four candidate pillars decided (none UNKNOWN) at this barrier.
    pub four_decided: bool,
    /// RVOL decided (not UNKNOWN) at this barrier.
    pub rvol_decided: bool,
    /// Features, recorded and never used as a gate.
    pub rvol_value: Option<i64>,
    pub rvol_same_time_milli: Option<u64>,
    pub rvol_recent_milli: Option<u64>,
}

impl ShadowEvent {
    /// Reduces one decision to what the definitions read. Reads only: a decision is immutable
    /// once made and nothing here changes it.
    pub fn of(d: &Decision) -> ShadowEvent {
        let state = |p: PillarId| d.pillars.iter().find(|r| r.pillar == p).map(|r| r.state);
        let candidate = CANDIDATE_PILLARS
            .iter()
            .all(|p| state(*p) == Some(Tri::Pass));
        let four_decided = CANDIDATE_PILLARS
            .iter()
            .all(|p| state(*p).is_some_and(|s| s != Tri::Unknown));
        ShadowEvent {
            run_id: d.run_id.clone(),
            source: d.source.clone(),
            fidelity: format!("{:?}", d.fidelity),
            policy: d.policy.clone(),
            trading_date: d.trading_date,
            symbol: d.symbol.clone(),
            cutoff: d.cutoff,
            candidate,
            strict: d.qualification == Qualification::Qualified,
            four_decided,
            rvol_decided: state(PillarId::Rvol).is_some_and(|s| s != Tri::Unknown),
            rvol_value: d
                .pillars
                .iter()
                .find(|r| r.pillar == PillarId::Rvol)
                .and_then(|r| r.value),
            rvol_same_time_milli: d.rvol.same_time_milli,
            rvol_recent_milli: d.rvol.recent_milli,
        }
    }

    /// `<run id>:<source>:<trading date>:<symbol>`: what makes repeats of one session one
    /// session, across reconnects and redeliveries. Two runs of one day are two sessions.
    pub fn session_id(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.run_id, self.source, self.trading_date, self.symbol
        )
    }

    /// Whether this decision came from a live run, by its own provenance.
    pub fn live(&self) -> bool {
        self.source == "live" && self.fidelity == format!("{:?}", Fidelity::RecordedLive)
    }
}

/// What the run itself did, as distinct from what it decided.
///
/// Without this the capture has no idea whether it was *there*: a symbol's first decision is its
/// first print, which says nothing about when monitoring began, and a quiet market produces no
/// decisions at all so nothing would advance the clock or ever close a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum LifecycleKind {
    /// Monitoring began at this point on the run's clock.
    Start,
    /// The run's clock reached this point. Emitted whether or not anything was decided, so a
    /// quiet market still settles its sessions.
    Clock,
    /// The run's clock jumped: the interval `from`..`at` was not monitored.
    Gap { from: i64 },
    /// Monitoring ended at this point.
    Stop,
}

/// One lifecycle record, with the run it belongs to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lifecycle {
    pub run_id: String,
    pub source: String,
    pub fidelity: String,
    #[serde(flatten)]
    pub kind: LifecycleKind,
    /// On the run's clock.
    pub at: i64,
}

impl Lifecycle {
    /// `<run id>:<source>`: which run's monitoring this describes.
    pub fn run_key(&self) -> String {
        format!("{}:{}", self.run_id, self.source)
    }
}

/// What one run monitored, and when.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Monitoring {
    /// The earliest point on the run's clock it was monitoring from.
    pub started_at: Option<i64>,
    /// The latest point its clock reached. What a session's closure is settled against.
    pub clock: i64,
    /// Intervals its clock skipped: not monitored, so nothing inside them is absent.
    pub gaps: Vec<(i64, i64)>,
    /// Gaps dropped at [`GAPS_PER_RUN`]. One is enough to make coverage unanswerable: an
    /// interval nobody has a record of is not an interval that was watched.
    pub gaps_dropped: u64,
    /// When monitoring last ended, if it has. Anything after it was not watched unless something
    /// resumed, and a resumption is recorded as a gap rather than silently closing the hole.
    pub stopped_at: Option<i64>,
}

impl Monitoring {
    /// Whether the run monitored the whole of `from`..`to`.
    ///
    /// Every way it can fail to have done so: it had not started, its clock never reached the
    /// end, a recorded gap overlaps, a gap was dropped so the record is incomplete, or it had
    /// stopped before the end. The last one used to be missed, so a `Stop` followed by a later
    /// `Clock` restored coverage over an interval nothing was watching.
    pub fn covered(&self, from: i64, to: i64) -> bool {
        if self.gaps_dropped > 0 {
            return false;
        }
        if self.stopped_at.is_some_and(|t| t < to) {
            return false;
        }
        self.started_at.is_some_and(|s| s <= from)
            && self.clock >= to
            && !self.gaps.iter().any(|(a, b)| *a < to && *b > from)
    }
}

/// One record the capture hands off: a decision, or something the run itself did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "r", rename_all = "snake_case")]
pub enum ShadowRecord {
    Decision(ShadowEvent),
    Lifecycle(Lifecycle),
}

/// A record as the log holds it: with the sequence it was given.
///
/// The sequence is taken **before** the record is offered to the queue, so a record the queue
/// refuses leaves a hole a later reader of the file can see. Counters in this process's memory
/// cannot do that job: the process exits, and a decision lost between two retained clock records
/// would afterwards read as a clean negative — which is the one reading the definitions forbid.
/// The admission journal is built the same way, for the same reason.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoggedRecord {
    /// Monotone from 1 within the epoch.
    pub s: u64,
    #[serde(flatten)]
    pub rec: ShadowRecord,
}

/// The header the appended log opens with, so a reader knows what the rows are and what made
/// them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowHeader {
    pub v: u32,
    /// [`DEFINITIONS`]: figures gathered under two definitions cannot be pooled. Owned, because
    /// a reader of an old log needs to see the identity that log was written under, not this
    /// build's.
    pub definitions: String,
    /// The commit this binary was built from (`unknown` for a local build), and a fingerprint of
    /// the executable itself — the same identity the admission journal records, for the same
    /// reason: a commit alone cannot tell two builds of one tree apart.
    pub build: String,
    pub code: String,
    /// Which boot of the process this log belongs to, counted from the logs already present.
    pub epoch: u64,
    pub opened_at: i64,
}

/// The line a capture writes when it closes cleanly: nothing followed what is above it.
///
/// A file that ends at EOF with no gaps proves no *interior* record was lost. It cannot prove
/// that nothing followed the last line — records still queued when the process died, a write that
/// never reached the disk. So a prefix is proven only as far as a seal, and whatever lies past one
/// is an explicitly unverified tail. The admission journal draws the same line for the same
/// reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogSeal {
    /// The highest sequence the capture had handed out when it closed.
    pub s: u64,
    /// Records written below this seal.
    pub n: u64,
    pub at: i64,
}

/// What a reader will take from one segment before it stops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LogLimits {
    pub records: usize,
    pub bytes: u64,
}

impl Default for LogLimits {
    /// Generous for a real day and far short of exhausting a reviewer's process.
    fn default() -> Self {
        LogLimits {
            records: 4_000_000,
            bytes: 2 << 30,
        }
    }
}

/// What lies past a log's proven prefix.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum LogTail {
    /// A seal covers every record: nothing followed.
    None,
    /// Readable, but not proven — and nothing says whether more followed.
    Unverified {
        records: u64,
        /// `shadow_log_unsealed`, `shadow_log_holes`, `shadow_log_torn_line`,
        /// `shadow_log_disordered`.
        reason: &'static str,
    },
}

/// What a log on disk says, read without this process's counters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LogRead {
    pub header: Option<ShadowHeader>,
    pub records: Vec<LoggedRecord>,
    /// Sequences missing before the highest one read: records the queue refused.
    pub holes: u64,
    /// The sequence a seal proves the file holds everything up to, when one is there.
    pub prefix_through: Option<u64>,
    pub tail: LogTail,
    /// Why the file cannot be used at all: `shadow_log_unreadable`,
    /// `shadow_log_header_unreadable`, `shadow_log_empty`, `shadow_log_schema`,
    /// `shadow_log_other_definitions`, `shadow_log_too_large`.
    pub unusable: Option<&'static str>,
}

impl LogRead {
    /// Whether a seal proves the file holds every record of its epoch, with no gap.
    ///
    /// Not "the file ends tidily": that is what [`LogTail::Unverified`] is for.
    pub fn complete(&self) -> bool {
        self.unusable.is_none()
            && self.holes == 0
            && self.tail == LogTail::None
            && self
                .prefix_through
                .is_some_and(|t| self.records.iter().all(|r| r.s <= t))
    }

    /// Whether a window ending at `processed_through` falls inside the proven prefix.
    ///
    /// A comparable window needs its end inside what the file can vouch for. A window whose end
    /// is past the seal — or in a file with no seal at all — rests on records nothing proves are
    /// all there.
    pub fn covers(&self, processed_through: u64) -> bool {
        self.unusable.is_none()
            && self.holes == 0
            && self.prefix_through.is_some_and(|t| t >= processed_through)
    }
}

/// Reads one log segment, from disk alone.
///
/// Nothing here consults the process that wrote it. That is the point: a reviewer opening the file
/// months later has only the file, and a decision lost between two retained clock records must be
/// visible as a gap rather than reading as a clean negative.
pub fn read_log(path: &std::path::Path) -> LogRead {
    read_log_within(path, LogLimits::default())
}

/// [`read_log`] under explicit limits.
pub fn read_log_within(path: &std::path::Path, limits: LogLimits) -> LogRead {
    let mut out = LogRead {
        header: None,
        records: Vec::new(),
        holes: 0,
        prefix_through: None,
        tail: LogTail::None,
        unusable: None,
    };
    let unusable = |mut out: LogRead, why: &'static str| {
        out.unusable = Some(why);
        out
    };
    // The file's own size is checked before a byte of it is parsed, so an oversized segment never
    // reaches memory at all.
    if std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) > limits.bytes {
        return unusable(out, "shadow_log_too_large");
    }
    let Ok(file) = std::fs::File::open(path) else {
        return unusable(out, "shadow_log_unreadable");
    };
    let mut lines = std::io::BufRead::lines(std::io::BufReader::new(file));
    match lines.next() {
        Some(Ok(l)) => match serde_json::from_str::<ShadowHeader>(&l) {
            Ok(h) => {
                // A segment under another schema or another set of definitions is not this
                // reader's to interpret, and its figures cannot be pooled with these.
                if h.v != LOG_SCHEMA {
                    out.header = Some(h);
                    return unusable(out, "shadow_log_schema");
                }
                if h.definitions != DEFINITIONS {
                    out.header = Some(h);
                    return unusable(out, "shadow_log_other_definitions");
                }
                out.header = Some(h);
            }
            Err(_) => return unusable(out, "shadow_log_header_unreadable"),
        },
        _ => return unusable(out, "shadow_log_empty"),
    }

    let mut torn: Option<&'static str> = None;
    let mut seal: Option<LogSeal> = None;
    for l in lines {
        let Ok(l) = l else {
            torn = Some("shadow_log_torn_line");
            break;
        };
        if l.is_empty() {
            continue;
        }
        // A seal is a different shape from a record, and ends the file.
        if let Ok(z) = serde_json::from_str::<LogSeal>(&l) {
            seal = Some(z);
            break;
        }
        match serde_json::from_str::<LoggedRecord>(&l) {
            Ok(r) => {
                if out.records.len() >= limits.records {
                    return unusable(out, "shadow_log_too_large");
                }
                out.records.push(r);
            }
            Err(_) => {
                // A half line from a process killed mid-write, or a corrupt one. Nothing after
                // it is trusted.
                torn = Some("shadow_log_torn_line");
                break;
            }
        }
    }

    let mut expect = 1u64;
    let mut disordered = false;
    for r in &out.records {
        if r.s < expect {
            disordered = true;
            break;
        }
        out.holes += r.s - expect;
        expect = r.s + 1;
    }

    // A seal is only a proof if it describes the file: the same number of records, and a
    // sequence that reaches them.
    let proven = seal.filter(|z| z.n == out.records.len() as u64 && !disordered && out.holes == 0);
    out.prefix_through = proven.map(|z| z.s);
    out.tail = match (disordered, out.holes > 0, torn, proven) {
        (true, _, _, _) => LogTail::Unverified {
            records: out.records.len() as u64,
            reason: "shadow_log_disordered",
        },
        (_, true, _, _) => LogTail::Unverified {
            records: out.records.len() as u64,
            reason: "shadow_log_holes",
        },
        (_, _, Some(reason), _) => LogTail::Unverified {
            records: out.records.len() as u64,
            reason,
        },
        (_, _, None, Some(_)) => LogTail::None,
        // Contiguous to EOF and unsealed: no interior record was lost, and nothing says whether
        // any followed.
        (_, _, None, None) => LogTail::Unverified {
            records: 0,
            reason: "shadow_log_unsealed",
        },
    };
    out
}

/// Records a gap, or counts it as dropped at the bound.
fn push_gap(m: &mut Monitoring, from: i64, to: i64) {
    if from >= to {
        return;
    }
    if m.gaps.len() >= GAPS_PER_RUN {
        m.gaps_dropped += 1;
        return;
    }
    m.gaps.push((from, to));
}

/// Reserves a segment of this boot's own in `dir`, returning it and its epoch.
///
/// Two boots on one date must not share a file. They would: a log named for the date alone is
/// opened by the second boot in append mode, its header is already there, and every record it
/// writes is then attributed to the first boot's build and epoch. So the file is created
/// exclusively, and the epoch is whichever number was free.
///
/// `(None, 0)` when no segment could be reserved, which leaves the capture in memory only — and
/// the report then withholds every figure rather than implying a corpus exists.
pub fn reserve_segment(dir: &std::path::Path) -> (Option<PathBuf>, u64) {
    if std::fs::create_dir_all(dir).is_err() {
        return (None, 0);
    }
    for epoch in 1..=10_000u64 {
        let path = dir.join(format!("{DEFINITIONS}-{epoch}.jsonl"));
        match std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
        {
            Ok(_) => return (Some(path), epoch),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return (None, 0),
        }
    }
    (None, 0)
}

/// A session boundary on a date, in run-clock nanoseconds.
fn boundary_ns(date: chrono::NaiveDate, at: (u32, u32)) -> Option<i64> {
    market_events::session::et(date, at.0, at.1, 0, 0).map(|t| t.nanos())
}

/// When a session of `date` is closed: the last bar's **barrier**
/// ([`SESSION_CLOSE_SETTLE_MS`]), never the bar's end.
pub fn close_ns(date: chrono::NaiveDate) -> Option<i64> {
    boundary_ns(date, SESSION_CLOSES_AT_ET).map(|t| t + SESSION_CLOSE_SETTLE_MS * 1_000_000)
}

/// One symbol-session being accumulated.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    /// `<run id>:<source>`: whose monitoring settles this session.
    pub run_key: String,
    pub symbol: String,
    pub trading_date: chrono::NaiveDate,
    pub live: bool,
    pub policy: String,
    /// The first and last barriers observed for this session, on the run's clock.
    pub observed_from: i64,
    pub observed_through: i64,
    /// Barriers observed, and how many had the four candidate pillars decided.
    pub bars: u64,
    pub four_decided_bars: u64,
    pub first_candidate_at: Option<i64>,
    pub first_strict_at: Option<i64>,
    /// Barriers at or after the candidate, and how many of them had RVOL decided. Whether the
    /// session would have qualified is observable only if **every** one of them did: a single
    /// decided barrier leaves the rest of the interval unknown, and the symbol might have
    /// qualified inside it.
    pub bars_after_candidate: u64,
    pub rvol_decided_bars_after_candidate: u64,
    /// Features taken at the candidate barrier and never afterwards: a feature read later is a
    /// different quantity wearing the same name.
    pub rvol_value_at_candidate: Option<i64>,
    pub rvol_same_time_milli_at_candidate: Option<u64>,
    pub rvol_recent_milli_at_candidate: Option<u64>,
}

impl Session {
    /// How the session is classified, given what the run monitored.
    ///
    /// Everything is settled against the run's own lifecycle: its clock closes the date, and its
    /// monitoring window and gaps decide whether anything can be read as absent.
    pub fn class(&self, run: &Monitoring) -> SessionClass {
        // A contradiction of the definition comes first: it is not a session to classify.
        if self.first_strict_at.is_some() && self.first_candidate_at.is_none() {
            return SessionClass::Rejected;
        }
        if let (Some(c), Some(s)) = (self.first_candidate_at, self.first_strict_at)
            && s < c
        {
            return SessionClass::Rejected;
        }
        let (Some(opens), Some(closes)) = (
            boundary_ns(self.trading_date, SESSION_OPENS_AT_ET),
            close_ns(self.trading_date),
        ) else {
            return SessionClass::Censored;
        };
        // Per date, on the run's own clock, which advances whether or not anything was decided.
        // Monotone, so a closure stays closed.
        if run.clock < closes {
            return SessionClass::Censored;
        }
        // Was the run *there* for the whole window? Not whether this symbol decided early —
        // almost no symbol's first print is at 04:00 — but whether monitoring covered it.
        if !run.covered(opens, closes) {
            return SessionClass::Censored;
        }
        match (self.first_candidate_at, self.first_strict_at) {
            (Some(_), Some(_)) => SessionClass::CandidateThenStrict,
            // Observable only if RVOL was decided at every barrier from the candidate on.
            (Some(_), None)
                if self.bars_after_candidate > 0
                    && self.rvol_decided_bars_after_candidate == self.bars_after_candidate =>
            {
                SessionClass::CandidateOnly
            }
            (Some(_), None) => SessionClass::Missing,
            // A negative needs the question answered at every barrier observed, not at one.
            (None, None) if self.bars > 0 && self.four_decided_bars == self.bars => {
                SessionClass::Neither
            }
            (None, None) => SessionClass::Missing,
            (None, Some(_)) => SessionClass::Rejected,
        }
    }

    /// The signed gap: strict minus candidate, so a positive value means the candidate came
    /// first. Non-negative by construction (see the module docs).
    pub fn gap_ns(&self) -> Option<i64> {
        match (self.first_candidate_at, self.first_strict_at) {
            (Some(c), Some(s)) if s >= c => Some(s - c),
            _ => None,
        }
    }
}

/// Bounded capture of symbol-sessions, folded off the core lock.
#[derive(Debug, Default)]
pub struct Shadow {
    by_id: BTreeMap<String, Session>,
    /// What each run monitored, keyed `<run id>:<source>`. One entry per run of the process, so
    /// it is bounded by the number of boots a day, not by traffic.
    runs: BTreeMap<String, Monitoring>,
    /// Session ids, least recently seen first, for the bound.
    order: VecDeque<String>,
    dropped_sessions: u64,
    dropped_runs: u64,
}

impl Shadow {
    pub fn new() -> Shadow {
        Shadow::default()
    }

    /// Folds one event in.
    ///
    /// Repeats are idempotent for what matters: the first candidate and first strict barriers are
    /// minima, so a reconnect that re-decides a symbol cannot move them, and the cohort counts
    /// sessions rather than observations — so no amount of repetition manufactures a floor.
    pub fn fold_record(&mut self, r: &ShadowRecord) {
        match r {
            ShadowRecord::Decision(e) => self.fold(e),
            ShadowRecord::Lifecycle(l) => self.fold_lifecycle(l),
        }
    }

    /// Folds one lifecycle record: what the run itself did.
    pub fn fold_lifecycle(&mut self, l: &Lifecycle) {
        let key = l.run_key();
        self.room_for_a_run(&key);
        let m = self.runs.entry(key).or_default();
        m.started_at = Some(m.started_at.map_or(l.at, |s| s.min(l.at)));
        // Anything arriving after a stop means monitoring resumed, and the interval between is a
        // hole. Recording it as a gap is what keeps a `Stop` followed by a `Clock` from reading
        // as continuous coverage.
        if let Some(stopped) = m.stopped_at
            && l.at > stopped
            && !matches!(l.kind, LifecycleKind::Stop)
        {
            push_gap(m, stopped, l.at);
            m.stopped_at = None;
        }
        m.clock = m.clock.max(l.at);
        match l.kind {
            LifecycleKind::Gap { from } => push_gap(m, from, l.at),
            LifecycleKind::Stop => m.stopped_at = Some(l.at),
            LifecycleKind::Start | LifecycleKind::Clock => {}
        }
    }

    /// Makes room for one more run, dropping the one whose clock is furthest behind.
    ///
    /// A dropped run takes its monitoring record with it, so the sessions that depended on it can
    /// no longer be called covered — which is why the sessions are dropped with it rather than
    /// left to be classified against nothing.
    fn room_for_a_run(&mut self, key: &str) {
        if self.runs.contains_key(key) || self.runs.len() < RUNS_TRACKED {
            return;
        }
        let Some(oldest) = self
            .runs
            .iter()
            .min_by_key(|(_, m)| m.clock)
            .map(|(k, _)| k.clone())
        else {
            return;
        };
        self.runs.remove(&oldest);
        self.dropped_runs += 1;
        let gone: Vec<String> = self
            .by_id
            .iter()
            .filter(|(_, s)| s.run_key == oldest)
            .map(|(k, _)| k.clone())
            .collect();
        for k in gone {
            self.by_id.remove(&k);
            self.order.retain(|x| *x != k);
            self.dropped_sessions += 1;
        }
    }

    /// What a run monitored, for a caller inspecting it.
    pub fn monitoring(&self, run_key: &str) -> Option<&Monitoring> {
        self.runs.get(run_key)
    }

    pub fn fold(&mut self, e: &ShadowEvent) {
        let id = e.session_id();
        // A decision is also evidence that the run was monitoring at that point, but never
        // evidence of when monitoring *began*: a symbol's first barrier is its first print.
        let run = self
            .runs
            .entry(format!("{}:{}", e.run_id, e.source))
            .or_default();
        run.clock = run.clock.max(e.cutoff);
        let fresh = !self.by_id.contains_key(&id);
        let run_key = format!("{}:{}", e.run_id, e.source);
        let s = self.by_id.entry(id.clone()).or_insert_with(|| Session {
            id: id.clone(),
            run_key,
            symbol: e.symbol.clone(),
            trading_date: e.trading_date,
            live: e.live(),
            policy: e.policy.clone(),
            observed_from: e.cutoff,
            observed_through: e.cutoff,
            bars: 0,
            four_decided_bars: 0,
            first_candidate_at: None,
            first_strict_at: None,
            bars_after_candidate: 0,
            rvol_decided_bars_after_candidate: 0,
            rvol_value_at_candidate: None,
            rvol_same_time_milli_at_candidate: None,
            rvol_recent_milli_at_candidate: None,
        });
        s.bars += 1;
        s.observed_from = s.observed_from.min(e.cutoff);
        s.observed_through = s.observed_through.max(e.cutoff);
        if e.four_decided {
            s.four_decided_bars += 1;
        }
        if e.candidate && s.first_candidate_at.is_none() {
            s.first_candidate_at = Some(e.cutoff);
            s.rvol_value_at_candidate = e.rvol_value;
            s.rvol_same_time_milli_at_candidate = e.rvol_same_time_milli;
            s.rvol_recent_milli_at_candidate = e.rvol_recent_milli;
        }
        if s.first_candidate_at.is_some_and(|c| e.cutoff >= c) {
            s.bars_after_candidate += 1;
            if e.rvol_decided {
                s.rvol_decided_bars_after_candidate += 1;
            }
        }
        if e.strict && s.first_strict_at.is_none() {
            s.first_strict_at = Some(e.cutoff);
        }
        if !fresh && let Some(i) = self.order.iter().position(|x| *x == id) {
            self.order.remove(i);
        }
        self.order.push_back(id);
        while self.order.len() > SESSIONS_TRACKED {
            if let Some(oldest) = self.order.pop_front()
                && self.by_id.remove(&oldest).is_some()
            {
                self.dropped_sessions += 1;
            }
        }
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// The sessions held, for a caller inspecting them.
    pub fn sessions(&self) -> impl Iterator<Item = &Session> {
        self.by_id.values()
    }

    /// The figures, settled against what each run monitored.
    ///
    /// Takes no clock of its own: the run's clock comes from its own lifecycle records, so a
    /// quiet market still settles and a replay is measured on the day it replayed.
    pub fn summarise(&self) -> ShadowReport {
        let mut r = ShadowReport {
            definitions: DEFINITIONS,
            min_sessions: MIN_SESSIONS,
            sessions_held: self.by_id.len(),
            dropped_sessions: self.dropped_sessions,
            dropped_runs: self.dropped_runs,
            gaps_dropped: self.runs.values().map(|m| m.gaps_dropped).sum(),
            runs: self.runs.len(),
            ..ShadowReport::default()
        };
        let mut gaps: Vec<i64> = Vec::new();
        for s in self.by_id.values() {
            if !s.live {
                r.synthetic_sessions += 1;
                continue;
            }
            let run = self.runs.get(&s.run_key).cloned().unwrap_or_default();
            match s.class(&run) {
                SessionClass::Rejected => r.rejected_sessions += 1,
                SessionClass::Censored => r.censored_sessions += 1,
                SessionClass::Missing => r.missing_sessions += 1,
                SessionClass::CandidateThenStrict => {
                    r.live_sessions += 1;
                    r.candidate_then_strict += 1;
                    if let Some(g) = s.gap_ns() {
                        gaps.push(g);
                    }
                }
                SessionClass::CandidateOnly => {
                    r.live_sessions += 1;
                    r.candidate_only += 1;
                }
                SessionClass::Neither => {
                    r.live_sessions += 1;
                    r.neither += 1;
                }
            }
        }
        r.outcome = if r.live_sessions < MIN_SESSIONS {
            ShadowOutcome::InsufficientData {
                sessions: r.live_sessions,
                min_sessions: MIN_SESSIONS,
            }
        } else {
            gaps.sort_unstable();
            ShadowOutcome::Observed {
                sessions: r.live_sessions,
                candidate_then_strict: r.candidate_then_strict,
                candidate_only: r.candidate_only,
                neither: r.neither,
                same_barrier: gaps.iter().filter(|g| **g == 0).count(),
                candidate_first: gaps.iter().filter(|g| **g > 0).count(),
                median_gap_ns: gaps.get(gaps.len() / 2).copied(),
            }
        };
        r
    }
}

/// The bounded queue, the sessions behind it, and the append-only log.
pub struct ShadowSink {
    tx: SyncSender<LoggedRecord>,
    /// Taken only by `absorb`, never together with the market's core lock.
    rx: Mutex<Receiver<LoggedRecord>>,
    shadow: Mutex<Shadow>,
    /// Where each event is appended, when a path was given. Without one the capture is in memory
    /// only and the report says so: a restart would lose the cohort's first-event identities.
    log: Option<PathBuf>,
    /// Sequences handed out, whether or not the queue took them.
    seq: AtomicU64,
    captured: AtomicU64,
    dropped: AtomicU64,
    absorbed: AtomicU64,
    persisted: AtomicU64,
    write_errors: AtomicU64,
    /// Times an `absorb` reached [`ABSORB_MAX`] and left records queued for the next call.
    backlog: AtomicU64,
    /// Whether the log's header has been written. Written once, before any record.
    opened: std::sync::atomic::AtomicBool,
    capacity: usize,
    /// Holds the append, so a test can show that nothing waits on it. Compiled only into this
    /// crate's own tests: a production build has no way to gate a write, by design.
    #[cfg(test)]
    hold: std::sync::atomic::AtomicBool,
    /// What made these records: the same build identity the admission journal carries.
    build: String,
    code: String,
    epoch: u64,
}

impl ShadowSink {
    /// `build` and `code` are this process's identity, as the admission journal records it: the
    /// commit and a fingerprint of the running executable. They go in the log's header, because a
    /// corpus nobody can attribute to a build is not reviewable evidence.
    pub fn new(
        capacity: usize,
        log: Option<PathBuf>,
        build: String,
        code: String,
        epoch: u64,
    ) -> ShadowSink {
        let capacity = capacity.max(1);
        let (tx, rx) = sync_channel(capacity);
        ShadowSink {
            tx,
            rx: Mutex::new(rx),
            shadow: Mutex::new(Shadow::new()),
            log,
            seq: AtomicU64::new(0),
            captured: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            absorbed: AtomicU64::new(0),
            persisted: AtomicU64::new(0),
            write_errors: AtomicU64::new(0),
            backlog: AtomicU64::new(0),
            opened: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            hold: std::sync::atomic::AtomicBool::new(false),
            capacity,
            build,
            code,
            epoch,
        }
    }

    /// Records one decision. Safe under the core lock: one atomic, a bounded `try_send` and no
    /// I/O. A full queue is a counted drop **and** a hole in the log's sequence.
    pub fn capture(&self, e: ShadowEvent) {
        self.push(ShadowRecord::Decision(e));
    }

    /// Records what the run itself did. Same queue, same bound, same sequence: the lifecycle is
    /// evidence like any other and is lost the same way if it is dropped.
    pub fn capture_lifecycle(&self, l: Lifecycle) {
        self.push(ShadowRecord::Lifecycle(l));
    }

    fn push(&self, rec: ShadowRecord) {
        // The sequence is taken here, before the queue can refuse it, so a refusal leaves a gap
        // on disk rather than only a number in this process's memory.
        let s = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        match self.tx.try_send(LoggedRecord { s, rec }) {
            Ok(()) => {
                self.captured.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Folds everything queued into the sessions and appends it to the log. Call with no other
    /// lock held. Returns how many events moved.
    pub fn absorb(&self) -> usize {
        let (n, lines) = {
            let rx = self.rx.lock().expect("shadow receiver lock");
            let mut shadow = self.shadow.lock().expect("shadow lock");
            let mut lines: Vec<u8> = Vec::new();
            let mut n = 0u64;
            // Bounded: the queue refills while this drains, so a loop that took whatever
            // appeared would have no bound at all. What is left is taken next time.
            while n < ABSORB_MAX as u64 {
                let Ok(r) = rx.try_recv() else { break };
                n += 1;
                self.absorbed.fetch_add(1, Ordering::Relaxed);
                if self.log.is_some() {
                    serde_json::to_writer(&mut lines, &r).expect("a shadow record serializes");
                    lines.push(b'\n');
                }
                shadow.fold_record(&r.rec);
            }
            if n == ABSORB_MAX as u64 {
                // The cap was reached, so records are still queued. They are not counted here —
                // counting them would mean draining them, which is exactly the loss this guards
                // against — and the next call takes them.
                self.backlog.fetch_add(1, Ordering::Relaxed);
            }
            (n, lines)
        };
        // Both guards are gone before the write. Holding the sessions lock across a file append
        // would block every report for the length of the write, and the write has no business
        // being inside it.
        if let Some(path) = &self.log
            && !lines.is_empty()
        {
            match self.append_with_header(path, &lines) {
                Ok(()) => {
                    self.persisted.fetch_add(n, Ordering::Relaxed);
                }
                Err(e) => {
                    self.write_errors.fetch_add(n, Ordering::Relaxed);
                    tracing::warn!(error = %e, "shadow capture: cannot append; the cohort is not reviewable from disk");
                }
            }
        }
        n as usize
    }

    /// Writes the seal: nothing followed what is in the file.
    ///
    /// Only honest where no further record can arrive, which is why the absorber calls it when it
    /// holds the last reference to the sink.
    pub fn seal(&self) {
        let Some(path) = &self.log else { return };
        let z = LogSeal {
            s: self.seq.load(Ordering::Relaxed),
            n: self.absorbed.load(Ordering::Relaxed) - self.write_errors.load(Ordering::Relaxed),
            at: crate::observe::wall_now(),
        };
        let mut line = serde_json::to_vec(&z).expect("a seal serializes");
        line.push(b'\n');
        if append(path, &line).is_err() {
            self.write_errors.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Holds or releases the append. Tests only: there is no production path to it.
    #[cfg(test)]
    pub(crate) fn hold_writes(&self, on: bool) {
        self.hold.store(on, Ordering::Relaxed);
    }

    /// Starts the capture's own thread: folds and appends on a timer until the sink is dropped.
    ///
    /// Not an async task. The quotes publisher used to call `absorb` on its tick, which put a
    /// blocking file write on a runtime worker — the same mistake as holding a lock across I/O,
    /// one layer up.
    pub fn spawn_absorber(sink: std::sync::Arc<ShadowSink>) -> std::thread::JoinHandle<()> {
        std::thread::Builder::new()
            .name("shadow-capture".into())
            .spawn(move || {
                loop {
                    // One strong reference left means the market has gone: fold what is there
                    // and stop.
                    let last = std::sync::Arc::strong_count(&sink) == 1;
                    sink.absorb();
                    if last {
                        // Nothing can hand over another record: this thread holds the only
                        // reference, so the seal is a fact rather than a hope.
                        sink.seal();
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
            })
            .expect("spawn the shadow capture")
    }

    /// Appends, writing the header first if it is not there yet.
    ///
    /// The flag is set only after the header is actually on disk: marking it written and then
    /// failing would leave every later record under no header at all, which is a corpus nobody
    /// can attribute to a build.
    fn append_with_header(&self, path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
        #[cfg(test)]
        while self.hold.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        if !self.opened.load(Ordering::Relaxed) {
            let header = ShadowHeader {
                v: LOG_SCHEMA,
                definitions: DEFINITIONS.to_string(),
                build: self.build.clone(),
                code: self.code.clone(),
                epoch: self.epoch,
                opened_at: crate::observe::wall_now(),
            };
            let mut line = serde_json::to_vec(&header).expect("a header serializes");
            line.push(b'\n');
            append(path, &line)?;
            self.opened.store(true, Ordering::Relaxed);
        }
        append(path, bytes)
    }

    /// The report, settled against what each run monitored.
    pub fn report(&self) -> ShadowReport {
        let mut r = self.shadow.lock().expect("shadow lock").summarise();
        r.captured = self.captured.load(Ordering::Relaxed);
        r.dropped_events = self.dropped.load(Ordering::Relaxed);
        r.absorbed = self.absorbed.load(Ordering::Relaxed);
        r.persisted = self.persisted.load(Ordering::Relaxed);
        r.write_errors = self.write_errors.load(Ordering::Relaxed);
        r.queue_capacity = self.capacity;
        r.durable = self.log.is_some();
        r.build = self.build.clone();
        r.code = self.code.clone();
        r.epoch = self.epoch;
        r.absorb_capped = self.backlog.load(Ordering::Relaxed);
        // A cohort cannot be called comparable over events that were dropped, never written, or
        // held only in this process's memory. The sessions are still counted and reported; what
        // is withheld is the figure.
        if !r.complete() {
            r.outcome = ShadowOutcome::InsufficientData {
                sessions: r.live_sessions,
                min_sessions: MIN_SESSIONS,
            };
        }
        r
    }
}

fn append(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    f.write_all(bytes)?;
    f.flush()
}

/// What the shadow has, or why it has nothing to say.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShadowOutcome {
    /// Fewer than [`MIN_SESSIONS`] live closed uncensored sessions, or a capture that cannot
    /// support a comparison: no figure is reported, by design.
    InsufficientData {
        sessions: usize,
        min_sessions: usize,
    },
    /// A description of what this process observed. The gap is non-negative by construction, so
    /// it describes how long RVOL was the outstanding pillar — not whether a candidate was worth
    /// looking at, and not a comparison of two methods.
    Observed {
        sessions: usize,
        candidate_then_strict: usize,
        candidate_only: usize,
        neither: usize,
        same_barrier: usize,
        candidate_first: usize,
        median_gap_ns: Option<i64>,
    },
}

impl Default for ShadowOutcome {
    fn default() -> Self {
        ShadowOutcome::InsufficientData {
            sessions: 0,
            min_sessions: MIN_SESSIONS,
        }
    }
}

impl ShadowOutcome {
    pub fn measured(&self) -> bool {
        matches!(self, ShadowOutcome::Observed { .. })
    }

    /// One log-friendly line. `insufficient_data` is said out loud, never implied by absence, and
    /// nothing here is phrased as a comparison or a claim.
    pub fn line(&self) -> String {
        match self {
            ShadowOutcome::InsufficientData {
                sessions,
                min_sessions,
            } => format!("insufficient_data ({sessions}/{min_sessions} live closed sessions)"),
            ShadowOutcome::Observed {
                sessions,
                candidate_then_strict,
                candidate_only,
                neither,
                same_barrier,
                candidate_first,
                median_gap_ns,
            } => format!(
                "observed over {sessions} live closed sessions: candidate then strict {candidate_then_strict} (same barrier {same_barrier}, candidate earlier {candidate_first}, median wait for the rvol pillar {}), candidate only {candidate_only}, neither {neither}",
                median_gap_ns
                    .map(|g| format!("{:.1}s", g as f64 / 1e9))
                    .unwrap_or_else(|| "n/a".into())
            ),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ShadowReport {
    /// [`DEFINITIONS`]: what these numbers were gathered under.
    pub definitions: &'static str,
    pub min_sessions: usize,
    /// Symbol-sessions held in memory, every kind.
    pub sessions_held: usize,
    /// Live sessions that closed, were observed from the start of their window, and could be
    /// classified.
    pub live_sessions: usize,
    pub candidate_then_strict: usize,
    pub candidate_only: usize,
    pub neither: usize,
    /// Not closed on the run's clock, or the run was not there for the start. Not negatives.
    pub censored_sessions: usize,
    /// Part of the window unobserved, or the outcome never observable. Not negatives.
    pub missing_sessions: usize,
    /// Contradictions of the definition, counted and never pooled.
    pub rejected_sessions: usize,
    /// Replay, fixture and reconstructed sessions, by their own provenance. Never pooled.
    pub synthetic_sessions: usize,
    /// Sessions dropped at the bound.
    pub dropped_sessions: u64,
    /// Runs dropped at [`RUNS_TRACKED`], with their sessions.
    pub dropped_runs: u64,
    /// Gaps dropped at [`GAPS_PER_RUN`]. A run with any is no longer able to claim coverage.
    pub gaps_dropped: u64,
    /// Events captured, dropped at the queue, folded, appended, and failed to append.
    pub captured: u64,
    pub dropped_events: u64,
    pub absorbed: u64,
    pub persisted: u64,
    pub write_errors: u64,
    pub queue_capacity: usize,
    /// Whether the capture has somewhere durable to write. Without it the cohort exists only in
    /// this process's memory, and no figure is reported over it.
    pub durable: bool,
    /// Runs whose monitoring the capture knows about.
    pub runs: usize,
    /// Times a fold reached its cap and left records queued. Not a loss — the next call takes
    /// them — but a sign the capture is behind.
    pub absorb_capped: u64,
    /// What made these records, as the log's header states it.
    pub build: String,
    pub code: String,
    pub epoch: u64,
    pub outcome: ShadowOutcome,
}

impl ShadowReport {
    /// Whether every event the capture saw reached both the sessions and the log.
    pub fn complete(&self) -> bool {
        self.durable
            && !self.build.is_empty()
            && self.build != "unknown"
            && !self.code.is_empty()
            && self.dropped_events == 0
            && self.write_errors == 0
            && self.dropped_sessions == 0
            && self.dropped_runs == 0
            && self.gaps_dropped == 0
            && self.persisted == self.absorbed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use stock_context::{
        Counts, InstrumentId, NewsSummary, PillarResult, RvolDiagnostics, Timeline, VolumeFacts,
    };

    const SEC: i64 = 1_000_000_000;

    fn day(n: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, n).expect("valid")
    }

    /// A barrier on `date` at the given ET time.
    fn at(date: NaiveDate, h: u32, m: u32) -> i64 {
        market_events::session::et(date, h, m, 0, 0)
            .expect("valid")
            .nanos()
    }

    fn pillar(p: PillarId, state: Tri) -> PillarResult {
        PillarResult::new(p, state, None, "", String::new(), "fixture")
    }

    /// One decision: the four candidate pillars at `four`, RVOL at `rvol`.
    fn decision(
        symbol: &str,
        date: NaiveDate,
        cutoff: i64,
        four: Tri,
        rvol: Tri,
        live: bool,
    ) -> Decision {
        let pillars = vec![
            pillar(PillarId::Price, four),
            pillar(PillarId::Gain, four),
            pillar(PillarId::Rvol, rvol),
            pillar(PillarId::Float, four),
            pillar(PillarId::News, four),
        ];
        let qualification = if four == Tri::Pass && rvol == Tri::Pass {
            Qualification::Qualified
        } else {
            Qualification::NotQualified
        };
        Decision {
            seq: 1,
            decision_id: format!("d:{symbol}:{cutoff}"),
            run_id: if live { "live".into() } else { "replay".into() },
            source: if live { "live".into() } else { "replay".into() },
            fidelity: if live {
                Fidelity::RecordedLive
            } else {
                Fidelity::SyntheticFixture
            },
            policy: "pillars-v2@fixture".into(),
            symbol: symbol.into(),
            instrument: InstrumentId::symbol(symbol),
            trading_date: date,
            bar_end: cutoff - 2 * SEC,
            cutoff,
            pillars,
            qualification,
            counts: Counts::default(),
            in_discovery_band: Some(true),
            gain_vs_stored_close_bp: None,
            volume: VolumeFacts {
                shares: 0,
                dollars: 0,
                trades: 0,
                coverage: stock_context::Coverage::Unverified,
                coverage_reason: None,
                integrity: Default::default(),
            },
            rvol: RvolDiagnostics {
                daily_sessions: 0,
                same_time_milli: Some(3_000),
                same_time_sessions: 0,
                same_time_reason: "ok",
                recent_milli: Some(4_000),
                recent_sessions: 0,
                recent_reason: "ok",
            },
            news: NewsSummary {
                items: Vec::new(),
                primary_verified: false,
                favorability: None,
                coverage: None,
            },
            timeline: Timeline::default(),
            membership: Default::default(),
            changed: Vec::new(),
        }
    }

    fn life(kind: LifecycleKind, at: i64) -> Lifecycle {
        Lifecycle {
            run_id: "live".into(),
            source: "live".into(),
            fidelity: format!("{:?}", Fidelity::RecordedLive),
            kind,
            at,
        }
    }

    /// A run that monitored `from`..`to` without a gap.
    fn monitored(s: &mut Shadow, from: i64, to: i64) {
        s.fold_lifecycle(&life(LifecycleKind::Start, from));
        s.fold_lifecycle(&life(LifecycleKind::Clock, to));
    }

    /// A run monitoring the whole of `date` and past its close.
    fn monitored_whole(s: &mut Shadow, date: NaiveDate) {
        monitored(s, at(date, 3, 0), at(date, 20, 5));
    }

    fn only(s: &Shadow) -> &Session {
        s.sessions().next().expect("a session")
    }

    fn run_of(s: &Shadow) -> Monitoring {
        s.monitoring("live:live").cloned().unwrap_or_default()
    }

    /// Nothing is reported before a forward cohort exists, and the report names what it would
    /// measure.
    #[test]
    fn nothing_is_reported_before_a_forward_cohort_exists() {
        let s = Shadow::new();
        let r = s.summarise();
        assert_eq!(
            r.outcome,
            ShadowOutcome::InsufficientData {
                sessions: 0,
                min_sessions: MIN_SESSIONS
            }
        );
        assert!(!r.outcome.measured());
        assert!(r.outcome.line().starts_with("insufficient_data"));
        assert_eq!(r.definitions, DEFINITIONS);
    }

    /// The candidate is the four non-RVOL pillars; the strict rule is all five. The gap is signed
    /// so a positive value means the candidate came first.
    #[test]
    fn the_candidate_is_four_pillars_and_the_gap_is_signed_from_it() {
        let d = day(16);
        let mut s = Shadow::new();
        monitored_whole(&mut s, d);
        // Four pass, RVOL still failing: a candidate and not strict.
        s.fold(&ShadowEvent::of(&decision(
            "ABCD",
            d,
            at(d, 9, 30),
            Tri::Pass,
            Tri::Fail,
            true,
        )));
        // Ten minutes later RVOL passes too: strict.
        s.fold(&ShadowEvent::of(&decision(
            "ABCD",
            d,
            at(d, 9, 40),
            Tri::Pass,
            Tri::Pass,
            true,
        )));
        let one = only(&s);
        assert_eq!(one.first_candidate_at, Some(at(d, 9, 30)));
        assert_eq!(one.first_strict_at, Some(at(d, 9, 40)));
        assert_eq!(
            one.gap_ns(),
            Some(600 * SEC),
            "strict minus candidate: positive means the candidate was first"
        );
        assert_eq!(
            one.class(&run_of(&s)),
            SessionClass::CandidateThenStrict,
            "and the symbol's own late first decision does not censor it"
        );
        // The features were taken at the candidate barrier, and are features only.
        assert_eq!(one.rvol_same_time_milli_at_candidate, Some(3_000));
        assert_eq!(one.rvol_recent_milli_at_candidate, Some(4_000));
    }

    /// A symbol whose first print is mid-morning is not censored for it. Reading the symbol's own
    /// first decision as the monitoring start censored every session there was: almost no
    /// symbol's first print is at 04:00.
    #[test]
    fn a_symbols_late_first_decision_is_not_a_late_start() {
        let d = day(16);
        let mut s = Shadow::new();
        monitored_whole(&mut s, d);
        s.fold(&ShadowEvent::of(&decision(
            "LATEPRINT",
            d,
            at(d, 11, 15),
            Tri::Fail,
            Tri::Fail,
            true,
        )));
        assert_eq!(
            only(&s).class(&run_of(&s)),
            SessionClass::Neither,
            "the run watched the window; this symbol simply had nothing until 11:15"
        );
        assert_eq!(s.summarise().live_sessions, 1);
    }

    /// Every time order, equality included, and the contradictions the definition forbids.
    #[test]
    fn every_time_order_is_handled_including_equality() {
        let d = day(16);

        // Both at one barrier: a zero gap, not a missing one.
        let mut s = Shadow::new();
        monitored_whole(&mut s, d);
        s.fold(&ShadowEvent::of(&decision(
            "SAME",
            d,
            at(d, 9, 0),
            Tri::Pass,
            Tri::Pass,
            true,
        )));
        assert_eq!(only(&s).gap_ns(), Some(0));
        assert_eq!(
            only(&s).class(&run_of(&s)),
            SessionClass::CandidateThenStrict
        );

        // Candidate only, with RVOL decided at every barrier after it: a real outcome.
        let mut s = Shadow::new();
        monitored_whole(&mut s, d);
        for k in 0..5 {
            s.fold(&ShadowEvent::of(&decision(
                "CAND",
                d,
                at(d, 9, k),
                Tri::Pass,
                Tri::Fail,
                true,
            )));
        }
        assert_eq!(only(&s).gap_ns(), None);
        assert_eq!(only(&s).class(&run_of(&s)), SessionClass::CandidateOnly);

        // Strict without a candidate contradicts the definition — four of five is a prerequisite
        // of five of five — so it is rejected rather than counted as an early strict.
        let mut s = Shadow::new();
        monitored_whole(&mut s, d);
        let mut odd = decision("ODD", d, at(d, 9, 0), Tri::Fail, Tri::Pass, true);
        odd.qualification = Qualification::Qualified;
        s.fold(&ShadowEvent::of(&odd));
        assert_eq!(only(&s).class(&run_of(&s)), SessionClass::Rejected);
        let r = s.summarise();
        assert_eq!(r.rejected_sessions, 1);
        assert_eq!(r.live_sessions, 0, "never pooled");

        // A strict barrier before its candidate is the same contradiction.
        let mut s = Shadow::new();
        monitored_whole(&mut s, d);
        let mut early =
            ShadowEvent::of(&decision("REV", d, at(d, 9, 0), Tri::Fail, Tri::Pass, true));
        early.strict = true;
        early.candidate = false;
        s.fold(&early);
        let mut later = ShadowEvent::of(&decision(
            "REV",
            d,
            at(d, 9, 30),
            Tri::Pass,
            Tri::Fail,
            true,
        ));
        later.candidate = true;
        s.fold(&later);
        assert_eq!(only(&s).class(&run_of(&s)), SessionClass::Rejected);
    }

    /// Closure is per date, on the run's own clock, and stays established once reached.
    ///
    /// One flag for every date taken from the wall clock reopened yesterday's closed sessions at
    /// this morning's 09:00 — the figures changed overnight without an observation.
    #[test]
    fn closure_is_per_date_on_the_run_clock_and_stable() {
        let (first, second) = (day(15), day(16));
        let mut s = Shadow::new();
        s.fold_lifecycle(&life(LifecycleKind::Start, at(first, 3, 0)));
        s.fold(&ShadowEvent::of(&decision(
            "ABCD",
            first,
            at(first, 9, 0),
            Tri::Pass,
            Tri::Fail,
            true,
        )));

        // Mid-session on the first date: it has not closed.
        s.fold_lifecycle(&life(LifecycleKind::Clock, at(first, 15, 0)));
        let r = s.summarise();
        assert_eq!((r.censored_sessions, r.live_sessions), (1, 0), "{r:?}");

        // Past the first date's close, and then into the second day.
        s.fold_lifecycle(&life(LifecycleKind::Clock, at(first, 20, 5)));
        s.fold(&ShadowEvent::of(&decision(
            "ABCD",
            second,
            at(second, 9, 0),
            Tri::Pass,
            Tri::Fail,
            true,
        )));
        assert_eq!(s.len(), 2, "two dates, two sessions");
        let r = s.summarise();
        assert_eq!(
            (r.live_sessions, r.censored_sessions),
            (1, 1),
            "per date, not one flag for all of them: {r:?}"
        );

        // The next morning the first date stays closed. This is the case that used to reopen.
        s.fold_lifecycle(&life(LifecycleKind::Clock, at(second, 9, 30)));
        let r = s.summarise();
        assert_eq!(
            (r.live_sessions, r.censored_sessions),
            (1, 1),
            "a closure does not reopen because a new day started: {r:?}"
        );

        // After the second date's close, both have settled.
        s.fold_lifecycle(&life(LifecycleKind::Clock, at(second, 20, 5)));
        let r = s.summarise();
        assert_eq!((r.live_sessions, r.censored_sessions), (2, 0));
    }

    /// A quiet market still settles its sessions: the clock advances whether or not anything was
    /// decided, which is the whole reason the lifecycle exists.
    #[test]
    fn a_quiet_market_still_settles() {
        let d = day(16);
        let mut s = Shadow::new();
        s.fold_lifecycle(&life(LifecycleKind::Start, at(d, 3, 0)));
        s.fold(&ShadowEvent::of(&decision(
            "ABCD",
            d,
            at(d, 9, 0),
            Tri::Fail,
            Tri::Fail,
            true,
        )));
        // Nothing decided after 09:00, so the only thing that can close the day is the clock.
        assert_eq!(s.summarise().censored_sessions, 1);
        s.fold_lifecycle(&life(LifecycleKind::Clock, at(d, 20, 5)));
        let r = s.summarise();
        assert_eq!((r.live_sessions, r.neither), (1, 1), "{r:?}");
    }

    /// A run that started late did not watch the window, so its sessions are censored however
    /// complete they look.
    #[test]
    fn a_late_start_is_censored_not_counted() {
        let d = day(16);
        let mut s = Shadow::new();
        monitored(&mut s, at(d, 19, 59), at(d, 20, 5));
        s.fold(&ShadowEvent::of(&decision(
            "LATE",
            d,
            at(d, 19, 59),
            Tri::Pass,
            Tri::Fail,
            true,
        )));
        let r = s.summarise();
        assert_eq!(
            (r.censored_sessions, r.live_sessions),
            (1, 0),
            "one barrier a minute before the close is not a session: {r:?}"
        );
    }

    /// A gap in monitoring censors the sessions whose window it falls inside: nothing inside an
    /// interval nobody watched can be read as absent.
    #[test]
    fn a_monitoring_gap_censors_the_window_it_falls_in() {
        let d = day(16);
        let mut s = Shadow::new();
        s.fold_lifecycle(&life(LifecycleKind::Start, at(d, 3, 0)));
        s.fold(&ShadowEvent::of(&decision(
            "ABCD",
            d,
            at(d, 9, 0),
            Tri::Fail,
            Tri::Fail,
            true,
        )));
        // The clock jumps over the middle of the day.
        s.fold_lifecycle(&life(
            LifecycleKind::Gap { from: at(d, 10, 0) },
            at(d, 14, 0),
        ));
        s.fold_lifecycle(&life(LifecycleKind::Clock, at(d, 20, 5)));
        let r = s.summarise();
        assert_eq!(
            (r.censored_sessions, r.live_sessions),
            (1, 0),
            "the run was not watching for four hours of it: {r:?}"
        );
        let run = run_of(&s);
        assert_eq!(run.gaps, vec![(at(d, 10, 0), at(d, 14, 0))]);
        assert!(!run.covered(at(d, 4, 0), at(d, 20, 0)));
        // A gap outside the window does not censor it.
        let other = day(17);
        assert!(
            run.covered(at(d, 4, 0), at(d, 9, 30)) || run.clock < at(d, 9, 30),
            "a window before the gap is still covered"
        );
        let _ = other;
    }

    /// An unobserved interval is not an absence: a negative needs the question answered at every
    /// barrier observed, not at one.
    #[test]
    fn a_gap_in_the_window_is_missing_not_a_negative() {
        let d = day(16);
        let mut s = Shadow::new();
        monitored_whole(&mut s, d);
        // One decided barrier, then many where the pillars were UNKNOWN.
        s.fold(&ShadowEvent::of(&decision(
            "GAP",
            d,
            at(d, 9, 0),
            Tri::Fail,
            Tri::Fail,
            true,
        )));
        for k in 1..20 {
            s.fold(&ShadowEvent::of(&decision(
                "GAP",
                d,
                at(d, 9, k),
                Tri::Unknown,
                Tri::Unknown,
                true,
            )));
        }
        let r = s.summarise();
        assert_eq!(
            (r.missing_sessions, r.neither),
            (1, 0),
            "one decided barrier among unknown ones is a gap: {r:?}"
        );
        assert_eq!(r.live_sessions, 0);

        // Decided at every barrier and nothing passing: a real negative.
        let mut s = Shadow::new();
        monitored_whole(&mut s, d);
        for k in 0..20 {
            s.fold(&ShadowEvent::of(&decision(
                "NEG",
                d,
                at(d, 9, k),
                Tri::Fail,
                Tri::Fail,
                true,
            )));
        }
        let r = s.summarise();
        assert_eq!((r.neither, r.missing_sessions), (1, 0));
        assert_eq!(r.live_sessions, 1);
    }

    /// One RVOL-decided barrier does not make the rest of the interval observable.
    ///
    /// The flag was sticky: a single decided barrier after the candidate read as "the outcome was
    /// observable", even if RVOL went unknown for the whole rest of the day — during which the
    /// symbol might have qualified with nothing to see it.
    #[test]
    fn one_rvol_known_barrier_does_not_mask_later_unknowns() {
        let d = day(16);
        let mut s = Shadow::new();
        monitored_whole(&mut s, d);
        // The candidate, with RVOL decided at that barrier.
        s.fold(&ShadowEvent::of(&decision(
            "MASK",
            d,
            at(d, 9, 0),
            Tri::Pass,
            Tri::Fail,
            true,
        )));
        // Then RVOL is unknown for the rest of the session.
        for k in 1..30 {
            s.fold(&ShadowEvent::of(&decision(
                "MASK",
                d,
                at(d, 9, k),
                Tri::Pass,
                Tri::Unknown,
                true,
            )));
        }
        let one = only(&s);
        assert_eq!(one.bars_after_candidate, 30);
        assert_eq!(one.rvol_decided_bars_after_candidate, 1);
        let r = s.summarise();
        assert_eq!(
            (r.missing_sessions, r.candidate_only),
            (1, 0),
            "the outcome was observable at one barrier and unknown for the rest: {r:?}"
        );

        // With RVOL decided throughout, the same shape is a real `CandidateOnly`.
        let mut s = Shadow::new();
        monitored_whole(&mut s, d);
        for k in 0..30 {
            s.fold(&ShadowEvent::of(&decision(
                "SEEN",
                d,
                at(d, 9, k),
                Tri::Pass,
                Tri::Fail,
                true,
            )));
        }
        let r = s.summarise();
        assert_eq!((r.candidate_only, r.missing_sessions), (1, 0));
    }

    /// A candidate whose RVOL never decided is missing, not a candidate that failed to qualify.
    #[test]
    fn a_candidate_whose_rvol_never_decided_is_missing() {
        let d = day(16);
        let mut s = Shadow::new();
        monitored_whole(&mut s, d);
        for k in 0..10 {
            s.fold(&ShadowEvent::of(&decision(
                "NOBASE",
                d,
                at(d, 9, k),
                Tri::Pass,
                Tri::Unknown,
                true,
            )));
        }
        let r = s.summarise();
        assert_eq!(
            (r.missing_sessions, r.candidate_only),
            (1, 0),
            "whether it would have qualified was never answerable: {r:?}"
        );
    }

    /// Repeats of one symbol-session are one session, so reconnects cannot manufacture a floor.
    #[test]
    fn repeats_of_one_session_are_one_session() {
        let d = day(16);
        let mut s = Shadow::new();
        monitored_whole(&mut s, d);
        for k in 0..(MIN_SESSIONS as i64 * 3) {
            s.fold(&ShadowEvent::of(&decision(
                "ABCD",
                d,
                at(d, 9, 0) + k * SEC,
                Tri::Pass,
                Tri::Fail,
                true,
            )));
        }
        assert_eq!(s.len(), 1, "one session");
        let r = s.summarise();
        assert_eq!(r.live_sessions, 1);
        assert!(
            !r.outcome.measured(),
            "repetition does not clear the floor: {:?}",
            r.outcome
        );
        assert_eq!(
            only(&s).first_candidate_at,
            Some(at(d, 9, 0)),
            "and the first barrier stays the first"
        );
    }

    /// Two runs of one symbol and date are two sessions, so a restart does not merge them.
    #[test]
    fn two_runs_of_one_day_are_two_sessions() {
        let d = day(16);
        let mut s = Shadow::new();
        monitored_whole(&mut s, d);
        s.fold(&ShadowEvent::of(&decision(
            "ABCD",
            d,
            at(d, 9, 0),
            Tri::Pass,
            Tri::Fail,
            true,
        )));
        let mut second = ShadowEvent::of(&decision(
            "ABCD",
            d,
            at(d, 10, 0),
            Tri::Pass,
            Tri::Pass,
            true,
        ));
        second.run_id = "live:second-boot".into();
        s.fold(&second);
        assert_eq!(s.len(), 2, "one per run, not one per symbol-date");
        // The second run has no lifecycle of its own, so its session is censored: this process
        // does not know when that run began monitoring.
        let r = s.summarise();
        assert_eq!(r.censored_sessions, 1, "{r:?}");
    }

    /// Provenance decides the cohort, not a caller: a replay is counted and never pooled.
    #[test]
    fn replays_are_kept_out_by_their_own_provenance() {
        let d = day(16);
        let mut s = Shadow::new();
        monitored_whole(&mut s, d);
        for k in 0..(MIN_SESSIONS as u32 * 2) {
            s.fold(&ShadowEvent::of(&decision(
                &format!("S{k}"),
                d,
                at(d, 9, 0),
                Tri::Pass,
                Tri::Pass,
                false,
            )));
        }
        let r = s.summarise();
        assert_eq!(r.synthetic_sessions, MIN_SESSIONS * 2);
        assert_eq!(r.live_sessions, 0);
        assert!(!r.outcome.measured(), "{:?}", r.outcome);
    }

    /// With a cohort the report describes what was observed, and never compares.
    #[test]
    fn a_cohort_is_described_and_never_compared() {
        let d = day(16);
        let mut s = Shadow::new();
        monitored_whole(&mut s, d);
        for k in 0..MIN_SESSIONS as i64 {
            let sym = format!("S{k}");
            s.fold(&ShadowEvent::of(&decision(
                &sym,
                d,
                at(d, 9, 0),
                Tri::Pass,
                Tri::Fail,
                true,
            )));
            s.fold(&ShadowEvent::of(&decision(
                &sym,
                d,
                at(d, 9, 0) + 60 * (k % 3) * SEC,
                Tri::Pass,
                Tri::Pass,
                true,
            )));
        }
        let r = s.summarise();
        assert_eq!(r.live_sessions, MIN_SESSIONS);
        match r.outcome {
            ShadowOutcome::Observed {
                candidate_then_strict,
                same_barrier,
                candidate_first,
                median_gap_ns,
                ..
            } => {
                assert_eq!(candidate_then_strict, MIN_SESSIONS);
                assert!(same_barrier > 0, "a third were at the same barrier");
                assert!(candidate_first > 0);
                assert!(
                    median_gap_ns.is_some_and(|g| g >= 0),
                    "non-negative by construction: {median_gap_ns:?}"
                );
            }
            other => panic!("expected an observation, got {other:?}"),
        }
        let line = r.outcome.line();
        for word in ["faster", "better", "improves", "outperform", "5/5"] {
            assert!(!line.contains(word), "describes, never claims: {line}");
        }
    }

    /// The session bound holds and what it drops is counted.
    #[test]
    fn the_capture_is_bounded_and_drops_are_counted() {
        let d = day(16);
        let mut s = Shadow::new();
        monitored_whole(&mut s, d);
        for k in 0..(SESSIONS_TRACKED as u32 + 7) {
            s.fold(&ShadowEvent::of(&decision(
                &format!("S{k}"),
                d,
                at(d, 9, 0),
                Tri::Fail,
                Tri::Fail,
                true,
            )));
        }
        let r = s.summarise();
        assert_eq!(r.dropped_sessions, 7);
        assert_eq!(s.len(), SESSIONS_TRACKED);
    }

    /// A stop followed by a later clock does not restore coverage over the interval nothing was
    /// watching: the resumption is a gap.
    ///
    /// `covered` ignored `stopped_at`, so a run that stopped at noon and whose clock later
    /// reached 20:05 — a restart reusing the run id, a replay seek — read as having watched the
    /// whole day.
    #[test]
    fn a_stop_then_a_clock_does_not_restore_coverage() {
        let d = day(16);
        let mut s = Shadow::new();
        s.fold_lifecycle(&life(LifecycleKind::Start, at(d, 3, 0)));
        s.fold(&ShadowEvent::of(&decision(
            "ABCD",
            d,
            at(d, 9, 0),
            Tri::Fail,
            Tri::Fail,
            true,
        )));
        s.fold_lifecycle(&life(LifecycleKind::Stop, at(d, 12, 0)));
        // Stopped before the close: nothing can be read as absent after noon.
        assert!(!run_of(&s).covered(at(d, 4, 0), at(d, 20, 0)));
        assert_eq!(s.summarise().censored_sessions, 1);

        // Monitoring resumes and the clock passes the close. The interval between is a gap, so
        // the window is still not covered.
        s.fold_lifecycle(&life(LifecycleKind::Clock, at(d, 20, 5)));
        let run = run_of(&s);
        assert_eq!(
            run.gaps,
            vec![(at(d, 12, 0), at(d, 20, 5))],
            "the resumption records the hole it left: {run:?}"
        );
        assert!(!run.covered(at(d, 4, 0), at(d, 20, 0)));
        let r = s.summarise();
        assert_eq!(
            (r.censored_sessions, r.live_sessions),
            (1, 0),
            "and the session stays censored: {r:?}"
        );
    }

    /// A clean stop after the close is not a gap: the run watched the window and then finished.
    #[test]
    fn a_stop_after_the_close_still_counts_the_session() {
        let d = day(16);
        let mut s = Shadow::new();
        monitored_whole(&mut s, d);
        s.fold(&ShadowEvent::of(&decision(
            "ABCD",
            d,
            at(d, 9, 0),
            Tri::Fail,
            Tri::Fail,
            true,
        )));
        s.fold_lifecycle(&life(LifecycleKind::Stop, at(d, 20, 10)));
        let r = s.summarise();
        assert_eq!((r.live_sessions, r.neither), (1, 1), "{r:?}");
    }

    /// Gaps per run are bounded, and a run whose gaps were dropped can no longer claim to have
    /// covered anything: an interval nobody has a record of is not an interval that was watched.
    #[test]
    fn gaps_are_bounded_and_a_dropped_gap_withdraws_coverage() {
        let d = day(16);
        let mut s = Shadow::new();
        s.fold_lifecycle(&life(LifecycleKind::Start, at(d, 3, 0)));
        s.fold(&ShadowEvent::of(&decision(
            "ABCD",
            d,
            at(d, 9, 0),
            Tri::Fail,
            Tri::Fail,
            true,
        )));
        // More gaps than the bound, all of them tiny and outside the window.
        let base = at(d, 21, 0);
        for k in 0..(GAPS_PER_RUN as i64 + 5) {
            s.fold_lifecycle(&life(
                LifecycleKind::Gap {
                    from: base + k * 2 * SEC,
                },
                base + k * 2 * SEC + SEC,
            ));
        }
        let run = run_of(&s);
        assert_eq!(run.gaps.len(), GAPS_PER_RUN, "the record is bounded");
        assert_eq!(run.gaps_dropped, 5, "and what it dropped is counted");
        assert!(
            !run.covered(at(d, 4, 0), at(d, 20, 0)),
            "with gaps missing from the record, coverage is unanswerable"
        );
        let r = s.summarise();
        assert_eq!(r.gaps_dropped, 5);
        assert_eq!(r.censored_sessions, 1, "{r:?}");
        assert!(!r.complete());
    }

    /// Runs are bounded too, and a dropped run takes its sessions with it rather than leaving
    /// them to be classified against no monitoring at all.
    #[test]
    fn runs_are_bounded_and_take_their_sessions_with_them() {
        let d = day(16);
        let mut s = Shadow::new();
        for k in 0..(RUNS_TRACKED + 3) {
            let mut l = life(LifecycleKind::Start, at(d, 3, 0) + k as i64 * SEC);
            l.run_id = format!("replay:{k}");
            l.source = "replay".into();
            s.fold_lifecycle(&l);
            let mut e = ShadowEvent::of(&decision(
                "ABCD",
                d,
                at(d, 9, 0),
                Tri::Fail,
                Tri::Fail,
                true,
            ));
            e.run_id = format!("replay:{k}");
            e.source = "replay".into();
            s.fold(&e);
        }
        let r = s.summarise();
        assert_eq!(r.runs, RUNS_TRACKED, "the record is bounded: {r:?}");
        assert_eq!(r.dropped_runs, 3);
        assert!(
            r.dropped_sessions >= 3,
            "and their sessions went with them: {r:?}"
        );
        assert!(!r.complete());
    }

    // --- the sink: a bounded handoff, folded and appended off the lock ------------------------

    fn sink_with(log: Option<PathBuf>) -> ShadowSink {
        ShadowSink::new(
            QUEUE_CAPACITY,
            log,
            "3f9a1c2d".into(),
            "a1b2c3d4e5f60718".into(),
            1,
        )
    }

    /// The capture hands off under the lock and folds off it, appending every record under a
    /// header that names the definitions and the build.
    #[test]
    fn the_sink_hands_off_bounded_and_persists_under_a_header() {
        let tmp = tempfile::tempdir().expect("tmp");
        let log = tmp.path().join("shadow").join("events.jsonl");
        let d = day(16);
        let sink = sink_with(Some(log.clone()));
        sink.capture_lifecycle(life(LifecycleKind::Start, at(d, 3, 0)));
        for k in 0..5u32 {
            sink.capture(ShadowEvent::of(&decision(
                &format!("S{k}"),
                d,
                at(d, 9, 0),
                Tri::Pass,
                Tri::Pass,
                true,
            )));
        }
        sink.capture_lifecycle(life(LifecycleKind::Clock, at(d, 20, 5)));
        // Nothing is folded until something absorbs it: the lock side only queues.
        assert_eq!(sink.report().sessions_held, 0);
        assert_eq!(sink.absorb(), 7);
        let r = sink.report();
        assert_eq!((r.captured, r.absorbed, r.persisted), (7, 7, 7));
        assert_eq!((r.dropped_events, r.write_errors), (0, 0));
        assert_eq!(r.sessions_held, 5);
        assert_eq!(r.runs, 1, "and it knows what the run monitored");
        assert_eq!(r.live_sessions, 5, "which settles them: {r:?}");
        assert!(r.durable && r.complete(), "{r:?}");

        // The header names what the rows are and what made them.
        let text = std::fs::read_to_string(&log).expect("the log");
        let rows: Vec<&str> = text.lines().collect();
        let header: ShadowHeader =
            serde_json::from_str(rows.first().expect("a header")).expect("a header");
        assert_eq!(header.definitions, DEFINITIONS);
        assert_eq!(header.v, LOG_SCHEMA);
        assert_eq!((header.build.as_str(), header.epoch), ("3f9a1c2d", 1));
        assert_eq!(header.code, "a1b2c3d4e5f60718");
        assert_eq!(rows.len() - 1, 7, "then one line per record");
    }

    /// A full queue drops and counts it, and the report then reports no figure: a cohort is not
    /// comparable over records nobody has.
    #[test]
    fn a_full_queue_withholds_the_figure() {
        let tmp = tempfile::tempdir().expect("tmp");
        let d = day(16);
        let sink = ShadowSink::new(
            2,
            Some(tmp.path().join("events.jsonl")),
            "3f9a1c2d".into(),
            "code".into(),
            1,
        );
        for k in 0..6u32 {
            sink.capture(ShadowEvent::of(&decision(
                &format!("S{k}"),
                d,
                at(d, 9, 0),
                Tri::Pass,
                Tri::Pass,
                true,
            )));
        }
        sink.absorb();
        let r = sink.report();
        assert_eq!((r.captured, r.dropped_events), (2, 4), "{r:?}");
        assert!(!r.complete());
        assert!(!r.outcome.measured());
    }

    /// Without somewhere durable to write, the capture is memory only and no figure is reported:
    /// a cohort a restart would lose is not a forward corpus.
    #[test]
    fn a_capture_with_nowhere_to_write_reports_nothing() {
        let d = day(16);
        let sink = sink_with(None);
        sink.capture_lifecycle(life(LifecycleKind::Start, at(d, 3, 0)));
        for k in 0..(MIN_SESSIONS as u32 + 10) {
            sink.capture(ShadowEvent::of(&decision(
                &format!("S{k}"),
                d,
                at(d, 9, 0),
                Tri::Pass,
                Tri::Pass,
                true,
            )));
        }
        sink.capture_lifecycle(life(LifecycleKind::Clock, at(d, 20, 5)));
        sink.absorb();
        let r = sink.report();
        assert!(
            r.live_sessions >= MIN_SESSIONS,
            "the floor is cleared: {r:?}"
        );
        assert!(!r.durable);
        assert!(!r.complete());
        assert!(
            !r.outcome.measured(),
            "and still no figure, because nothing could review it: {:?}",
            r.outcome
        );
    }

    /// A build that cannot name itself cannot produce a reviewable corpus either.
    #[test]
    fn an_unknown_build_withholds_the_figure() {
        let tmp = tempfile::tempdir().expect("tmp");
        let d = day(16);
        let sink = ShadowSink::new(
            QUEUE_CAPACITY,
            Some(tmp.path().join("events.jsonl")),
            "unknown".into(),
            "code".into(),
            1,
        );
        sink.capture_lifecycle(life(LifecycleKind::Start, at(d, 3, 0)));
        for k in 0..(MIN_SESSIONS as u32 + 5) {
            sink.capture(ShadowEvent::of(&decision(
                &format!("S{k}"),
                d,
                at(d, 9, 0),
                Tri::Pass,
                Tri::Pass,
                true,
            )));
        }
        sink.capture_lifecycle(life(LifecycleKind::Clock, at(d, 20, 5)));
        sink.absorb();
        let r = sink.report();
        assert!(r.live_sessions >= MIN_SESSIONS);
        assert!(!r.complete(), "{r:?}");
        assert!(!r.outcome.measured());
    }

    /// A write that fails is counted, and withholds the figure for the same reason.
    #[test]
    fn a_failed_write_withholds_the_figure() {
        let tmp = tempfile::tempdir().expect("tmp");
        // A file where the log's directory belongs, so the append cannot create it.
        let blocker = tmp.path().join("shadow");
        std::fs::write(&blocker, b"x").expect("write");
        let d = day(16);
        let sink = sink_with(Some(blocker.join("events.jsonl")));
        sink.capture(ShadowEvent::of(&decision(
            "ABCD",
            d,
            at(d, 9, 0),
            Tri::Pass,
            Tri::Pass,
            true,
        )));
        sink.absorb();
        let r = sink.report();
        assert_eq!(r.write_errors, 1, "{r:?}");
        assert!(!r.complete());
        assert!(!r.outcome.measured());
    }

    /// A fold is bounded: the queue refills while it drains, so without a cap the call would have
    /// no bound. What is left is taken next time, never dropped.
    #[test]
    fn a_fold_is_bounded_and_leaves_the_rest_for_next_time() {
        let d = day(16);
        let over = ABSORB_MAX + 100;
        let sink = ShadowSink::new(over + 10, None, "3f9a1c2d".into(), "code".into(), 1);
        for k in 0..over {
            sink.capture(ShadowEvent::of(&decision(
                &format!("S{k}"),
                d,
                at(d, 9, 0),
                Tri::Fail,
                Tri::Fail,
                true,
            )));
        }
        assert_eq!(sink.report().dropped_events, 0, "the queue held them all");
        let first = sink.absorb();
        assert_eq!(first, ABSORB_MAX, "one call takes at most its cap");
        assert_eq!(sink.report().absorb_capped, 1);
        let second = sink.absorb();
        assert_eq!(second, 100, "and the rest are taken next time, not lost");
        let r = sink.report();
        assert_eq!(r.absorbed, over as u64);
        assert_eq!(r.dropped_events, 0);
    }

    /// Two boots in one directory get segments of their own, each under its own header.
    ///
    /// A log named for the date alone was shared: the second boot opened it in append mode, found
    /// a header already there, and wrote every record under the first boot's build and epoch.
    #[test]
    fn two_boots_reserve_separate_segments_with_their_own_headers() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("shadow");
        let d = day(16);

        let (first_path, first_epoch) = reserve_segment(&dir);
        let (second_path, second_epoch) = reserve_segment(&dir);
        let first_path = first_path.expect("a segment");
        let second_path = second_path.expect("a second segment");
        assert_ne!(first_path, second_path, "not the same file");
        assert_eq!((first_epoch, second_epoch), (1, 2), "numbered per boot");

        let write = |path: PathBuf, epoch: u64, build: &str| {
            let sink = ShadowSink::new(
                QUEUE_CAPACITY,
                Some(path),
                build.to_string(),
                "code".into(),
                epoch,
            );
            sink.capture(ShadowEvent::of(&decision(
                "ABCD",
                d,
                at(d, 9, 0),
                Tri::Pass,
                Tri::Pass,
                true,
            )));
            sink.absorb();
        };
        write(first_path.clone(), first_epoch, "buildone");
        write(second_path.clone(), second_epoch, "buildtwo");

        for (path, epoch, build) in [
            (first_path, 1u64, "buildone"),
            (second_path, 2u64, "buildtwo"),
        ] {
            let text = std::fs::read_to_string(&path).expect("the segment");
            let rows: Vec<&str> = text.lines().collect();
            let h: ShadowHeader =
                serde_json::from_str(rows.first().expect("a header")).expect("a header");
            assert_eq!(
                (h.epoch, h.build.as_str()),
                (epoch, build),
                "each boot's records are under its own identity: {path:?}"
            );
            assert_eq!(rows.len(), 2, "a header and one record: {path:?}");
        }
    }

    /// A header that could not be written is retried rather than skipped: records under no
    /// header are a corpus nobody can attribute to a build.
    #[test]
    fn a_header_that_failed_is_retried() {
        let tmp = tempfile::tempdir().expect("tmp");
        let blocker = tmp.path().join("blocked");
        std::fs::write(&blocker, b"x").expect("write");
        let d = day(16);
        let path = blocker.join("events.jsonl");
        let sink = sink_with(Some(path.clone()));
        let event = || {
            ShadowEvent::of(&decision(
                "ABCD",
                d,
                at(d, 9, 0),
                Tri::Pass,
                Tri::Pass,
                true,
            ))
        };
        sink.capture(event());
        sink.absorb();
        assert_eq!(
            sink.report().write_errors,
            1,
            "the header could not be written"
        );

        // The obstruction is removed; the next fold writes the header it owed.
        std::fs::remove_file(&blocker).expect("unblock");
        sink.capture(event());
        sink.absorb();
        let text = std::fs::read_to_string(&path).expect("the segment");
        let rows: Vec<&str> = text.lines().collect();
        let h: ShadowHeader =
            serde_json::from_str(rows.first().expect("a header")).expect("a header");
        assert_eq!(h.definitions, DEFINITIONS, "the header is there after all");
        assert_eq!(rows.len(), 2);
        // The run is still reported as incomplete: the first fold's record never landed.
        assert!(!sink.report().complete());
    }

    /// The close endpoint is the last bar's barrier, not the bar's end.
    ///
    /// The policy's grid settles a bar two seconds after it closes, so the canonical decision
    /// covering the 20:00 bar has a cutoff of 20:00:02. Closing at 20:00 flat settled the session
    /// before that decision was processed, and a symbol that qualified in the last bar would have
    /// been counted a negative.
    #[test]
    fn the_close_endpoint_is_the_last_barrier() {
        let d = day(16);
        let grid = &stock_context::Policy::current().body.grid;
        assert_eq!(
            SESSION_CLOSE_SETTLE_MS, grid.settle_ms,
            "the frozen endpoint follows the policy's settle; a changed policy fails here rather \
             than moving the definition quietly"
        );
        let end = at(d, 20, 0);
        let barrier = end + grid.settle_ms * 1_000_000;
        assert_eq!(close_ns(d), Some(barrier));

        // A clock at the bar's end has not closed the session: the decision is still to come.
        let mut s = Shadow::new();
        monitored(&mut s, at(d, 3, 0), end);
        s.fold(&ShadowEvent::of(&decision(
            "LASTBAR",
            d,
            at(d, 19, 59),
            Tri::Fail,
            Tri::Fail,
            true,
        )));
        assert_eq!(
            s.summarise().censored_sessions,
            1,
            "still open at 20:00:00, because the 20:00 bar is not decided yet"
        );

        // The barrier arrives, and with it the last bar's decision — which qualifies. Settling a
        // moment early would have called this session a negative.
        s.fold(&ShadowEvent::of(&decision(
            "LASTBAR",
            d,
            barrier,
            Tri::Pass,
            Tri::Pass,
            true,
        )));
        let r = s.summarise();
        assert_eq!(
            (r.live_sessions, r.candidate_then_strict, r.neither),
            (1, 1, 0),
            "the last bar counted: {r:?}"
        );
    }

    /// The same boundary through the real engine: a board advanced to the bar's end has not
    /// decided it, and the decision arrives at the barrier.
    #[test]
    fn the_engine_decides_the_last_bar_at_the_barrier() {
        use market_events::{Price, Trade, UnixNanos};
        let d = day(16);
        let grid = stock_context::Policy::current().body.grid.clone();
        let mut e = stock_context::Engine::new(
            stock_context::Policy::current(),
            stock_context::RunInfo {
                run_id: "live".into(),
                source: "live".into(),
                fidelity: Fidelity::RecordedLive,
                numerator_coverage: stock_context::Coverage::Unverified,
            },
        );
        // A print inside the 20:00 bar.
        let exec = at(d, 20, 0) - SEC;
        e.apply(
            &Trade::new("ABCD", UnixNanos(exec), Price::from_cents(500), 100, 1),
            stock_context::Verdict {
                ohlc: true,
                volume: true,
            },
            UnixNanos(exec + 1_000_000),
        );
        // At the bar's end, the bar covering that print has not been decided.
        let at_end = e.advance_to(UnixNanos(at(d, 20, 0)));
        let covered = |ds: &[Decision]| {
            ds.iter()
                .any(|x| x.bar_end >= at(d, 20, 0) && x.cutoff >= at(d, 20, 0))
        };
        assert!(
            !covered(&at_end),
            "the 20:00 bar is not decided at 20:00:00: {at_end:?}"
        );
        // At the barrier it is, with the cutoff the settle puts it at.
        let at_barrier = e.advance_to(UnixNanos(at(d, 20, 0) + grid.settle_ms * 1_000_000));
        let last = at_barrier.last().expect("the last bar is decided");
        assert_eq!(
            last.cutoff,
            last.bar_end + grid.settle_ms * 1_000_000,
            "a decision's cutoff is its bar's barrier"
        );
        assert!(
            last.cutoff <= close_ns(d).expect("a close"),
            "and the shadow's close endpoint is at or after it, so the decision is never missed: \
             {} vs {:?}",
            last.cutoff,
            close_ns(d)
        );
    }

    /// A decision the queue refused leaves a hole in the log, and a reader of the file alone sees
    /// it.
    ///
    /// This is the case counters in memory cannot cover. The process that dropped the record
    /// exits; a reviewer opens the file later and finds clock records either side of where the
    /// decision should have been. Without a sequence that reads as a symbol which was watched and
    /// never qualified — a clean negative — which is exactly the inference the definitions
    /// forbid. The sequence is taken before the queue can refuse, so the gap is on disk.
    #[test]
    fn a_dropped_decision_leaves_a_hole_a_reader_of_the_disk_can_see() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (log, epoch) = reserve_segment(tmp.path());
        let log = log.expect("a segment");
        let d = day(16);
        // A queue of one: the second record offered in any pair is refused.
        let sink = ShadowSink::new(
            1,
            Some(log.clone()),
            "testbuild".into(),
            "code".into(),
            epoch,
        );

        let event =
            |cutoff: i64| ShadowEvent::of(&decision("ABCD", d, cutoff, Tri::Pass, Tri::Pass, true));
        // Offered, taken, folded: sequence 1.
        sink.capture_lifecycle(life(LifecycleKind::Start, at(d, 3, 0)));
        sink.absorb();
        // Offered, taken: sequence 2. Then a decision offered while the queue is full: sequence
        // 3, refused.
        sink.capture(event(at(d, 9, 0)));
        sink.capture(event(at(d, 9, 1)));
        sink.absorb();
        // And a clock record after it: sequence 4, taken.
        sink.capture_lifecycle(life(LifecycleKind::Clock, at(d, 20, 5)));
        sink.absorb();

        let r = sink.report();
        assert_eq!(
            r.dropped_events, 1,
            "this process knows it dropped one: {r:?}"
        );
        assert!(!r.complete());

        // Now read only the file, as a reviewer would.
        let read = read_log(&log);
        let header = read.header.as_ref().expect("a header");
        assert_eq!(header.definitions, DEFINITIONS);
        assert_eq!(header.epoch, epoch);
        assert_eq!(
            read.records.len(),
            3,
            "three records reached the file: {:?}",
            read.records.iter().map(|x| x.s).collect::<Vec<_>>()
        );
        let seqs: Vec<u64> = read.records.iter().map(|x| x.s).collect();
        assert_eq!(seqs, vec![1, 2, 4], "with a gap where the third would be");
        assert_eq!(read.holes, 1, "which the reader counts without being told");
        assert_eq!(
            read.tail,
            LogTail::Unverified {
                records: 3,
                reason: "shadow_log_holes"
            }
        );
        assert_eq!(read.prefix_through, None, "a hole proves no prefix");
        assert!(
            !read.complete(),
            "so the file is not a whole account of its epoch"
        );
        assert!(
            !read.covers(4),
            "and no window ending inside it is comparable"
        );
        // The clock records either side are both there, which is exactly why the gap matters.
        assert!(
            read.records
                .iter()
                .filter(|x| matches!(x.rec, ShadowRecord::Lifecycle(_)))
                .count()
                == 2
        );
    }

    /// A whole log reads as whole, so the incomplete verdicts above mean something.
    #[test]
    fn a_whole_log_reads_as_whole() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (log, epoch) = reserve_segment(tmp.path());
        let log = log.expect("a segment");
        let d = day(16);
        let sink = sink_with(Some(log.clone()));
        sink.capture_lifecycle(life(LifecycleKind::Start, at(d, 3, 0)));
        for k in 0..5u32 {
            sink.capture(ShadowEvent::of(&decision(
                &format!("S{k}"),
                d,
                at(d, 9, 0),
                Tri::Pass,
                Tri::Pass,
                true,
            )));
        }
        sink.absorb();

        // Contiguous to EOF and unsealed: no interior record was lost, and nothing says whether
        // any followed. That is a prefix with an unknown tail, not a whole epoch.
        let read = read_log(&log);
        assert_eq!(read.records.len(), 6);
        assert_eq!(read.holes, 0);
        assert_eq!(
            read.tail,
            LogTail::Unverified {
                records: 0,
                reason: "shadow_log_unsealed"
            },
            "an unsealed file does not get to call itself whole"
        );
        assert!(!read.complete());
        assert!(!read.covers(6), "nothing is proven yet");

        // Sealed — which only genuine quiescence may do, and here the test holds the only
        // reference — and the prefix is proven through what it covers.
        sink.seal();
        let read = read_log(&log);
        assert_eq!(read.prefix_through, Some(6));
        assert_eq!(read.tail, LogTail::None);
        assert!(read.complete());
        assert!(
            read.covers(6),
            "a window ending inside the prefix is comparable"
        );
        assert!(!read.covers(7), "one ending past it is not");
        let _ = epoch;
    }

    /// A torn final line is named, and nothing after it is trusted.
    #[test]
    fn a_torn_log_line_is_named() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (log, _) = reserve_segment(tmp.path());
        let log = log.expect("a segment");
        let d = day(16);
        let sink = sink_with(Some(log.clone()));
        sink.capture_lifecycle(life(LifecycleKind::Start, at(d, 3, 0)));
        sink.absorb();
        // A process killed mid-write.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&log)
            .expect("append");
        std::io::Write::write_all(&mut f, b"{\"s\":2,\"r\":\"deci").expect("partial");
        drop(f);
        let read = read_log(&log);
        assert_eq!(
            read.tail,
            LogTail::Unverified {
                records: 1,
                reason: "shadow_log_torn_line"
            }
        );
        assert!(!read.complete());
        assert_eq!(read.records.len(), 1, "what was whole is still readable");
    }

    /// A file with no header is not a log: the records cannot be attributed to a build.
    #[test]
    fn a_log_without_a_header_is_not_a_log() {
        let tmp = tempfile::tempdir().expect("tmp");
        let path = tmp.path().join("headerless.jsonl");
        std::fs::write(&path, b"{\"s\":1,\"r\":\"lifecycle\",\"kind\":\"start\"}\n")
            .expect("write");
        let read = read_log(&path);
        assert_eq!(read.unusable, Some("shadow_log_header_unreadable"));
        assert!(read.header.is_none());
        assert!(!read.complete());
        // And a file that is not there at all.
        assert_eq!(
            read_log(&tmp.path().join("missing.jsonl")).unusable,
            Some("shadow_log_unreadable")
        );
    }

    /// A segment under another schema or another set of definitions is not this reader's to
    /// interpret, and its figures cannot be pooled with these.
    #[test]
    fn a_log_of_another_schema_or_definition_is_refused() {
        let tmp = tempfile::tempdir().expect("tmp");
        let write = |name: &str, header: serde_json::Value| {
            let p = tmp.path().join(name);
            let mut text = header.to_string();
            text.push('\n');
            text.push_str("{\"s\":1,\"r\":\"lifecycle\",\"run_id\":\"x\",\"source\":\"x\",\"fidelity\":\"x\",\"kind\":\"start\",\"at\":1}\n");
            std::fs::write(&p, text).expect("write");
            p
        };
        let base = serde_json::json!({
            "v": LOG_SCHEMA,
            "definitions": DEFINITIONS,
            "build": "b",
            "code": "c",
            "epoch": 1,
            "opened_at": 0
        });
        let mut other_schema = base.clone();
        other_schema["v"] = serde_json::json!(LOG_SCHEMA + 1);
        let read = read_log(&write("schema.jsonl", other_schema));
        assert_eq!(read.unusable, Some("shadow_log_schema"));
        assert!(read.records.is_empty(), "nothing is taken from it");

        let mut other_defs = base.clone();
        other_defs["definitions"] = serde_json::json!("shadow-something-else-v1");
        let read = read_log(&write("defs.jsonl", other_defs));
        assert_eq!(read.unusable, Some("shadow_log_other_definitions"));
        assert!(
            read.header.is_some(),
            "the header is reported so a reader knows what it found"
        );

        // The matching one reads, so the refusals above mean something.
        let read = read_log(&write("ok.jsonl", base));
        assert_eq!(read.unusable, None);
        assert_eq!(read.records.len(), 1);
    }

    /// A read is bounded by records and by bytes, so a runaway file cannot exhaust a reader.
    #[test]
    fn a_read_is_bounded_by_records_and_bytes() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (log, epoch) = reserve_segment(tmp.path());
        let log = log.expect("a segment");
        let d = day(16);
        let sink = sink_with(Some(log.clone()));
        for k in 0..20u32 {
            sink.capture(ShadowEvent::of(&decision(
                &format!("S{k}"),
                d,
                at(d, 9, 0),
                Tri::Pass,
                Tri::Pass,
                true,
            )));
        }
        sink.absorb();
        let _ = epoch;

        // Past the record cap: refused rather than read into memory.
        let read = read_log_within(
            &log,
            LogLimits {
                records: 5,
                ..LogLimits::default()
            },
        );
        assert_eq!(read.unusable, Some("shadow_log_too_large"));

        // Past the byte cap: refused before a byte is parsed.
        let read = read_log_within(
            &log,
            LogLimits {
                records: 1_000,
                bytes: 10,
            },
        );
        assert_eq!(read.unusable, Some("shadow_log_too_large"));
        assert!(read.header.is_none(), "not even the header was read");

        // Inside both: read.
        let read = read_log_within(&log, LogLimits::default());
        assert_eq!(read.unusable, None);
        assert_eq!(read.records.len(), 20);
    }

    /// The definitions are frozen, and RVOL is deliberately not a candidate pillar: it is the one
    /// whose wait is being measured.
    #[test]
    fn the_definitions_are_named_and_exclude_rvol() {
        assert_eq!(DEFINITIONS, "shadow-candidate-vs-strict-v1");
        assert!(!CANDIDATE_PILLARS.contains(&PillarId::Rvol));
        assert_eq!(CANDIDATE_PILLARS.len(), 4);
        assert_eq!(SESSION_OPENS_AT_ET, (4, 0));
        assert_eq!(SESSION_CLOSES_AT_ET, (20, 0));
        assert_eq!(SESSION_CLOSE_SETTLE_MS, 2_000);
        assert_eq!(GAP_SECS, 300);
    }
}
