//! The admission journal's protocol: what a prefix may be called, what a tail may not, how
//! segments of successive boots join, and how deliveries of the same print are told apart.
//!
//! The claim this file guards is narrow and exact. A journal says: *these inputs, in this order,
//! were admitted by a build with this identity, and everything up to here is on disk*. From that
//! a restart can rebuild a symbol's trade-derived facts and know they are the facts the run had.
//! Everything else — which pillars passed, when a symbol reached 5/5, where a setup froze its
//! levels — is outside the proof's scope by construction, because a restore does not take any of
//! it from a recording.
//!
//! The failures this pins are the ones that would let a restore claim more than it has:
//!
//! - a contiguous file read as a whole day when trailing records were lost;
//! - a checkpoint believed after the records behind it changed;
//! - two boots' admissions stitched into one order;
//! - a redelivery joined to its original's admission;
//! - a development build's journal taken as evidence about another development build.

use chrono::NaiveDate;
use market_archive::admissions::{
    self, AdmissionRecord, AdmissionWriter, JournalIdentity, Kind, ProofScope, Tail, UNKNOWN_BUILD,
    Unusable,
};
use std::io::Write;
use std::path::Path;

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 16).expect("valid")
}

fn ident() -> JournalIdentity {
    JournalIdentity {
        build: "3f9a1c2d".into(),
        code: "a1b2c3d4e5f60718".into(),
        source: "live".into(),
        fidelity: "recorded_live".into(),
        pillars: "pillars-v2@aa".into(),
        panel: "ross-panel-v1@bb".into(),
        setup: "setups-v0@cc".into(),
        conditions: "conditions-alpaca-minute-bars-v1".into(),
    }
}

/// A print admitted at sequence `s` with payload hash `h`.
fn print_at(s: u64, h: u64) -> AdmissionRecord {
    AdmissionRecord {
        s,
        a: 1_000_000 + s as i64,
        w: Kind::Trade,
        y: Some("ABCD".into()),
        h,
        i: Some(format!("t{h}")),
        e: Some(900_000 + s as i64),
        r: Some(1_000_000 + s as i64 - 500),
        o: Some(true),
        vo: Some(true),
    }
}

fn open(dir: &Path) -> AdmissionWriter {
    AdmissionWriter::open(dir, day(), 1_000_000, ident()).expect("open")
}

/// A sealed segment is proven to its end: the run stopped cleanly, so nothing followed.
#[test]
fn a_sealed_segment_is_proven_to_its_end() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path().join("live");
    let mut w = open(&dir);
    assert_eq!(w.epoch(), 1);
    w.append(&[print_at(1, 11), print_at(2, 22)])
        .expect("append");
    w.append(&[print_at(3, 33)]).expect("append");
    w.seal(2_000_000).expect("seal");

    let got = admissions::read(&dir, day(), &ident());
    assert_eq!(got.unusable, None);
    let p = got.proven.expect("a proof");
    assert_eq!((p.seq, p.epoch, p.sealed), (3, 1, true));
    assert_eq!(got.tail, Tail::None);
    assert_eq!(got.proven_records().len(), 3);
    assert_eq!(got.reason(), None);
    assert!(got.has_proof());
    assert_eq!(
        got.headers[0].scope,
        ProofScope::FactsV1,
        "the header names what the proof is about"
    );
}

/// Records past the last checkpoint are an explicitly unverified tail, not part of the proof.
///
/// This is the case a contiguous sequence cannot detect by itself: the file ends tidily, every
/// sequence is present, and yet nothing says whether more was admitted and lost. The reader
/// therefore never reports a whole day — it reports a prefix and a tail.
#[test]
fn records_past_the_last_checkpoint_are_an_unverified_tail() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path().join("live");
    let mut w = open(&dir);
    w.append(&[print_at(1, 11), print_at(2, 22)])
        .expect("append");
    w.checkpoint(1_500_000).expect("checkpoint");
    w.append(&[print_at(3, 33), print_at(4, 44)])
        .expect("append");
    w.flush().expect("flush");

    let got = admissions::read(&dir, day(), &ident());
    assert_eq!(got.unusable, None);
    let p = got.proven.expect("a proof");
    assert_eq!(p.seq, 2, "proven only through the checkpoint");
    assert!(!p.sealed);
    assert_eq!(
        got.tail,
        Tail::Unverified {
            records: 2,
            reason: "journal_unproven_tail"
        }
    );
    assert_eq!(got.proven_records().len(), 2);
    assert_eq!(got.records.len(), 4, "the tail is readable, just not proven");
    assert_eq!(got.reason(), Some("journal_unproven_tail"));
}

