//! The server's **admission journal**: which inputs this process admitted, in what order, under
//! which rules (`live/admissions-<epoch>.jsonl`, one segment per boot per trading date).
//!
//! # What a recording cannot answer
//!
//! The scanner marks a print by its **delivery** time — `receive_ts`, when the collector got it
//! from the provider — and folds it into the bar whose settle it arrived before. A restore reads
//! the same field back out of the recording, so the marks themselves reproduce. What a recording
//! alone cannot answer is whether the restore is replaying *the same inputs in the same order*
//! the run admitted them:
//!
//! - the recorder has a queue of its own and drops under pressure, so the file may be missing
//!   events the market admitted, and nothing in the file says which;
//! - its order is the recorder's, not the order the market core processed them in;
//! - a redelivery is a *different occurrence* of the same economic print, and the file cannot say
//!   which occurrence the market counted and which it refused;
//! - corrections and cancels are not in a symbol's print file in the order they were applied, so
//!   a replay has no way to place them in the stream;
//! - an imported day carries no delivery times from this run at all, so its marks fall back to
//!   execution times: a different arithmetic, not this run's receipts.
//!
//! So "the restored day equals the live day" was an assumption. This journal is what makes it a
//! question with an answer.
//!
//! # Scope
//!
//! The journal orders the inputs to a symbol's **trade-derived facts** and nothing else
//! ([`ProofScope`]), because that is all a restore adopts. Pillar verdicts, Ross Panel membership,
//! 5/5 entry times and frozen setup levels are not restored from a recording at all — the live
//! run's own published state is kept and the symbol is decided again at the next barrier — so
//! evidence batches, halts and LULD bands are deliberately not journalled. Ordering records
//! nothing reads would be work pretending to be rigour.
//!
//! # What is written
//!
//! One line per admitted input, in admission order, carrying:
//!
//! - a **sequence** taken at the admission itself, so a record the writer dropped leaves a hole
//!   the reader sees rather than a shorter file that reads as complete;
//! - the **kind** of input ([`Kind`]), so corrections, cancels and clock advances are ordered
//!   with the prints rather than guessed at;
//! - a **per-occurrence** identity: a hash of the exact payload admitted and which occurrence of
//!   that payload this was. A redelivery shares its symbol, trade id and execution time with the
//!   original, so the economic identity cannot join deliveries one-to-one; the pair (payload,
//!   occurrence) can;
//! - the **verdict applied** — whether the print updated OHLC and volume — so a restore does not
//!   have to re-derive it and be believed;
//! - the producer's observation (`r`) kept **apart** from this server's admission time (`a`). A
//!   collector timestamp is never relabelled a server admission; the difference between the two
//!   is also the only honest measure of how far behind its collector this process runs.
//!
//! # What may be called exact, and what may not
//!
//! A contiguous sequence proves no *interior* record was lost. It cannot prove that nothing
//! followed the last line: trailing records lost in the queue, a write that never reached the
//! disk, or a process killed outright all leave a file that is contiguous to its end. So the
//! reader never reports a whole day as exact. It reports an **exact prefix** through the last
//! validated [`Checkpoint`] (or [`Seal`], at a clean stop), which names the sequence and time it
//! covers and a digest of the lines behind it; everything after that boundary is an explicitly
//! unverified [`Tail`]. A window crossing the tail, a restart gap or an interior hole is not
//! exact.
//!
//! The three ways data can be missing are kept apart, because they mean different things: a
//! **stream** gap means the numerator itself is short, a **recorder** drop means the recording is
//! short while the live numerator was fine, and a **journal** loss means the order cannot be
//! proved while both the stream and the recording may be complete.
//!
//! # Epochs
//!
//! Every boot writes its own segment, numbered from the segments already present, and names its
//! predecessor and the sequence that predecessor was proven through. Two processes therefore
//! never append ambiguous sequence 1 to one file, and a restart may begin recording immediately
//! without invalidating the prefix a symbol's restore is about to read. Segments are joined by
//! their declared predecessor chain, and a chain with a missing or disagreeing link is reported
//! rather than stitched.
//!
//! # Identity
//!
//! The header names the build, source, fidelity and the exact policy bytes every admission was
//! judged by. An identity that does not name a real build is never exact: `0.1.0` is shared by
//! every development binary, and "some build called 0.1.0 admitted these" is not evidence about
//! this one's rules.

use crate::error::ArchiveError;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

/// Segment file names: `admissions-<epoch>.jsonl`.
pub const SEGMENT_PREFIX: &str = "admissions-";
pub const SEGMENT_EXT: &str = "jsonl";

