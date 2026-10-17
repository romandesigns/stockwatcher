//! Provider parity: the same archived events through the ReplayProvider and through a
//! LiveProvider-shaped feed produce identical chart-engine output.

use chart_engine::wire::ChartUpdate;
use chart_engine::{ChartEngine, Timeframe};
use chrono::NaiveDate;
use market_archive::fixture::write_fixture_day;
use market_archive::{Archive, ArchiveConfig};
use market_events::MarketEvent;
use market_providers::{
    ChannelFeed, ClockMode, FeedItem, LiveOptions, LiveProvider, MarketDataProvider,
    ProviderConfig, ProviderError, ReplayProvider, Speed,
};
use market_replay::{ArchiveDaySource, EventSource, Merger, ReplayOptions, day_bounds};
use std::sync::Arc;
use tokio::sync::mpsc;

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 15).unwrap()
}

/// Feeds items into an engine (rebuilding on Reset) and returns the tape rows it produced.
struct Consumer {
    engine: ChartEngine,
    tape: Vec<ChartUpdate>,
}

impl Consumer {
    fn new() -> Self {
        Consumer {
            engine: ChartEngine::default(),
            tape: Vec::new(),
        }
    }

    fn take(&mut self, item: &FeedItem) {
        match item {
            FeedItem::Reset(_) => *self = Consumer::new(),
            FeedItem::Event(e) => self.tape.extend(
                self.engine
                    .apply(e)
                    .into_iter()
                    .filter(|u| matches!(u, ChartUpdate::Tape(_))),
            ),
            FeedItem::Advance(t) => {
                self.engine.advance_clock(*t);
            }
            _ => {}
        }
    }
}

#[tokio::test]
async fn replay_and_live_shaped_feed_produce_identical_engine_output() {
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
    let source: Arc<dyn EventSource> = Arc::new(ArchiveDaySource::new(archive, day()).unwrap());
    let end = day_bounds(day()).unwrap().1;

    // Replay provider, max speed, whole day.
    let replay = Box::new(ReplayProvider::new(
        source.clone(),
        ReplayOptions {
            speed: Speed::Max,
            ..ReplayOptions::default()
        },
    ));
    assert_eq!(replay.mode(), ClockMode::Replay);
    let mut s = replay.start().unwrap();
    assert!(s.info.fixture && s.control.is_some());
    let mut a = Consumer::new();
    while let Some(item) = s.items.recv().await {
        a.take(&item);
        if matches!(&item, FeedItem::Clock(c) if c.finished) {
            break;
        }
    }

    // Live-shaped: the same events arrive over a channel in delivery order, stamped with their
    // delivery time as the receive time (what a recorder would have written).
    let (tx, rx) = mpsc::channel::<MarketEvent>(1024);
    let live = Box::new(LiveProvider::with_feed(
        Box::new(ChannelFeed {
            label: "parity".into(),
            symbols: syms.clone(),
            rx,
        }),
        LiveOptions {
            idle_tick: None,
            ..LiveOptions::default()
        },
    ));
    assert_eq!(live.mode(), ClockMode::Live);
    let mut ls = live.start().unwrap();
    assert!(ls.control.is_none());
    let src = source.clone();
    let producer = tokio::spawn(async move {
        let mut m = Merger::new(src.open().unwrap());
        let mut n = 0;
        while let Some((d, mut e)) = m.pop() {
            let r = Some(market_events::UnixNanos(d));
            match &mut e {
                MarketEvent::Trade(t) => t.receive_ts = r,
                MarketEvent::Quote(q) => q.receive_ts = r,
                MarketEvent::Correction(c) => c.receive_ts = r,
                MarketEvent::Cancel(c) => c.receive_ts = r,
                MarketEvent::Status(s) => s.receive_ts = r,
            }
            tx.send(e).await.unwrap();
            n += 1;
        }
        n
    });
    let mut b = Consumer::new();
    while let Some(item) = ls.items.recv().await {
        b.take(&item);
    }
    let sent = producer.await.unwrap();
    assert!(sent > 60_000);

    // Both reach the end of the day.
    a.engine.advance_clock(end);
    b.engine.advance_clock(end);
    assert_eq!(a.tape.len(), b.tape.len());
    assert_eq!(
        a.tape, b.tape,
        "tape rows (how every print was counted) differ"
    );
    for sym in &syms {
        for tf in Timeframe::ALL {
            assert_eq!(
                a.engine.snapshot(sym, tf, None),
                b.engine.snapshot(sym, tf, None),
                "{sym} {tf}"
            );
        }
        assert_eq!(a.engine.quality(sym), b.engine.quality(sym));
        for sess in a.engine.sessions(sym) {
            assert_eq!(
                a.engine.profile(sym, sess, None),
                b.engine.profile(sym, sess, None)
            );
        }
        // Non-vacuous: the fixture's late, duplicate, correction and cancel paths were exercised.
        let q: Vec<_> = a.engine.quality(sym).into_iter().map(|(_, q)| q).collect();
        assert!(q.iter().map(|q| q.late_excluded).sum::<u64>() > 0, "{sym}");
        assert!(
            q.iter().map(|q| q.cancels_applied).sum::<u64>() == 1,
            "{sym}"
        );
        assert!(
            q.iter().map(|q| q.corrections_applied).sum::<u64>() == 1,
            "{sym}"
        );
    }
}

#[tokio::test]
async fn live_mode_without_a_client_is_not_configured() {
    let cfg = ProviderConfig::from_lookup(|k| {
        (k == "MARKET_DATA_MODE" || k == "LIVE_PROVIDER_ENABLED").then(|| {
            if k == "MARKET_DATA_MODE" {
                "live".to_string()
            } else {
                "1".to_string()
            }
        })
    })
    .unwrap();
    let p = market_providers::from_config(&cfg).unwrap();
    assert_eq!(p.mode(), ClockMode::Live);
    match p.start() {
        Err(ProviderError::NotConfigured(m)) => assert!(m.contains("not implemented"), "{m}"),
        Err(e) => panic!("unexpected {e}"),
        Ok(_) => panic!("live provider started without a client"),
    }
}

#[tokio::test]
async fn replay_mode_with_an_empty_archive_reports_no_data() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("empty").to_string_lossy().into_owned();
    let cfg = ProviderConfig::from_lookup(|k| (k == "ARCHIVE_ROOT").then(|| root.clone())).unwrap();
    match market_providers::from_config(&cfg) {
        Err(ProviderError::NoData(_)) => {}
        Err(e) => panic!("unexpected {e}"),
        Ok(_) => panic!("replay started on an empty archive"),
    }
}
