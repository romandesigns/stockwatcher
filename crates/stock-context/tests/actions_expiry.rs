//! A corporate-action assumption expires on the clock, and recovers on a fresh check.
//!
//! The gain and RVOL pillars rest on knowing today's share basis. When that knowledge is
//! "a fully paginated provider check found no known action", it is an **assumption** the coverage
//! policy only permits while the check is fresh (`corporate-actions-coverage-v1`,
//! `freshness_secs`). The engine used to check only that `actions_checked_through` reached the
//! evaluated date, so once a record was in the store the assumption held for ever: if the
//! maintainer stopped checking — a real outage — a stale "no known action" kept supporting a PASS
//! that nothing was still verifying. `docs/stock-context/ross-panel-v1.md` says so out loud.
//!
//! The record now carries the horizon its assumption is valid to, and the engine refuses it past
//! that horizon with no new print and no new record: the flip is time-triggered. A `BarVerified`
//! or `AnnouncedAction` basis is a finding about what happened, not an assumption about what did
//! not, and does not expire.

use chrono::NaiveDate;
use market_events::session::et;
use market_events::{Price, Trade, UnixNanos};
use stock_context::*;

const VOL: Verdict = Verdict {
    ohlc: true,
    volume: true,
};
const S: i64 = 1_000_000_000;
/// The v1 coverage policy's freshness limit (90 minutes).
const FRESH: i64 = 5_400 * S;

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 16).unwrap()
}
fn prev() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 15).unwrap()
}
fn t(h: u32, m: u32, s: u32) -> i64 {
    et(day(), h, m, s, 0).unwrap().nanos()
}
fn inst(sym: &str) -> InstrumentId {
    InstrumentId::symbol(sym)
}

fn engine() -> Engine {
    Engine::new(
        Policy::v1(),
        RunInfo {
            run_id: "fixture".into(),
            source: "replay".into(),
            fidelity: Fidelity::SyntheticFixture,
            numerator_coverage: Coverage::Complete,
        },
    )
}

/// The provenance of a record that rests on a fresh check finding no known action, retrieved at
/// `checked_at` and therefore good until `checked_at + FRESH`.
fn assumed(checked_at: i64) -> ActionsEvidence {
    ActionsEvidence {
        source: ActionsSource::ProviderCheckedNoKnownAction,
        basis_date: Some(day()),
        coverage: Some(CoverageState::NoKnownActionProviderChecked),
        coverage_policy: Some("corporate-actions-coverage-v1@fixture".into()),
        announcement: Some(format!("check:{checked_at}")),
        assumption: Some("unchanged_share_basis_assumed".into()),
        coverage_at: Some(checked_at),
        valid_until: Some(checked_at + FRESH),
        ..ActionsEvidence::default()
    }
}

/// The provenance of a record the daily bars themselves verified: a finding, never expiring.
fn bar_verified() -> ActionsEvidence {
    ActionsEvidence {
        source: ActionsSource::BarVerified,
        basis_date: Some(day()),
        bar_verified_through: Some(day()),
        coverage_at: None,
        valid_until: None,
        ..ActionsEvidence::default()
    }
}

fn close(version: u32, actions: ActionsEvidence, observed: i64) -> PrevClose {
    PrevClose {
        id: "close:ABCD".into(),
        version,
        instrument: inst("ABCD"),
        date: prev(),
        close_micros: 4_000_000,
        basis: CloseBasis::Raw,
        actions_checked_through: Some(day()),
        actions,
        observed_at: observed,
    }
}

fn daily(version: u32, basis: ActionsEvidence, observed: i64) -> DailyBaseline {
    let mut sessions = Vec::new();
    let mut d = prev();
    while sessions.len() < 30 {
        if market_events::is_trading_day(d) {
            sessions.push(SessionVolume {
                date: d,
                volume: 1_000,
                complete: true,
                comparable: true,
            });
        }
        d = d.pred_opt().unwrap();
    }
    DailyBaseline {
        id: "daily:ABCD".into(),
        version,
        instrument: inst("ABCD"),
        sessions,
        basis,
        observed_at: observed,
    }
}