/// The only schema this module writes and the only one it reads.
pub const SCHEMA: u32 = 1;

/// A build identity that names no build. Never exact.
pub const UNKNOWN_BUILD: &str = "unknown";

/// Records between checkpoints. A checkpoint is the only thing that makes a prefix exact, so
/// this is also the most records a clean run can leave unproven.
pub const CHECKPOINT_EVERY: u64 = 2_048;

/// What a reader will take from one date's journal before it stops.
///
/// The limits are enforced **while reading**, not after: a cap checked once the whole day is
/// already in memory bounds nothing. A journal past them is reported unusable, so the restore
/// falls back to the recording's own order and labels itself — the figures still come back, and
/// the process does not spend unbounded memory proving they are in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub records: usize,
    pub bytes: u64,
}

impl Default for Limits {
    /// Roughly a very heavy day's admissions, several times over: large enough that a real day
    /// is never refused, small enough that a runaway file cannot exhaust the process.
    fn default() -> Self {
        Limits {
            records: 4_000_000,
            bytes: 2 << 30,
        }
    }
}

/// What a set of admissions was made under.
///
/// Compared whole: a restore refuses to claim receipt-order exactness across any difference,
/// because admissions made under other rules are not evidence about these.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalIdentity {
    /// The source the build was made from: a commit id, or [`UNKNOWN_BUILD`] when the build was
    /// not told one. A package version is not a build identity — every development binary of a
    /// crate carries the same one.
    pub build: String,
    /// A fingerprint of the running executable itself, taken once at startup.
    ///
    /// The commit alone is not enough. Two binaries can be built from one commit with different
    /// working trees — which is exactly how this audit was carried out — and they admit prints
    /// under different code while reporting the same `build`. Empty when the process could not
    /// fingerprint itself, which is never exact.
    pub code: String,
    /// `live` or `replay`: a replay's admissions are not a live run's.
    pub source: String,
    /// The run's own fidelity label (`stock_context::Fidelity`), so a reconstruction is never
    /// mistaken for a recorded live run.
    pub fidelity: String,
    /// `pillars-v2@<sha256>`, byte-for-byte over the policy file.
    pub pillars: String,
    /// `ross-panel-v1@<sha256>`.
    pub panel: String,
    /// `setups-v0@<sha256>` (EXPERIMENTAL).
    pub setup: String,
    /// The conditions policy that decided each print's verdict.
    pub conditions: String,
}

impl JournalIdentity {
    /// Whether this identity names a real build: a known source commit **and** a fingerprint of
    /// the executable that ran. Either one missing means the journal can never yield an exact
    /// prefix, however whole the file is — there would be nothing to tie the admissions to code
    /// anyone can read, or nothing to tell two builds of one commit apart.
    pub fn names_a_build(&self) -> bool {
        !self.build.is_empty() && self.build != UNKNOWN_BUILD && !self.code.is_empty()
    }
}

/// What kind of input an admission was.
///
/// The scope is deliberately narrow: these are the inputs to a symbol's **trade-derived facts**,
/// which is all a restore adopts ([`ProofScope`]). A print changes the volume, a correction or
/// cancel changes it back, and a clock advance decides which barrier folds a print in and takes
/// the cumulative checkpoint. Evidence batches, halts, LULD bands and status changes are not
/// here: they move pillar verdicts, membership and setups, and a restore does not take any of
/// those from a recording, so ordering them would be recording something nothing reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    #[serde(rename = "t")]
    Trade,
    #[serde(rename = "c")]
    Correction,
    #[serde(rename = "x")]
    Cancel,
    /// A clock advance: it decides barriers, so its place in the order is part of the arithmetic.
    #[serde(rename = "k")]
    Clock,
}

/// What a proven prefix of this journal is a proof **about**.
///
/// Named in the header so a reader can never take it for more. The journal orders the inputs to
/// a symbol's trade-derived facts, so an exact prefix proves the day's volume, dollar volume,
/// trade count, price and tick history, day high, setup bars and cumulative checkpoints were
/// replayed from the same inputs in the same order. It proves nothing about pillar verdicts,
/// Ross Panel membership, 5/5 entry times or frozen setup levels: those depend on evidence the
/// journal does not order, and a restore does not restore them. It keeps whatever the live run
/// genuinely published and decides again, prospectively, at the next barrier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProofScope {
    /// Trade-derived facts only.
    #[serde(rename = "facts-v1")]
    FactsV1,
}

