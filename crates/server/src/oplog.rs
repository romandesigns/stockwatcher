//! Operational logs on the data volume (owner, 2026-10-26: "are we generating any logs on
//! everything stockwatcher related for backtracking if needed?"). Three daily JSONL files, one
//! writer each, all of the same kind ([`DailyLog`]):
//!
//! - **Notices** (`ARCHIVE_ROOT/notices/<ET date>.jsonl`, [`NOTICES_DIR`]): every notice the
//!   server decided, sent or held and why (`crate::alerts`, "Ledger"). 90 days.
//! - **Server log** (`DATA_DIR/logs/server-<ET date>.jsonl`, [`SERVER_LOG_DIR`]): the process's own
//!   tracing output, the same JSON lines stdout carries. 30 days.
//! - **Cue transitions** (`ARCHIVE_ROOT/transitions/<ET date>.jsonl`, [`TRANSITIONS_DIR`]): the
//!   micro-pullback episodes' transitions the context engine drains with the Detection Panel off
//!   (`crate::context::ContextBoard::drain_outputs`), which were counted and dropped before. With
//!   the Detection Panel on they also travel in its own journal (`crate::detection`). 90 days.
//!
//! # Why not the container log
//!
//! Docker keeps `json-file` logs inside the container (10 MB × 5 here) and a deploy recreates the
//! container, so every deploy erased them: on 2026-10-26 three deploys wiped the morning's
//! "runners push: held back" lines that had just explained a missed SRTM pullback notice. These
//! files are on the `stockwatcher_data` volume and survive deploys. stdout is unchanged.
//!
//! # Why a writer of our own (not `tracing-appender`)
//!
//! `tracing-appender` would cover the server log's rotation, but it has no per-day size cap and
//! names files by UTC date, and the notices and transitions need the same write-behind daily file
//! anyway. One small writer serves all three, with no new dependency.
//!
//! # Never in the way
//!
//! [`DailyLog::send`] is a `try_send` into a bounded queue drained by one OS thread; a record
//! whose queue is full is dropped and counted, never waited for. Rendering (JSON serialization)
//! happens on that thread, so a caller under a lock only moves an owned value. The thread
//! flushes after each batch. A day file stops at its cap with one `{"kind":"capped"}` line, so a
//! runaway cannot fill the volume; files older than the retention window are deleted when the
//! writer starts and whenever the day changes. File errors are reported on stderr (not through
//! tracing: the server log's own writer must not log into itself) and never stop the market.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};

use chrono::{NaiveDate, TimeZone};

/// Notices, under `ARCHIVE_ROOT`.
pub const NOTICES_DIR: &str = "notices";
/// Cue transitions, under `ARCHIVE_ROOT`.
pub const TRANSITIONS_DIR: &str = "transitions";
/// The server log, under `DATA_DIR`.
pub const SERVER_LOG_DIR: &str = "logs";

/// Days of notices and transitions kept.
pub const NOTICES_DAYS: u32 = 90;
pub const TRANSITIONS_DAYS: u32 = 90;
/// Days of server log kept.
pub const SERVER_LOG_DAYS: u32 = 30;

/// One day's file stops here (with one `capped` line). Measured 2026-10-26: the container log ran
/// ~5.6 KB a minute in pre-market, so a whole 04:00–20:00 day is ~5–20 MB; the caps leave room
/// for a busy day several times over without letting a loop fill the volume.
pub const SERVER_LOG_CAP: u64 = 128 * 1024 * 1024;
pub const NOTICES_CAP: u64 = 32 * 1024 * 1024;
pub const TRANSITIONS_CAP: u64 = 64 * 1024 * 1024;

const QUEUE: usize = 8192;

type Render = Box<dyn FnOnce() -> String + Send>;

struct Record {
    date: NaiveDate,
    render: Render,
}

