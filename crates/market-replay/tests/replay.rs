use chart_engine::{ChartEngine, Timeframe};
use chrono::NaiveDate;
use market_archive::fixture::write_fixture_day;
use market_archive::{Archive, ArchiveConfig};
use market_events::session::et;
use market_events::{MarketEvent, UnixNanos};
use market_replay::merge::raw_delivery;
use market_replay::{
    ArchiveDaySource, Command, EventSource, FeedItem, ReplayCore, ReplayOptions, Speed, spawn,
};
use std::sync::Arc;
use std::time::Duration;

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 15).unwrap()
}

fn fixture_source() -> (tempfile::TempDir, Arc<dyn EventSource>) {
    let tmp = tempfile::tempdir().unwrap();
    let archive = Archive::new(ArchiveConfig {
        root: tmp.path().join("archive"),
        retention_days: 0,
        min_free_bytes: 0,
    });
    let syms: Vec<String> = ["SMPL", "FXLO", "FXMD"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    write_fixture_day(&archive, day(), &syms, 7).unwrap();
    let src = ArchiveDaySource::new(archive, day()).unwrap();
    (tmp, Arc::new(src))
}

fn events_of(items: &[FeedItem]) -> Vec<MarketEvent> {
    items
        .iter()
        .filter_map(|i| match i {
            FeedItem::Event(e) => Some(e.clone()),
            _ => None,
        })
        .collect()
}

fn run_core_to(core: &mut ReplayCore, until: UnixNanos) -> Vec<FeedItem> {
    let mut out = Vec::new();
    while !core.advance(until, 1000, &mut out).unwrap() {}
    out
}

fn feed(engine: &mut ChartEngine, items: &[FeedItem]) {
    for i in items {
        match i {
            FeedItem::Event(e) => {
                engine.apply(e);
            }
            FeedItem::Advance(t) => {
                engine.advance_clock(*t);
            }
            _ => {}
        }
    }
}

fn assert_same_state(a: &ChartEngine, b: &ChartEngine, symbols: &[String]) {
    for s in symbols {
        for tf in Timeframe::ALL {
            let sa = a.snapshot(s, tf, None);
            let sb = b.snapshot(s, tf, None);
            assert_eq!(sa, sb, "{s} {tf}");
        }
        for bin in [1, 2, 5] {
            let bin = market_events::Price::from_cents(bin);
            for sess in a.sessions(s) {
                assert_eq!(
                    a.profile_rows(s, sess, bin),
                    b.profile_rows(s, sess, bin),
                    "{s} {sess} profile"
                );
            }
        }
        assert_eq!(a.quality(s), b.quality(s), "{s} quality");
    }
}

#[test]
fn same_archive_yields_identical_event_sequence_twice() {
    let (_tmp, src) = fixture_source();
    let end = market_replay::day_bounds(day()).unwrap().1;
    let mut c1 = ReplayCore::new(src.clone(), 1_000_000_000).unwrap();
    let mut c2 = ReplayCore::new(src.clone(), 1_000_000_000).unwrap();
    let a = run_core_to(&mut c1, end);
    // Different batch sizes and intermediate stops must not change anything.
    let mut b = Vec::new();
    let mut t = c2.start().nanos();
    while t < end.nanos() {
        t = (t + 7_919_000_000).min(end.nanos());
        while !c2.advance(UnixNanos(t), 37, &mut b).unwrap() {}
    }
    assert!(c1.finished() && c2.finished());
    let (ea, eb) = (events_of(&a), events_of(&b));
    assert!(ea.len() > 60_000, "{}", ea.len());
    assert_eq!(ea, eb);

    // Merge invariants: delivery never decreases; each symbol keeps its file (arrival) order.
    let mut running = std::collections::HashMap::<String, i64>::new();
    let mut last = i64::MIN;
    for e in &ea {
        let s = e.symbol().unwrap().to_string();
        let d = running
            .get(&s)
            .copied()
            .unwrap_or(i64::MIN)
            .max(raw_delivery(e));
        running.insert(s, d);
        assert!(d >= last);
        last = d;
    }
    for sym in src.symbols() {
        let file: Vec<MarketEvent> = market_archive::Archive::new(ArchiveConfig {
            root: _tmp.path().join("archive"),
            ..ArchiveConfig::default()
        })
        .reader(day(), &sym)
        .unwrap()
        .map(|r| r.unwrap().event)
        .collect();
        let replayed: Vec<MarketEvent> = ea
            .iter()
            .filter(|e| e.symbol() == Some(sym.as_str()))
            .cloned()
            .collect();
        assert_eq!(file, replayed, "{sym}");
    }
}

#[test]
fn seek_rebuild_equals_linear_play() {
    let (_tmp, src) = fixture_source();
    // After the 11:00 correction was published and inside the regular session.
    for target in [
        et(day(), 11, 7, 3, 250),
        et(day(), 12, 0, 45, 0),
        et(day(), 3, 0, 0, 0),
    ] {
        let target = target.unwrap();
        // Linear: odd-sized hops (as wall ticks would produce), engine fed as it goes.
        let mut lin = ReplayCore::new(src.clone(), 1_000_000_000).unwrap();
        let mut e1 = ChartEngine::default();
        let mut t = lin.start().nanos();
        while t < target.nanos() {
            t = (t + 3_333_333_333).min(target.nanos());
            let items = run_core_to(&mut lin, UnixNanos(t));
            feed(&mut e1, &items);
        }
        // Seek: play somewhere else first, then reset and jump straight to the target.
        let mut sk = ReplayCore::new(src.clone(), 1_000_000_000).unwrap();
        let mut e2 = ChartEngine::default();
        let _ = run_core_to(&mut sk, et(day(), 15, 0, 0, 0).unwrap());
        sk.reset().unwrap();
        let items = run_core_to(&mut sk, target);
        feed(&mut e2, &items);
        assert_eq!(lin.now(), sk.now());
        assert_eq!(lin.emitted(), sk.emitted());
        assert_same_state(&e1, &e2, &src.symbols());
    }
}

#[test]
fn step_emits_exactly_one_event() {
    let (_tmp, src) = fixture_source();
    let mut c = ReplayCore::new(src, 1_000_000_000).unwrap();
    let _ = run_core_to(&mut c, et(day(), 10, 40, 0, 0).unwrap());
    let before = c.emitted();
    let next = c.next_delivery().unwrap();
    let mut out = Vec::new();
    assert!(c.step(&mut out).unwrap());
    assert_eq!(events_of(&out).len(), 1);
    assert_eq!(c.emitted(), before + 1);
    assert_eq!(c.now(), next);
}

async fn collect_until_finished(rx: &mut tokio::sync::mpsc::Receiver<FeedItem>) -> Vec<FeedItem> {
    let mut items = Vec::new();
    while let Some(i) = rx.recv().await {
        let done = matches!(&i, FeedItem::Clock(c) if c.finished);
        items.push(i);
        if done {
            break;
        }
    }
    items
}

#[tokio::test]
async fn driver_at_max_is_deterministic_and_matches_the_core() {
    let (_tmp, src) = fixture_source();
    let mut runs = Vec::new();
    for batch in [512, 4096] {
        let (_h, mut rx) = spawn(
            src.clone(),
            ReplayOptions {
                speed: Speed::Max,
                batch,
                channel: 64,
                ..ReplayOptions::default()
            },
        )
        .unwrap();
        let items = collect_until_finished(&mut rx).await;
        assert!(matches!(items[0], FeedItem::Reset(_)));
        runs.push(items);
    }
    assert_eq!(events_of(&runs[0]), events_of(&runs[1]));
    let mut core = ReplayCore::new(src.clone(), 1_000_000_000).unwrap();
    let all = run_core_to(&mut core, market_replay::day_bounds(day()).unwrap().1);
    assert_eq!(events_of(&runs[0]), events_of(&all));
    // Same derived state at the end of the day whatever the batching.
    let mut e1 = ChartEngine::default();
    let mut e2 = ChartEngine::default();
    feed(&mut e1, &runs[0]);
    feed(&mut e2, &all);
    assert_same_state(&e1, &e2, &src.symbols());
}

#[tokio::test(start_paused = true)]
async fn paced_play_moves_virtual_time_by_speed_times_wall_time() {
    let (_tmp, src) = fixture_source();
    let start = et(day(), 10, 40, 0, 0).unwrap();
    let (h, mut rx) = spawn(
        src.clone(),
        ReplayOptions {
            start: Some(start),
            speed: Speed::Times(60),
            autoplay: false,
            ..ReplayOptions::default()
        },
    )
    .unwrap();
    // Drain the initial catch-up.
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let s = h.command(Command::Pause).await.unwrap();
    assert_eq!(s.now, start);
    h.command(Command::Play).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1000)).await;
    let s = h.command(Command::Pause).await.unwrap();
    let advanced = s.now.nanos() - start.nanos();
    // 60× for one wall second = 60 virtual seconds, give or take one 50 ms tick (3 s).
    assert!(
        (57_000_000_000..=63_000_000_000).contains(&advanced),
        "advanced {advanced} ns"
    );
    // Speed change, then step-interval to the next 1-minute boundary.
    let s = h.command(Command::Speed(Speed::Times(2))).await.unwrap();
    assert_eq!(s.speed, Speed::Times(2));
    let s = h
        .command(Command::StepInterval(60_000_000_000))
        .await
        .unwrap();
    assert_eq!(s.now.nanos() % 60_000_000_000, 0);
    assert!(!s.playing);
    // Seek back: clock lands exactly on the target.
    let s = h.command(Command::Seek(start)).await.unwrap();
    assert_eq!(s.now, start);
    drop(h);
    drain.abort();
}