/// One line of a segment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "k")]
pub enum Line {
    #[serde(rename = "head")]
    Header(Header),
    #[serde(rename = "a")]
    Admission(AdmissionRecord),
    #[serde(rename = "ck")]
    Checkpoint(Checkpoint),
    #[serde(rename = "seal")]
    Seal(Seal),
}

/// The segment a run wrote before this one, and how far it was proven.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Predecessor {
    pub epoch: u64,
    /// The sequence the predecessor's own last checkpoint or seal proved.
    pub through_seq: u64,
    pub through_at: i64,
    /// Whether the predecessor had stopped cleanly. `false` means its tail is unverified and the
    /// gap between it and this segment is unknown — a restart gap, not a continuation.
    pub sealed: bool,
}

/// The first line of a segment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    /// [`SCHEMA`].
    pub v: u32,
    pub day: NaiveDate,
    /// This boot's segment number, from 1, ascending within the day.
    pub epoch: u64,
    /// When this process started admitting, its own clock, UTC nanoseconds.
    pub started_at: i64,
    /// What a proven prefix of this segment is a proof about.
    pub scope: ProofScope,
    /// The segment this run continues from, when there was one.
    pub predecessor: Option<Predecessor>,
    pub identity: JournalIdentity,
}

/// One admitted input.
///
/// Short field names: there is one of these per input of the day and the file is read in full at
/// every restart.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionRecord {
    /// Monotone from 1 **within the segment**, assigned at the admission rather than by the
    /// writer, so a dropped record leaves a visible hole.
    pub s: u64,
    /// This server's own clock at the admission, UTC nanoseconds. Not the mark the engine folds
    /// the print by, and never a collector's timestamp.
    pub a: i64,
    pub w: Kind,
    /// The symbol, where the input has one (a clock advance and an evidence batch do not).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<String>,
    /// Hash of the exact payload admitted ([`payload_hash`]).
    ///
    /// Not unique: a redelivery is the same payload admitted again. Which *occurrence* a record
    /// is follows from its place in the sequence, so the writer does not have to keep a count per
    /// payload for the whole day — the reader derives it once, when a restore asks
    /// ([`Admissions::index`]).
    pub h: u64,
    /// The provider's trade id, for the ledger's economic identity and for diagnosis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub i: Option<String>,
    /// Execution time, UTC nanoseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub e: Option<i64>,
    /// The producer's observation of it (`receive_ts`), kept apart from `a`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r: Option<i64>,
    /// The verdict actually applied: whether it updated OHLC, and whether it counted volume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub o: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vo: Option<bool>,
}

/// A proof that everything up to `s` is on disk.
///
/// Written after a successful append and flush. `d` is a rolling digest of every admission line
/// in the segment so far, so a checkpoint cannot vouch for records that were changed or
/// reordered underneath it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub s: u64,
    pub at: i64,
    /// Admission lines behind this checkpoint.
    pub n: u64,
    pub d: u64,
}

/// A clean stop: the segment is finished and its tail is not in doubt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seal {
    pub s: u64,
    pub at: i64,
    pub n: u64,
    pub d: u64,
}

/// The rolling digest over admission lines: FNV-1a over each line's bytes, order-dependent.
///
/// Not a cryptographic commitment — it detects loss, truncation and reordering, which is what a
/// local restore needs; it is not a defence against someone editing the file deliberately.
pub fn roll(digest: u64, line: &[u8]) -> u64 {
    let mut h = digest;
    for b in line {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// The starting digest.
pub const DIGEST_SEED: u64 = 0xcbf2_9ce4_8422_2325;

/// Hash of an exact payload, for per-occurrence identity.
pub fn payload_hash(bytes: &[u8]) -> u64 {
    roll(DIGEST_SEED, bytes)
}

/// How far a segment chain is proven, and what lies beyond it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Proven {
    /// The last sequence a validated checkpoint or seal covers, within [`Proven::epoch`].
    pub seq: u64,
    pub at: i64,
    pub epoch: u64,
    /// The proof was a seal: the run stopped cleanly and nothing followed.
    pub sealed: bool,
}

/// What lies past the proven prefix.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum Tail {
    /// The proof reaches the end of the chain: there is no unverified tail.
    None,
    /// Records past the last proof, or a hole, or a torn line. Readable, but not proven — and
    /// nothing says whether more followed.
    Unverified {
        /// Admission lines past the proof that were still readable.
        records: u64,
        /// `journal_unproven_tail`, `journal_holes`, `journal_torn_line`,
        /// `journal_unsealed_restart`.
        reason: &'static str,
    },
}