/// The ET calendar date of an instant (epoch ms): the file a record goes to. Calendar, not
/// trading, date: an overnight line at 21:00 belongs to that evening's file.
pub fn et_date_ms(ms: i64) -> NaiveDate {
    chrono_tz::America::New_York
        .timestamp_millis_opt(ms)
        .single()
        .map_or_else(NaiveDate::default, |t| t.date_naive())
}

/// Now, epoch ms (wall clock).
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// A daily JSONL log written behind the caller (module docs).
#[derive(Clone)]
pub struct DailyLog {
    tx: SyncSender<Record>,
    dropped: Arc<AtomicU64>,
}

impl std::fmt::Debug for DailyLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DailyLog")
            .field("dropped", &self.dropped())
            .finish()
    }
}

/// Where and how a [`DailyLog`] writes.
#[derive(Clone, Debug)]
pub struct DailyLogConfig {
    pub dir: PathBuf,
    /// File name before the date (`"server-"`, or `""` for `<date>.jsonl`).
    pub prefix: &'static str,
    /// Days kept (0 = keep everything).
    pub retention_days: u32,
    pub cap_bytes: u64,
}

impl DailyLogConfig {
    pub fn file(&self, date: NaiveDate) -> PathBuf {
        self.dir.join(format!("{}{date}.jsonl", self.prefix))
    }

    /// The date of one of this log's files, else `None` (another prefix, a stray file).
    pub fn date_of(&self, path: &Path) -> Option<NaiveDate> {
        let name = path.file_name()?.to_str()?;
        let stem = name.strip_prefix(self.prefix)?.strip_suffix(".jsonl")?;
        NaiveDate::parse_from_str(stem, "%Y-%m-%d").ok()
    }

    /// Deletes this log's files older than the window ending at `today`; returns their dates.
    pub fn prune(&self, today: NaiveDate) -> Vec<NaiveDate> {
        let mut gone = Vec::new();
        if self.retention_days == 0 {
            return gone;
        }
        let cutoff = today - chrono::Duration::days(i64::from(self.retention_days));
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return gone;
        };
        for e in rd.flatten() {
            let p = e.path();
            if let Some(d) = self.date_of(&p)
                && d < cutoff
                && std::fs::remove_file(&p).is_ok()
            {
                gone.push(d);
            }
        }
        gone.sort();
        gone
    }
}

impl DailyLog {
    /// Starts the writer thread. `name` labels the thread and its stderr messages.
    pub fn start(name: &'static str, cfg: DailyLogConfig) -> DailyLog {
        let (tx, rx) = sync_channel::<Record>(QUEUE);
        let dropped = Arc::new(AtomicU64::new(0));
        let d = dropped.clone();
        std::thread::Builder::new()
            .name(format!("oplog-{name}"))
            .spawn(move || write_loop(name, cfg, rx, d))
            .expect("spawn oplog writer");
        DailyLog { tx, dropped }
    }