#[tokio::test]
async fn driver_seek_matches_linear_state() {
    let (_tmp, src) = fixture_source();
    let target = et(day(), 11, 2, 0, 0).unwrap();
    let (h, mut rx) = spawn(
        src.clone(),
        ReplayOptions {
            start: Some(et(day(), 9, 0, 0, 0).unwrap()),
            autoplay: false,
            ..ReplayOptions::default()
        },
    )
    .unwrap();
    let consumer = tokio::spawn(async move {
        let mut engine = ChartEngine::default();
        let mut resets = 0;
        while let Some(i) = rx.recv().await {
            match &i {
                FeedItem::Reset(_) => {
                    resets += 1;
                    engine = ChartEngine::default();
                }
                FeedItem::Clock(c) if resets == 2 && !c.playing => return (engine, c.clone()),
                _ => {}
            }
            feed(&mut engine, std::slice::from_ref(&i));
        }
        panic!("stream ended");
    });
    h.command(Command::Seek(target)).await.unwrap();
    let (engine, clock) = consumer.await.unwrap();
    assert_eq!(clock.now, target);
    let mut core = ReplayCore::new(src.clone(), 1_000_000_000).unwrap();
    let mut linear = ChartEngine::default();
    let items = run_core_to(&mut core, target);
    feed(&mut linear, &items);
    assert_same_state(&engine, &linear, &src.symbols());
}