/// Float and news evidence enough for the other two pillars, so an expiry can be seen moving the
/// whole verdict and not just one pillar.
fn evidence(e: &mut Engine) {
    e.add_float(FloatObservation {
        id: "float:ABCD".into(),
        version: 1,
        instrument: inst("ABCD"),
        shares: 5_000_000,
        provider: "fixture".into(),
        effective_as_of: None,
        observed_at: t(4, 0, 0),
        quality: vec![],
    });
    e.add_news(NewsItem {
        id: "news:ABCD:1".into(),
        version: 1,
        instrument: inst("ABCD"),
        event_type: "clinical_result".into(),
        source: "alpaca/benzinga".into(),
        credible_source: true,
        relevant: true,
        substantive: true,
        authenticity: Authenticity::ReportedUnverified,
        favorability: Favorability::Undetermined,
        published_at: Some(t(7, 0, 0)),
        observed_at: t(7, 0, 1),
        tag: Tag::Present,
        content_key: "fixture".into(),
        revision_at: Some(0),
        rules: "fixture".into(),
    });
}

/// One print 45 % over the $4.00 close and far over 5x the 1,000-share mean.
fn print_at(e: &mut Engine, exec: i64) {
    e.apply(
        &Trade::new(
            "ABCD",
            UnixNanos(exec),
            Price::from_micros(5_800_000),
            10_000,
            1,
        ),
        VOL,
        UnixNanos(exec + 100_000_000),
    );
}

/// Prints every five minutes over `[from, until)` so the price never goes stale
/// (`price_staleness.regular_secs`, 600 s), then decides through `until`.
fn walk(e: &mut Engine, from: i64, until: i64) {
    let mut x = from;
    while x < until {
        print_at(e, x);
        x += 300 * S;
    }
    e.advance_to(UnixNanos(until));
}

fn pillar(d: &Decision, p: PillarId) -> (Tri, &'static str) {
    let r = d.pillars.iter().find(|x| x.pillar == p).expect("a pillar");
    (r.state, r.reason)
}

fn latest(e: &Engine) -> &Decision {
    e.current().next().expect("a decision")
}

/// A check at 09:00 under the v1 policy is good until 10:30.
fn checked_at() -> i64 {
    t(9, 0, 0)
}
fn horizon() -> i64 {
    checked_at() + FRESH
}

#[test]
fn a_no_known_action_assumption_expires_with_no_new_evidence_at_all() {
    let mut e = engine();
    evidence(&mut e);
    e.add_prev_close(close(1, assumed(checked_at()), checked_at()));
    e.add_daily(daily(1, assumed(checked_at()), checked_at()));
    // Inside the 90 minutes from 09:00: both pillars rest on the assumption and pass, and the
    // stock is a qualified five-of-five.
    walk(&mut e, t(9, 30, 1), t(10, 29, 0));
    assert_eq!(pillar(latest(&e), PillarId::Gain).0, Tri::Pass);
    assert_eq!(pillar(latest(&e), PillarId::Rvol).0, Tri::Pass);
    assert_eq!(latest(&e).qualification, Qualification::Qualified);

    // Past the horizon, with no new record of any kind (the print only keeps the price fresh, so
    // nothing but the clock can explain the change).
    print_at(&mut e, t(10, 31, 0));
    e.advance_to(UnixNanos(t(10, 33, 0)));
    assert_eq!(
        pillar(latest(&e), PillarId::Gain),
        (Tri::Unknown, "actions_coverage_expired")
    );
    assert_eq!(
        pillar(latest(&e), PillarId::Rvol),
        (Tri::Unknown, "actions_coverage_expired")
    );
    assert_eq!(
        latest(&e).qualification,
        Qualification::Undetermined,
        "an expired assumption is UNKNOWN, never a FAIL and never a stale PASS"
    );
    assert_eq!(
        latest(&e).gain_vs_stored_close_bp,
        Some(4_500),
        "the gain against the stored close is a diagnostic and stays visible"
    );
}