/// Trailing records lost before they reached the disk leave a shorter proven prefix, not a
/// shorter whole day: the checkpoint still names what it covers.
#[test]
fn trailing_loss_shortens_the_proof_and_not_the_claim() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path().join("live");
    let mut w = open(&dir);
    w.append(&[print_at(1, 11), print_at(2, 22)])
        .expect("append");
    w.checkpoint(1_500_000).expect("checkpoint");
    drop(w);
    // Sequences 3 and 4 were admitted and never written: nothing in the file mentions them.
    let got = admissions::read(&dir, day(), &ident());
    assert_eq!(got.proven.expect("a proof").seq, 2);
    assert_eq!(got.holes, 0, "there is no interior hole to find");
    assert_eq!(
        got.tail,
        Tail::None,
        "and nothing claims the day ended at sequence 2"
    );
    // The honest reading: the proof is a prefix through 2, and a restore of a window past it is
    // not exact. That is what the proven boundary is for.
    assert_eq!(got.proven_records().len(), 2);
}

/// An interior hole — a record the queue dropped — is found, and makes the whole segment's tail
/// unverified rather than the file merely shorter.
#[test]
fn an_interior_hole_is_found_and_unproves_the_tail() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path().join("live");
    let mut w = open(&dir);
    w.append(&[print_at(1, 11), print_at(4, 44)])
        .expect("append");
    w.seal(2_000_000).expect("seal");
    let got = admissions::read(&dir, day(), &ident());
    assert_eq!(got.holes, 2, "sequences 2 and 3 never reached the file");
    assert_eq!(
        got.tail,
        Tail::Unverified {
            records: 2,
            reason: "journal_holes"
        }
    );
    assert!(
        got.proven.is_none(),
        "a hole is not a prefix: nothing is proven, {:?}",
        got.proven
    );
    assert!(got.proven_records().is_empty());
    assert_eq!(got.reason(), Some("journal_holes"));
}

/// A torn final line is one kind of tail loss, and is named as such.
#[test]
fn a_torn_line_is_a_named_tail_loss() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path().join("live");
    let mut w = open(&dir);
    w.append(&[print_at(1, 11)]).expect("append");
    w.checkpoint(1_500_000).expect("checkpoint");
    let path = w.path().to_path_buf();
    drop(w);
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("append");
    f.write_all(b"{\"k\":\"a\",\"s\":2,\"a\":1,\"w\":\"t\",\"h\":2")
        .expect("partial");
    drop(f);
    let got = admissions::read(&dir, day(), &ident());
    assert_eq!(
        got.tail,
        Tail::Unverified {
            records: 0,
            reason: "journal_torn_line"
        },
        "the tear is named, and nothing readable follows it"
    );
    // The checkpoint before the tear still stands: protecting a prefix from exactly this is what
    // a checkpoint is for. What it does not do is say anything about what came after.
    let p = got.proven.expect("the checkpointed prefix stands");
    assert_eq!((p.seq, p.sealed), (1, false));
    assert_eq!(got.proven_records().len(), 1);
    assert_eq!(got.reason(), Some("journal_torn_line"));
}

/// A checkpoint is only believed if the records behind it are the ones it describes.
///
/// A corrupted or rewritten row changes the digest, and the checkpoint vouching for it stops
/// being a proof — otherwise an edited file would read as exact.
#[test]
fn a_checkpoint_over_changed_records_is_not_a_proof() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path().join("live");
    let mut w = open(&dir);
    w.append(&[print_at(1, 11), print_at(2, 22)])
        .expect("append");
    w.seal(2_000_000).expect("seal");
    let path = w.path().to_path_buf();
    drop(w);

    // Change one admitted size after the fact: same sequence, different payload.
    let text = std::fs::read_to_string(&path).expect("read");
    let changed = text.replacen("\"e\":900001", "\"e\":900009", 1);
    assert_ne!(changed, text, "the fixture really changed a row");
    std::fs::write(&path, changed).expect("write");

    let got = admissions::read(&dir, day(), &ident());
    assert_eq!(
        got.tail,
        Tail::Unverified {
            records: 2,
            reason: "journal_torn_line"
        },
        "the seal no longer describes the file"
    );
    assert!(got.proven.is_none());
}