/// Two fixture days of one archive with different symbol sets, the source opened on `day()`.
fn two_day_source() -> (tempfile::TempDir, Arc<dyn EventSource>, NaiveDate) {
    let tmp = tempfile::tempdir().unwrap();
    let archive = Archive::new(ArchiveConfig {
        root: tmp.path().join("archive"),
        retention_days: 0,
        min_free_bytes: 0,
    });
    let other = NaiveDate::from_ymd_opt(2026, 10, 14).unwrap();
    let syms = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    write_fixture_day(&archive, day(), &syms(&["SMPL", "FXLO"]), 7).unwrap();
    write_fixture_day(&archive, other, &syms(&["FXMD", "FXHI", "SMPL"]), 11).unwrap();
    let src = ArchiveDaySource::new(archive, day()).unwrap();
    (tmp, Arc::new(src), other)
}

#[tokio::test]
async fn day_switch_announces_the_source_then_rebuilds_like_a_seek_at_the_start_time() {
    let (_tmp, src, other) = two_day_source();
    let start_time = chrono::NaiveTime::from_hms_opt(10, 40, 0).unwrap();
    let (h, mut rx) = spawn(
        src.clone(),
        ReplayOptions {
            start: Some(et(day(), 10, 40, 0, 0).unwrap()),
            start_time: Some(start_time),
            autoplay: false,
            ..ReplayOptions::default()
        },
    )
    .unwrap();
    // The initial catch-up ends with its Clock; then only the switch's items arrive.
    let consumer = tokio::spawn(async move {
        let mut items = Vec::new();
        let mut caught_up = false;
        while let Some(i) = rx.recv().await {
            let clock = matches!(i, FeedItem::Clock(_));
            if caught_up {
                items.push(i);
                if clock {
                    return items;
                }
            } else if clock {
                caught_up = true;
            }
        }
        panic!("stream ended");
    });
    let s = h.command(Command::Day(other)).await.unwrap();
    assert_eq!(s.day, Some(other));
    assert_eq!(s.now, et(other, 10, 40, 0, 0).unwrap());
    assert!(!s.playing, "paused before, paused after");
    let items = consumer.await.unwrap();
    let FeedItem::Source(info) = &items[0] else {
        panic!("expected Source first, got {:?}", items[0]);
    };
    assert_eq!(info.day, other);
    assert_eq!(info.symbols, ["FXHI", "FXMD", "SMPL"]);
    assert!(info.fixture);
    assert!(info.label.contains("2026-10-14"));
    assert!(matches!(&items[1], FeedItem::Reset(c) if c.day == Some(other)));
    // What follows the Reset is exactly a linear play of the new day to the start time.
    let new_src: Arc<dyn EventSource> = src.for_day(other).unwrap();
    let mut core = ReplayCore::new(new_src.clone(), 1_000_000_000).unwrap();
    let linear = run_core_to(&mut core, et(other, 10, 40, 0, 0).unwrap());
    assert_eq!(events_of(&items[2..]), events_of(&linear));
    let mut switched = ChartEngine::default();
    let mut expected = ChartEngine::default();
    feed(&mut switched, &items[2..]);
    feed(&mut expected, &linear);
    assert_same_state(&switched, &expected, &new_src.symbols());
}

