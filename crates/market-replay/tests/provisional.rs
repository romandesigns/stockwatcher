//! Replaying a live-recorded (provisional) day: refused while it is still being written, opened
//! from the recording once sealed, and from the import once the import has replaced it.

use chrono::NaiveDate;
use market_archive::{Archive, ArchiveConfig, LiveWriter};
use market_events::{MarketEvent, Price, Trade, UnixNanos};
use market_replay::{ArchiveDaySource, EventSource};

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 15).unwrap()
}

fn count(src: &ArchiveDaySource) -> usize {
    src.open()
        .unwrap()
        .into_iter()
        .map(|s| s.filter(Result::is_ok).count())
        .sum()
}

#[test]
fn a_recorded_day_replays_once_sealed_and_then_as_imported() {
    let tmp = tempfile::tempdir().unwrap();
    let a = Archive::new(ArchiveConfig {
        root: tmp.path().join("archive"),
        retention_days: 0,
        min_free_bytes: 0,
    });
    let t0 = market_events::session::et(day(), 10, 0, 0, 0).unwrap();
    let mut w = LiveWriter::open(&a, day(), 0, "test").unwrap();
    for i in 0..10u64 {
        let ts = UnixNanos(t0.0 + i as i64 * 1_000_000_000);
        let mut t = Trade::new("SMPL", ts, Price::from_cents(500), 100, i + 1);
        t.receive_ts = Some(ts);
        w.append(&MarketEvent::Trade(t)).unwrap();
    }
    w.finish().unwrap();

    let e = ArchiveDaySource::new(a.clone(), day()).err().unwrap();
    assert!(e.contains("still being recorded"), "{e}");

    a.seal_live(day()).unwrap();
    let src = ArchiveDaySource::new(a.clone(), day()).unwrap();
    assert!(src.provisional());
    assert!(src.label().contains("provisional"), "{}", src.label());
    assert_eq!(src.symbols(), ["SMPL"]);
    assert_eq!(count(&src), 10);

    // The import lands while the replay is open on the recording: the next open (a seek) reads
    // the import instead of the deleted recording.
    a.mark_importing(day()).unwrap();
    market_archive::fixture::write_fixture_day(&a, day(), &["SMPL".to_string()], 7).unwrap();
    a.finish_import(day()).unwrap();
    assert!(count(&src) > 10_000, "the imported (fixture) day");
    assert!(!ArchiveDaySource::new(a, day()).unwrap().provisional());
}