/// Why a chain cannot be used at all.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum Unusable {
    /// No segment for the day.
    None,
    /// A named reason: `journal_schema`, `journal_other_date`, `journal_other_build`,
    /// `journal_other_code`, `journal_unknown_build`, `journal_other_policy`,
    /// `journal_other_source`, `journal_header_unreadable`, `journal_unreadable`,
    /// `journal_broken_chain`.
    Reason(&'static str),
}

/// One admission line with the segment it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Admitted {
    pub epoch: u64,
    pub rec: AdmissionRecord,
}

/// A day's admission journal: every segment of it, what is proven, and what is not.
#[derive(Clone, Debug)]
pub struct Admissions {
    /// Segment headers in epoch order.
    pub headers: Vec<Header>,
    /// Admission lines of the proven prefix **and** the unverified tail, in (epoch, sequence)
    /// order, each with the segment it came from. Sequences restart per segment, so the epoch is
    /// part of a record's position. [`Admissions::proven_records`] is the part that is proven.
    pub records: Vec<Admitted>,
    /// How far the chain is proven, when at all.
    pub proven: Option<Proven>,
    pub tail: Tail,
    /// Interior sequence holes: records the writer dropped.
    pub holes: u64,
    /// `None` when the chain is usable as far as it goes.
    pub unusable: Option<Unusable>,
}

impl Admissions {
    /// Nothing at all for this day.
    pub fn missing() -> Admissions {
        Admissions {
            headers: Vec::new(),
            records: Vec::new(),
            proven: None,
            tail: Tail::None,
            holes: 0,
            unusable: Some(Unusable::None),
        }
    }

    /// Admission lines inside the proven prefix, in order. A restore may replay these and claim
    /// receipt-order exactness over [`ProofScope`]; past them it may not.
    pub fn proven_records(&self) -> &[Admitted] {
        let Some(p) = self.proven else {
            return &[];
        };
        let n = self
            .records
            .iter()
            .position(|a| a.epoch > p.epoch || (a.epoch == p.epoch && a.rec.s > p.seq))
            .unwrap_or(self.records.len());
        &self.records[..n]
    }

    /// Why a restore from this journal may not claim exactness, for a coverage label.
    pub fn reason(&self) -> Option<&'static str> {
        match (&self.unusable, &self.tail) {
            (Some(Unusable::None), _) => Some("no_admission_journal"),
            (Some(Unusable::Reason(r)), _) => Some(r),
            (None, Tail::Unverified { reason, .. }) => Some(reason),
            (None, Tail::None) => None,
        }
    }

    /// Whether anything at all is proven.
    pub fn has_proof(&self) -> bool {
        self.unusable.is_none() && self.proven.is_some()
    }

    /// An index over the **proven** prefix for joining a recording's events to these admissions,
    /// one occurrence each. The unverified tail is deliberately left out: joining against records
    /// that may be incomplete would produce a replay that looks exact and is not.
    pub fn index(&self) -> Occurrences {
        let mut by_payload: HashMap<u64, Vec<usize>> = HashMap::new();
        for (idx, a) in self.proven_records().iter().enumerate() {
            by_payload.entry(a.rec.h).or_default().push(idx);
        }
        Occurrences {
            by_payload,
            taken: HashMap::new(),
            offered: HashMap::new(),
        }
    }
}

/// A one-to-one join between a recording's events and the journal's admissions.
///
/// A redelivery shares its economic identity with the original, so the join has to be by payload
/// **and occurrence**: the first time a payload is offered it joins occurrence 1, the second time
/// occurrence 2. An offered payload with no matching occurrence is a record the journal does not
/// have, and a journal occurrence nothing was offered for is one the recording does not have.
/// Either way the replay is not of the same stream and must not be called exact.
pub struct Occurrences {
    /// Record indices per payload, in admission order.
    by_payload: HashMap<u64, Vec<usize>>,
    /// How many occurrences of each payload have been joined so far.
    taken: HashMap<u64, usize>,
    /// How many were offered, joined or not.
    offered: HashMap<u64, usize>,
}

impl Occurrences {
    /// Joins the next unconsumed occurrence of `hash`, returning its index in
    /// [`Admissions::proven_records`]. `None` when the journal has no occurrence left: the
    /// recording holds a delivery this run did not admit, or did not admit inside the proof.
    pub fn next(&mut self, hash: u64) -> Option<usize> {
        *self.offered.entry(hash).or_insert(0) += 1;
        let n = self.taken.entry(hash).or_insert(0);
        let out = self.by_payload.get(&hash).and_then(|v| v.get(*n)).copied();
        if out.is_some() {
            *n += 1;
        }
        out
    }

