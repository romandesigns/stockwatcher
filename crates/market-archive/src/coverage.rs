//! Coverage records: a durable, append-only account of **whether the live market was collecting**,
//! so a later question ("for SYMBOL on DAY between A and B, did we see everything?") has an answer
//! that does not depend on the data itself.
//!
//! # Why not infer it from the events
//!
//! A symbol with no prints for an hour is usually just quiet: absence of data is not a gap. The
//! only honest evidence of collection is the collector's own state: the process was running, the
//! stream was subscribed, nothing was dropped. So the recorder (`market-providers`, `recorder.rs`)
//! writes those facts here as they happen, and a no-trade interval is never recorded or inferred as
//! an outage.
//!
//! # Layout and format (schema version [`COVERAGE_SCHEMA`])
//!
//! ```text
//! $ARCHIVE_ROOT/coverage/2026-10-19.jsonl   # one file per trading date, JSON lines, append-only
//! ```
//!
//! Each line is one [`CoverageRecord`]: `{"v":1,"ts":<UTC ns>,"kind":"…", …}`. Kinds:
//!
//! | kind | meaning |
//! |---|---|
//! | `process_start` | the recorder started (a new process); the link starts `connecting` |
//! | `process_stop` | a clean stop: everything appended was written; nothing is collected until the next start |
//! | `heartbeat` | still alive (every [`HEARTBEAT_EVERY`]); a silence over [`HEARTBEAT_GRACE`] means the process died |
//! | `day_open` / `day_close` | a running process crossed into a new trading date; `day_open` repeats the state |
//! | `link` | the stream's state changed: `up` (subscribed: events flow, or the market is quiet), `connecting`, `down` |
//! | `subscription` | what the stream subscribes: `"all"` or `{"symbols":[…]}` |
//! | `drop` | events were dropped between `from` and `to` (`stage`: `stream` = the socket reader shed them, so the market never saw them; `recorder` = only the recording lost them) |
//! | `recording_stopped` | the recorder stopped writing the day (file error, space floor, the import started it) |
//! | `torn_tail` | a symbol's file had a torn last frame (a killed process); `cut_bytes` were truncated |
//!
//! Outside the coverage directory on purpose: a day directory is replaced by the import and
//! removed by the rolling window, and coverage must exist even for a day nothing was recorded on
//! (a holiday with the stream up all day is *complete*, not *unverified*). The files are a few
//! hundred KB a day (dominated by one heartbeat a minute) and are pruned with
//! [`Archive::prune_coverage`] once no data of their day is left.
//!
//! # The question ([`Archive::coverage`])
//!
//! For a symbol, a day and a time range, the answer is a [`Collection`]:
//!
//! - **`Partial`**: some of the range was *known* not to be collected (the link was not up, the
//!   process had stopped cleanly, events were dropped, the symbol was not subscribed; with
//!   [`CoverageBasis::Recording`] also a stopped recording or a torn tail).
//! - **`Unverified`**: nothing says it was missed, but not all of it is vouched for: no coverage
//!   file (every day before this existed), a heartbeat silence (a killed process), the time after
//!   the last record, or an unreadable line.
//! - **`Complete`**: every instant of the range lies in a stretch where the process was alive,
//!   the link up, the symbol subscribed and nothing dropped.
//!
//! Partial wins over unverified: a known gap is the stronger statement. There is no `Complete`
//! without records, so an old day can never read as complete.
//!
//! [`CoverageBasis::Stream`] asks about what the live market itself saw (the stock-context
//! engine's question about today's numerator); [`CoverageBasis::Recording`] about what is on disk
//! in the receipt-order recording (`live/`, or its preserved original), which can also lose events
//! the market saw (the recorder's queue overflowed, a killed process's unflushed frames).

use crate::error::ArchiveError;
use crate::{Archive, valid_symbol};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

