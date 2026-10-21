//! Detection transport and append-only research records. The semantic detector lives only in
//! stock-context. Everything here runs after releasing the market core: wall-clock telemetry,
//! JSON, bounded history, fan-out and file writes cannot change a detector cutoff.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};

pub const HISTORY_CAPACITY: usize = 2048;
pub const JOURNAL_QUEUE: usize = 2048;
const SCHEMA: u16 = 1;
const DEFINITION: &str = "detection-transition-log-v1";
static NEXT_BOOT: AtomicU64 = AtomicU64::new(1);

/// Server timestamps are telemetry. `transition` is the unchanged deterministic core record.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Emission {
    pub sequence: u64,
    /// Persisted per record because a replay day switch can change fixture/reconstructed mode.
    pub source: String,
    pub emitted_at: i64,
    pub input_watermark: u64,
    pub restored: bool,
    pub transition: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JournalHeader {
    pub schema: u16,
    pub definition: String,
    pub source: String,
    pub boot: String,
    pub code_commit: String,
    pub code_identity: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Record {
    Header(JournalHeader),
    Transition {
        run_epoch: String,
        emission: Emission,
    },
}

struct State {
    enabled: bool,
    source: String,
    epoch: String,
    cursor: u64,
    input_watermark: u64,
    snapshot: Value,
    telemetry: Value,
    transitions: VecDeque<Emission>,
    next_transition: u64,
    history_dropped: u64,
}

/// A market has one cache and one bounded journal writer. It never subscribes to charts.
pub struct Runtime {
    state: Mutex<State>,
    boot: String,
    next_epoch: AtomicU64,
    queue: Option<mpsc::SyncSender<Record>>,
    queue_dropped: AtomicU64,
    writes: Arc<AtomicU64>,
    write_errors: Arc<AtomicU64>,
    header: JournalHeader,
    log: Option<PathBuf>,
}

impl Runtime {
    pub fn new(source: &str, directory: Option<&Path>) -> Self {
        let boot = format!(
            "{}-{}-{}",
            std::process::id(),
            crate::observe::wall_now(),
            NEXT_BOOT.fetch_add(1, Ordering::Relaxed)
        );
        let header = JournalHeader {
            schema: SCHEMA,
            definition: DEFINITION.into(),
            source: source.into(),
            boot: boot.clone(),
            code_commit: crate::COMMIT.into(),
            code_identity: crate::admit::code_id_for_records().into(),
        };
        let writes = Arc::new(AtomicU64::new(0));
        let write_errors = Arc::new(AtomicU64::new(0));
        // Reserve rather than append: a new process cannot attribute its records to an old
        // executable's header. Opening/hashing happens once, outside the core lock.
        let reserved = directory.and_then(|d| {
            std::fs::create_dir_all(d).ok()?;
            let path = d.join(format!("detection-{source}-{boot}.jsonl"));
            let file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .ok()?;
            Some((path, file))
        });
        let (queue, log) = if let Some((path, mut file)) = reserved {
            let (tx, rx) = mpsc::sync_channel::<Record>(JOURNAL_QUEUE);
            let h = header.clone();
            let written = writes.clone();
            let errors = write_errors.clone();
            std::thread::Builder::new()
                .name("detection-journal".into())
                .spawn(move || {
                    let mut healthy = write_record(&mut file, &Record::Header(h)).is_ok();
                    if !healthy {
                        errors.fetch_add(1, Ordering::Relaxed);
                    }
                    for record in rx {
                        if healthy && write_record(&mut file, &record).is_ok() {
                            written.fetch_add(1, Ordering::Relaxed);
                        } else {
                            healthy = false;
                            errors.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    // A flush is not a proof that every producer input reached this queue. There
                    // is deliberately no whole-epoch completion seal in this format.
                    let _ = file.flush();
                })
                .expect("spawn detection journal");
            (Some(tx), Some(path))
        } else {
            (None, None)
        };
        Self {
            state: Mutex::new(State {
                enabled: false,
                source: source.into(),
                epoch: String::new(),
                cursor: 0,
                input_watermark: 0,
                snapshot: Value::Null,
                telemetry: json!({}),
                transitions: VecDeque::new(),
                next_transition: 1,
                history_dropped: 0,
            }),
            boot,
            next_epoch: AtomicU64::new(0),
            queue,
            queue_dropped: AtomicU64::new(0),
            writes,
            write_errors,
            header,
            log,
        }
    }

    /// Called at an actual market rebuild. Old asynchronous work is rejected by `accept`.
    pub fn reset(&self, enabled: bool, source: &str) -> String {
        let epoch = format!(
            "{}:{}",
            self.boot,
            self.next_epoch.fetch_add(1, Ordering::Relaxed) + 1
        );
        let mut s = self.state.lock().expect("detection state");
        *s = State {
            enabled,
            source: source.into(),
            epoch: epoch.clone(),
            cursor: 0,
            input_watermark: 0,
            snapshot: Value::Null,
            telemetry: json!({}),
            transitions: VecDeque::new(),
            next_transition: 1,
            history_dropped: 0,
        };
        epoch
    }

    pub fn accept(
        &self,
        epoch: &str,
        snapshot: Value,
        telemetry: Value,
        transitions: Vec<Value>,
        input_watermark: u64,
        restored: bool,
    ) {
        let emitted_at = crate::observe::wall_now();
        let mut s = self.state.lock().expect("detection state");
        if !s.enabled || s.epoch != epoch || input_watermark < s.input_watermark {
            return;
        }
        s.cursor += 1;
        s.input_watermark = input_watermark;
        s.snapshot = snapshot;
        s.telemetry = telemetry;
        for transition in transitions {
            let emission = Emission {
                sequence: s.next_transition,
                source: s.source.clone(),
                emitted_at,
                input_watermark,
                restored,
                transition,
            };
            s.next_transition += 1;
            if s.transitions.len() == HISTORY_CAPACITY {
                s.transitions.pop_front();
                s.history_dropped += 1;
            }
            s.transitions.push_back(emission.clone());
            if let Some(q) = &self.queue
                && q.try_send(Record::Transition {
                    run_epoch: epoch.into(),
                    emission,
                })
                .is_err()
            {
                self.queue_dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// A complete bounded replacement, including all retained significant transitions. Cursor
    /// gaps in these replacement frames do not mean a delta is missing. History coverage does.
    pub fn frame(&self, kind: &str) -> (String, u64, String) {
        let s = self.state.lock().expect("detection state");
        let frame = json!({
            "type":kind,"enabled":s.enabled,"source":s.source,"runEpoch":s.epoch,
            "cursor":s.cursor,"inputWatermark":s.input_watermark,"snapshot":s.snapshot,
            "transitions":s.transitions,"experimental":true,"alertsEnabled":false,
            "replacement":true,
            "history":{"capacity":HISTORY_CAPACITY,"dropped":s.history_dropped,
                "first_sequence":s.transitions.front().map(|e|e.sequence),
                "last_sequence":s.transitions.back().map(|e|e.sequence)},
            "telemetry":{"engine":s.telemetry,"journal_queue_dropped":self.queue_dropped.load(Ordering::Relaxed),
                "journal_written":self.writes.load(Ordering::Relaxed),"journal_write_errors":self.write_errors.load(Ordering::Relaxed),
                "durable_target":self.log.is_some(),"tail":"unverified"},
            "provenance":self.header,
        }).to_string();
        (s.epoch.clone(), s.cursor, frame)
    }

    pub fn log_path(&self) -> Option<&Path> {
        self.log.as_deref()
    }

    pub fn is_epoch(&self, epoch: &str) -> bool {
        self.state.lock().expect("detection state").epoch == epoch
    }
}

fn write_record(file: &mut impl Write, record: &Record) -> std::io::Result<()> {
    serde_json::to_writer(&mut *file, record)?;
    file.write_all(b"\n")?;
    file.flush()
}

/// Bounded offline reader. Prefix integrity and an unknown tail are separate claims: an EOF
/// cannot prove that the last producer transition was recorded. Never restore these as alerts.
#[derive(Debug, Serialize)]
pub struct JournalRead {
    pub header: Option<JournalHeader>,
    pub transitions: Vec<(String, Emission)>,
    pub prefix_integrity: bool,
    pub tail: &'static str,
    pub stopped_reason: Option<String>,
}

pub fn read_journal(path: &Path) -> std::io::Result<JournalRead> {
    const MAX_BYTES: usize = 64 * 1024 * 1024;
    const MAX_LINE: usize = 1024 * 1024;
    const MAX_RECORDS: usize = 100_000;
    let mut read = JournalRead {
        header: None,
        transitions: Vec::new(),
        prefix_integrity: true,
        tail: "unverified",
        stopped_reason: None,
    };
    let mut reader = BufReader::new(std::fs::File::open(path)?);
    let mut bytes = 0;
    let mut expected: HashMap<String, u64> = HashMap::new();
    loop {
        let mut line = Vec::new();
        let n = (&mut reader)
            .take((MAX_LINE + 1) as u64)
            .read_until(b'\n', &mut line)?;
        if n == 0 {
            break;
        }
        bytes += n;
        let reason = if bytes > MAX_BYTES {
            Some("byte_bound")
        } else if n > MAX_LINE {
            Some("line_bound")
        } else if read.transitions.len() >= MAX_RECORDS {
            Some("record_bound")
        } else if line.last() != Some(&b'\n') {
            Some("torn_tail")
        } else {
            None
        };
        if let Some(reason) = reason {
            read.stopped_reason = Some(reason.into());
            break;
        }
        let record: Record = match serde_json::from_slice(&line) {
            Ok(r) => r,
            Err(_) => {
                read.stopped_reason = Some("invalid_record".into());
                break;
            }
        };
        match record {
            Record::Header(h) if read.header.is_none() && read.transitions.is_empty() => {
                if h.schema != SCHEMA || h.definition != DEFINITION {
                    read.stopped_reason = Some("unsupported_header".into());
                    break;
                }
                read.header = Some(h);
            }
            Record::Transition {
                run_epoch,
                emission,
            } if read.header.is_some() => {
                if expected.len() >= 64 && !expected.contains_key(&run_epoch) {
                    read.stopped_reason = Some("epoch_bound".into());
                    break;
                }
                let next = expected.entry(run_epoch.clone()).or_insert(1);
                if emission.sequence != *next {
                    read.stopped_reason = Some("sequence_gap_or_disorder".into());
                    break;
                }
                *next += 1;
                read.transitions.push((run_epoch, emission));
            }
            _ => {
                read.stopped_reason = Some("header_order".into());
                break;
            }
        }
    }
    read.prefix_integrity = read.header.is_some() && read.stopped_reason.is_none();
    Ok(read)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_history_is_bounded_and_old_epochs_cannot_contaminate_reset() {
        let r = Runtime::new("fixture", None);
        let epoch = r.reset(true, "fixture");
        let records = (0..HISTORY_CAPACITY + 3)
            .map(|n| json!({"transition_id":format!("t{n}")}))
            .collect();
        r.accept(&epoch, json!({"active":[]}), json!({}), records, 50, false);
        let (_, cursor, frame) = r.frame("detection.snapshot");
        let frame: Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(cursor, 1);
        assert_eq!(
            frame["transitions"].as_array().unwrap().len(),
            HISTORY_CAPACITY
        );
        assert_eq!(frame["history"]["dropped"], 3);
        assert_eq!(frame["history"]["first_sequence"], 4);
        assert_eq!(frame["telemetry"]["durable_target"], false);
        let next = r.reset(true, "fixture");
        r.accept(
            &epoch,
            json!({"old":true}),
            json!({}),
            vec![json!({})],
            999,
            false,
        );
        let (current, cursor, frame) = r.frame("detection.snapshot");
        assert_eq!(current, next);
        assert_eq!(cursor, 0);
        assert_eq!(
            serde_json::from_str::<Value>(&frame).unwrap()["transitions"],
            json!([])
        );
    }

    #[test]
    fn off_core_journal_roundtrip_has_build_source_identity_and_unverified_tail() {
        let dir = tempfile::tempdir().unwrap();
        let r = Runtime::new("fixture", Some(dir.path()));
        let epoch = r.reset(true, "fixture");
        r.accept(
            &epoch,
            json!({}),
            json!({}),
            vec![json!({"transition_id":"stable"})],
            7,
            true,
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while r.writes.load(Ordering::Relaxed) != 1 && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert_eq!(r.writes.load(Ordering::Relaxed), 1);
        let read = read_journal(r.log_path().unwrap()).unwrap();
        assert!(read.prefix_integrity);
        assert_eq!(read.tail, "unverified");
        assert_eq!(read.header.as_ref().unwrap().source, "fixture");
        assert_eq!(read.header.as_ref().unwrap().code_commit, crate::COMMIT);
        assert!(!read.header.as_ref().unwrap().code_identity.is_empty());
        assert_eq!(read.transitions[0].0, epoch);
        assert_eq!(read.transitions[0].1.input_watermark, 7);
        assert!(read.transitions[0].1.restored);
        assert_eq!(read.transitions[0].1.transition["transition_id"], "stable");
    }

    #[test]
    fn reader_refuses_schema_holes_and_unbounded_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        let r = Runtime::new("fixture", None);
        let mut h = r.header.clone();
        h.schema = 99;
        let mut data = Vec::new();
        write_record(&mut data, &Record::Header(h)).unwrap();
        std::fs::write(&path, &data).unwrap();
        assert_eq!(
            read_journal(&path).unwrap().stopped_reason.as_deref(),
            Some("unsupported_header")
        );
        data.clear();
        write_record(&mut data, &Record::Header(r.header)).unwrap();
        write_record(
            &mut data,
            &Record::Transition {
                run_epoch: "e".into(),
                emission: Emission {
                    sequence: 2,
                    source: "fixture".into(),
                    emitted_at: 0,
                    input_watermark: 9,
                    restored: false,
                    transition: json!({}),
                },
            },
        )
        .unwrap();
        std::fs::write(&path, &data).unwrap();
        assert_eq!(
            read_journal(&path).unwrap().stopped_reason.as_deref(),
            Some("sequence_gap_or_disorder")
        );
        std::fs::write(&path, vec![b'x'; 1024 * 1024 + 10]).unwrap();
        assert_eq!(
            read_journal(&path).unwrap().stopped_reason.as_deref(),
            Some("line_bound")
        );
    }

    #[test]
    fn saturated_journal_cannot_block_state_and_exposes_every_lost_transition() {
        let mut r = Runtime::new("fixture", None);
        let (tx, _blocked_reader) = mpsc::sync_channel(1);
        r.queue = Some(tx);
        let epoch = r.reset(true, "fixture");
        r.accept(
            &epoch,
            json!({"active":[]}),
            json!({}),
            vec![json!({}), json!({}), json!({})],
            1,
            false,
        );
        let (_, _, frame) = r.frame("detection.snapshot");
        let frame: Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(frame["telemetry"]["journal_queue_dropped"], 2);
        assert_eq!(frame["transitions"].as_array().unwrap().len(), 3);
    }
}