    /// Deliveries offered that no admission was left for.
    pub fn missing(&self) -> u64 {
        self.offered
            .iter()
            .map(|(h, n)| {
                let have = self.by_payload.get(h).map_or(0, |v| v.len());
                n.saturating_sub(have) as u64
            })
            .sum()
    }

    /// Admissions nothing was offered for: the recording is short of what the run admitted.
    pub fn surplus(&self) -> u64 {
        self.by_payload
            .iter()
            .map(|(h, v)| {
                v.len()
                    .saturating_sub(self.offered.get(h).copied().unwrap_or(0)) as u64
            })
            .sum()
    }

    /// Whether every admission in the proof was joined to exactly one delivery and no delivery
    /// was left over: the only state in which a replay is of the same stream.
    pub fn one_to_one(&self) -> bool {
        self.missing() == 0 && self.surplus() == 0
    }
}

/// The epoch a new segment of `day` should take: one past the highest already there.
pub fn next_epoch(dir: &Path) -> u64 {
    segments(dir).last().map(|(e, _)| e + 1).unwrap_or(1)
}

/// Segment files in `dir`, ascending by epoch.
fn segments(dir: &Path) -> Vec<(u64, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(u64, PathBuf)> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter_map(|p| {
            let name = p.file_name()?.to_str()?;
            let rest = name.strip_prefix(SEGMENT_PREFIX)?;
            let num = rest.strip_suffix(SEGMENT_EXT)?.strip_suffix('.')?;
            Some((num.parse::<u64>().ok()?, p))
        })
        .collect();
    out.sort_by_key(|(e, _)| *e);
    out
}

/// An append-only writer for one boot's segment.
pub struct AdmissionWriter {
    path: PathBuf,
    file: File,
    epoch: u64,
    digest: u64,
    /// Admission lines appended by this writer.
    lines: u64,
    /// The highest sequence appended.
    last_seq: u64,
    /// Admission lines since the last checkpoint.
    since_checkpoint: u64,
    /// The last sequence a checkpoint or seal proved.
    proven: u64,
}

impl AdmissionWriter {
    /// Opens a **new** segment for `day` under `dir`, taking the next epoch and naming the
    /// predecessor it continues from.
    ///
    /// Never appends to another run's segment: two processes cannot be put in one order after the
    /// fact, and a restart has to be able to record immediately without disturbing the prefix a
    /// symbol's restore is reading.
    pub fn open(
        dir: &Path,
        day: NaiveDate,
        started_at: i64,
        identity: JournalIdentity,
    ) -> Result<AdmissionWriter, ArchiveError> {
        std::fs::create_dir_all(dir).map_err(|e| ArchiveError::io(dir, e))?;
        let epoch = next_epoch(dir);
        let predecessor = (epoch > 1).then(|| {
            // How far the segment before this one was proven, read from that file itself under
            // its own identity. Filtering by *this* process's identity here would make the link
            // empty whenever the predecessor was another build — or, for a build that cannot name
            // itself, always — and the link is a statement about files, not about rules.
            let prior_epoch = epoch - 1;
            let path = dir.join(format!(
                "{SEGMENT_PREFIX}{prior_epoch}.{SEGMENT_EXT}"
            ));
            let mut budget = Limits::default();
            match read_segment(&path, day, None, &mut budget) {
                Ok(seg) => match seg.proven {
                    Some((seq, at, sealed)) => Predecessor {
                        epoch: prior_epoch,
                        through_seq: seq,
                        through_at: at,
                        sealed,
                    },
                    // Nothing proven in what came before; the link says so rather than claiming
                    // a continuation.
                    None => Predecessor {
                        epoch: prior_epoch,
                        through_seq: 0,
                        through_at: 0,
                        sealed: false,
                    },
                },
                Err(_) => Predecessor {
                    epoch: prior_epoch,
                    through_seq: 0,
                    through_at: 0,
                    sealed: false,
                },
            }
        });
        let path = dir.join(format!("{SEGMENT_PREFIX}{epoch}.{SEGMENT_EXT}"));
        let mut file = OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&path)
            .map_err(|e| ArchiveError::io(&path, e))?;
        let h = Line::Header(Header {
            v: SCHEMA,
            day,
            epoch,
            started_at,
            scope: ProofScope::FactsV1,
            predecessor,
            identity,
        });
        let mut line = serde_json::to_vec(&h).expect("header serializes");
        line.push(b'\n');
        file.write_all(&line)
            .map_err(|e| ArchiveError::io(&path, e))?;
        Ok(AdmissionWriter {
            path,
            file,
            epoch,
            digest: DIGEST_SEED,
            lines: 0,
            last_seq: 0,
            since_checkpoint: 0,
            proven: 0,
        })
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Appends a batch, then a checkpoint when enough has accumulated.
    ///
    /// A partial write leaves a torn line, which the reader reports as an unverified tail rather
    /// than trusting it.
    pub fn append(&mut self, batch: &[AdmissionRecord]) -> Result<(), ArchiveError> {
        if batch.is_empty() {
            return Ok(());
        }
        let mut buf = Vec::with_capacity(batch.len() * 128);
        for r in batch {
            let start = buf.len();
            serde_json::to_writer(&mut buf, &Line::Admission(r.clone()))
                .expect("record serializes");
            buf.push(b'\n');
            self.digest = roll(self.digest, &buf[start..]);
            self.lines += 1;
            self.since_checkpoint += 1;
            self.last_seq = self.last_seq.max(r.s);
        }
        self.file
            .write_all(&buf)
            .map_err(|e| ArchiveError::io(&self.path, e))?;
        if self.since_checkpoint >= CHECKPOINT_EVERY {
            let at = batch.last().map(|r| r.a).unwrap_or_default();
            self.checkpoint(at)?;
        }
        Ok(())
    }

