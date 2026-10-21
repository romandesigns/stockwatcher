//! What a restart recovers for the scanner, and what it refuses to claim.
//!
//! The audit's F1: the restore replayed each symbol's recorded day into the chart engine, the
//! quote board and the pressure board — and not into the context engine, which had no restore
//! path at all. So after every restart (and every deploy is one) a symbol's volume, trades, day
//! high, price history and setup bars began again at zero, and the RVOL numerator became "volume
//! since the process started" against a 30-session full-day mean. A stock that had already traded
//! five times its normal day was invisible until enough *new* volume arrived.
//!
//! The restore this file exercises is deliberately narrower than "carry on where we left off":
//!
//! - the day's **facts** come back — volume, dollar volume, trade count, price history, day high,
//!   setup bars — because the recording is evidence about prints that really executed;
//! - **decisions do not**. Membership, 5/5 entry times and frozen setup levels are claims that
//!   this run observed something at a time, and a recording written by a previous process is not
//!   evidence of that. The live run's own published state is kept and the symbol is decided again,
//!   prospectively, at the next barrier;
//! - whether the restore replayed the same inputs in the same order the run admitted them is a
//!   question with an answer, not an assumption: the admission journal
//!   (`market_archive::admissions`). Where it cannot be answered the figures still come back, and
//!   the symbol says the order is unproven so no window over the restored prefix is reportable.
//!
//! Every fixture is synthetic and offline. A local build cannot name itself
//! (`STOCKWATCHER_COMMIT` is `unknown`), so these runs take the unverified path by construction —
//! which is the honest outcome for a development binary and is asserted as such.

use std::path::Path;
use std::time::{Duration, Instant};

use market_archive::{Archive, ArchiveConfig};
use market_events::{MarketEvent, UnixNanos};
use market_providers::recorder::receive_time;
use market_providers::{ChannelFeed, LiveOptions, LiveProvider, MarketDataProvider, RecorderConfig};
use market_replay::{ArchiveDaySource, EventSource as _, Merger};
use serde_json::Value;
use stockwatcher_server::Market;
use tokio::sync::mpsc;

fn day() -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(2026, 10, 15).expect("valid")
}

fn et(h: u32, m: u32) -> UnixNanos {
    market_events::session::et(day(), h, m, 0, 0).expect("valid")
}

const SYMS: [&str; 2] = ["SMPL", "FXMD"];

fn archive(root: &Path) -> Archive {
    Archive::new(ArchiveConfig {
        root: root.to_path_buf(),
        retention_days: 0,
        min_free_bytes: 0,
    })
}

/// The fixture day in delivery order, stamped with the delivery time as the receive time, as the
/// live provider stamps them.
fn fixture_events(scratch: &Path) -> Vec<MarketEvent> {
    let a = archive(scratch);
    let syms: Vec<String> = SYMS.iter().map(|s| s.to_string()).collect();
    market_archive::fixture::write_fixture_day(&a, day(), &syms, 7).expect("fixture");
    let source = ArchiveDaySource::new(a, day()).expect("source");
    let mut m = Merger::new(source.open().expect("open"));
    let mut out = Vec::new();
    while let Some((d, mut e)) = m.pop() {
        let r = Some(UnixNanos(d));
        match &mut e {
            MarketEvent::Trade(t) => t.receive_ts = r,
            MarketEvent::Quote(q) => q.receive_ts = r,
            MarketEvent::Correction(c) => c.receive_ts = r,
            MarketEvent::Cancel(c) => c.receive_ts = r,
            MarketEvent::Status(s) => s.receive_ts = r,
        }
        out.push(e);
    }
    out
}

