//! The offline volume study on fixture days (the only kind of archived day a test can make) with
//! hand-set provider volumes. Fixture data is never market evidence: the harness refuses it unless
//! told otherwise, which is the first thing checked.

use chrono::NaiveDate;
use market_archive::fixture::{DEFAULT_SEED, write_fixture_day};
use market_archive::{Adjustment, Archive, ArchiveConfig, BarFetch, DailyBar, RawDayBars};
use market_providers::compat::{CompatOptions, compare, our_volume};

fn d(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

fn raw_day(day: NaiveDate, rows: &[(&str, f64, u64)]) -> RawDayBars {
    let fetch = BarFetch {
        retrieved_at: chrono::DateTime::parse_from_rfc3339("2026-10-20T02:00:00Z")
            .unwrap()
            .to_utc(),
        provider: "alpaca".into(),
        feed: "sip".into(),
        adjustment: Adjustment::Raw,
        timeframe: "1Day".into(),
        from: day,
        to: day,
        symbols: rows.len(),
        symbols_per_request: 200,
        requests: 1,
        asof: None,
        command: "test".into(),
    };
    let mut raw = RawDayBars::new(day, fetch);
    for (s, c, v) in rows {
        raw.bars.insert(
            s.to_string(),
            DailyBar {
                o: None,
                h: None,
                l: None,
                c: *c,
                v: Some(*v),
                n: Some(100),
                vw: None,
            },
        );
    }
    raw
}

#[test]
fn provider_vs_eligible_volume_with_errors_strata_exclusions_and_the_owner_thresholds() {
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
    let (d1, d2) = (d("2026-10-14"), d("2026-10-15"));
    let syms: Vec<String> = ["AAA", "BBB", "CCC", "DDD"].map(String::from).to_vec();
    write_fixture_day(&a, d1, &syms, DEFAULT_SEED).unwrap();
    write_fixture_day(&a, d2, &syms, DEFAULT_SEED).unwrap();
    let ours: Vec<u64> = syms
        .iter()
        .map(|s| our_volume(&a, d2, s).unwrap().volume)
        .collect();
    assert!(ours.iter().all(|v| *v > 0));
    // d2: AAA/BBB exact, CCC +1.5 % (inside ±2 %), DDD +10 %; AAA up 20 % on the day (gainer).
    a.merge_raw_bars([raw_day(
        d2,
        &[
            ("AAA", 3.6, ours[0]),
            ("BBB", 3.0, ours[1]),
            ("CCC", 3.0, (ours[2] as f64 * 1.015).round() as u64),
            ("DDD", 3.0, (ours[3] as f64 * 1.10).round() as u64),
        ],
    )])
    .unwrap();
    // d1 lacks DDD's bar entirely: excluded with a reason, never a zero.
    let ours1: Vec<u64> = syms
        .iter()
        .map(|s| our_volume(&a, d1, s).unwrap().volume)
        .collect();
    a.merge_raw_bars([raw_day(
        d1,
        &[
            ("AAA", 3.0, ours1[0]),
            ("BBB", 3.0, ours1[1]),
            ("CCC", 3.0, ours1[2]),
        ],
    )])
    .unwrap();

    let opt = |fixture: bool, min_sample: usize| {
        let mut o = CompatOptions::new(d1, d2);
        o.min_sample = min_sample;
        o.include_fixture = fixture;
        o.min_sessions = 2;
        o
    };
    // Fixture days are refused by default, with the reason recorded.
    let r = compare(&a, &opt(false, 1)).unwrap();
    assert_eq!(r.symbol_days, 0);
    assert!(r.excluded.iter().any(|e| e.reason == "fixture_day"));

    let r = compare(&a, &opt(true, 1)).unwrap();
    assert_eq!(r.symbol_days, 7);
    assert!(r.excluded.iter().any(|e| e.date == d1
        && e.symbol.as_deref() == Some("DDD")
        && e.reason == "not_in_raw_bars"));
    // Errors: |Vp − Ve| / Ve, signed, absolute, quantiles.
    let max = r.relative_error.max.unwrap();
    assert!((max - 0.10).abs() < 1e-4, "{max}");
    assert!(
        r.signed_error_mean.unwrap() > 0.0,
        "the provider counts more here"
    );
    assert_eq!(r.within_2pct, 6.0 / 7.0);
    assert_eq!(
        r.verdict, "outside_engineering_tolerance",
        "6/7 < the required 95 %"
    );
    assert!(r.verdict_note.contains("not a certification"));
    assert!(r.requirements.contains("PR #51"));
    // Strata: the gainer is reported apart.
    let g = &r.by_stratum["relevance:gainer_10pct"];
    assert_eq!(g.symbol_days, 1);
    assert_eq!(g.relative_error.max, Some(0.0));
    assert!(r.by_stratum.contains_key("price:2to20"));
    // Per session: our volume adds up; only pre/regular/after counted.
    let total: u64 = r.rows.iter().map(|x| x.ours.volume).sum();
    assert_eq!(r.by_session.values().sum::<u64>(), total);
    assert!(r.trade_count_ratio.n > 0);
    // The minimum sample: 7 symbol-days decide nothing.
    assert_eq!(
        compare(&a, &opt(true, 100)).unwrap().verdict,
        "insufficient_sample"
    );
    // A fixed seed picks the same symbol-days.
    let again = compare(&a, &opt(true, 1)).unwrap();
    assert_eq!(
        again
            .rows
            .iter()
            .map(|x| (&x.date, &x.symbol))
            .collect::<Vec<_>>(),
        r.rows
            .iter()
            .map(|x| (&x.date, &x.symbol))
            .collect::<Vec<_>>()
    );

    // Everything within tolerance → within the engineering tolerance.
    let mut raw = a.read_raw_bars(d2).unwrap().unwrap();
    raw.bars.get_mut("DDD").unwrap().v = Some(ours[3]);
    a.write_raw_bars(&raw).unwrap();
    assert_eq!(
        compare(&a, &opt(true, 1)).unwrap().verdict,
        "within_engineering_tolerance"
    );
}