#[tokio::test]
async fn a_day_that_cannot_be_opened_is_refused_and_nothing_changes() {
    let (_tmp, src, _) = two_day_source();
    let (h, mut rx) = spawn(
        src,
        ReplayOptions {
            autoplay: false,
            ..ReplayOptions::default()
        },
    )
    .unwrap();
    let before = h.command(Command::Pause).await.unwrap();
    while let Ok(i) = rx.try_recv() {
        drop(i);
    }
    let missing = NaiveDate::from_ymd_opt(2026, 10, 13).unwrap();
    let err = h.command(Command::Day(missing)).await.unwrap_err();
    assert!(
        matches!(&err, market_replay::CommandError::Rejected(m) if m.contains("2026-10-13")),
        "{err:?}"
    );
    assert_eq!(h.state(), before);
    // No Source, no Reset: the consumer saw nothing at all.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn a_playing_replay_keeps_playing_on_the_new_day() {
    let (_tmp, src, other) = two_day_source();
    let (h, mut rx) = spawn(
        src,
        ReplayOptions {
            speed: Speed::Times(1),
            autoplay: true,
            ..ReplayOptions::default()
        },
    )
    .unwrap();
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let s = h.command(Command::Day(other)).await.unwrap();
    assert!(s.playing);
    assert_eq!(s.speed, Speed::Times(1));
    // No start_time configured: the new day starts at its first session start.
    assert_eq!(s.now, market_replay::day_bounds(other).unwrap().0);
    drain.abort();
}

