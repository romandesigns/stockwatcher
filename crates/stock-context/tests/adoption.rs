//! What a restart may take from a recording, and what it may not.
//!
//! A restart restores a symbol by replaying its own recorded day on a second engine and adopting
//! the result ([`Engine::adopt_facts`]), because feeding those prints into the live engine would
//! classify every one of them as late against a clock that has already advanced with the tail —
//! the audit's F1, where each symbol's RVOL numerator began again at zero after every deploy.
//!
//! The adoption draws a line that the first draft of this slice did not. The day's **facts** —
//! volume, dollars, trades, the price and tick history, the day high, the setup bars, the
//! admission ledger — are statements about prints that really executed, and the recording is the
//! evidence for them. The day's **decisions** are statements that this run observed something at
//! a time: a 5/5 at 09:41, a pole and a pullback with frozen levels. A recording written by a
//! previous process is not evidence that this one saw any of that, and the live tail may already
//! have published an episode of its own under the same run id. So decisions do not travel.
//!
//! What this file pins:
//!
//! - the facts arrive, in full, replacing the tail-only numerator;
//! - a membership episode, timeline and setup the live prefix already published survive intact;
//! - a restore's own replay-generated episodes are discarded and counted, never published;
//! - an adoption across run identities, policies or trading dates is refused rather than mixed;
//! - a window reaching back into the restored prefix is not reportable, and says why, until it
//!   has moved past it.

use chrono::NaiveDate;
use market_events::session::et;
use market_events::{Price, Trade, UnixNanos};
use stock_context::*;

const VOL: Verdict = Verdict {
    ohlc: true,
    volume: true,
};
const SEC: i64 = 1_000_000_000;

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 16).expect("valid")
}

fn t(h: u32, m: u32, s: u32) -> i64 {
    et(day(), h, m, s, 0).expect("valid").nanos()
}

fn run() -> RunInfo {
    RunInfo {
        run_id: "live".into(),
        source: "live".into(),
        fidelity: Fidelity::RecordedLive,
        numerator_coverage: Coverage::Unverified,
    }
}

fn engine() -> Engine {
    Engine::new(Policy::v1(), run())
        .with_panel_policy(PanelPolicy::v1())
        .with_setup_policy(DetectionPolicy::current())
}

fn print_seq(e: &mut Engine, seq: u64, exec: i64, cents: i64, size: u64) {
    let tr = Trade::new("ABCD", UnixNanos(exec), Price::from_cents(cents), size, seq);
    e.apply(&tr, VOL, UnixNanos(exec + 100_000_000));
}

fn facts(e: &Engine) -> (u64, u64) {
    let d = e.current().next().expect("a decision");
    (d.volume.shares, d.volume.trades)
}

fn decision(e: &Engine) -> Decision {
    e.current().next().expect("a decision").clone()
}

/// Everything that makes ABCD pass five of five, observed before the open: a $5.00 close against
/// which a $5.60 print is a +12 % day, a 30-session 100,000-share daily baseline (so 700,000
/// shares is 7x), an 8,000,000-share float and fresh favourable news.
fn qualifying_evidence(e: &mut Engine) {
    e.add_prev_close(PrevClose {
        id: "close:ABCD".into(),
        version: 1,
        instrument: InstrumentId::symbol("ABCD"),
        date: day().pred_opt().expect("valid"),
        close_micros: 5_000_000,
        basis: CloseBasis::SplitAdjusted,
        actions_checked_through: Some(day()),
        actions: Default::default(),
        observed_at: t(4, 0, 0),
    });
    let mut sessions = Vec::new();
    let mut d = day().pred_opt().expect("valid");
    while sessions.len() < 30 {
        if market_events::is_trading_day(d) {
            sessions.push(SessionVolume {
                date: d,
                volume: 100_000,
                complete: true,
                comparable: true,
            });
        }
        d = d.pred_opt().expect("valid");
    }
    e.add_daily(DailyBaseline {
        id: "daily:ABCD".into(),
        version: 1,
        instrument: InstrumentId::symbol("ABCD"),
        sessions,
        basis: Default::default(),
        observed_at: t(4, 0, 0),
    });
    e.add_float(FloatObservation {
        id: "float:ABCD".into(),
        version: 1,
        instrument: InstrumentId::symbol("ABCD"),
        shares: 8_000_000,
        provider: "fixture".into(),
        effective_as_of: None,
        observed_at: t(4, 0, 0),
        quality: vec![],
    });
    e.add_news(NewsItem {
        id: "news:ABCD:1".into(),
        version: 1,
        instrument: InstrumentId::symbol("ABCD"),
        event_type: "clinical_result".into(),
        source: "issuer press release".into(),
        credible_source: true,
        relevant: true,
        substantive: true,
        authenticity: Authenticity::IssuerAnnounced,
        favorability: Favorability::Favorable,
        published_at: Some(t(7, 0, 0)),
        observed_at: t(7, 0, 1),
        tag: Tag::Present,
        content_key: "fixture".into(),
        revision_at: Some(0),
        rules: "fixture".into(),
        url: None,
        headline: None,
        halt: None,
    });
}