/// The coverage directory at the archive root.
pub const COVERAGE_DIR: &str = "coverage";
/// The record schema this build writes and reads. A line with another `v` is unreadable.
pub const COVERAGE_SCHEMA: u32 = 1;
/// How often a running recorder writes a heartbeat.
pub const HEARTBEAT_EVERY: Duration = Duration::from_secs(60);
/// A silence longer than this between records of one process means it was not running.
pub const HEARTBEAT_GRACE: Duration = Duration::from_secs(150);
/// What a killed process may have held unflushed (`live::FRAME_MAX_AGE` plus the recorder's 1 s
/// flush tick, with margin): for the recording, the end of a segment that ended without a clean
/// stop is not vouched for by this much.
pub const UNFLUSHED_SLACK: Duration = Duration::from_secs(12);

/// The stream's state as recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkStatus {
    /// Subscribed: events flow, or the market is genuinely quiet.
    Up,
    Connecting,
    Down,
}

/// What the stream subscribes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Subscription {
    /// The whole market (`LIVE_SYMBOLS=*`).
    All,
    Symbols(Vec<String>),
}

impl Subscription {
    pub fn includes(&self, symbol: &str) -> bool {
        match self {
            Subscription::All => true,
            Subscription::Symbols(l) => l.iter().any(|s| s == symbol),
        }
    }
}

/// Where events were dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DropStage {
    /// The stream reader shed them (the consumer lagged): the market never saw them either.
    Stream,
    /// The recorder's queue overflowed: the market applied them, the recording lacks them.
    Recorder,
}

/// One fact (module docs, table of kinds).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CoverageEvent {
    ProcessStart {
        version: String,
    },
    ProcessStop,
    Heartbeat,
    DayOpen {
        link: LinkStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subscription: Option<Subscription>,
    },
    DayClose,
    Link {
        state: LinkStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    Subscription {
        symbols: Subscription,
    },
    Drop {
        stage: DropStage,
        /// UTC ns: the drops happened somewhere in `[from, to]`.
        from: i64,
        to: i64,
        count: u64,
    },
    RecordingStopped {
        reason: String,
    },
    TornTail {
        symbol: String,
        /// Length kept (the last whole frame's end).
        kept_bytes: u64,
        cut_bytes: u64,
    },
}

/// One line of a coverage file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageRecord {
    /// Schema version ([`COVERAGE_SCHEMA`]).
    pub v: u32,
    /// When it happened, UTC ns (wall clock).
    pub ts: i64,
    #[serde(flatten)]
    pub event: CoverageEvent,
}

impl CoverageRecord {
    pub fn new(ts: i64, event: CoverageEvent) -> Self {
        CoverageRecord {
            v: COVERAGE_SCHEMA,
            ts,
            event,
        }
    }

    /// A record stamped with the wall clock now.
    pub fn now(event: CoverageEvent) -> Self {
        Self::new(wall_now(), event)
    }
}

/// The wall clock, UTC ns.
pub fn wall_now() -> i64 {
    chrono::Utc::now().timestamp_nanos_opt().unwrap_or(i64::MAX)
}

/// A day's coverage file as read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CoverageLog {
    pub records: Vec<CoverageRecord>,
    /// Lines that did not parse (another schema version, corruption, a torn last line). Any of
    /// them could have been a gap, so a log with one never answers `Complete`.
    pub unreadable: u64,
}

impl CoverageLog {
    pub fn parse(bytes: &[u8]) -> Self {
        let mut log = CoverageLog::default();
        let complete = bytes.last().is_none_or(|b| *b == b'\n');
        let mut lines: Vec<&[u8]> = bytes.split(|b| *b == b'\n').collect();
        // `split` yields a final empty slice after a trailing newline; without one, the last
        // line is torn (a crash mid-append).
        let last = lines.pop();
        if !complete && last.is_some_and(|l| !l.is_empty()) {
            log.unreadable += 1;
        }
        for line in lines {
            if line.iter().all(|b| b.is_ascii_whitespace()) {
                continue;
            }
            match serde_json::from_slice::<CoverageRecord>(line) {
                Ok(r) if r.v == COVERAGE_SCHEMA => log.records.push(r),
                _ => log.unreadable += 1,
            }
        }
        log
    }
}

/// Collection verdict for a symbol over a range (module docs). The stock-context engine's
/// `Coverage` maps one to one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Collection {
    Complete,
    Partial,
    Unverified,
}

/// Which collection the question is about (module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoverageBasis {
    /// What the live market received.
    Stream,
    /// What the receipt-order recording holds.
    Recording,
}