/// A restart writes its own segment and names the one it continues from, so the prefix a symbol's
/// restore is about to read stays readable and proven while the new run is already recording.
///
/// The first draft appended to one file per day and restarted the sequence at 1, which made every
/// same-day restart unusable: the new run's first records collided with the old run's before any
/// symbol had been restored.
#[test]
fn a_restart_writes_its_own_segment_and_keeps_the_old_prefix() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path().join("live");
    let mut first = open(&dir);
    first.append(&[print_at(1, 11), print_at(2, 22)]).expect("append");
    first.seal(1_900_000).expect("seal");
    drop(first);

    // The new boot starts admitting immediately, before any restore has read the old segment.
    let mut second = open(&dir);
    assert_eq!(second.epoch(), 2, "its own segment");
    second.append(&[print_at(1, 77)]).expect("append");
    second.seal(2_900_000).expect("seal");

    let got = admissions::read(&dir, day(), &ident());
    assert_eq!(got.unusable, None, "{:?}", got.unusable);
    assert_eq!(got.records.len(), 3, "both boots, in epoch order");
    assert_eq!(
        (got.records[0].epoch, got.records[2].epoch),
        (1, 2),
        "each record knows which boot admitted it"
    );
    let p = got.proven.expect("a proof");
    assert_eq!((p.epoch, p.seq), (2, 1));
    assert_eq!(got.proven_records().len(), 3, "both segments are proven");
    // The link is declared, not inferred.
    let pred = got.headers[1].predecessor.as_ref().expect("a predecessor");
    assert_eq!((pred.epoch, pred.through_seq, pred.sealed), (1, 2, true));
}

/// A predecessor that never sealed leaves an unknown gap: the earlier run may have admitted
/// inputs that reached neither file, so the chain is not exact across the restart.
#[test]
fn an_unsealed_predecessor_leaves_a_declared_gap() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path().join("live");
    let mut first = open(&dir);
    first.append(&[print_at(1, 11), print_at(2, 22)]).expect("append");
    first.checkpoint(1_500_000).expect("checkpoint");
    // Killed: no seal.
    drop(first);
    let mut second = open(&dir);
    let pred = {
        let got = admissions::read(&dir, day(), &ident());
        got.headers[1]
            .predecessor
            .clone()
            .expect("a predecessor")
    };
    assert_eq!(
        (pred.epoch, pred.through_seq, pred.sealed),
        (1, 2, false),
        "the link says the predecessor was not sealed"
    );
    second.append(&[print_at(1, 77)]).expect("append");
    second.seal(2_900_000).expect("seal");

    let got = admissions::read(&dir, day(), &ident());
    assert_eq!(
        got.tail,
        Tail::Unverified {
            records: 1,
            reason: "journal_unsealed_restart"
        },
        "the restart gap is declared rather than closed over"
    );
    assert_eq!(
        got.proven.expect("a proof").epoch,
        1,
        "the proof stops at the last segment before the gap"
    );
    assert_eq!(got.proven_records().len(), 2);
}

/// A journal from another build, policy set or source is not evidence about this process's
/// admissions, and the reasons are named apart.
#[test]
fn another_builds_journal_is_a_named_mismatch() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path().join("live");
    let mut w = open(&dir);
    w.append(&[print_at(1, 11)]).expect("append");
    w.seal(2_000_000).expect("seal");

    for (want, why) in [
        (
            JournalIdentity {
                build: "deadbeef".into(),
                ..ident()
            },
            "journal_other_build",
        ),
        (
            // The audit's own case: one commit, two working trees, two implementations.
            JournalIdentity {
                code: "ffffffffffffffff".into(),
                ..ident()
            },
            "journal_other_code",
        ),
        (
            JournalIdentity {
                pillars: "pillars-v3@ee".into(),
                ..ident()
            },
            "journal_other_policy",
        ),
        (
            JournalIdentity {
                source: "replay".into(),
                ..ident()
            },
            "journal_other_source",
        ),
        (
            JournalIdentity {
                fidelity: "reconstructed_history".into(),
                ..ident()
            },
            "journal_other_policy",
        ),
    ] {
        let got = admissions::read(&dir, day(), &want);
        assert_eq!(got.unusable, Some(Unusable::Reason(why)), "{why}");
        assert!(got.proven.is_none(), "{why}: nothing is taken from it");
        assert_eq!(got.reason(), Some(why));
    }
}

