//! The offline volume-compatibility harness on a fixture day (the only kind of archived day a
//! test can make) with hand-set provider volumes. Fixture data is never market evidence: the
//! harness refuses it unless told otherwise, which is the first thing checked.

use chrono::NaiveDate;
use market_archive::fixture::{DEFAULT_SEED, write_fixture_day};
use market_archive::{Adjustment, Archive, ArchiveConfig, BarFetch, DailyBar, RawDayBars};
use market_providers::compat::{CompatOptions, compare, our_volume};

fn d(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

#[test]
fn ours_vs_provider_by_session_and_condition_with_the_owner_thresholds() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_string_lossy().to_string();
    let a = Archive::new(
        ArchiveConfig::from_lookup(|k| match k {
            "ARCHIVE_ROOT" => Some(root.clone()),
            "ARCHIVE_MIN_FREE_MB" => Some("0".into()),
            _ => None,
        })
        .unwrap(),
    );
    let day = d("2026-10-15");
    let syms: Vec<String> = ["AAA", "BBB", "CCC", "DDD"].map(String::from).to_vec();
    write_fixture_day(&a, day, &syms, DEFAULT_SEED).unwrap();

    // Provider volumes: AAA and BBB exactly ours, CCC 1.5 % above (inside ±2 %), DDD 10 % above.
    let ours: Vec<u64> = syms
        .iter()
        .map(|s| our_volume(&a, day, s).unwrap().ours)
        .collect();
    assert!(ours.iter().all(|v| *v > 0));
    let fetch = BarFetch {
        retrieved_at: chrono::DateTime::parse_from_rfc3339("2026-10-16T02:00:00Z")
            .unwrap()
            .to_utc(),
        provider: "alpaca".into(),
        feed: "sip".into(),
        adjustment: Adjustment::Raw,
        timeframe: "1Day".into(),
        from: day,
        to: day,
        symbols: 4,
        symbols_per_request: 200,
        requests: 1,
        asof: None,
        command: "test".into(),
    };
    let mut raw = RawDayBars::new(day, fetch);
    for (i, s) in syms.iter().enumerate() {
        let v = match i {
            2 => (ours[i] as f64 * 1.015).round() as u64,
            3 => (ours[i] as f64 * 1.10).round() as u64,
            _ => ours[i],
        };
        raw.bars.insert(
            s.clone(),
            DailyBar {
                o: None,
                h: None,
                l: None,
                c: 3.0,
                v: Some(v),
                n: None,
                vw: None,
            },
        );
    }
    a.merge_raw_bars([raw]).unwrap();

    let opt = |include_fixture, min_sample| CompatOptions {
        from: day,
        to: day,
        sample: 500,
        min_sample,
        include_fixture,
    };
    // Fixture days are refused by default.
    assert_eq!(compare(&a, &opt(false, 1)).unwrap().symbol_days, 0);

    let r = compare(&a, &opt(true, 1)).unwrap();
    assert_eq!(r.symbol_days, 4);
    assert_eq!(r.within_2pct, 0.75);
    assert_eq!(r.within_1pct, 0.5);
    assert_eq!(r.verdict, "incompatible", "75 % < the required 95 %");
    // Per session, our volume adds up to our total; only pre/regular/after are counted.
    assert_eq!(r.by_session.values().sum::<u64>(), r.ours_total);
    assert!(
        r.by_session
            .keys()
            .all(|k| ["pre", "regular", "after"].contains(&k.as_str()))
    );
    // Every print is attributed to its condition codes, counted or not.
    assert!(!r.by_condition.is_empty());
    assert!(r.note.contains("PR #51"));
    // The default minimum sample: 4 symbol-days decide nothing.
    assert_eq!(
        compare(&a, &opt(true, 100)).unwrap().verdict,
        "insufficient_sample"
    );

    // Within tolerance everywhere → compatible.
    let mut raw = a.read_raw_bars(day).unwrap().unwrap();
    raw.bars.get_mut("DDD").unwrap().v = Some(ours[3]);
    a.write_raw_bars(&raw).unwrap();
    assert_eq!(compare(&a, &opt(true, 1)).unwrap().verdict, "compatible");
}