/// Why part of a range was not collected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GapReason {
    /// The stream was connecting or down.
    LinkDown,
    /// The process had stopped cleanly and not started again.
    ProcessStopped,
    StreamDrop,
    RecorderDrop,
    RecordingStopped,
    NotSubscribed,
    TornTail,
}

/// A known gap, `[from, to)` UTC ns, clipped to the question's range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct CoverageGap {
    pub from: i64,
    pub to: i64,
    pub reason: GapReason,
}

/// The answer to [`Archive::coverage`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CoverageReport {
    pub collection: Collection,
    /// Known gaps (why it is `Partial`).
    pub gaps: Vec<CoverageGap>,
    /// Stretches nothing vouches for, `[from, to)` UTC ns, merged (why it is `Unverified`).
    pub unverified: Vec<(i64, i64)>,
}

impl CoverageReport {
    fn unverified_whole(from: i64, to: i64) -> Self {
        CoverageReport {
            collection: Collection::Unverified,
            gaps: Vec::new(),
            unverified: vec![(from, to.max(from + 1))],
        }
    }
}

impl Archive {
    pub fn coverage_dir(&self) -> PathBuf {
        self.root().join(COVERAGE_DIR)
    }

    pub fn coverage_path(&self, day: NaiveDate) -> PathBuf {
        self.coverage_dir()
            .join(format!("{}.jsonl", day.format("%Y-%m-%d")))
    }

    /// Appends records to `day`'s coverage file in one write, then `fdatasync`s it: the records
    /// are few (a heartbeat a minute) and a gap record must survive the crash that follows it.
    /// Not subject to the space floor: it is a few hundred bytes, and the record of a stopped
    /// recording is most needed exactly when the disk is full.
    pub fn append_coverage(
        &self,
        day: NaiveDate,
        records: &[CoverageRecord],
    ) -> Result<(), ArchiveError> {
        if records.is_empty() {
            return Ok(());
        }
        let dir = self.coverage_dir();
        std::fs::create_dir_all(&dir).map_err(|e| ArchiveError::io(&dir, e))?;
        let mut buf = Vec::with_capacity(records.len() * 96);
        for r in records {
            serde_json::to_writer(&mut buf, r).expect("coverage record serializes");
            buf.push(b'\n');
        }
        let path = self.coverage_path(day);
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| ArchiveError::io(&path, e))?;
        f.write_all(&buf).map_err(|e| ArchiveError::io(&path, e))?;
        f.sync_data().map_err(|e| ArchiveError::io(&path, e))
    }

    /// `day`'s coverage records; `None` when the day has no coverage file.
    pub fn read_coverage(&self, day: NaiveDate) -> Result<Option<CoverageLog>, ArchiveError> {
        let path = self.coverage_path(day);
        match std::fs::read(&path) {
            Ok(b) => Ok(Some(CoverageLog::parse(&b))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(ArchiveError::io(&path, e)),
        }
    }

    /// Days with a coverage file, ascending.
    pub fn coverage_days(&self) -> Result<Vec<NaiveDate>, ArchiveError> {
        let dir = self.coverage_dir();
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(ArchiveError::io(&dir, e)),
        };
        let mut days: Vec<NaiveDate> = rd
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter_map(|n| NaiveDate::parse_from_str(n.strip_suffix(".jsonl")?, "%Y-%m-%d").ok())
            .collect();
        days.sort();
        Ok(days)
    }

    /// **The coverage question** (module docs): was `symbol` collected on `day` over
    /// `[from, to)` (UTC ns)? A point question (`from == to`) asks about that instant. Judged
    /// against the wall clock now: a process heard from within [`HEARTBEAT_GRACE`] is taken to be
    /// still running, so a range ending now can be `Complete` while the market is live.
    ///
    /// Never `Complete` without evidence: a day without a coverage file (every day recorded before
    /// coverage existed, or with `LIVE_RECORD=0`) is `Unverified` over the whole range. With
    /// [`CoverageBasis::Recording`] a day whose recording is gone (neither `live/` nor a preserved
    /// original) is `Unverified` too: there is nothing left to be complete.
    pub fn coverage(
        &self,
        day: NaiveDate,
        symbol: &str,
        from: i64,
        to: i64,
        basis: CoverageBasis,
    ) -> Result<CoverageReport, ArchiveError> {
        self.coverage_at(day, symbol, from, to, basis, wall_now())
    }

    /// [`Archive::coverage`] judged at `now` (UTC ns) instead of the wall clock.
    pub fn coverage_at(
        &self,
        day: NaiveDate,
        symbol: &str,
        from: i64,
        to: i64,
        basis: CoverageBasis,
        now: i64,
    ) -> Result<CoverageReport, ArchiveError> {
        if !valid_symbol(symbol) {
            return Err(ArchiveError::InvalidSymbol(symbol.to_string()));
        }
        if basis == CoverageBasis::Recording
            && !self.live_manifest_path(day).is_file()
            && !self.original_manifest_path(day).is_file()
        {
            return Ok(CoverageReport::unverified_whole(from, to));
        }
        Ok(match self.read_coverage(day)? {
            Some(log) => classify(&log, symbol, from, to, basis, now),
            None => CoverageReport::unverified_whole(from, to),
        })
    }

    /// Deletes coverage files older than every day that still has data (an import, a live
    /// recording or a preserved original) and older than `keep_from` (the recorder passes the day
    /// it records). Returns the days deleted. With no data at all and no `keep_from` nothing is
    /// deleted: there is nothing to measure age against.
    pub fn prune_coverage(
        &self,
        keep_from: Option<NaiveDate>,
    ) -> Result<Vec<NaiveDate>, ArchiveError> {
        let oldest = [
            self.days()?.first().copied(),
            self.live_days()?.first().copied(),
            self.original_days()?.first().copied(),
            keep_from,
        ]
        .into_iter()
        .flatten()
        .min();
        let Some(oldest) = oldest else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for d in self.coverage_days()?.into_iter().filter(|d| *d < oldest) {
            let p = self.coverage_path(d);
            match std::fs::remove_file(&p) {
                Ok(()) => out.push(d),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(ArchiveError::io(&p, e)),
            }
        }
        Ok(out)
    }
}