    /// Queues a record for `date`'s file; `render` makes its line (no newline) on the writer
    /// thread. Never blocks: a full queue drops the record and counts it.
    pub fn send(&self, date: NaiveDate, render: impl FnOnce() -> String + Send + 'static) {
        match self.tx.try_send(Record {
            date,
            render: Box::new(render),
        }) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Records dropped because the queue was full (or the writer gone).
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

struct Open {
    date: NaiveDate,
    out: BufWriter<File>,
    bytes: u64,
    capped: bool,
}

fn write_loop(name: &str, cfg: DailyLogConfig, rx: Receiver<Record>, dropped: Arc<AtomicU64>) {
    if let Err(e) = std::fs::create_dir_all(&cfg.dir) {
        eprintln!("oplog {name}: cannot create {}: {e}", cfg.dir.display());
    }
    let mut open: Option<Open> = None;
    let mut pruned_for: Option<NaiveDate> = None;
    while let Ok(first) = rx.recv() {
        let mut batch = vec![first];
        while let Ok(r) = rx.try_recv() {
            batch.push(r);
            if batch.len() >= 512 {
                break;
            }
        }
        for r in batch {
            if pruned_for.is_none_or(|p| r.date > p) {
                cfg.prune(r.date);
                pruned_for = Some(r.date);
            }
            if open.as_ref().is_none_or(|o| o.date != r.date) {
                if let Some(mut o) = open.take() {
                    let _ = o.out.flush();
                }
                let path = cfg.file(r.date);
                match std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                {
                    Ok(f) => {
                        let bytes = f.metadata().map(|m| m.len()).unwrap_or(0);
                        open = Some(Open {
                            date: r.date,
                            out: BufWriter::new(f),
                            bytes,
                            capped: bytes >= cfg.cap_bytes,
                        });
                    }
                    Err(e) => {
                        eprintln!("oplog {name}: cannot open {}: {e}", path.display());
                        dropped.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                }
            }
            let Some(o) = open.as_mut() else { continue };
            if o.capped {
                dropped.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            let mut line = (r.render)();
            line.push('\n');
            if o.bytes + line.len() as u64 > cfg.cap_bytes {
                o.capped = true;
                let note = format!(
                    "{{\"kind\":\"capped\",\"v\":1,\"log\":\"{name}\",\"cap_bytes\":{},\"at_ms\":{}}}\n",
                    cfg.cap_bytes,
                    now_ms()
                );
                let _ = o.out.write_all(note.as_bytes());
                dropped.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            match o.out.write_all(line.as_bytes()) {
                Ok(()) => o.bytes += line.len() as u64,
                Err(e) => {
                    eprintln!("oplog {name}: write failed: {e}");
                    dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        if let Some(o) = open.as_mut() {
            let _ = o.out.flush();
        }
    }
    if let Some(mut o) = open.take() {
        let _ = o.out.flush();
    }
}

/// Removes anything that looks like an Expo push token from text bound for a log (an Expo error
/// detail can quote the token it failed on). Tokens are device credentials, never logged.
pub fn redact_tokens(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = ["ExponentPushToken[", "ExpoPushToken["]
        .iter()
        .filter_map(|p| rest.find(p))
        .min()
    {
        out.push_str(&rest[..i]);
        match rest[i..].find(']') {
            Some(j) => {
                out.push_str("[push token]");
                rest = &rest[i + j + 1..];
            }
            None => {
                out.push_str("[push token]");
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// A `tracing_subscriber` writer that copies each formatted event into the server log
/// ([`SERVER_LOG_DIR`]). The formatter writes one whole line per event into the writer it makes
/// (`make_writer` per event), so the buffer is sent as one record when it is dropped.
#[derive(Clone)]
pub struct ServerLogWriter(pub DailyLog);

pub struct ServerLogLine {
    log: DailyLog,
    buf: Vec<u8>,
}

impl Write for ServerLogLine {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for ServerLogLine {
    fn drop(&mut self) {
        let mut line = std::mem::take(&mut self.buf);
        while line.last().is_some_and(|c| *c == b'\n' || *c == b'\r') {
            line.pop();
        }
        if line.is_empty() {
            return;
        }
        let date = et_date_ms(now_ms());
        self.log
            .send(date, move || String::from_utf8_lossy(&line).into_owned());
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for ServerLogWriter {
    type Writer = ServerLogLine;
    fn make_writer(&'a self) -> ServerLogLine {
        ServerLogLine {
            log: self.0.clone(),
            buf: Vec::with_capacity(256),
        }
    }
}

// ---- Durable copy (the Pi) -------------------------------------------------------------------
//
// The same share the preserved originals go to (`market_archive::retention`, SETUP.md §9): the
// Pi's NFS export, bound into the container as `/durable`. `LIVE_ORIGINAL_DURABLE_DIR` is
// `/durable/originals`, a directory that exists only on the Pi, so its presence is the proof the
// share is mounted; the logs go beside it in `/durable/logs/<source>/`, created on first use only
// then (never on the VPS's own disk behind an unmounted mount point). `LOG_DURABLE_DIR` names the
// destination explicitly instead; it must then already exist.
//
// Every completed day's file (date before today, ET) is copied when the copy is missing or its
// size differs (a Runners day file gets its `day` line at 20:00 or the next morning), through a
// `.incoming-` name renamed into place. Local files are never deleted by this: each source keeps
// its own retention. Runs every 30 minutes on its own thread.

/// One directory of day files to copy.
#[derive(Clone, Debug)]
pub struct DurableSource {
    /// Its folder under `/durable/logs/`.
    pub name: &'static str,
    pub dir: PathBuf,
}

/// Where the logs' durable copy goes, or `None` when there is none (module docs).
pub fn durable_logs_root(
    log_dir: Option<&str>,
    originals_dir: Option<&str>,
) -> Option<Result<PathBuf, String>> {
    if let Some(d) = log_dir.filter(|d| !d.trim().is_empty()) {
        let p = PathBuf::from(d.trim());
        return Some(if p.is_dir() {
            Ok(p)
        } else {
            Err(format!("LOG_DURABLE_DIR {} is not available", p.display()))
        });
    }
    let originals = PathBuf::from(originals_dir.filter(|d| !d.trim().is_empty())?.trim());
    if !originals.is_dir() {
        return Some(Err(format!(
            "{} is not available (the Pi's share is not mounted)",
            originals.display()
        )));
    }
    let root = originals.parent()?.join("logs");
    Some(
        std::fs::create_dir_all(&root)
            .map(|()| root)
            .map_err(|e| e.to_string()),
    )
}

/// The date in a day file's name (`<date>.jsonl`, `server-<date>.jsonl`), else `None`.
pub fn day_file_date(path: &Path) -> Option<NaiveDate> {
    let stem = path.file_name()?.to_str()?.strip_suffix(".jsonl")?;
    let tail = stem.get(stem.len().checked_sub(10)?..)?;
    NaiveDate::parse_from_str(tail, "%Y-%m-%d").ok()
}

/// What one pass copied.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CopyReport {
    pub copied: Vec<String>,
    pub failed: Vec<String>,
}

/// Copies every completed day's file of `sources` under `dest_root` (module docs). Pure apart from
/// the files; `today` = the ET date now.
pub fn copy_finished(sources: &[DurableSource], dest_root: &Path, today: NaiveDate) -> CopyReport {
    let mut report = CopyReport::default();
    for s in sources {
        let Ok(rd) = std::fs::read_dir(&s.dir) else {
            continue;
        };
        let dest_dir = dest_root.join(s.name);
        let mut files: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                !p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with('.'))
                    && day_file_date(p).is_some_and(|d| d < today)
            })
            .collect();
        files.sort();
        for src in files {
            let Some(name) = src.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                continue;
            };
            let dest = dest_dir.join(&name);
            let src_len = std::fs::metadata(&src).map(|m| m.len()).ok();
            let dest_len = std::fs::metadata(&dest).map(|m| m.len()).ok();
            if src_len.is_none() || src_len == dest_len {
                continue;
            }
            let res = (|| -> std::io::Result<()> {
                std::fs::create_dir_all(&dest_dir)?;
                let tmp = dest_dir.join(format!(".incoming-{name}"));
                std::fs::copy(&src, &tmp)?;
                // Flushed to the Pi before it takes the final name (a writable handle: Windows refuses
                // FlushFileBuffers on a read-only one).
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(&tmp)?
                    .sync_all()?;
                std::fs::rename(&tmp, &dest)
            })();
            match res {
                Ok(()) => report.copied.push(format!("{}/{name}", s.name)),
                Err(e) => report.failed.push(format!("{}/{name}: {e}", s.name)),
            }
        }
    }
    report
}

/// Every 30 minutes, copies the finished day files to the Pi (module docs). `root` is resolved
/// on every pass, so a share that comes back is used without a restart.
pub fn spawn_durable_copy(
    sources: Vec<DurableSource>,
    log_dir: Option<String>,
    originals_dir: Option<String>,
) {
    let _ = std::thread::Builder::new()
        .name("oplog-durable".into())
        .spawn(move || {
            let mut warned: Option<NaiveDate> = None;
            loop {
                let today = et_date_ms(now_ms());
                match durable_logs_root(log_dir.as_deref(), originals_dir.as_deref()) {
                    None => return,
                    Some(Err(why)) => {
                        if warned != Some(today) {
                            tracing::warn!(reason = %why, "logs durable copy: destination unavailable; local logs kept");
                            warned = Some(today);
                        }
                    }
                    Some(Ok(root)) => {
                        let r = copy_finished(&sources, &root, today);
                        if !r.copied.is_empty() || !r.failed.is_empty() {
                            tracing::info!(copied = r.copied.len(), failed = ?r.failed, dest = %root.display(), "logs durable copy");
                        }
                    }
                }
                std::thread::sleep(std::time::Duration::from_secs(30 * 60));
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durable_copy_takes_completed_days_once_and_again_when_they_grow() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let notices = src.path().join("notices");
        let logs = src.path().join("logs");
        std::fs::create_dir_all(&notices).unwrap();
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(notices.join("2026-10-23.jsonl"), "a\n").unwrap();
        std::fs::write(notices.join("2026-10-26.jsonl"), "today\n").unwrap();
        std::fs::write(logs.join("server-2026-10-23.jsonl"), "s\n").unwrap();
        std::fs::write(notices.join("pins.json"), "{}").unwrap();
        let sources = vec![
            DurableSource {
                name: "notices",
                dir: notices.clone(),
            },
            DurableSource {
                name: "server",
                dir: logs,
            },
        ];
        let r = copy_finished(&sources, dest.path(), day(26));
        assert_eq!(
            r.copied,
            vec!["notices/2026-10-23.jsonl", "server/server-2026-10-23.jsonl"]
        );
        assert!(r.failed.is_empty());
        // Today's file and stray files stay put; a second pass copies nothing.
        assert!(!dest.path().join("notices/2026-10-26.jsonl").exists());
        assert!(!dest.path().join("notices/pins.json").exists());
        assert_eq!(
            copy_finished(&sources, dest.path(), day(26)),
            CopyReport::default()
        );
        // A finished day that grew (the 20:00 `day` line written the next morning) is copied again.
        std::fs::write(notices.join("2026-10-23.jsonl"), "a\nday\n").unwrap();
        assert_eq!(
            copy_finished(&sources, dest.path(), day(26)).copied,
            vec!["notices/2026-10-23.jsonl"]
        );
        assert_eq!(
            std::fs::read_to_string(dest.path().join("notices/2026-10-23.jsonl")).unwrap(),
            "a\nday\n"
        );
    }

    #[test]
    fn the_durable_root_needs_the_pis_share() {
        let share = tempfile::tempdir().unwrap();
        let originals = share.path().join("originals");
        // The share not mounted: originals/ missing, nothing created.
        let r = durable_logs_root(None, Some(originals.to_str().unwrap()));
        assert!(matches!(r, Some(Err(_))));
        assert!(!share.path().join("logs").exists());
        // Mounted: logs/ is made beside originals/.
        std::fs::create_dir_all(&originals).unwrap();
        let r = durable_logs_root(None, Some(originals.to_str().unwrap()));
        assert_eq!(r, Some(Ok(share.path().join("logs"))));
        // Not configured at all: no copy.
        assert_eq!(durable_logs_root(None, None), None);
        // An explicit destination must already exist.
        assert!(matches!(
            durable_logs_root(Some("/no/such/dir"), None),
            Some(Err(_))
        ));
    }

    #[test]
    fn day_files_are_recognised_with_or_without_a_prefix() {
        assert_eq!(
            day_file_date(Path::new("x/server-2026-10-23.jsonl")),
            Some(day(23))
        );
        assert_eq!(day_file_date(Path::new("2026-10-23.jsonl")), Some(day(23)));
        assert_eq!(day_file_date(Path::new("pins.json")), None);
        assert_eq!(
            day_file_date(Path::new(".incoming-2026-10-23.jsonl")),
            Some(day(23))
        );
    }

    fn wait_for(path: &Path, lines: usize) -> Vec<String> {
        for _ in 0..200 {
            let t = std::fs::read_to_string(path).unwrap_or_default();
            let v: Vec<String> = t.lines().map(str::to_owned).collect();
            if v.len() >= lines {
                return v;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("{} never reached {lines} lines", path.display());
    }

    fn day(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, d).unwrap()
    }

    #[test]
    fn writes_by_date_prunes_the_window_and_caps_a_day() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = DailyLogConfig {
            dir: dir.path().into(),
            prefix: "server-",
            retention_days: 30,
            cap_bytes: 64,
        };
        // An old file of this log and one of another prefix: only the first goes.
        std::fs::write(
            cfg.file(NaiveDate::from_ymd_opt(2026, 9, 1).unwrap()),
            "x\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("2026-09-01.jsonl"), "y\n").unwrap();
        let log = DailyLog::start("test", cfg.clone());
        log.send(day(26), || "{\"a\":1}".into());
        log.send(day(27), || "{\"b\":2}".into());
        // Past the 64-byte cap: one capped line, then nothing.
        log.send(day(26), || "{\"c\":3}".into());
        log.send(day(26), || format!("{{\"long\":\"{}\"}}", "z".repeat(80)));
        log.send(day(26), || "{\"d\":4}".into());
        let d26 = wait_for(&cfg.file(day(26)), 3);
        assert_eq!(d26[0], "{\"a\":1}");
        assert_eq!(d26[1], "{\"c\":3}");
        assert!(d26[2].starts_with("{\"kind\":\"capped\""), "{d26:?}");
        assert_eq!(wait_for(&cfg.file(day(27)), 1), vec!["{\"b\":2}"]);
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(
            std::fs::read_to_string(cfg.file(day(26)))
                .unwrap()
                .lines()
                .count(),
            3
        );
        assert!(
            !cfg.file(NaiveDate::from_ymd_opt(2026, 9, 1).unwrap())
                .exists()
        );
        assert!(dir.path().join("2026-09-01.jsonl").exists());
        assert!(log.dropped() >= 2);
    }

    #[test]
    fn the_server_log_gets_one_line_per_event_in_todays_file() {
        use tracing_subscriber::fmt::MakeWriter;
        let dir = tempfile::tempdir().unwrap();
        let cfg = DailyLogConfig {
            dir: dir.path().into(),
            prefix: "server-",
            retention_days: SERVER_LOG_DAYS,
            cap_bytes: SERVER_LOG_CAP,
        };
        let w = ServerLogWriter(DailyLog::start("server-test", cfg.clone()));
        for msg in ["one", "two"] {
            let mut line = w.make_writer();
            line.write_all(
                format!(
                    "{{\"message\":\"{msg}\"}}
"
                )
                .as_bytes(),
            )
            .unwrap();
        }
        let got = wait_for(&cfg.file(et_date_ms(now_ms())), 2);
        assert_eq!(got, vec!["{\"message\":\"one\"}", "{\"message\":\"two\"}"]);
    }

    #[test]
    fn push_tokens_never_reach_a_log() {
        assert_eq!(
            redact_tokens("DeviceNotRegistered: ExponentPushToken[abc123] gone, ExpoPushToken[x]"),
            "DeviceNotRegistered: [push token] gone, [push token]"
        );
        assert_eq!(redact_tokens("plain"), "plain");
        assert_eq!(
            redact_tokens("cut ExponentPushToken[abc"),
            "cut [push token]"
        );
    }

    #[test]
    fn et_dates_follow_new_york() {
        // 2026-10-27 03:30 UTC is still the 26th in New York (EDT, UTC−4).
        let ms = chrono::Utc
            .with_ymd_and_hms(2026, 10, 27, 3, 30, 0)
            .unwrap()
            .timestamp_millis();
        assert_eq!(et_date_ms(ms), day(26));
    }
}
