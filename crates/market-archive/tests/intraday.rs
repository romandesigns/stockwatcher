//! The intraday diagnostic curve producer over a written archive day.
//!
//! The curves are the denominator for the same-time and recent-window RVOL **diagnostics**.
//! `stock_context::IntradayBaseline` has been in the engine since the pillars went in with no
//! producer anywhere, so those measures were permanently `None` (the audit's F5/F10). Nothing
//! here changes the RVOL pillar, its 30-session daily denominator, the diagnostic's minimum
//! number of sessions, or what the panel admits.

use chrono::NaiveDate;
use market_archive::intraday::{
    CURVE_RULES_VERSION, CurveFidelity, MinuteVolumeSeries, STEP_SECS, derive_curve,
    intraday_baseline,
};
use market_archive::{Archive, ArchiveConfig, FsyncPolicy, ImportProvenance};
use market_events::session::et;
use market_events::{
    Cancel, CancelAction, Correction, MarketEvent, Price, Source, Trade, UnixNanos,
};

const SYM: &str = "AAAA";

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 15).expect("valid")
}

fn archive(dir: &std::path::Path) -> Archive {
    Archive::new(ArchiveConfig {
        root: dir.join("archive"),
        retention_days: 0,
        min_free_bytes: 0,
    })
}

/// ET nanoseconds on the fixture day.
fn t(h: u32, m: u32, s: u32) -> i64 {
    et(day(), h, m, s, 0).expect("valid").nanos()
}

/// Seconds since 04:00 ET.
fn off(h: u32, m: u32, s: u32) -> i64 {
    (t(h, m, s) - t(4, 0, 0)) / 1_000_000_000
}

fn trade(seq: u64, exec: i64, size: u64, conditions: &[&str]) -> MarketEvent {
    let t = Trade::new(SYM, UnixNanos(exec), Price::from_cents(500), size, seq)
        .with_conditions(conditions.iter().copied());
    MarketEvent::Trade(Trade {
        source: Source::AlpacaSip,
        exchange: "V".into(),
        ..t
    })
}

/// Writes `events` as the day's file for `SYM` and returns the day manifest. `imported` marks the
/// entry as a final provider import, which is what makes a session usable as prior history.
fn write_day(a: &Archive, events: &[MarketEvent], imported: bool) -> market_archive::DayManifest {
    let mut w = a
        .writer(day(), SYM, FsyncPolicy::OnFinalize)
        .expect("writer");
    for e in events {
        w.append(e).expect("append");
    }
    let mut entry = w.finalize().expect("finalize");
    if imported {
        entry.import = Some(ImportProvenance {
            provider: "alpaca".into(),
            dataset: "trades".into(),
            endpoint: format!("/v2/stocks/{SYM}/trades"),
            feed: "sip".into(),
            start: "2026-10-15T08:00:00Z".into(),
            end: "2026-10-16T00:00:00Z".into(),
            imported_at: "2026-10-16T02:00:00Z".into(),
            pages: 1,
            complete: true,
            updates: Default::default(),
            out_of_order: 0,
        });
    }
    a.commit(day(), entry).expect("commit")
}