    /// Writes a checkpoint: everything appended so far is on disk, and this is its digest.
    ///
    /// Flushes first, because a checkpoint that reached the disk before the records it vouches
    /// for would be a proof of something that is not there.
    pub fn checkpoint(&mut self, at: i64) -> Result<(), ArchiveError> {
        self.file
            .flush()
            .map_err(|e| ArchiveError::io(&self.path, e))?;
        let c = Line::Checkpoint(Checkpoint {
            s: self.last_seq,
            at,
            n: self.lines,
            d: self.digest,
        });
        let mut line = serde_json::to_vec(&c).expect("checkpoint serializes");
        line.push(b'\n');
        self.file
            .write_all(&line)
            .map_err(|e| ArchiveError::io(&self.path, e))?;
        self.file
            .flush()
            .map_err(|e| ArchiveError::io(&self.path, e))?;
        self.since_checkpoint = 0;
        self.proven = self.last_seq;
        Ok(())
    }

    /// Closes the segment cleanly: nothing followed what is written.
    pub fn seal(&mut self, at: i64) -> Result<(), ArchiveError> {
        self.file
            .flush()
            .map_err(|e| ArchiveError::io(&self.path, e))?;
        let z = Line::Seal(Seal {
            s: self.last_seq,
            at,
            n: self.lines,
            d: self.digest,
        });
        let mut line = serde_json::to_vec(&z).expect("seal serializes");
        line.push(b'\n');
        self.file
            .write_all(&line)
            .map_err(|e| ArchiveError::io(&self.path, e))?;
        self.file
            .flush()
            .map_err(|e| ArchiveError::io(&self.path, e))?;
        self.proven = self.last_seq;
        Ok(())
    }

    pub fn flush(&mut self) -> Result<(), ArchiveError> {
        self.file
            .flush()
            .map_err(|e| ArchiveError::io(&self.path, e))
    }