/// With no print and no record past the horizon either: the barrier comes due on the clock alone.
#[test]
fn the_flip_happens_at_the_first_barrier_past_the_horizon() {
    let mut e = engine();
    e.add_prev_close(close(1, assumed(checked_at()), checked_at()));
    e.add_daily(daily(1, assumed(checked_at()), checked_at()));
    walk(&mut e, t(9, 30, 1), t(10, 29, 0));
    let mut ds = e.advance_to(UnixNanos(t(10, 36, 0)));
    ds.retain(|d| d.changed.contains(&"pillars.gain"));
    let flip = ds.first().expect("a gain change");
    assert_eq!(pillar(flip, PillarId::Gain).1, "actions_coverage_expired");
    assert!(
        flip.cutoff > horizon(),
        "the flip is past the horizon, {} vs {}",
        flip.cutoff,
        horizon()
    );
    assert!(
        flip.cutoff - horizon() <= 17 * S,
        "and within one bar plus its settle, not whenever something else happens to arrive: {}s",
        (flip.cutoff - horizon()) / S
    );
}

#[test]
fn an_expired_assumption_recovers_on_a_fresh_check() {
    let mut e = engine();
    evidence(&mut e);
    e.add_prev_close(close(1, assumed(checked_at()), checked_at()));
    e.add_daily(daily(1, assumed(checked_at()), checked_at()));
    walk(&mut e, t(9, 30, 1), t(10, 29, 0));
    print_at(&mut e, t(10, 31, 0));
    e.advance_to(UnixNanos(t(10, 33, 0)));
    assert_eq!(
        pillar(latest(&e), PillarId::Gain).1,
        "actions_coverage_expired"
    );

    // The maintainer checks again at 10:32: a new version of each record, observed then.
    let again = t(10, 32, 0);
    e.add_prev_close(close(2, assumed(again), again));
    e.add_daily(daily(2, assumed(again), again));
    e.advance_to(UnixNanos(t(10, 34, 0)));
    assert_eq!(pillar(latest(&e), PillarId::Gain).0, Tri::Pass);
    assert_eq!(pillar(latest(&e), PillarId::Rvol).0, Tri::Pass);
    assert_eq!(latest(&e).qualification, Qualification::Qualified);

    // And it expires again on the new horizon: recovery is not permanent either.
    walk(&mut e, t(10, 35, 0), again + FRESH - 60 * S);
    assert_eq!(pillar(latest(&e), PillarId::Gain).0, Tri::Pass);
    print_at(&mut e, again + FRESH + 30 * S);
    e.advance_to(UnixNanos(again + FRESH + 120 * S));
    assert_eq!(
        pillar(latest(&e), PillarId::Gain).1,
        "actions_coverage_expired"
    );
}

#[test]
fn a_bar_verified_basis_never_expires() {
    let mut e = engine();
    evidence(&mut e);
    e.add_prev_close(close(1, bar_verified(), checked_at()));
    e.add_daily(daily(1, bar_verified(), checked_at()));
    // Two whole freshness windows past the record, with the price kept fresh by prints.
    walk(&mut e, t(9, 30, 1), t(12, 1, 0));
    assert!(t(12, 1, 0) > checked_at() + 2 * FRESH);
    assert_eq!(
        pillar(latest(&e), PillarId::Gain),
        (Tri::Pass, "at_or_above_min"),
        "the bars themselves were consistent: a finding, not an assumption"
    );
    assert_eq!(pillar(latest(&e), PillarId::Rvol).0, Tri::Pass);
}

#[test]
fn valid_at_is_the_only_gate_and_a_missing_horizon_never_expires() {
    let a = assumed(checked_at());
    assert!(a.valid_at(horizon()), "at the horizon it still holds");
    assert!(!a.valid_at(horizon() + 1));
    let b = bar_verified();
    assert!(b.valid_at(i64::MAX), "no horizon, no expiry");
}
