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

/// Owned pure records copied out under core, serialized/stamped only after that lock is gone.
pub(crate) struct CoreCapture {
    pub epoch: String,
    pub snapshot: Option<stock_context::DetectionSnapshot>,
    pub transitions: Vec<stock_context::DetectionTransition>,
    pub telemetry: stock_context::DetectionTelemetry,
    pub context: Vec<stock_context::Decision>,
    pub policy_hash: String,
    pub research_band: (i64, i64),
}

#[derive(Default)]
pub struct Capture {
    pub epoch: String,
    pub snapshot: Option<Value>,
    pub telemetry: Value,
    pub transitions: Vec<Value>,
    pub input_watermark: u64,
    pub restored: bool,
    pub rebuilding: bool,
    pub context: Value,
    pub profile: Value,
    pub policy_hash: Option<String>,
}

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
    pub detector_policy: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JournalStatus {
    pub run_epoch: String,
    pub source: String,
    pub input_watermark: u64,
    pub as_of: Option<i64>,
    pub data_status: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Record {
    Header(JournalHeader),
    Transition {
        journal_sequence: u64,
        run_epoch: String,
        emission: Emission,
    },
    Status {
        journal_sequence: u64,
        status: JournalStatus,
    },
}

#[derive(Clone)]
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
    rebuilding: bool,
    context: Value,
    profile: Value,
    health: Value,
    policy_hash: Option<String>,
    snapshot_captures: u64,
}

/// A market has one cache and one bounded journal writer. It never subscribes to charts.
pub struct Runtime {
    state: Mutex<State>,
    boot: String,
    next_epoch: AtomicU64,
    queue: Option<mpsc::SyncSender<Record>>,
    queue_dropped: AtomicU64,
    journal_sequence: AtomicU64,
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
            detector_policy: stock_context::DetectionPolicy::v0().tag(),
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
                rebuilding: false,
                context: Value::Null,
                profile: Value::Null,
                health: Value::Null,
                policy_hash: None,
                snapshot_captures: 0,
            }),
            boot,
            next_epoch: AtomicU64::new(0),
            queue,
            queue_dropped: AtomicU64::new(0),
            journal_sequence: AtomicU64::new(0),
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
        let profile = s.profile.clone();
        let policy_hash = s.policy_hash.clone();
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
            rebuilding: false,
            context: json!({}),
            profile,
            health: Value::Null,
            policy_hash,
            snapshot_captures: 0,
        };
        epoch
    }

    pub fn accept(&self, capture: Capture) {
        let Capture {
            epoch,
            snapshot,
            telemetry,
            transitions,
            input_watermark,
            restored,
            rebuilding,
            context,
            profile,
            policy_hash,
        } = capture;
        let emitted_at = crate::observe::wall_now();
        let mut s = self.state.lock().expect("detection state");
        if !s.enabled || s.epoch != epoch || input_watermark < s.input_watermark {
            return;
        }
        if snapshot.is_some() {
            s.snapshot_captures += 1;
        }
        if transitions.is_empty()
            && snapshot.as_ref().is_some_and(|v| *v == s.snapshot)
            && telemetry == s.telemetry
            && context == s.context
            && profile == s.profile
            && rebuilding == s.rebuilding
            && input_watermark == s.input_watermark
        {
            return;
        }
        s.cursor += 1;
        s.input_watermark = input_watermark;
        s.telemetry = telemetry;
        s.rebuilding = rebuilding;
        if let Some(snapshot) = snapshot {
            let health = snapshot.get("data_status").cloned().unwrap_or(Value::Null);
            if health != s.health {
                if let Some(queue) = &self.queue
                    && queue
                        .try_send(Record::Status {
                            journal_sequence: self.journal_sequence.fetch_add(1, Ordering::Relaxed)
                                + 1,
                            status: JournalStatus {
                                run_epoch: epoch.clone(),
                                source: s.source.clone(),
                                input_watermark,
                                as_of: snapshot.get("as_of").and_then(Value::as_i64),
                                data_status: health.clone(),
                            },
                        })
                        .is_err()
                {
                    self.queue_dropped.fetch_add(1, Ordering::Relaxed);
                }
                s.health = health;
            }
            s.snapshot = snapshot;
            s.context = context;
            s.profile = profile;
            s.policy_hash = policy_hash;
        }
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
                    journal_sequence: self.journal_sequence.fetch_add(1, Ordering::Relaxed) + 1,
                    run_epoch: epoch.clone(),
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
        // Serialization can be far slower than copying bounded records. Do not make the
        // admitted input path wait for a subscriber's JSON construction.
        let s = self.state.lock().expect("detection state").clone();
        let frame = frame_with_type(
            kind,
            json!({
                "enabled":s.enabled,"source":s.source,"runEpoch":s.epoch,
                "cursor":s.cursor,"inputWatermark":s.input_watermark,"snapshot":s.snapshot,
                "rebuilding":s.rebuilding,"profile":s.profile,"context":s.context,
                "policyHash":s.policy_hash,
                "transitions":s.transitions,"experimental":true,"alertsEnabled":false,
                "replacement":true,
                "history":{"capacity":HISTORY_CAPACITY,"dropped":s.history_dropped,
                    "first_sequence":s.transitions.front().map(|e|e.sequence),
                    "last_sequence":s.transitions.back().map(|e|e.sequence)},
                "telemetry":{"engine":s.telemetry,"journal_queue_dropped":self.queue_dropped.load(Ordering::Relaxed),
                    "snapshot_captures":s.snapshot_captures,"capture_batches":s.cursor,"transitions_captured":s.next_transition-1,
                    "journal_written":self.writes.load(Ordering::Relaxed),"journal_write_errors":self.write_errors.load(Ordering::Relaxed),
                    "durable_target":self.log.is_some(),"tail":"unverified"},
                "provenance":self.header,
            }),
        );
        (s.epoch.clone(), s.cursor, frame)
    }

    pub fn log_path(&self) -> Option<&Path> {
        self.log.as_deref()
    }

    pub fn is_epoch(&self, epoch: &str) -> bool {
        self.state.lock().expect("detection state").epoch == epoch
    }

    pub fn signature(&self) -> (String, u64) {
        let s = self.state.lock().expect("detection state");
        (s.epoch.clone(), s.cursor)
    }
}