/// The Practice-page freeze (2026-11-26): a consumer that stops reading (the market's ingest
/// behind at the open) used to hold every command until its backlog drained. Pause, play and
/// speed now apply during the stalled flush; a seek still waits for it, and commands keep their
/// order.
#[tokio::test(start_paused = true)]
async fn pause_and_play_apply_while_the_consumer_is_stalled() {
    let (_tmp, src) = fixture_source();
    let start = et(day(), 9, 30, 0, 0).unwrap();
    let (h, mut rx) = spawn(
        src.clone(),
        ReplayOptions {
            start: Some(start),
            speed: Speed::Times(600),
            autoplay: false,
            batch: 8,
            channel: 16,
            ..ReplayOptions::default()
        },
    )
    .unwrap();
    // Let the initial seek through, then stop reading.
    let mut seen = Vec::new();
    while let Some(i) = rx.recv().await {
        let done = matches!(&i, FeedItem::Clock(c) if c.now == start);
        seen.push(i);
        if done {
            break;
        }
    }
    h.command(Command::Play).await.unwrap();
    // Paced at 600×: a wall second is ten market minutes, far more than 16 items.
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert!(h.state().playing);
    // Nobody reads: the driver is stuck in a flush. Pause still answers, promptly.
    let paused = tokio::time::timeout(Duration::from_millis(50), h.command(Command::Pause))
        .await
        .expect("pause answered during a stalled flush")
        .unwrap();
    assert!(!paused.playing);
    assert!(!h.state().playing);
    // A seek waits for the flush; play queued behind it stays behind it.
    let seek = tokio::spawn({
        let h = h.clone();
        async move { h.command(Command::Seek(start)).await }
    });
    let play = tokio::spawn({
        let h = h.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(1)).await;
            h.command(Command::Play).await
        }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !seek.is_finished(),
        "a seek waits for the flush it would interrupt"
    );
    assert!(
        !play.is_finished(),
        "play does not overtake the seek queued before it"
    );
    // Reading again: everything applies, in order, and the stream confirms the pause.
    let drain = tokio::spawn(async move {
        let mut clocks = Vec::new();
        while let Some(i) = rx.recv().await {
            if let FeedItem::Clock(c) = i {
                clocks.push(c);
            }
        }
        clocks
    });
    let s = seek.await.unwrap().unwrap();
    assert_eq!(s.now, start);
    let p = play.await.unwrap().unwrap();
    assert!(p.playing);
    drop(h);
    drain.abort();
    let _ = drain.await;
}

/// A replay that fell behind (its consumer stalled) paces on from where it is instead of
/// sprinting through the whole debt: the owner's clock froze, then jumped ten seconds at once.
#[tokio::test(start_paused = true)]
async fn a_replay_behind_wall_time_drops_the_debt_instead_of_sprinting() {
    let (_tmp, src) = fixture_source();
    let start = et(day(), 10, 0, 0, 0).unwrap();
    let (h, mut rx) = spawn(
        src.clone(),
        ReplayOptions {
            start: Some(start),
            speed: Speed::Times(60),
            autoplay: false,
            batch: 8,
            channel: 16,
            ..ReplayOptions::default()
        },
    )
    .unwrap();
    while let Some(i) = rx.recv().await {
        if matches!(&i, FeedItem::Clock(c) if c.now == start) {
            break;
        }
    }
    h.command(Command::Play).await.unwrap();
    // Nobody reads for 20 wall seconds: twenty market minutes at 60×.
    tokio::time::sleep(Duration::from_secs(20)).await;
    let drain = tokio::spawn(async move {
        let mut clocks = Vec::new();
        while let Some(i) = rx.recv().await {
            if let FeedItem::Clock(c) = i {
                clocks.push(c.now);
            }
        }
        clocks
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    let s = h.command(Command::Pause).await.unwrap();
    let ran = (s.now.nanos() - start.nanos()) / 1_000_000_000;
    // About one MAX_CATCH_UP (60 s at 60×) plus the half second since, not the 20 minutes owed.
    assert!(ran < 200, "the replay sprinted {ran} s after the stall");
    assert!(ran >= 30, "the replay did not go on ({ran} s)");
    drop(h);
    drain.abort();
}