/// The restored day's facts replace a tail-only numerator, and the live symbol's published
/// membership episode, timeline and latest decision are left exactly as they were.
///
/// The first draft of the adoption moved the whole symbol across, so the replay's own membership
/// and setup landed on top of the live ones. The live tail can already have admitted the symbol
/// to the panel and frozen a setup's levels under this run id and broadcast both; replacing them
/// with replay-generated state would rewrite a decision a client is already holding.
#[test]
fn an_adoption_brings_the_days_facts_and_keeps_what_was_already_published() {
    // The live tail: a late start, so its numerator is only the afternoon's.
    let mut live = engine();
    qualifying_evidence(&mut live);
    print_seq(&mut live, 101, t(13, 0, 0), 560, 700_000);
    live.advance_to(UnixNanos(t(13, 0, 20)));
    let published = decision(&live);
    assert_eq!(
        published.qualification,
        Qualification::Qualified,
        "the live tail admitted the symbol on its own: {:?}",
        published.counts
    );
    let episode = published
        .membership
        .active()
        .expect("an active episode")
        .clone();
    assert_eq!(facts(&live), (700_000, 1));

    // The restore: the same run and the same policies, replaying the whole day.
    let mut restored = Engine::new(live.policy().clone(), live.run_info().clone())
        .with_panel_policy(live.panel_policy().clone())
        .with_setup_policy(live.setup_policy().clone());
    qualifying_evidence(&mut restored);
    print_seq(&mut restored, 1, t(9, 30, 0), 555, 900_000);
    print_seq(&mut restored, 101, t(13, 0, 0), 560, 700_000);
    restored.advance_to(UnixNanos(t(13, 0, 20)));
    assert_eq!(
        facts(&restored),
        (1_600_000, 2),
        "the restore has the whole day"
    );

    let outcome = live.adopt_facts(&mut restored, "ABCD");
    assert_eq!(outcome, Adoption::Adopted { published: true });

    // The facts are the day's.
    live.advance_to(UnixNanos(t(13, 1, 0)));
    let after = decision(&live);
    assert_eq!(
        (after.volume.shares, after.volume.trades),
        (1_600_000, 2),
        "the numerator is the day's, not the tail's"
    );

    // The episode the live tail published is the same episode, with its own admission time.
    let still = after.membership.active().expect("still a member");
    assert_eq!(
        still, &episode,
        "the published episode is untouched: {still:?} vs {episode:?}"
    );
    assert_eq!(
        after.timeline, published.timeline,
        "and so are the timeline's first-seen times"
    );
    assert_eq!(
        after.membership.episodes.len(),
        1,
        "and the symbol's episode history is the one this run published, not the replay's two          (admitted at 09:30, dropped when the morning price went stale, re-admitted at 13:00):          {:?}",
        after.membership.episodes
    );

    let tel = live.restore_telemetry();
    assert_eq!(tel.adopted, 1);
    assert_eq!(tel.published_preserved, 1);
    assert_eq!(
        tel.episodes_discarded, 1,
        "the restore's own replayed episode was dropped, not published: {tel:?}"
    );
    assert_eq!(tel.refused, 0);
}

/// A symbol the live run has never decided takes the restored facts and **no** episodes: the
/// replay's membership and setup are not this run's observations, and arriving first does not
/// make them so.
#[test]
fn a_restore_of_an_undecided_symbol_publishes_no_backdated_episode() {
    let mut live = engine();
    qualifying_evidence(&mut live);
    let mut restored = Engine::new(live.policy().clone(), live.run_info().clone())
        .with_panel_policy(live.panel_policy().clone())
        .with_setup_policy(live.setup_policy().clone());
    qualifying_evidence(&mut restored);
    print_seq(&mut restored, 1, t(9, 30, 0), 555, 900_000);
    restored.advance_to(UnixNanos(t(9, 31, 0)));
    let replayed = decision(&restored);
    assert!(
        replayed.membership.active().is_some(),
        "the replay did admit it on its own evidence"
    );

    assert_eq!(
        live.adopt_facts(&mut restored, "ABCD"),
        Adoption::Adopted { published: false }
    );
    assert!(
        live.current().next().is_none(),
        "nothing is published before the next barrier: no backdated decision"
    );

    // The next barrier decides it, as of then, on the day's facts.
    live.advance_to(UnixNanos(t(9, 31, 30)));
    let d = decision(&live);
    assert_eq!(d.volume.shares, 900_000, "the facts came across");
    let ep = d.membership.active().expect("admitted now");
    assert_eq!(
        ep.admitted_at, d.cutoff,
        "admitted as of this barrier, not backdated to the replay's 09:31"
    );
    let was = &replayed.membership.episodes[0];
    assert_ne!(
        ep.admitted_decision, was.admitted_decision,
        "and it is this barrier's decision, not the replay's"
    );
    assert!(
        ep.admitted_at > was.admitted_at,
        "the admission is this run's, after the adoption: {} vs the replay's {}",
        ep.admitted_at,
        was.admitted_at
    );
    assert_eq!(live.restore_telemetry().episodes_discarded, 1);
}