#[test]
fn a_curve_counts_the_same_prints_the_live_numerator_counts() {
    let dir = tempfile::tempdir().expect("tmp");
    let a = archive(dir.path());
    let events = vec![
        // 09:30:01, 100 regular shares.
        trade(1, t(9, 30, 1), 100, &["@"]),
        // 09:30:02, the same print again (a reconnect replay): a duplicate, not 100 more.
        trade(1, t(9, 30, 1), 100, &["@"]),
        // 09:35:00, 200 shares on an official-close condition: no volume under the policy
        // (the same rule the live numerator applies, market_events::conditions).
        trade(2, t(9, 35, 0), 200, &["M"]),
        // 09:40:00, 300 regular shares.
        trade(3, t(9, 40, 0), 300, &["@"]),
    ];
    let m = write_day(&a, &events, true);
    let r = derive_curve(&a, &m, SYM, day(), 42)
        .expect("derive")
        .expect("a curve for a symbol the day has");

    assert_eq!(r.rules, CURVE_RULES_VERSION);
    assert_eq!(r.step_secs, STEP_SECS);
    assert_eq!(
        r.observed_at, 42,
        "the derivation time, never a market time"
    );
    assert_eq!(
        r.total(),
        400,
        "100 + 300; the excluded condition and the duplicate are out"
    );
    assert_eq!(r.coverage.counted, 2);
    assert_eq!(r.coverage.duplicates, 1);
    assert_eq!(r.coverage.excluded, 1);
    assert_eq!(r.coverage.first_ts, Some(t(9, 30, 1)));
    assert_eq!(r.coverage.last_ts, Some(t(9, 40, 0)));

    // The curve reads the cumulative at a decision boundary with no interpolation.
    assert_eq!(r.at(off(9, 30, 0)), 0, "before the first print");
    assert_eq!(r.at(off(9, 30, 15)), 100);
    assert_eq!(
        r.at(off(9, 39, 45)),
        100,
        "the excluded print added nothing"
    );
    assert_eq!(r.at(off(9, 40, 15)), 400);
    assert!(
        r.cum
            .windows(2)
            .all(|w| w[0].0 < w[1].0 && w[0].1 <= w[1].1),
        "ascending in both axes: {:?}",
        r.cum
    );
    assert_eq!(
        r.cum[0],
        (0, 0),
        "a lookup before any print is a covered zero"
    );

    // Provenance: the file and the manifest's own hash of it, so the curve is reproducible.
    let entry = m.symbols.get(SYM).expect("the entry");
    assert_eq!(r.source.file, entry.file);
    assert_eq!(r.source.sha256, entry.sha256);
    assert_eq!(r.source.events, entry.events);
    assert_eq!(r.source.fidelity, CurveFidelity::ImportedHistory);
    assert!(r.complete && r.usable_as_history());
}

#[test]
fn a_cancel_and_a_correction_move_the_curve_and_an_ambiguous_one_is_counted() {
    let dir = tempfile::tempdir().expect("tmp");
    let a = archive(dir.path());
    let cancel = MarketEvent::Cancel(Cancel {
        symbol: SYM.into(),
        event_ts: UnixNanos(t(9, 31, 0)),
        receive_ts: None,
        source: Source::AlpacaSip,
        exchange: "V".into(),
        tape: Default::default(),
        trade_id: 1,
        price: Price::from_cents(500),
        size: 100,
        action: CancelAction::Cancel,
    });
    let correction = MarketEvent::Correction(Correction {
        symbol: SYM.into(),
        event_ts: UnixNanos(t(9, 41, 0)),
        receive_ts: None,
        source: Source::AlpacaSip,
        exchange: "V".into(),
        tape: Default::default(),
        original_id: 3,
        original_price: Price::from_cents(500),
        original_size: 300,
        original_conditions: vec!["@".into()],
        corrected_id: 3,
        corrected_price: Price::from_cents(500),
        corrected_size: 50,
        corrected_conditions: vec!["@".into()],
    });
    // Names a print that was never in the file: resolvable to nothing, so counted, not guessed.
    let orphan = MarketEvent::Cancel(Cancel {
        symbol: SYM.into(),
        event_ts: UnixNanos(t(9, 42, 0)),
        receive_ts: None,
        source: Source::AlpacaSip,
        exchange: "V".into(),
        tape: Default::default(),
        trade_id: 99,
        price: Price::from_cents(500),
        size: 10,
        action: CancelAction::Cancel,
    });
    let events = vec![
        trade(1, t(9, 30, 1), 100, &["@"]),
        cancel,
        trade(3, t(9, 40, 0), 300, &["@"]),
        correction,
        orphan,
    ];
    let m = write_day(&a, &events, true);
    let r = derive_curve(&a, &m, SYM, day(), 1)
        .expect("derive")
        .expect("a curve");

    assert_eq!(r.at(off(9, 30, 15)), 100, "counted when it printed");
    assert_eq!(
        r.at(off(9, 31, 15)),
        0,
        "and taken back when it was cancelled"
    );
    assert_eq!(r.at(off(9, 40, 15)), 300);
    assert_eq!(r.at(off(9, 41, 15)), 50, "corrected down to 50");
    assert_eq!(r.total(), 50);
    assert_eq!(r.coverage.revisions_applied, 2);
    assert_eq!(r.coverage.unresolved_revisions, 1, "the orphan, counted");
}