    /// Admission lines this writer has appended, and how many of them a checkpoint covers.
    pub fn written(&self) -> (u64, u64) {
        (self.lines, self.proven)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Reads every segment of `day` under `dir` and says how far the chain is proven, under
/// [`Limits::default`].
pub fn read(dir: &Path, day: NaiveDate, want: &JournalIdentity) -> Admissions {
    read_within(dir, day, want, Limits::default())
}

/// Reads every segment of `day` under `dir` and says how far the chain is proven.
///
/// `want` is this process's own identity: a segment written under other rules, or by a build that
/// cannot name itself, is reported rather than used. `limits` bound what is taken into memory as
/// it is read.
pub fn read_within(
    dir: &Path,
    day: NaiveDate,
    want: &JournalIdentity,
    limits: Limits,
) -> Admissions {
    let segs = segments(dir);
    if segs.is_empty() {
        return Admissions::missing();
    }
    let mut headers: Vec<Header> = Vec::new();
    let mut records: Vec<Admitted> = Vec::new();
    let mut proven: Option<Proven> = None;
    let mut holes = 0u64;
    let mut unproven_tail = 0u64;
    // Why the tail is not proven. Collected rather than taken first-come, because several can be
    // true at once and the most specific one is the useful answer.
    let mut reasons: Vec<&'static str> = Vec::new();
    // Set once something between segments is unknown: no later proof may reach across it.
    let mut gap = false;

    let mut budget = limits;
    for (epoch, path) in segs {
        let seg = match read_segment(&path, day, Some(want), &mut budget) {
            Ok(s) => s,
            Err(why) => {
                return Admissions {
                    headers,
                    records,
                    proven,
                    tail: Tail::None,
                    holes,
                    unusable: Some(Unusable::Reason(why)),
                };
            }
        };
        // The chain: this segment must name the one before it, and that one must have been
        // proven at least as far as this one claims it was.
        //
        // Not *exactly* as far: a predecessor can still be sealing its own tail when its
        // successor opens — a restart is quick and a writer flushes on a timer — so its proof
        // legitimately advances after the claim was written. What cannot happen is the proof
        // going backwards: that would mean the file lost records the successor had seen, and the
        // two cannot be stitched.
        if let (Some(prev), Some(p)) = (seg.header.predecessor.as_ref(), proven.as_ref())
            && (prev.epoch != p.epoch || prev.through_seq > p.seq)
        {
            return Admissions {
                headers,
                records,
                proven,
                tail: Tail::Unverified {
                    records: unproven_tail,
                    reason: "journal_broken_chain",
                },
                holes,
                unusable: Some(Unusable::Reason("journal_broken_chain")),
            };
        }
        // A predecessor that never sealed leaves an unknown gap between the two segments: the
        // earlier run may have admitted inputs that are not in either file, so the chain's proof
        // cannot reach across it however whole this segment is.
        //
        // Judged by the predecessor's own proof as it reads **now**, not by what its successor
        // recorded at the time: the successor's claim can predate the predecessor's seal, and
        // declaring a gap that the files themselves disprove would be its own kind of wrong.
        let predecessor_sealed = match (seg.header.predecessor.as_ref(), proven.as_ref()) {
            (Some(prev), Some(p)) => prev.epoch == p.epoch && p.sealed,
            (Some(_), None) => false,
            (None, _) => true,
        };
        if !predecessor_sealed && seg.header.predecessor.is_some() {
            reasons.push("journal_unsealed_restart");
            gap = true;
        }
        let mut seg = seg;
        headers.push(std::mem::replace(
            &mut seg.header,
            Header {
                v: SCHEMA,
                day,
                epoch,
                started_at: 0,
                scope: ProofScope::FactsV1,
                predecessor: None,
                identity: want.clone(),
            },
        ));
        holes += seg.holes;
        // A segment's own prefix is proven when a checkpoint or seal describes it and no record
        // was dropped *inside* that prefix. A torn line or a hole **after** the proof does not
        // unprove it — protecting the prefix from exactly that is what a checkpoint is for — but
        // it does mean nothing later in the chain can be proven either.
        match seg.proven {
            Some(p) if !gap && seg.holes_before(p.0) == 0 => {
                proven = Some(Proven {
                    seq: p.0,
                    at: p.1,
                    epoch,
                    sealed: p.2,
                });
                let past = seg.records.iter().filter(|r| r.s > p.0).count() as u64;
                unproven_tail += past;
                if past > 0 {
                    reasons.push("journal_unproven_tail");
                }
            }
            _ => {
                unproven_tail += seg.records.len() as u64;
                reasons.push("journal_unproven_tail");
            }
        }
        if seg.holes > 0 {
            reasons.push("journal_holes");
        }
        if let Some(why) = seg.torn {
            reasons.push(why);
        }
        // Anything unproven in this segment is an unknown quantity between it and the next one,
        // so no later segment's checkpoint can extend the chain's proof across it.
        if seg.torn.is_some() || seg.holes > 0 || !seg.proven.is_some_and(|p| p.2) {
            gap = true;
        }
        records.extend(seg.records.into_iter().map(|rec| Admitted { epoch, rec }));
    }

    // The most specific reason first: a dropped record and a torn line say where the loss was,
    // a restart gap says the two runs cannot be joined, and an unproven tail is the residual.
    let tail_reason = ["journal_holes", "journal_torn_line", "journal_unsealed_restart", "journal_unproven_tail"]
        .into_iter()
        .find(|r| reasons.contains(r));
    // A proof is only a proof if the build that wrote it can be named.
    if !headers.iter().all(|h| h.identity.names_a_build()) {
        return Admissions {
            headers,
            records,
            proven: None,
            tail: Tail::None,
            holes,
            unusable: Some(Unusable::Reason("journal_unknown_build")),
        };
    }

    let tail = match tail_reason {
        Some(reason) => Tail::Unverified {
            records: unproven_tail,
            reason,
        },
        None if unproven_tail > 0 => Tail::Unverified {
            records: unproven_tail,
            reason: "journal_unproven_tail",
        },
        None => Tail::None,
    };
    Admissions {
        headers,
        records,
        proven,
        tail,
        holes,
        unusable: None,
    }
}

struct Segment {
    header: Header,
    records: Vec<AdmissionRecord>,
    /// (sequence, time, sealed) of the last validated checkpoint or seal.
    proven: Option<(u64, i64, bool)>,
    holes: u64,
    /// `journal_torn_line` when the file ended mid-line or held an unparsable one.
    torn: Option<&'static str>,
}

impl Segment {
    /// Records the writer dropped at or before `seq`: the only holes that can unprove a prefix.
    fn holes_before(&self, seq: u64) -> u64 {
        let mut holes = 0;
        let mut expect = 1u64;
        for r in self.records.iter().take_while(|r| r.s <= seq) {
            holes += r.s - expect;
            expect = r.s + 1;
        }
        holes
    }
}

/// Reads one segment. `want` is the identity to require; `None` accepts the header's own, which
/// is what link-building needs — how far a predecessor was proven is a fact about its file, not a
/// judgement about whether this process would use it.
fn read_segment(
    path: &Path,
    day: NaiveDate,
    want: Option<&JournalIdentity>,
    budget: &mut Limits,
) -> Result<Segment, &'static str> {
    // The file's own size is checked before a byte of it is parsed, so an oversized segment never
    // reaches memory at all.
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if size > budget.bytes {
        return Err("journal_too_large");
    }
    budget.bytes -= size;
    let file = File::open(path).map_err(|_| "journal_unreadable")?;
    let mut lines = BufReader::new(file).lines();
    let header: Header = match lines.next() {
        Some(Ok(l)) => match serde_json::from_str::<Line>(&l) {
            Ok(Line::Header(h)) => h,
            _ => return Err("journal_header_unreadable"),
        },
        _ => return Err("journal_header_unreadable"),
    };
    if header.v != SCHEMA {
        return Err("journal_schema");
    }
    if header.day != day {
        return Err("journal_other_date");
    }
    if let Some(want) = want
        && header.identity != *want
    {
        // Named apart: a rebuilt binary is routine, a changed policy file is a deliberate rule
        // change, and a replay is not a live run. Conflating them would hide which happened.
        return Err(if header.identity.source != want.source {
            "journal_other_source"
        } else if !header.identity.names_a_build() || !want.names_a_build() {
            "journal_unknown_build"
        } else if header.identity.build != want.build {
            "journal_other_build"
        } else if header.identity.code != want.code {
            // The same commit, a different binary: a modified working tree, which is a different
            // implementation however identical the recorded source is.
            "journal_other_code"
        } else {
            "journal_other_policy"
        });
    }

    let mut records: Vec<AdmissionRecord> = Vec::new();
    let mut digest = DIGEST_SEED;
    let mut proven = None;
    let mut torn = None;
    for l in lines {
        let Ok(l) = l else {
            torn = Some("journal_torn_line");
            break;
        };
        if l.is_empty() {
            continue;
        }
        match serde_json::from_str::<Line>(&l) {
            Ok(Line::Admission(r)) => {
                if budget.records == 0 {
                    return Err("journal_too_large");
                }
                budget.records -= 1;
                let mut bytes =
                    serde_json::to_vec(&Line::Admission(r.clone())).expect("record serializes");
                bytes.push(b'\n');
                digest = roll(digest, &bytes);
                records.push(r);
            }
            Ok(Line::Checkpoint(c)) => {
                // A checkpoint only proves a prefix it actually describes: the same number of
                // lines, hashing to the same digest. Anything else means the file changed
                // underneath it.
                if c.n == records.len() as u64 && c.d == digest {
                    proven = Some((c.s, c.at, false));
                } else {
                    torn = Some("journal_torn_line");
                    break;
                }
            }
            Ok(Line::Seal(z)) => {
                if z.n == records.len() as u64 && z.d == digest {
                    proven = Some((z.s, z.at, true));
                } else {
                    torn = Some("journal_torn_line");
                }
                break;
            }
            Ok(Line::Header(_)) => {
                // Two headers in one file: two writers shared it, and no order can be recovered.
                return Err("journal_broken_chain");
            }
            Err(_) => {
                torn = Some("journal_torn_line");
                break;
            }
        }
    }

    let mut holes = 0u64;
    let mut expect = 1u64;
    for r in &records {
        if r.s < expect {
            return Err("journal_broken_chain");
        }
        holes += r.s - expect;
        expect = r.s + 1;
    }
    Ok(Segment {
        header,
        records,
        proven,
        holes,
        torn,
    })
}