/// One stretch of a process's life within a day file, with its current state.
struct Seg {
    start: i64,
    last: i64,
    link: LinkStatus,
    link_since: i64,
    sub: Option<Subscription>,
    sub_since: i64,
    recording: bool,
    rec_since: i64,
}

impl Seg {
    fn new(ts: i64, link: LinkStatus, sub: Option<Subscription>) -> Self {
        Seg {
            start: ts,
            last: ts,
            link,
            link_since: ts,
            sub,
            sub_since: ts,
            recording: true,
            rec_since: ts,
        }
    }

    /// The same process carrying on after a silence: same state, new start.
    fn resume(&self, ts: i64) -> Self {
        let mut s = Seg::new(ts, self.link, self.sub.clone());
        s.recording = self.recording;
        s
    }
}

/// What [`classify`] accumulates.
struct Acc<'a> {
    symbol: &'a str,
    rec: bool,
    /// Stretches whose state is known (segments, and clean-stop gaps).
    known: Vec<(i64, i64)>,
    /// Stretches inside segments nothing vouches for (subscription unknown).
    unknown: Vec<(i64, i64)>,
    gaps: Vec<CoverageGap>,
    /// `(end, unclean)` of every closed segment as it was heard from (before any slack), and
    /// every segment's start.
    ends: Vec<(i64, bool)>,
    starts: Vec<i64>,
}