#[test]
fn a_live_recording_is_carried_but_never_counted_as_prior_history() {
    let dir = tempfile::tempdir().expect("tmp");
    let a = archive(dir.path());
    let m = write_day(&a, &[trade(1, t(9, 30, 1), 100, &["@"])], false);
    let r = derive_curve(&a, &m, SYM, day(), 1)
        .expect("derive")
        .expect("a curve");
    assert_eq!(r.source.fidelity, CurveFidelity::RecordedLive);
    assert!(
        !r.complete && !r.usable_as_history(),
        "provisional: its own gaps are not provable from the file"
    );
    assert_eq!(
        r.total(),
        100,
        "the volume is still reported, honestly labelled"
    );
}

#[test]
fn a_symbol_the_day_does_not_have_gets_no_curve_at_all() {
    let dir = tempfile::tempdir().expect("tmp");
    let a = archive(dir.path());
    let m = write_day(&a, &[trade(1, t(9, 30, 1), 100, &["@"])], true);
    assert!(
        derive_curve(&a, &m, "ZZZZ", day(), 1)
            .expect("derive")
            .is_none(),
        "missing, not zero"
    );
}

#[test]
fn curves_round_trip_and_assemble_into_the_engines_diagnostic_baseline() {
    let dir = tempfile::tempdir().expect("tmp");
    let a = archive(dir.path());
    let m = write_day(&a, &[trade(1, t(9, 30, 1), 100, &["@"])], true);
    let r = derive_curve(&a, &m, SYM, day(), 7)
        .expect("derive")
        .expect("a curve");
    a.write_intraday_curve(&r).expect("write");
    // Idempotent: the same derivation replaces itself rather than accumulating.
    a.write_intraday_curve(&r).expect("write again");
    assert_eq!(
        a.read_intraday_curve(day(), SYM).expect("read").as_ref(),
        Some(&r)
    );
    assert!(
        a.read_intraday_curve(day(), "ZZZZ")
            .expect("read")
            .is_none()
    );

    let dates = [day() - chrono::Duration::days(1), day()];
    let stored = a.read_intraday_curves(SYM, &dates).expect("read");
    assert_eq!(stored.len(), 1, "a missing date is simply absent");

    let b = intraday_baseline(&stored, SYM).expect("a baseline");
    assert_eq!(b.instrument, stock_context::InstrumentId::symbol(SYM));
    assert_eq!(b.observed_at, 7, "the latest derivation time");
    assert_eq!(b.sessions.len(), 1);
    assert!(b.sessions[0].complete);
    assert_eq!(b.sessions[0].at(off(9, 30, 15)), 100);
    assert!(
        b.id.starts_with(CURVE_RULES_VERSION),
        "the rules version is in the record id: {}",
        b.id
    );

    // Nothing stored: no baseline is invented.
    assert!(intraday_baseline(&[], SYM).is_none());
}

/// A provider's one-minute series is a separate, coarser thing with its own name. There is no
/// conversion into a curve anywhere, because spreading a minute's volume over four
/// fifteen-second points would invent data the provider never gave.
#[test]
fn provider_minute_bars_are_a_separate_series_and_never_a_curve() {
    let s = MinuteVolumeSeries {
        symbol: SYM.into(),
        date: day(),
        provider: "fmp".into(),
        minutes: vec![(off(9, 30, 0), 1_000), (off(9, 31, 0), 2_000)],
        observed_at: 1,
    };
    // It is not an IntradayCurveRecord and cannot be read as one: the field name differs, the
    // resolution differs, and no `From`/`Into` exists between them.
    let json = serde_json::to_string(&s).expect("serializes");
    assert!(
        json.contains("minutes") && !json.contains("\"cum\""),
        "{json}"
    );
    assert!(
        serde_json::from_str::<market_archive::IntradayCurveRecord>(&json).is_err(),
        "a minute series must not deserialize as a curve"
    );
}