fn live(root: &Path, now: UnixNanos, preload: bool) -> (Market, mpsc::Sender<MarketEvent>) {
    let (tx, rx) = mpsc::channel(4096);
    let mut rec = RecorderConfig::new(archive(root).config().clone());
    rec.today = Some(day());
    let session = Box::new(LiveProvider::with_feed(
        Box::new(ChannelFeed {
            label: "restart test".into(),
            symbols: Vec::new(),
            rx,
        }),
        LiveOptions {
            idle_tick: None,
            record: Some(rec),
            ..LiveOptions::default()
        },
    ))
    .start()
    .expect("session");
    (
        Market::from_live_session(session, archive(root), now, preload),
        tx,
    )
}

/// A restart that reads the archive but records nothing, so a corrupted recording stays
/// corrupted: a running recorder opens the same file and would rewrite its tail.
fn reader_only(root: &Path, now: UnixNanos) -> (Market, mpsc::Sender<MarketEvent>) {
    let (tx, rx) = mpsc::channel(64);
    let session = Box::new(LiveProvider::with_feed(
        Box::new(ChannelFeed {
            label: "reader only".into(),
            symbols: Vec::new(),
            rx,
        }),
        LiveOptions {
            idle_tick: None,
            record: None,
            ..LiveOptions::default()
        },
    ))
    .start()
    .expect("session");
    (
        Market::from_live_session(session, archive(root), now, false),
        tx,
    )
}