impl Acc<'_> {
    fn gap(&mut self, from: i64, to: i64, reason: GapReason) {
        if to > from {
            self.gaps.push(CoverageGap { from, to, reason });
        }
    }

    /// Pushes the state stretches open since their change up to `t` (a state change or the end).
    fn link_until(&mut self, s: &Seg, t: i64) {
        if s.link != LinkStatus::Up {
            self.gap(s.link_since, t, GapReason::LinkDown);
        }
    }

    fn sub_until(&mut self, s: &Seg, t: i64) {
        match &s.sub {
            Some(sub) if sub.includes(self.symbol) => {}
            Some(_) => self.gap(s.sub_since, t, GapReason::NotSubscribed),
            None if t > s.sub_since => self.unknown.push((s.sub_since, t)),
            None => {}
        }
    }

    fn rec_until(&mut self, s: &Seg, t: i64) {
        if self.rec && !s.recording {
            self.gap(s.rec_since, t, GapReason::RecordingStopped);
        }
    }

    /// Closes a segment at `end`. An unclean end (no stop, no day close: a killed process or a
    /// stalled one) is vouched for only up to the last record, and for the recording
    /// [`UNFLUSHED_SLACK`] less.
    fn close(&mut self, s: Seg, end: i64, unclean: bool) {
        self.ends.push((end, unclean));
        let end = if unclean && self.rec {
            (end - UNFLUSHED_SLACK.as_nanos() as i64).max(s.start)
        } else {
            end
        };
        let end = end.max(s.start);
        self.known.push((s.start, end));
        let (link_t, sub_t, rec_t) = (
            end.max(s.link_since),
            end.max(s.sub_since),
            end.max(s.rec_since),
        );
        self.link_until(&s, link_t);
        self.sub_until(&s, sub_t);
        self.rec_until(&s, rec_t);
    }
}