/// A build that cannot name itself never yields a proof, however whole the file is.
///
/// Every development binary carries the same package version, so "a build called 0.1.0 admitted
/// these" says nothing about which rules were in force. The journal is still readable; it just
/// is not evidence.
#[test]
fn an_unknown_build_can_never_be_exact() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path().join("live");
    let anon = JournalIdentity {
        build: UNKNOWN_BUILD.into(),
        ..ident()
    };
    assert!(!anon.names_a_build());
    // And a known commit with no executable fingerprint is no better: it cannot tell two builds
    // of that commit apart, which is exactly the case this guards.
    assert!(
        !JournalIdentity {
            code: String::new(),
            ..ident()
        }
        .names_a_build()
    );
    let mut w = AdmissionWriter::open(&dir, day(), 1_000_000, anon.clone()).expect("open");
    w.append(&[print_at(1, 11)]).expect("append");
    w.seal(2_000_000).expect("seal");

    let got = admissions::read(&dir, day(), &anon);
    assert_eq!(
        got.unusable,
        Some(Unusable::Reason("journal_unknown_build")),
        "a whole file from an unnamed build is still not a proof"
    );
    assert!(!got.has_proof());
    assert!(!anon.names_a_build());
    assert!(ident().names_a_build());
}

/// Deliveries of the same print are joined one at a time, in order, and a mismatch in either
/// direction is reported rather than smoothed over.
///
/// A redelivery has the same symbol, trade id and execution time as the original — the recording
/// cannot tell them apart. The journal can: each delivery is a separate occurrence of the same
/// payload, and the join consumes them one by one.
#[test]
fn deliveries_of_one_print_join_one_at_a_time() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path().join("live");
    let mut w = open(&dir);
    // The same payload admitted twice (a reconnect replay), with another print between them.
    w.append(&[print_at(1, 11), print_at(2, 22), print_at(3, 11)])
        .expect("append");
    w.seal(2_000_000).expect("seal");
    let got = admissions::read(&dir, day(), &ident());

    let mut j = got.index();
    assert_eq!(j.next(11), Some(0), "the first delivery joins the first");
    assert_eq!(j.next(22), Some(1));
    assert_eq!(
        j.next(11),
        Some(2),
        "the second joins the second admission, not the first again"
    );
    assert_eq!((j.missing(), j.surplus()), (0, 0));
    assert!(j.one_to_one());

    // A third delivery the run never admitted: the recording holds more than the journal.
    let mut j = got.index();
    for h in [11, 22, 11, 11] {
        j.next(h);
    }
    assert_eq!(j.missing(), 1, "the extra delivery has no admission left");
    assert_eq!(j.surplus(), 0);
    assert!(!j.one_to_one());

    // And a recording short of what the run admitted.
    let mut j = got.index();
    j.next(11);
    assert_eq!(
        (j.missing(), j.surplus()),
        (0, 2),
        "two admissions nothing was offered for"
    );
    assert!(!j.one_to_one());
}

/// Nothing at all for the day is named as such, not reported as an empty day.
#[test]
fn no_journal_is_not_an_empty_day() {
    let tmp = tempfile::tempdir().expect("tmp");
    let got = admissions::read(tmp.path(), day(), &ident());
    assert_eq!(got.unusable, Some(Unusable::None));
    assert_eq!(got.reason(), Some("no_admission_journal"));
    assert!(got.records.is_empty() && got.proven.is_none());
}

/// A journal of another trading date is not this day's.
#[test]
fn a_journal_of_another_date_is_refused() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path().join("live");
    let mut w = open(&dir);
    w.append(&[print_at(1, 11)]).expect("append");
    w.seal(2_000_000).expect("seal");
    let other = NaiveDate::from_ymd_opt(2026, 10, 15).expect("valid");
    assert_eq!(
        admissions::read(&dir, other, &ident()).unusable,
        Some(Unusable::Reason("journal_other_date"))
    );
}

/// Corrections, cancels and clock advances are ordered with the prints: the inputs to the facts,
/// and only those.
#[test]
fn revisions_and_clock_advances_are_in_the_order() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path().join("live");
    let mut w = open(&dir);
    let cancel = AdmissionRecord {
        s: 2,
        w: Kind::Cancel,
        o: None,
        vo: None,
        ..print_at(2, 99)
    };
    let tick = AdmissionRecord {
        s: 3,
        w: Kind::Clock,
        y: None,
        i: None,
        e: None,
        r: None,
        o: None,
        vo: None,
        ..print_at(3, 7)
    };
    w.append(&[print_at(1, 11), cancel, tick]).expect("append");
    w.seal(2_000_000).expect("seal");
    let got = admissions::read(&dir, day(), &ident());
    let kinds: Vec<Kind> = got.proven_records().iter().map(|a| a.rec.w).collect();
    assert_eq!(kinds, vec![Kind::Trade, Kind::Cancel, Kind::Clock]);
    // The producer's observation is kept apart from the server's admission.
    let t = &got.proven_records()[0].rec;
    assert_ne!(t.r, Some(t.a), "a collector timestamp is not an admission");
    assert_eq!(t.o, Some(true), "and the verdict applied is recorded");
}
