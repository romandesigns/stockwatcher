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
//! Every fixture is synthetic and offline. An unnamed build cannot prove journal identity. A
//! named build can prove its admission prefix, but a held print's provisional condition count
//! still does not prove the chart verdict; that distinction remains explicit after restoration.

use std::path::Path;
use std::time::{Duration, Instant};

use market_archive::{Archive, ArchiveConfig};
use market_events::{MarketEvent, UnixNanos};
use market_providers::recorder::receive_time;
use market_providers::{
    ChannelFeed, LiveOptions, LiveProvider, MarketDataProvider, RecorderConfig,
};
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

/// How long a wait may take before it fails. Every test in this file plays a whole fixture day
/// through a live market, and cargo runs them at once: on a two-core CI runner the nine share
/// the cores, and a run that takes 40 s on a good day needed more than its 30 s step budget
/// twice in a row on 2026-11-27 (reproduced here pinned to one core, on 72a662a4 as on main).
/// The waits are liveness, not timing claims: a wider bound proves the same thing, and a
/// recorder that really dropped events still fails, only later.
const WAIT: Duration = Duration::from_secs(120);

async fn until(what: &str, f: impl Fn() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !f() {
        assert!(Instant::now() < deadline, "never: {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Waits until `root`'s recording holds `n` events of the symbols, for at most [`WAIT`] like
/// [`until`]. Bounded by time, not by polls: each poll decodes the whole recording, and on a slow
/// runner 2,000 of them took the whole job's 20 minutes (2026-11-23) before saying anything.
async fn recorded_in(root: &Path, n: usize) {
    let a = archive(root);
    let deadline = Instant::now() + WAIT;
    let mut have = 0;
    while Instant::now() < deadline {
        have = SYMS
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
    panic!("the recording never reached {n} events (it holds {have}): did the recorder drop?");
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
        after.state_json()["history"]["pending"],
        2,
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
    assert_eq!(
        tail["integrity"]["history_pending"], true,
        "held facts are explicitly provisional before adoption"
    );
    let held_decision: Value = serde_json::from_str(&after.context_decision(SYMS[0])).unwrap();
    let held_rvol = held_decision["decision"]["pillars"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["pillar"] == "rvol")
        .unwrap();
    assert_eq!(held_rvol["state"], "UNKNOWN");
    assert_eq!(
        held_rvol["reason"], "history_pending",
        "a provisional tail cannot qualify a stock on RVOL"
    );
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
        assert_eq!(
            got["integrity"]["history_pending"], false,
            "successful factual adoption clears only the temporary pending guard"
        );
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
        // Whatever it could not establish is named, including a held print's unknown chart
        // verdict in a named build and an unprovable journal identity in an unnamed build.
        assert!(
            got["coverage_reason"].is_string(),
            "{s}: whatever the restore could not establish is named: {got}"
        );
        assert!(
            matches!(
                got["coverage"].as_str(),
                Some("partial") | Some("unverified")
            ),
            "{s}: and a restored day is never complete: {got}"
        );
        // The reason distinguishes an unknown executable from known admissions whose chart
        // verdicts were still provisional. Neither can become a fully verified restored day.
        if got["coverage"] == "unverified" {
            let expected = if stockwatcher_server::COMMIT == "unknown" {
                "journal_unknown_build"
            } else {
                "journal_verdict_unproven"
            };
            assert_eq!(
                got["coverage_reason"], expected,
                "{s}: the reason this build's restored inputs remain unverified: {got}"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn detection_requests_pending_history_without_preload_or_any_chart() {
    let tmp = tempfile::tempdir().expect("tmp");
    let events = fixture_events(&tmp.path().join("scratch"));
    let split = events
        .iter()
        .position(|e| receive_time(e) >= et(11, 0))
        .unwrap();
    let root = tmp.path().join("run");
    let (before, tx) = live(&root, et(4, 0), true);
    for e in &events[..split] {
        tx.send(e.clone()).await.unwrap();
    }
    drop(tx);
    until("recording run ended", || {
        before.state_json()["status"]["state"] == "stopped"
    })
    .await;
    drop(before);
    recorded_in(&root, split).await;
    let (after, tx) = live(&root, et(11, 0), false);
    after.set_detection_panel(true);
    assert_eq!(after.state_json()["history"]["pending"], 2);
    for e in events.iter().skip(split).take(500) {
        tx.send(e.clone()).await.unwrap();
    }
    until(
        "detector independently requested and loaded both histories",
        || after.state_json()["history"]["pending"] == 0,
    )
    .await;
    after.publish_detection();
    let frame: Value = serde_json::from_str(&after.detection_snapshot().2).unwrap();
    assert_eq!(frame["enabled"], true);
    assert_eq!(frame["snapshot"]["monitored"]["symbols_history_pending"], 0);
    assert_eq!(
        frame["snapshot"]["data_status"]["history_pending_symbols"],
        serde_json::json!([])
    );
    assert_eq!(
        serde_json::from_str::<Value>(&after.ross_snapshot().1).unwrap()["enabled"],
        false
    );
    drop(tx);
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

/// With the background preload on and nothing else asking, every symbol still ends up loaded
/// while the tail streams. A load the tail outpaces defers, and nothing used to ask for it again
/// unless a chart opened the symbol: an empty chart and a history-pending (RVOL UNKNOWN) row for
/// the life of the process. The loader now retries deferred loads itself.
#[tokio::test(flavor = "multi_thread")]
async fn the_background_loader_finishes_every_symbol_under_a_moving_tail() {
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

    let (after, tx) = live(&root, et(11, 0), true);
    after.set_stock_context(true);
    let tail: Vec<MarketEvent> = events[split..].to_vec();
    let streaming = tokio::spawn(async move {
        for e in tail {
            if tx.send(e).await.is_err() {
                break;
            }
        }
    });
    until("the background loader loaded every symbol", || {
        after.state_json()["history"]["pending"] == 0
    })
    .await;
    streaming.await.expect("the stream task");
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
    assert!(
        decoded > 0,
        "and left a readable prefix of {decoded} records"
    );

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

/// A shutdown through the real `close_recording` boundary, with more than one batch queued and
/// offers still arriving: the journal is drained and waited for, and nothing on disk claims that
/// nothing followed.
///
/// `close_recording` closes WebSocket sessions, but the market ingest and clock tasks keep
/// running and keep admitting. So the shutdown cannot establish quiescence, and the journal it
/// leaves is a checkpointed prefix with an unverified tail — not a sealed one. What it *can*
/// guarantee is that it does not return while records are still queued, which is what the
/// discarded writer handle used to allow.
#[tokio::test(flavor = "multi_thread")]
async fn a_shutdown_waits_for_the_journal_and_claims_no_clean_stop() {
    let tmp = tempfile::tempdir().expect("tmp");
    let events = fixture_events(&tmp.path().join("scratch"));
    let root = tmp.path().join("run");
    let (market, tx) = live(&root, et(4, 0), true);
    market.set_stock_context(true);

    // Enough to need several writer batches.
    let bulk = events.len().min(12_000);
    for e in &events[..bulk] {
        tx.send(e.clone()).await.expect("send");
    }
    until("the market took the bulk", || {
        market.state_json()["seq"].as_u64().unwrap_or(0) >= (bulk / 2) as u64
    })
    .await;

    // Keep offering while the shutdown runs: this is the schedule the seal could not survive.
    let late = {
        let tail: Vec<MarketEvent> = events[bulk..].iter().take(2_000).cloned().collect();
        let tx = tx.clone();
        tokio::spawn(async move {
            for e in tail {
                if tx.send(e).await.is_err() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
    };
    market.close_recording(Duration::from_secs(10)).await;
    let _ = late.await;
    drop(tx);

    // Whatever the race did, the file is internally consistent and nothing is sealed.
    let dir = archive(&root).live_dir(day());
    let segments: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .expect("the live directory")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(market_archive::admissions::SEGMENT_PREFIX))
        })
        .collect();
    assert_eq!(segments.len(), 1, "one boot, one segment: {segments:?}");
    let text = std::fs::read_to_string(&segments[0]).expect("the segment");
    assert!(
        text.contains("\"k\":\"ck\""),
        "the prefix was checkpointed before the shutdown returned"
    );
    assert!(
        !text.contains("\"k\":\"seal\""),
        "and nothing claims a clean stop while the market was still admitting"
    );

    // The shutdown waited: everything the writer had accepted is on disk, so the file has no
    // hole where a record was queued and the process exited.
    let lines = text.lines().count();
    assert!(lines > 1, "the journal has records, not just a header");
    let report = market
        .admission_report()
        .expect("a live market with a recording keeps a journal");
    assert_eq!(
        report.written + report.dropped,
        report.admitted,
        "every record that took a sequence is either written or a counted queue drop: {report:?}"
    );
    // Offers the fence turned away are counted apart: the engine admitted them and the journal
    // does not have them, which is exactly why no seal is written.
    assert!(
        report.fenced > 0,
        "this fixture really did keep admitting across the shutdown: {report:?}"
    );
    assert!(
        !report.complete(),
        "and the journal says so rather than reading as a whole day: {report:?}"
    );
    assert_eq!(report.errors, 0, "{report:?}");
}

/// The shadow measurement, captured through the real market: decisions off the engine, a bounded
/// handoff under the core lock, folding and an append-only log off it — and `insufficient_data`,
/// which is the only honest answer this process can give.
///
/// An earlier draft folded into an in-memory summary under the lock with no durable handoff and
/// no production caller, so the enable path gathered nothing a restart could review. This drives
/// the seam the market actually uses.
#[tokio::test(flavor = "multi_thread")]
async fn the_shadow_captures_through_the_market_and_reports_insufficient_data() {
    let tmp = tempfile::tempdir().expect("tmp");
    let events = fixture_events(&tmp.path().join("scratch"));
    let root = tmp.path().join("run");
    let (market, tx) = live(&root, et(4, 0), true);
    market.set_stock_context(true);

    // Nothing has been decided yet, so there is nothing to report but the definitions.
    let before = market
        .shadow_report()
        .expect("a context board brings a capture");
    assert_eq!(before.definitions, "shadow-candidate-vs-strict-v1");
    assert_eq!(before.sessions_held, 0);
    assert!(!before.outcome.measured());

    // Inside the queue's bound, so this exercises the seam rather than the drop path (which has
    // its own test over the sink). Nothing here folds: the capture's own thread does that, and
    // the test waits for it exactly as a reader would.
    for e in events.iter().take(4_000) {
        tx.send(e.clone()).await.expect("send");
    }
    drop(tx);
    until("the market finished", || {
        market.state_json()["status"]["state"] == "stopped"
    })
    .await;
    until(
        "the capture's own thread folded and wrote what it saw",
        || {
            market.shadow_report().is_some_and(|r| {
                r.absorbed > 0 && r.absorbed == r.captured && r.persisted == r.absorbed
            })
        },
    )
    .await;

    let r = market.shadow_report().expect("a capture");
    assert!(
        r.captured > 0,
        "decisions reached the capture through the market: {r:?}"
    );
    assert_eq!(
        r.absorbed, r.captured,
        "and every one was folded off the lock: {r:?}"
    );
    assert!(r.sessions_held > 0, "{r:?}");
    assert_eq!(r.dropped_events, 0, "{r:?}");

    // The log is on disk and reads back as the events it was, so a restart can review the cohort
    // rather than losing it with the process.
    assert!(r.durable, "the capture has somewhere to write: {r:?}");
    assert_eq!(r.persisted, r.absorbed, "{r:?}");
    let dir = root.join("shadow");
    let logs: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .expect("the shadow directory")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    assert_eq!(logs.len(), 1, "one segment for this boot: {logs:?}");
    let text = std::fs::read_to_string(&logs[0]).expect("the log");
    let rows: Vec<&str> = text.lines().collect();
    // A header naming the definitions and the build, then one line per record.
    let header: serde_json::Value =
        serde_json::from_str(rows.first().expect("a header")).expect("a header");
    assert_eq!(header["definitions"], "shadow-candidate-vs-strict-v1");
    assert!(header["epoch"].as_u64().is_some_and(|e| e >= 1), "{header}");
    assert!(
        header["code"].as_str().is_some_and(|c| !c.is_empty()),
        "{header}"
    );
    assert_eq!(
        rows.len() as u64 - 1,
        r.persisted,
        "every appended record is a line"
    );
    // The decisions among them carry the rules they were made under.
    let parsed: Vec<serde_json::Value> = rows[1..]
        .iter()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .collect();
    let first = parsed
        .iter()
        .find(|v| v.get("symbol").is_some_and(|s| s.is_string()))
        .expect("some records are decisions");
    assert!(first["cutoff"].is_i64(), "{first}");
    assert!(first["candidate"].is_boolean(), "{first}");
    assert_eq!(
        first["policy"].as_str().map(|p| p.starts_with("pillars-")),
        Some(true),
        "the rules the decision was made under travel with it: {first}"
    );
    // And the run's own lifecycle is in there too: without it nothing could say whether the run
    // was watching, and every session would be censored for the wrong reason.
    assert!(
        parsed.iter().any(|v| v.get("kind").is_some()),
        "the lifecycle is recorded beside the decisions"
    );
    // Every record carries its sequence, so a reader of the file alone can see a loss rather
    // than inferring a clean negative from a gap between two retained clock records.
    let seqs: Vec<u64> = parsed.iter().filter_map(|v| v["s"].as_u64()).collect();
    assert_eq!(seqs.len(), parsed.len(), "every record is sequenced");
    assert_eq!(
        seqs,
        (1..=seqs.len() as u64).collect::<Vec<_>>(),
        "and this run lost none of them"
    );
    assert!(
        r.runs >= 1,
        "so the capture knows what the run monitored: {r:?}"
    );

    // And the figure itself: nothing. The fixture day is one date of one run, far short of the
    // floor, and every session is censored because the run's clock never reached the close.
    assert!(
        !r.outcome.measured(),
        "no figure from a fixture day: {:?}",
        r.outcome
    );
    assert!(
        r.outcome.line().starts_with("insufficient_data"),
        "{}",
        r.outcome.line()
    );
    assert_eq!(
        r.live_sessions, 0,
        "a run that stopped mid-session has nothing closed: {r:?}"
    );
    assert!(
        r.censored_sessions > 0 || r.synthetic_sessions > 0,
        "the sessions it did see are censored or synthetic, never negatives: {r:?}"
    );
    assert_eq!(
        r.neither, 0,
        "and nothing unobserved was counted as an absence: {r:?}"
    );
}

/// With nowhere to put its log, the capture still runs and the report withholds every figure.
///
/// This is the no-segment case: the directory is blocked, so none is reserved and nothing is ever
/// appended. What it pins is the honesty of the outcome — a cohort held only in memory is not a
/// forward corpus — and not the behaviour of a slow write, which a gated append proves directly
/// (`market::tests::a_stalled_shadow_append_holds_neither_the_reporter_nor_the_publisher`).
#[tokio::test(flavor = "multi_thread")]
async fn a_capture_with_nowhere_to_write_still_runs_and_reports_nothing() {
    let tmp = tempfile::tempdir().expect("tmp");
    let events = fixture_events(&tmp.path().join("scratch"));
    let root = tmp.path().join("run");
    // A file where the shadow directory belongs, so no segment can be reserved and no append can
    // land.
    std::fs::create_dir_all(&root).expect("root");
    std::fs::write(root.join("shadow"), b"not a directory").expect("blocker");

    let (market, tx) = live(&root, et(4, 0), true);
    market.set_stock_context(true);
    for e in events.iter().take(3_000) {
        tx.send(e.clone()).await.expect("send");
    }
    drop(tx);
    // The market finishes normally: nothing waits on the capture.
    until("the market finished", || {
        market.state_json()["status"]["state"] == "stopped"
    })
    .await;
    // And it still serves its own state.
    assert!(market.state_json()["seq"].as_u64().unwrap_or(0) > 0);

    let r = market.shadow_report().expect("a capture");
    assert!(
        !r.durable,
        "no segment could be reserved, and the report says so: {r:?}"
    );
    assert!(
        !r.complete(),
        "so the cohort is not reviewable: {:?}",
        r.outcome
    );
    assert!(
        !r.outcome.measured(),
        "and no figure is reported over it: {:?}",
        r.outcome
    );
    assert!(
        r.outcome.line().starts_with("insufficient_data"),
        "{}",
        r.outcome.line()
    );
    // The sessions it saw are still counted — the capture works, it just has nowhere to write —
    // and nothing unobserved became a negative.
    assert_eq!(r.neither, 0, "{r:?}");
    assert_eq!(r.write_errors, 0, "nothing was even attempted: {r:?}");
}