/// Merges spans into sorted, non-overlapping ones.
fn merge(mut v: Vec<(i64, i64)>) -> Vec<(i64, i64)> {
    v.retain(|(a, b)| b > a);
    v.sort();
    let mut out: Vec<(i64, i64)> = Vec::with_capacity(v.len());
    for (a, b) in v {
        match out.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// `[from, to)` minus `cover` (merged).
fn subtract(from: i64, to: i64, cover: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    let mut at = from;
    for &(a, b) in cover {
        if b <= at {
            continue;
        }
        if a >= to {
            break;
        }
        if a > at {
            out.push((at, a.min(to)));
        }
        at = at.max(b);
        if at >= to {
            break;
        }
    }
    if at < to {
        out.push((at, to));
    }
    out
}

/// The verdict for `symbol` over `[from, to)` from one day's records, judged at `now` (module
/// docs). Pure: [`Archive::coverage`] reads the file and calls this.
pub fn classify(
    log: &CoverageLog,
    symbol: &str,
    from: i64,
    to: i64,
    basis: CoverageBasis,
    now: i64,
) -> CoverageReport {
    let to = to.max(from + 1);
    let grace = HEARTBEAT_GRACE.as_nanos() as i64;
    let rec = basis == CoverageBasis::Recording;
    let mut records: Vec<&CoverageRecord> = log.records.iter().collect();
    // Notes are stamped when they happen and written up to a second later, between heartbeats.
    records.sort_by_key(|r| r.ts);
    let mut acc = Acc {
        symbol,
        rec,
        known: Vec::new(),
        unknown: Vec::new(),
        gaps: Vec::new(),
        ends: Vec::new(),
        starts: Vec::new(),
    };
    let mut seg: Option<Seg> = None;
    let mut stopped_since: Option<i64> = None;
    let mut torn: Vec<i64> = Vec::new();
    let mut unplaceable = false;

    for r in records {
        let t = r.ts;
        let starts_segment = matches!(
            r.event,
            CoverageEvent::ProcessStart { .. } | CoverageEvent::DayOpen { .. }
        );
        // A silence: the process was not heard from; whatever comes next resumes it.
        if !starts_segment && let Some(s) = seg.take_if(|s| t - s.last > grace) {
            let resumed = s.resume(t);
            let last = s.last;
            acc.close(s, last, true);
            acc.starts.push(t);
            seg = Some(resumed);
        }
        // A new segment closes the old one at its own last record, not at this one.
        if !starts_segment && let Some(s) = seg.as_mut() {
            s.last = s.last.max(t);
        }
        match &r.event {
            CoverageEvent::ProcessStart { .. } | CoverageEvent::DayOpen { .. } => {
                if let Some(s) = seg.take() {
                    let last = s.last;
                    acc.close(s, last, true);
                }
                if let Some(since) = stopped_since.take() {
                    acc.known.push((since, t));
                    acc.gap(since, t, GapReason::ProcessStopped);
                }
                acc.starts.push(t);
                seg = Some(match &r.event {
                    CoverageEvent::DayOpen { link, subscription } => {
                        Seg::new(t, *link, subscription.clone())
                    }
                    _ => Seg::new(t, LinkStatus::Connecting, None),
                });
            }
            CoverageEvent::ProcessStop => {
                if let Some(s) = seg.take() {
                    acc.close(s, t, false);
                }
                stopped_since = Some(t);
            }
            CoverageEvent::DayClose => {
                if let Some(s) = seg.take() {
                    acc.close(s, t, false);
                }
            }
            CoverageEvent::Heartbeat => {}
            CoverageEvent::Link { state, .. } => {
                if let Some(s) = seg.take() {
                    let mut s = s;
                    if *state != s.link {
                        acc.link_until(&s, t);
                        s.link = *state;
                        s.link_since = t;
                    }
                    seg = Some(s);
                }
            }
            CoverageEvent::Subscription { symbols } => {
                if let Some(mut s) = seg.take() {
                    acc.sub_until(&s, t);
                    s.sub = Some(symbols.clone());
                    s.sub_since = t;
                    seg = Some(s);
                }
            }
            CoverageEvent::RecordingStopped { .. } => {
                if let Some(mut s) = seg.take() {
                    if s.recording {
                        s.recording = false;
                        s.rec_since = t;
                    }
                    seg = Some(s);
                }
            }
            CoverageEvent::Drop {
                stage,
                from: a,
                to: b,
                ..
            } => match stage {
                DropStage::Stream => acc.gap(*a, (*b).max(*a + 1), GapReason::StreamDrop),
                DropStage::Recorder if rec => {
                    acc.gap(*a, (*b).max(*a + 1), GapReason::RecorderDrop)
                }
                DropStage::Recorder => {}
            },
            CoverageEvent::TornTail { symbol: s, .. } => {
                if rec && s == symbol {
                    torn.push(t);
                }
            }
        }
    }
    if let Some(since) = stopped_since {
        acc.known.push((since, i64::MAX));
        acc.gap(since, i64::MAX, GapReason::ProcessStopped);
    }
    if let Some(s) = seg.take() {
        // Heard from recently: still running, vouched for up to now. Otherwise it died.
        if now - s.last <= grace {
            let end = now.max(s.start);
            acc.close(s, end, false);
        } else {
            let last = s.last;
            acc.close(s, last, true);
        }
    }
    // A torn tail lost the symbol's last frame of the run that ended uncleanly before the repair:
    // from that end (less the slack) until the next segment began.
    for t in torn {
        let end = acc
            .ends
            .iter()
            .filter(|(e, unclean)| *unclean && *e <= t)
            .map(|(e, _)| *e)
            .max();
        match end {
            Some(e) => {
                let next = acc.starts.iter().copied().filter(|s| *s >= e).min();
                acc.gap(
                    e - UNFLUSHED_SLACK.as_nanos() as i64,
                    next.unwrap_or(t).max(e + 1),
                    GapReason::TornTail,
                );
            }
            None => unplaceable = true,
        }
    }

    let mut gaps: Vec<CoverageGap> = acc
        .gaps
        .iter()
        .filter_map(|g| {
            let (a, b) = (g.from.max(from), g.to.min(to));
            (b > a).then_some(CoverageGap {
                from: a,
                to: b,
                reason: g.reason,
            })
        })
        .collect();
    gaps.sort_by_key(|g| (g.from, g.to));
    let known = merge(acc.known);
    let mut unverified = subtract(from, to, &known);
    unverified.extend(
        acc.unknown
            .iter()
            .map(|(a, b)| ((*a).max(from), (*b).min(to))),
    );
    let unverified = merge(unverified);
    let collection = if !gaps.is_empty() {
        Collection::Partial
    } else if !unverified.is_empty() || log.unreadable > 0 || unplaceable {
        Collection::Unverified
    } else {
        Collection::Complete
    };
    CoverageReport {
        collection,
        gaps,
        unverified,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: i64 = 1_000_000_000;

    fn rec(ts: i64, e: CoverageEvent) -> CoverageRecord {
        CoverageRecord::new(ts * S, e)
    }

    fn start(ts: i64) -> CoverageRecord {
        rec(
            ts,
            CoverageEvent::ProcessStart {
                version: "t".into(),
            },
        )
    }

    fn link(ts: i64, state: LinkStatus) -> CoverageRecord {
        rec(
            ts,
            CoverageEvent::Link {
                state,
                detail: None,
            },
        )
    }

    fn all(ts: i64) -> CoverageRecord {
        rec(
            ts,
            CoverageEvent::Subscription {
                symbols: Subscription::All,
            },
        )
    }

    fn hb(from: i64, to: i64) -> Vec<CoverageRecord> {
        (from..=to)
            .step_by(60)
            .map(|t| rec(t, CoverageEvent::Heartbeat))
            .collect()
    }

    fn log(mut v: Vec<CoverageRecord>) -> CoverageLog {
        v.sort_by_key(|r| r.ts);
        CoverageLog {
            records: v,
            unreadable: 0,
        }
    }

    fn ask(l: &CoverageLog, from: i64, to: i64, basis: CoverageBasis, now: i64) -> CoverageReport {
        classify(l, "QUIET", from * S, to * S, basis, now * S)
    }

    /// Up and subscribed with heartbeats: complete, though the symbol printed nothing at all.
    fn healthy() -> Vec<CoverageRecord> {
        let mut v = vec![start(0), all(0), link(1, LinkStatus::Up)];
        v.extend(hb(60, 3_600));
        v.push(rec(3_630, CoverageEvent::ProcessStop));
        v
    }

    #[test]
    fn a_quiet_symbol_on_a_healthy_link_is_complete_not_an_outage() {
        let l = log(healthy());
        for basis in [CoverageBasis::Stream, CoverageBasis::Recording] {
            let r = ask(&l, 100, 3_000, basis, 10_000);
            assert_eq!(r.collection, Collection::Complete, "{basis:?}: {r:?}");
            assert!(r.gaps.is_empty() && r.unverified.is_empty());
        }
        // Before the link came up: partial (connecting is a known gap).
        assert_eq!(
            ask(&l, 0, 100, CoverageBasis::Stream, 10_000).collection,
            Collection::Partial
        );
        // After the clean stop: known not collected.
        let after = ask(&l, 3_700, 3_800, CoverageBasis::Stream, 10_000);
        assert_eq!(after.collection, Collection::Partial);
        assert_eq!(after.gaps[0].reason, GapReason::ProcessStopped);
    }

    #[test]
    fn a_disconnect_is_a_gap_exactly_where_it_happened() {
        let mut v = healthy();
        v.push(link(1_000, LinkStatus::Down));
        v.push(link(1_005, LinkStatus::Connecting));
        v.push(link(1_010, LinkStatus::Up));
        let l = log(v);
        let r = ask(&l, 900, 1_100, CoverageBasis::Stream, 10_000);
        assert_eq!(r.collection, Collection::Partial);
        assert_eq!(
            r.gaps,
            [
                CoverageGap {
                    from: 1_000 * S,
                    to: 1_005 * S,
                    reason: GapReason::LinkDown
                },
                CoverageGap {
                    from: 1_005 * S,
                    to: 1_010 * S,
                    reason: GapReason::LinkDown
                }
            ]
        );
        assert_eq!(
            ask(&l, 1_010, 1_100, CoverageBasis::Stream, 10_000).collection,
            Collection::Complete
        );
    }

    #[test]
    fn a_killed_process_is_unverified_and_a_torn_tail_marks_the_recording_partial() {
        let mut v = vec![start(0), all(0), link(1, LinkStatus::Up)];
        v.extend(hb(60, 600)); // killed some time after 600
        v.push(start(2_000));
        v.push(all(2_000));
        v.push(link(2_001, LinkStatus::Up));
        v.extend(hb(2_060, 3_000));
        let l = log(v.clone());
        let r = ask(&l, 500, 1_000, CoverageBasis::Stream, 3_010);
        assert_eq!(r.collection, Collection::Unverified, "{r:?}");
        assert_eq!(r.unverified, [(600 * S, 1_000 * S)]);
        // The recording vouches 12 s less before an unclean end.
        let r = ask(&l, 500, 600, CoverageBasis::Recording, 3_010);
        assert_eq!(r.unverified, [(588 * S, 600 * S)]);
        // The repair after the restart found a torn tail of QUIET: a known loss.
        v.push(rec(
            2_002,
            CoverageEvent::TornTail {
                symbol: "QUIET".into(),
                kept_bytes: 10,
                cut_bytes: 5,
            },
        ));
        let l = log(v);
        let r = ask(&l, 500, 1_000, CoverageBasis::Recording, 3_010);
        assert_eq!(r.collection, Collection::Partial);
        assert_eq!(r.gaps[0].reason, GapReason::TornTail);
        // ...but not for the stream, nor for another symbol.
        assert_eq!(
            ask(&l, 500, 1_000, CoverageBasis::Stream, 3_010).collection,
            Collection::Unverified
        );
    }

    #[test]
    fn drops_count_for_their_stage_and_subscriptions_scope_symbols() {
        let mut v = healthy();
        v.push(rec(
            1_001,
            CoverageEvent::Drop {
                stage: DropStage::Recorder,
                from: 1_000 * S,
                to: 1_001 * S,
                count: 7,
            },
        ));
        let l = log(v.clone());
        assert_eq!(
            ask(&l, 900, 1_100, CoverageBasis::Stream, 10_000).collection,
            Collection::Complete,
            "the market saw what the recorder dropped"
        );
        let r = ask(&l, 900, 1_100, CoverageBasis::Recording, 10_000);
        assert_eq!(r.gaps[0].reason, GapReason::RecorderDrop);

        let mut v = vec![
            start(0),
            rec(
                0,
                CoverageEvent::Subscription {
                    symbols: Subscription::Symbols(vec!["OTHER".into()]),
                },
            ),
            link(1, LinkStatus::Up),
        ];
        v.extend(hb(60, 600));
        let r = ask(&log(v), 100, 200, CoverageBasis::Stream, 610);
        assert_eq!(r.gaps[0].reason, GapReason::NotSubscribed);
    }

    #[test]
    fn a_running_process_vouches_up_to_now_and_no_further() {
        let mut v = vec![start(0), all(0), link(1, LinkStatus::Up)];
        v.extend(hb(60, 600));
        let l = log(v);
        assert_eq!(
            ask(&l, 100, 650, CoverageBasis::Stream, 650).collection,
            Collection::Complete
        );
        // Later than now: not vouched for yet.
        assert_eq!(
            ask(&l, 100, 700, CoverageBasis::Stream, 650).collection,
            Collection::Unverified
        );
        // Never heard from again: it died after its last heartbeat.
        let r = ask(&l, 100, 650, CoverageBasis::Stream, 5_000);
        assert_eq!(r.unverified, [(600 * S, 650 * S)]);
    }

    #[test]
    fn day_open_carries_the_state_into_a_new_day_file() {
        let mut v = vec![rec(
            0,
            CoverageEvent::DayOpen {
                link: LinkStatus::Up,
                subscription: Some(Subscription::All),
            },
        )];
        v.extend(hb(60, 600));
        v.push(rec(620, CoverageEvent::DayClose));
        let l = log(v);
        assert_eq!(
            ask(&l, 0, 620, CoverageBasis::Recording, 99_999).collection,
            Collection::Complete
        );
    }

    #[test]
    fn the_file_format_round_trips_and_unknown_lines_forbid_complete() {
        let recs = healthy();
        let mut bytes = Vec::new();
        for r in &recs {
            serde_json::to_writer(&mut bytes, r).unwrap();
            bytes.push(b'\n');
        }
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(text.starts_with(r#"{"v":1,"ts":0,"kind":"process_start","version":"t"}"#));
        assert!(text.contains(r#""kind":"subscription","symbols":"all""#));
        let l = CoverageLog::parse(&bytes);
        assert_eq!(l.records, recs);
        assert_eq!(l.unreadable, 0);
        // A future schema's line and a torn last line.
        bytes.extend_from_slice(b"{\"v\":2,\"ts\":5,\"kind\":\"heartbeat\"}\n{\"v\":1,\"ts\"");
        let l = CoverageLog::parse(&bytes);
        assert_eq!(l.unreadable, 2);
        assert_eq!(
            ask(&l, 100, 3_000, CoverageBasis::Stream, 10_000).collection,
            Collection::Unverified
        );
    }
}