/// Shared client tees inspect the type prefix before invoking expensive chart/paper parsers.
/// serde_json maps sort keys; explicitly write the protocol discriminator first, once.
pub(crate) fn frame_with_type(kind: &str, object: Value) -> String {
    let body = object.to_string();
    debug_assert!(body.starts_with('{'));
    format!(
        "{{\"type\":{},{}",
        serde_json::to_string(kind).expect("frame type"),
        &body[1..]
    )
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
    pub status: Vec<JournalStatus>,
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
        status: Vec::new(),
        prefix_integrity: true,
        tail: "unverified",
        stopped_reason: None,
    };
    let mut reader = BufReader::new(std::fs::File::open(path)?);
    let mut bytes = 0;
    let mut expected: HashMap<String, u64> = HashMap::new();
    let mut next_journal = 1;
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
        } else if read.transitions.len() + read.status.len() >= MAX_RECORDS {
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
        let journal_sequence = match &record {
            Record::Header(_) => None,
            Record::Transition {
                journal_sequence, ..
            }
            | Record::Status {
                journal_sequence, ..
            } => Some(*journal_sequence),
        };
        if let Some(sequence) = journal_sequence {
            if sequence != next_journal {
                read.stopped_reason = Some("journal_sequence_gap_or_disorder".into());
                break;
            }
            next_journal += 1;
        }
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
                ..
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
            Record::Status { status, .. } if read.header.is_some() => {
                if expected.len() >= 64 && !expected.contains_key(&status.run_epoch) {
                    read.stopped_reason = Some("epoch_bound".into());
                    break;
                }
                expected.entry(status.run_epoch.clone()).or_insert(1);
                read.status.push(status);
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

    fn accept(
        r: &Runtime,
        epoch: &str,
        snapshot: Value,
        telemetry: Value,
        transitions: Vec<Value>,
        input_watermark: u64,
        restored: bool,
    ) {
        r.accept(Capture {
            epoch: epoch.into(),
            snapshot: Some(snapshot),
            telemetry,
            transitions,
            input_watermark,
            restored,
            ..Capture::default()
        });
    }

    #[test]
    fn replacement_history_is_bounded_and_old_epochs_cannot_contaminate_reset() {
        let r = Runtime::new("fixture", None);
        let epoch = r.reset(true, "fixture");
        let records = (0..HISTORY_CAPACITY + 3)
            .map(|n| json!({"transition_id":format!("t{n}")}))
            .collect();
        accept(
            &r,
            &epoch,
            json!({"active":[]}),
            json!({}),
            records,
            50,
            false,
        );
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
        accept(
            &r,
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
        accept(
            &r,
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
        write_record(&mut data, &Record::Header(r.header.clone())).unwrap();
        write_record(
            &mut data,
            &Record::Transition {
                journal_sequence: 1,
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
        data.clear();
        write_record(&mut data, &Record::Header(r.header)).unwrap();
        for sequence in [1, 3] {
            write_record(
                &mut data,
                &Record::Status {
                    journal_sequence: sequence,
                    status: JournalStatus {
                        run_epoch: "e".into(),
                        source: "fixture".into(),
                        input_watermark: sequence,
                        as_of: None,
                        data_status: json!({}),
                    },
                },
            )
            .unwrap();
        }
        std::fs::write(&path, &data).unwrap();
        assert_eq!(
            read_journal(&path).unwrap().stopped_reason.as_deref(),
            Some("journal_sequence_gap_or_disorder")
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
        accept(
            &r,
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