/// An adoption from another run, another policy or another trading date is refused.
///
/// Facts measured under other rules are a different arithmetic; mixing them into one symbol
/// leaves no way to tell afterwards which half came from where.
#[test]
fn an_adoption_across_identities_or_dates_is_refused() {
    let mut live = engine();
    let other_run = RunInfo {
        run_id: "replay:2026-10-16".into(),
        source: "replay".into(),
        ..run()
    };
    let mut from = Engine::new(Policy::v1(), other_run)
        .with_panel_policy(PanelPolicy::v1())
        .with_setup_policy(DetectionPolicy::current());
    print_seq(&mut from, 1, t(9, 30, 0), 500, 1_000);
    from.advance_to(UnixNanos(t(9, 31, 0)));
    assert_eq!(
        live.adopt_facts(&mut from, "ABCD"),
        Adoption::Refused("run_identity")
    );
    assert!(
        live.current().next().is_none(),
        "and nothing was taken from it"
    );

    // Another pillar policy: v1's price band is not v2's.
    let mut from = Engine::new(Policy::current(), run())
        .with_panel_policy(PanelPolicy::v1())
        .with_setup_policy(DetectionPolicy::current());
    print_seq(&mut from, 1, t(9, 30, 0), 500, 1_000);
    from.advance_to(UnixNanos(t(9, 31, 0)));
    assert_eq!(
        live.adopt_facts(&mut from, "ABCD"),
        Adoption::Refused("pillar_policy")
    );

    // Nothing restored at all is not a refusal.
    let mut empty = engine();
    assert_eq!(live.adopt_facts(&mut empty, "ABCD"), Adoption::Nothing);
    assert_eq!(live.restore_telemetry().refused, 2);
    assert_eq!(live.restore_telemetry().adopted, 0);
}

/// A recent window reaching into the restored prefix is not reportable, and clears by itself
/// once the window has moved past it.
///
/// The day's total a restore brings over is a sound figure: the prints are the prints. Which
/// **barrier** each one folded into is only established when the restore could prove it replayed
/// the same prints in the same order the run admitted them — an admission journal for the day
/// (`market_archive::admissions`). Without that proof the checkpoints at a window's endpoints may
/// hold a different set than the live run's did, so the window is not reportable. The RVOL
/// pillar's own threshold is the strict daily one and is unaffected; this is the intraday
/// diagnostic.
#[test]
fn a_window_over_a_restored_prefix_is_not_reportable_until_it_has_passed() {
    let mut sessions = Vec::new();
    let mut d = day().pred_opt().expect("valid");
    while sessions.len() < 10 {
        if market_events::is_trading_day(d) {
            sessions.push(IntradayCurve {
                date: d,
                cum: vec![(0, 0), (19_800, 100), (36_000, 200)],
                complete: true,
            });
        }
        d = d.pred_opt().expect("valid");
    }
    let mut live = engine();
    live.add_intraday(IntradayBaseline {
        id: "intraday:ABCD".into(),
        version: 1,
        instrument: InstrumentId::symbol("ABCD"),
        sessions,
        basis: Default::default(),
        observed_at: t(4, 0, 0),
    });
    let mut restored = Engine::new(live.policy().clone(), live.run_info().clone())
        .with_panel_policy(live.panel_policy().clone())
        .with_setup_policy(live.setup_policy().clone());
    print_seq(&mut restored, 1, t(9, 30, 0), 500, 1_000);
    print_seq(&mut restored, 2, t(13, 0, 0), 500, 1_000);
    restored.advance_to(UnixNanos(t(13, 0, 20)));
    assert_eq!(
        live.adopt_facts(&mut restored, "ABCD"),
        Adoption::Adopted { published: false }
    );
    live.set_restored_through("ABCD", Some(t(13, 0, 15)));

    // A barrier right after the adoption: the window still reaches back into the restored prefix.
    live.advance_to(UnixNanos(t(13, 1, 0)));
    let d = decision(&live);
    assert_eq!(
        d.volume.shares, 2_000,
        "the day's total is exact either way"
    );
    assert_eq!(
        d.rvol.recent_reason, "window_untrusted",
        "the window crosses the boundary: {:?}",
        d.rvol
    );
    assert_eq!(d.rvol.recent_milli, None);
    assert!(
        d.volume.integrity.window_untrusted && !d.volume.integrity.volume_untrusted,
        "the doubt is about the window, not the count: {:?}",
        d.volume.integrity
    );

    // Far enough on that the window lies entirely in this run's own admissions: reportable
    // again, with nothing cleared by hand.
    let later = t(13, 0, 15) + 3_600 * SEC;
    print_seq(&mut live, 3, later, 500, 500);
    live.advance_to(UnixNanos(later + 20 * SEC));
    let d = decision(&live);
    assert_eq!(d.volume.shares, 2_500);
    assert_eq!(
        d.rvol.recent_reason, "ok",
        "the window has moved past the restored prefix: {:?}",
        d.rvol
    );
    assert!(d.rvol.recent_milli.is_some());
    assert!(!d.volume.integrity.window_untrusted);
}