async fn until(what: &str, f: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !f() {
        assert!(Instant::now() < deadline, "never: {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Waits until `root`'s recording holds `n` events of the symbols.
async fn recorded_in(root: &Path, n: usize) {
    let a = archive(root);
    for _ in 0..2000 {
        let have: usize = SYMS
            .iter()
            .map(|s| {
                a.live_reader(day(), s, None)
                    .map(|r| r.map_while(Result::ok).count())
                    .unwrap_or(0)
            })
            .sum();
        if have >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the recording never reached {n} events");
}

fn volume(m: &Market, symbol: &str) -> Value {
    let d: Value = serde_json::from_str(&m.context_decision(symbol)).expect("decision json");
    d["decision"]["volume"].clone()
}

/// A restart recovers the day's facts, and says the order it replayed them in is unproven.
///
/// The figures are compared with an uninterrupted run over the same recorded session. Two claims
/// are made and no more: the restored day is never **overstated** (an inflated numerator is not a
/// safe lower bound and could invent a qualifying stock), and it is effectively the whole day
/// rather than the tail the broken run had on its own. Any shortfall has to be labelled.
#[tokio::test(flavor = "multi_thread")]
async fn a_restart_recovers_the_days_facts_and_labels_what_it_cannot_prove() {
    let tmp = tempfile::tempdir().expect("tmp");
    let events = fixture_events(&tmp.path().join("scratch"));
    let split = events
        .iter()
        .position(|e| receive_time(e) >= et(11, 0))
        .expect("a midday event");

    // An uninterrupted run over the whole day, with the scanner on.
    let whole = tmp.path().join("whole");
    let (unbroken, tx) = live(&whole, et(4, 0), true);
    unbroken.set_stock_context(true);
    for e in &events {
        tx.send(e.clone()).await.expect("send");
    }
    drop(tx);
    until("the unbroken run ended", || {
        unbroken.state_json()["status"]["state"] == "stopped"
    })
    .await;
    until("the unbroken run loaded its symbols", || {
        unbroken.state_json()["history"]["pending"] == 0
    })
    .await;
    let want: Vec<Value> = SYMS.iter().map(|s| volume(&unbroken, s)).collect();
    assert!(
        want.iter().all(|v| v["shares"].as_u64().unwrap_or(0) > 0),
        "the unbroken run really counted something: {want:?}"
    );

    // The same session, interrupted: the morning recorded, then a restart at 11:00 that streams
    // the afternoon before anything is loaded. A handful of events are kept back so that, after
    // the load, a barrier still arrives — the adoption brings facts and deliberately leaves the
    // last published decision alone, so the restored day appears in the *next* decision, not by
    // rewriting the one a client already has.
    let keep_back = 200;
    let root = tmp.path().join("restarted");
    let (before, tx) = live(&root, et(4, 0), true);
    before.set_stock_context(true);
    for e in &events[..split] {
        tx.send(e.clone()).await.expect("send");
    }
    drop(tx);
    until("the first run ended", || {
        before.state_json()["status"]["state"] == "stopped"
    })
    .await;
    before.checkpoint_quotes();
    drop(before);
    recorded_in(&root, split).await;

    let (after, tx) = live(&root, et(11, 0), false);
    after.set_stock_context(true);
    assert_eq!(
        after.state_json()["history"]["pending"], 2,
        "nothing loaded yet"
    );
    let cut = events.len() - keep_back;
    for e in &events[split..cut] {
        tx.send(e.clone()).await.expect("send");
    }
    until("the afternoon was ingested", || {
        after.state_json()["seq"].as_u64().unwrap_or(0) >= (cut - split) as u64
    })
    .await;
    // The recorder has a queue of its own, so the afternoon reaches the file a moment after the
    // ingest takes it. Loading before it lands is a real shortfall — the restore would read a
    // file that is still short — and it is labelled `recorder_drops`, which is correct but is a
    // different case from the one this test is about.
    recorded_in(&root, cut).await;

    // Before the load the scanner has only the tail: the bug this fixes.
    let tail = volume(&after, SYMS[0]);
    assert!(
        tail["shares"].as_u64().unwrap_or(0) < want[0]["shares"].as_u64().expect("shares"),
        "the tail alone is short of the day: {tail:?} vs {:?}",
        want[0]
    );

    for s in SYMS {
        assert!(after.load_history(s).is_some(), "{s} loads");
    }
    // The rest of the stream: the next barrier decides each symbol again, now on the day's facts.
    for e in &events[cut..] {
        tx.send(e.clone()).await.expect("send");
    }
    drop(tx);
    until("the run ended", || {
        after.state_json()["status"]["state"] == "stopped"
    })
    .await;

    for (s, want) in SYMS.iter().zip(&want) {
        let got = volume(&after, s);
        let have = got["shares"].as_u64().unwrap_or(0);
        let full = want["shares"].as_u64().expect("shares");
        assert!(
            have <= full,
            "{s}: the restored day is never overstated: {have} vs {full}"
        );
        assert!(
            have * 100 >= full * 95,
            "{s}: the restored day is effectively the whole day: {have} vs {full}"
        );
        assert!(
            have > tail["shares"].as_u64().unwrap_or(0) || *s != SYMS[0],
            "{s}: and more than the tail alone had"
        );
        // Whatever it could not establish is named. A local build cannot identify itself, so the
        // admission order is unprovable here by construction and the symbol says so.
        assert!(
            got["coverage_reason"].is_string(),
            "{s}: whatever the restore could not establish is named: {got}"
        );
        assert!(
            matches!(got["coverage"].as_str(), Some("partial") | Some("unverified")),
            "{s}: and a restored day is never complete: {got}"
        );
        // A local build cannot identify itself (`STOCKWATCHER_COMMIT` is `unknown`), so the
        // admission order is unprovable here by construction and that is the reason reported —
        // unless something worse happened, which would be a `partial` day with its own reason.
        if got["coverage"] == "unverified" {
            assert_eq!(
                got["coverage_reason"], "journal_unknown_build",
                "{s}: the reason a development build's order is unproven: {got}"
            );
        }
    }
}

/// A restored symbol publishes no episode it did not observe, and the next barrier decides it on
/// the day's facts.
///
/// Carrying a replay's membership across would backdate a 5/5 this process never saw — under its
/// own run id, so nothing downstream could tell. The facts come over; the episodes do not.
#[tokio::test(flavor = "multi_thread")]
async fn a_restored_symbol_publishes_no_episode_it_did_not_observe() {
    let tmp = tempfile::tempdir().expect("tmp");
    let events = fixture_events(&tmp.path().join("scratch"));
    let split = events
        .iter()
        .position(|e| receive_time(e) >= et(11, 0))
        .expect("a midday event");

    let root = tmp.path().join("run");
    let (before, tx) = live(&root, et(4, 0), true);
    before.set_stock_context(true);
    for e in &events[..split] {
        tx.send(e.clone()).await.expect("send");
    }
    drop(tx);
    until("the first run ended", || {
        before.state_json()["status"]["state"] == "stopped"
    })
    .await;
    drop(before);
    recorded_in(&root, split).await;

    let (after, tx) = live(&root, et(11, 0), false);
    after.set_stock_context(true);
    for e in &events[split..] {
        tx.send(e.clone()).await.expect("send");
    }
    drop(tx);
    until("the afternoon was ingested", || {
        after.state_json()["status"]["state"] == "stopped"
    })
    .await;
    let before_load: Value =
        serde_json::from_str(&after.context_decision(SYMS[0])).expect("decision");
    assert!(after.load_history(SYMS[0]).is_some());
    let d: Value = serde_json::from_str(&after.context_decision(SYMS[0])).expect("decision");

    // Whatever the live tail had published is still what is published: the adoption brought
    // facts, so nothing about the decision itself was rewritten by the replay.
    assert_eq!(
        d["decision"]["membership"], before_load["decision"]["membership"],
        "the published membership is untouched by the restore"
    );
    assert_eq!(
        d["decision"]["timeline"], before_load["decision"]["timeline"],
        "and so are the timeline's first-seen times"
    );
    // Every episode, if any, was admitted at a cutoff this run actually reached — never at one
    // from the recorded morning it did not see.
    let eps = d["decision"]["membership"]["episodes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let first_cutoff = et(11, 0).0;
    for e in &eps {
        assert!(
            e["admitted_at"].as_i64().unwrap_or(0) >= first_cutoff,
            "no episode is backdated into the morning this process did not observe: {e}"
        );
    }
}

/// A load with a live tail arriving under it completes, off the core lock, and the day it
/// restores is sound.
///
/// The tail used to be an unbounded `Vec` fed through the restore **inside** the core lock: a
/// symbol with a slow load held the whole live tail in memory and paused every dashboard in the
/// process while it replayed. Now the catch-up runs off the lock in rounds and only a short
/// atomic section closes the gap — so events keep arriving mid-load, which is what this streams.
///
/// The bound itself and the deferral accounting are pinned as unit tests over the real structure
/// (`crates/server/src/history.rs`), because filling a 50,000-event hold through a provider
/// session would be a slow test of the same arithmetic.
#[tokio::test(flavor = "multi_thread")]
async fn a_load_with_a_live_tail_under_it_completes_off_the_lock() {
    let tmp = tempfile::tempdir().expect("tmp");
    let events = fixture_events(&tmp.path().join("scratch"));
    let split = events
        .iter()
        .position(|e| receive_time(e) >= et(11, 0))
        .expect("a midday event");

    let root = tmp.path().join("run");
    let (before, tx) = live(&root, et(4, 0), true);
    for e in &events[..split] {
        tx.send(e.clone()).await.expect("send");
    }
    drop(tx);
    until("the first run ended", || {
        before.state_json()["status"]["state"] == "stopped"
    })
    .await;
    drop(before);
    recorded_in(&root, split).await;

    // A restart that starts loading while the afternoon is still coming in.
    let (after, tx) = live(&root, et(11, 0), false);
    after.set_stock_context(true);
    let streaming = {
        let tail: Vec<MarketEvent> = events[split..].to_vec();
        tokio::spawn(async move {
            for e in tail {
                if tx.send(e).await.is_err() {
                    break;
                }
            }
        })
    };
    // Loads run against a moving tail; a deferred one is retried, which is the contract.
    let mut loaded = 0;
    for _ in 0..200 {
        loaded = SYMS
            .iter()
            .filter(|s| {
                after.state_json()["history"]["pending"] == 0 || after.load_history(s).is_some()
            })
            .count();
        if loaded == SYMS.len() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(loaded, SYMS.len(), "every symbol loaded while the tail ran");
    streaming.await.expect("the stream task");
    until("the afternoon was ingested", || {
        after.state_json()["status"]["state"] == "stopped"
    })
    .await;

    for s in SYMS {
        let got = volume(&after, s);
        assert!(
            got["shares"].as_u64().unwrap_or(0) > 0,
            "{s}: the restored day has volume: {got}"
        );
        assert_ne!(
            got["coverage"], "complete",
            "{s}: and never claims a complete day: {got}"
        );
    }
}

/// A load whose archive read is short reports the symbol's day as partial, never as a day with
/// no trades.
///
/// An unreadable or truncated file used to come back as an empty `Vec` — the same value as "the
/// symbol did not trade that day" — so a storage fault restored a symbol with zero volume and
/// nothing said why.
#[tokio::test(flavor = "multi_thread")]
async fn a_truncated_recording_is_partial_and_not_an_empty_day() {
    let tmp = tempfile::tempdir().expect("tmp");
    let events = fixture_events(&tmp.path().join("scratch"));
    let root = tmp.path().join("run");
    let (before, tx) = live(&root, et(4, 0), true);
    for e in &events {
        tx.send(e.clone()).await.expect("send");
    }
    drop(tx);
    until("the first run ended", || {
        before.state_json()["status"]["state"] == "stopped"
    })
    .await;
    drop(before);
    recorded_in(&root, events.len()).await;

    // Corrupt one symbol's recording in the middle, so a record will not decode.
    let a = archive(&root);
    let path = a.live_path(day(), SYMS[0]);
    let mut bytes = std::fs::read(&path).expect("read");
    let whole = bytes.len();
    for b in bytes.iter_mut().skip(whole / 2).take(4096) {
        *b = 0xff;
    }
    std::fs::write(&path, &bytes).expect("write");
    // The fixture has to actually break the read, or this test would pass on a healthy file.
    let (decoded, failed) = {
        let r = a.live_reader(day(), SYMS[0], None).expect("open");
        let mut n = 0usize;
        let mut failed = false;
        for x in r {
            match x {
                Ok(_) => n += 1,
                Err(_) => {
                    failed = true;
                    break;
                }
            }
        }
        (n, failed)
    };
    assert!(failed, "the fixture really corrupted the recording");
    assert!(decoded > 0, "and left a readable prefix of {decoded} records");

    let (after, tx) = reader_only(&root, et(20, 0));
    after.set_stock_context(true);
    let read = after.load_history(SYMS[0]).expect("the load still runs");
    assert!(
        read > 0,
        "what was readable was read, rather than the day being abandoned"
    );
    // A barrier after the load, so the symbol is decided on the restored facts: the adoption
    // brings facts and leaves the last published decision alone, so there is nothing to read
    // until the next one.
    assert!(after.load_history(SYMS[1]).is_some());
    for e in events.iter().rev().take(400).rev() {
        tx.send(e.clone()).await.expect("send");
    }
    drop(tx);
    until("the trailing events were ingested", || {
        after.state_json()["status"]["state"] == "stopped"
    })
    .await;
    let got = volume(&after, SYMS[0]);
    assert_eq!(
        got["coverage"], "partial",
        "a short read is a partial day, not a day with no trades: {got}"
    );
    assert!(
        matches!(
            got["coverage_reason"].as_str(),
            Some("archive_truncated") | Some("archive_unreadable")
        ),
        "and the reason names the storage fault: {got}"
    );

    // And the other symbol, whose file is intact, is not tarred with it.
    let fine = volume(&after, SYMS[1]);
    assert!(
        fine["coverage_reason"] != "archive_truncated",
        "one symbol's corrupt file is not another's: {fine}"
    );
}
