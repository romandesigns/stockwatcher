//! The intraday diagnostic curve producer over a written archive day.
//!
//! The curves are the denominator for the same-time and recent-window RVOL **diagnostics**.
//! `stock_context::IntradayBaseline` has been in the engine since the pillars went in with no
//! producer anywhere, so those measures were permanently `None` (the audit's F5/F10). Nothing
//! here changes the RVOL pillar, its 30-session daily denominator, the diagnostic's minimum
//! number of sessions, or what the panel admits.

use chrono::NaiveDate;
use market_archive::intraday::{
    Basis, CURVE_RULES_VERSION, CurveFidelity, MinuteVolumeSeries, STEP_SECS, derive_curve,
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

/// A restatement basis with `factors` and no expiry (bar-verified), observed at `observed_at`.
fn basis_of<const N: usize>(factors: [(NaiveDate, f64); N], observed_at: i64) -> Basis {
    Basis {
        factors: factors.into_iter().collect(),
        evidence: stock_context::ActionsEvidence {
            source: stock_context::ActionsSource::BarVerified,
            ..Default::default()
        },
        observed_at,
        source: "fixture:baseline".into(),
    }
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
    assert!(r.collection_complete && r.usable_as_history());
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
        !r.collection_complete && !r.usable_as_history(),
        "provisional: its own gaps are not provable from the file"
    );
    assert_eq!(r.unusable_reason(), Some("provisional_recording"));
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

    let basis = basis_of([(day(), 1.0)], 9);
    let b = intraday_baseline(&stored, SYM, &basis)
        .baseline
        .expect("a baseline");
    assert_eq!(b.instrument, stock_context::InstrumentId::symbol(SYM));
    assert_eq!(
        b.observed_at, 9,
        "the latest of EVERY input: the curve was derived at 7, the basis observed at 9"
    );
    assert_eq!(b.sessions.len(), 1);
    assert!(b.sessions[0].complete);
    assert_eq!(b.sessions[0].at(off(9, 30, 15)), 100);
    assert!(
        b.id.starts_with(CURVE_RULES_VERSION),
        "the rules version is in the record id: {}",
        b.id
    );

    // Nothing stored: no baseline is invented.
    assert!(intraday_baseline(&[], SYM, &basis).baseline.is_none());
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

// --- review corrections: derivation completeness, window eligibility, share basis ------------

/// A curve whose input was collected whole but whose derivation could not place a correction or a
/// cancel is **not** a usable denominator.
///
/// Input collection completeness and derivation completeness are different questions. The import
/// reporting every page fetched says nothing about whether a revision found its print, and an
/// unapplied revision leaves the session's volume different from the volume that stands —
/// publishing a number from that is worse than publishing none.
#[test]
fn an_unapplied_revision_makes_a_complete_day_unusable() {
    let dir = tempfile::tempdir().expect("tmp");
    let a = archive(dir.path());
    // The import says the window was collected whole, and a cancel names a print that is not in
    // the file.
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
    let m = write_day(&a, &[trade(1, t(9, 30, 1), 100, &["@"]), orphan], true);
    let r = derive_curve(&a, &m, SYM, day(), 1)
        .expect("derive")
        .expect("a curve");
    assert!(r.collection_complete, "the input was collected whole");
    assert_eq!(r.coverage.unresolved_revisions, 1);
    assert_eq!(
        r.unusable,
        Some(market_archive::CurveUnusable::UnappliedRevisions)
    );
    assert!(!r.usable_as_history());
    assert_eq!(r.unusable_reason(), Some("unapplied_revisions"));

    // And the assembly leaves it out, with the reason, rather than offering a denominator.
    let asm = intraday_baseline(&[r], SYM, &basis_of([(day(), 1.0)], 1));
    assert_eq!(asm.usable, 0);
    assert_eq!(asm.suppressed.get("unapplied_revisions"), Some(&1));
    assert!(
        !asm.baseline.expect("carried").sessions[0].complete,
        "carried for the record, never counted as a prior session"
    );
}

/// A print just before the window is not in the curve, and a correction of it cannot put it
/// there.
///
/// Two separate traps. The offset was computed with integer division, which truncates toward
/// zero, so a print a millisecond before 04:00 divided to offset 0 and passed a `>= 0` check as
/// if it were inside the day. And the held print recorded only its condition eligibility, so a
/// correction added the corrected size to a total the original had never been part of.
#[test]
fn a_print_outside_the_window_stays_out_through_a_correction() {
    let dir = tempfile::tempdir().expect("tmp");
    let a = archive(dir.path());
    let before_open = t(4, 0, 0) - 1_000_000; // 03:59:59.999
    let correction = MarketEvent::Correction(Correction {
        symbol: SYM.into(),
        event_ts: UnixNanos(t(4, 1, 0)),
        receive_ts: None,
        source: Source::AlpacaSip,
        exchange: "V".into(),
        tape: Default::default(),
        original_id: 1,
        original_price: Price::from_cents(500),
        original_size: 100,
        original_conditions: vec!["@".into()],
        corrected_id: 1,
        corrected_price: Price::from_cents(500),
        corrected_size: 200,
        corrected_conditions: vec!["@".into()],
    });
    let events = vec![
        trade(1, before_open, 100, &["@"]),
        trade(2, t(4, 0, 1), 50, &["@"]),
        correction,
    ];
    let m = write_day(&a, &events, true);
    let r = derive_curve(&a, &m, SYM, day(), 1)
        .expect("derive")
        .expect("a curve");
    assert_eq!(
        r.total(),
        50,
        "only the 04:00:01 print is in the day; the correction of the 03:59:59.999 one is not"
    );
    assert_eq!(r.coverage.outside_window, 1);
    assert_eq!(r.coverage.counted, 1);
    assert_eq!(
        r.coverage.revisions_applied, 1,
        "it was placed, and changed nothing"
    );
    assert_eq!(r.at(0), 0, "nothing before the window's own start");
    assert!(r.usable_as_history());
}

/// Prior sessions are restated onto the evaluated date's share basis, exactly as the daily
/// denominator restates its own.
///
/// A curve holds the shares that session printed. After a 5-for-1 split the same participation
/// prints five times the share count, so comparing today's cumulative with a prior session's raw
/// shares reads as five times the volume — a fabricated same-time RVOL, in the direction that
/// invents opportunities. A date with no factor had no verifiable basis and is suppressed rather
/// than restated by a guess.
#[test]
fn prior_sessions_are_restated_onto_todays_share_basis() {
    let dir = tempfile::tempdir().expect("tmp");
    let a = archive(dir.path());
    let m = write_day(&a, &[trade(1, t(9, 30, 1), 1_000, &["@"])], true);
    let r = derive_curve(&a, &m, SYM, day(), 5)
        .expect("derive")
        .expect("a curve");
    assert_eq!(r.total(), 1_000, "the shares that session printed");

    // A 5-for-1 split since: one share then is five shares now, so the session's 1,000 shares
    // are 5,000 on today's basis — the factor the daily denominator used.
    let asm = intraday_baseline(std::slice::from_ref(&r), SYM, &basis_of([(day(), 5.0)], 5));
    let b = asm.baseline.expect("a baseline");
    assert_eq!(asm.usable, 1);
    assert!(asm.suppressed.is_empty());
    assert!(b.sessions[0].complete);
    assert_eq!(
        b.sessions[0].at(off(9, 30, 15)),
        5_000,
        "restated, so it is not read as five times the volume"
    );

    // Same basis: the stored counts, untouched.
    let asm = intraday_baseline(std::slice::from_ref(&r), SYM, &basis_of([(day(), 1.0)], 5));
    assert_eq!(
        asm.baseline.expect("a baseline").sessions[0].at(off(9, 30, 15)),
        1_000
    );

    // No factor for the date: not comparable at all, so suppressed with the reason.
    let asm = intraday_baseline(std::slice::from_ref(&r), SYM, &Basis::default());
    assert_eq!(asm.usable, 0);
    assert_eq!(
        asm.suppressed.get(market_archive::INCOMPARABLE_BASIS),
        Some(&1)
    );
    assert!(!asm.baseline.expect("carried").sessions[0].complete);
}

/// A factor verified later today cannot reach an earlier decision.
///
/// The engine joins evidence by `observed_at <= cutoff`, so a record must be invisible until all
/// of it existed. The assembly used to stamp the record with only the curves' derivation times,
/// which backdated a basis an announcement check verified hours later — future knowledge reaching
/// an earlier cutoff, the one rule the whole pipeline rests on.
#[test]
fn a_later_verified_basis_cannot_reach_an_earlier_cutoff() {
    let dir = tempfile::tempdir().expect("tmp");
    let a = archive(dir.path());
    let m = write_day(&a, &[trade(1, t(9, 30, 1), 1_000, &["@"])], true);
    // The curves were derived overnight; the basis was only verified at 09:00 today.
    let derived_at = t(4, 0, 0);
    let verified_at = t(9, 0, 0);
    let r = derive_curve(&a, &m, SYM, day(), derived_at)
        .expect("derive")
        .expect("a curve");
    let b = intraday_baseline(&[r], SYM, &basis_of([(day(), 1.0)], verified_at))
        .baseline
        .expect("a baseline");
    assert_eq!(
        b.observed_at, verified_at,
        "the later of the two, so the record is invisible before the basis existed"
    );

    // A store joins it by that time: a 04:00 decision cannot see it, a 09:00 one can.
    let mut store = stock_context::EvidenceStore::default();
    store.add_intraday(b);
    let inst = stock_context::InstrumentId::symbol(SYM);
    assert!(
        store.intraday(&inst, verified_at - 1).is_none(),
        "not yet: the basis had not been verified"
    );
    assert!(store.intraday(&inst, verified_at).is_some());
}

/// An expired restatement basis suppresses the diagnostic, and a fresh check brings it back.
///
/// The factors can rest on a check that found no known action, which the coverage policy permits
/// only while it is fresh. Past that horizon nothing is verifying the share basis the prior
/// sessions were restated onto — and a stale basis can be a whole split out, which is the
/// direction that invents volume. The strict daily pillar and the diagnostic's minimum number of
/// sessions are untouched by any of this.
#[test]
fn an_expired_restatement_basis_suppresses_the_diagnostic() {
    let dir = tempfile::tempdir().expect("tmp");
    let a = archive(dir.path());
    let m = write_day(&a, &[trade(1, t(9, 30, 1), 1_000, &["@"])], true);
    let r = derive_curve(&a, &m, SYM, day(), t(4, 0, 0))
        .expect("derive")
        .expect("a curve");

    // A basis resting on an 08:00 check: good for 90 minutes under the v1 coverage policy.
    let checked = t(8, 0, 0);
    let horizon = checked + 5_400 * 1_000_000_000;
    let assumed = Basis {
        factors: [(day(), 1.0)].into_iter().collect(),
        evidence: stock_context::ActionsEvidence {
            source: stock_context::ActionsSource::ProviderCheckedNoKnownAction,
            assumption: Some("unchanged_share_basis_assumed".into()),
            coverage_at: Some(checked),
            valid_until: Some(horizon),
            ..Default::default()
        },
        observed_at: checked,
        source: "fixture:baseline".into(),
    };
    let b = intraday_baseline(std::slice::from_ref(&r), SYM, &assumed)
        .baseline
        .expect("a baseline");
    assert_eq!(
        b.basis.valid_until,
        Some(horizon),
        "the horizon travels on it"
    );
    assert!(b.basis.valid_at(horizon));
    assert!(
        !b.basis.valid_at(horizon + 1),
        "and the engine refuses it past the horizon"
    );

    // A fresh check: a new basis, a later observation, a later horizon.
    let again = horizon + 60 * 1_000_000_000;
    let fresh = Basis {
        evidence: stock_context::ActionsEvidence {
            coverage_at: Some(again),
            valid_until: Some(again + 5_400 * 1_000_000_000),
            ..assumed.evidence.clone()
        },
        observed_at: again,
        ..assumed.clone()
    };
    let b2 = intraday_baseline(std::slice::from_ref(&r), SYM, &fresh)
        .baseline
        .expect("a baseline");
    assert_eq!(b2.observed_at, again, "visible from the fresh check on");
    assert!(b2.basis.valid_at(again + 1));

    // A finding, not an assumption, has no horizon at all.
    let verified = intraday_baseline(std::slice::from_ref(&r), SYM, &basis_of([(day(), 1.0)], 1))
        .baseline
        .expect("a baseline");
    assert_eq!(verified.basis.valid_until, None);
    assert!(verified.basis.valid_at(i64::MAX));
}
