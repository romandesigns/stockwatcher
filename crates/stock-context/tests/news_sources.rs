//! pillars-v6 (SEC EDGAR filings as a second news source) and ross-panel-v2 (news optional)
//! through the engine: which source may pass the news pillar under which policy, that an offering
//! filing is flagged and never passes, that a filing never counts before its acceptance time, and
//! that the owner's switch changes membership at the next decision barrier.

use chrono::NaiveDate;
use market_events::session::et;
use market_events::{Price, Trade, UnixNanos};
use stock_context::*;

const VOL: Verdict = Verdict {
    ohlc: true,
    volume: true,
};
const S: i64 = 1_000_000_000;

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

fn engine(policy: Policy) -> Engine {
    Engine::new(
        policy,
        RunInfo {
            run_id: "fixture".into(),
            source: "replay".into(),
            fidelity: Fidelity::SyntheticFixture,
            numerator_coverage: Coverage::Complete,
        },
    )
}

fn print(e: &mut Engine, sym: &str, exec: i64, micros: i64, size: u64) {
    e.apply(
        &Trade::new(sym, UnixNanos(exec), Price::from_micros(micros), size, 1),
        VOL,
        UnixNanos(exec + 100_000_000),
    );
}

/// Close, a 30-session baseline and a small float: price, gain, RVOL and float can all pass once
/// the symbol trades +16 % on 60x volume. No news: each test adds its own.
fn evidence(e: &mut Engine, sym: &str) {
    e.add_prev_close(PrevClose {
        id: format!("close:{sym}"),
        version: 1,
        instrument: inst(sym),
        date: prev(),
        close_micros: 5_000_000,
        basis: CloseBasis::SplitAdjusted,
        actions_checked_through: Some(day()),
        actions: Default::default(),
        observed_at: t(4, 0, 0),
    });
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
    e.add_daily(DailyBaseline {
        id: format!("daily:{sym}"),
        version: 1,
        instrument: inst(sym),
        sessions,
        basis: Default::default(),
        observed_at: t(4, 0, 0),
    });
    e.add_float(FloatObservation {
        id: format!("float:{sym}"),
        version: 1,
        instrument: inst(sym),
        shares: 5_000_000,
        provider: "fixture".into(),
        effective_as_of: None,
        observed_at: t(4, 0, 0),
        quality: vec![],
    });
}

/// An SEC filing as `market_archive::evidence_load::filing_items` makes it.
fn filing(sym: &str, form: &str, accepted: i64, observed: i64) -> NewsItem {
    let offering = form.starts_with("S-") || form.starts_with("F-") || form.starts_with("424B");
    NewsItem {
        id: format!("news:sec_edgar:0000000000-26-{sym}{form}"),
        version: 1,
        instrument: inst(sym),
        event_type: if offering {
            OFFERING_EVENT.into()
        } else {
            "current_report".into()
        },
        source: format!("sec_edgar/{form}"),
        credible_source: true,
        relevant: true,
        substantive: !offering,
        authenticity: Authenticity::IssuerAnnounced,
        favorability: Favorability::Undetermined,
        published_at: Some(accepted),
        observed_at: observed,
        tag: Tag::Present,
        content_key: format!("{sym}{form}"),
        revision_at: Some(accepted),
        rules: "filing-rules-v1".into(),
        url: Some(format!(
            "https://www.sec.gov/Archives/edgar/data/1/000000000026{sym}/0000000000-26-{sym}-index.htm"
        )),
        headline: None,
        halt: None,
    }
}

/// Prints every 5 s from 09:30 to `to`, at +16 % on the day.
fn trade_until(e: &mut Engine, sym: &str, to: i64) {
    let mut at = t(9, 30, 0);
    while at < to {
        print(e, sym, at, 5_800_000, 1_000);
        at += 5 * S;
    }
}

fn news_pillar(e: &Engine, sym: &str) -> (Tri, &'static str) {
    let d = e.current().find(|d| d.symbol == sym).expect("decided");
    let p = d
        .pillars
        .iter()
        .find(|p| p.pillar == PillarId::News)
        .unwrap();
    (p.state, p.reason)
}

fn row<'a>(p: &'a PanelSnapshot, sym: &str) -> Option<&'a PanelRow> {
    p.rows.iter().find(|r| r.symbol == sym)
}

#[test]
fn an_8k_passes_the_news_pillar_under_v6_and_names_its_source_but_not_under_v5() {
    for (policy, passes) in [(Policy::v6(), true), (Policy::v5(), false)] {
        let mut e = engine(policy.clone());
        evidence(&mut e, "FILR");
        e.add_news(filing("FILR", "8-K", t(9, 12, 0), t(9, 12, 40)));
        trade_until(&mut e, "FILR", t(9, 33, 0));
        e.advance_to(UnixNanos(t(9, 33, 3)));
        let (state, _) = news_pillar(&e, "FILR");
        assert_eq!(state == Tri::Pass, passes, "{}", policy.body.id);
        let member = e.panel().and_then(|p| row(p, "FILR").cloned());
        assert_eq!(member.is_some(), passes, "{}", policy.body.id);
        if let Some(r) = member {
            let ev = r.news_evidence.expect("the source that passed it");
            assert_eq!(ev.source, "sec_edgar");
            assert_eq!(ev.form.as_deref(), Some("8-K"));
            assert_eq!(ev.published_at, Some(t(9, 12, 0)), "the acceptance time");
            assert!(
                ev.url
                    .unwrap()
                    .starts_with("https://www.sec.gov/Archives/edgar/data/")
            );
            assert!(r.offering.is_none() && !r.news_unconfirmed);
        }
    }
}

#[test]
fn a_benzinga_story_still_passes_under_v6_and_is_named_benzinga() {
    let mut e = engine(Policy::v6());
    evidence(&mut e, "BZNG");
    let mut n = filing("BZNG", "8-K", t(7, 0, 0), t(7, 0, 1));
    n.id = "news:alpaca:123".into();
    n.source = "alpaca/benzinga".into();
    n.event_type = "clinical_result".into();
    n.authenticity = Authenticity::ReportedUnverified;
    n.url = Some("https://www.benzinga.com/x".into());
    e.add_news(n);
    trade_until(&mut e, "BZNG", t(9, 33, 0));
    e.advance_to(UnixNanos(t(9, 33, 3)));
    let p = e.panel().expect("a panel");
    let ev = row(p, "BZNG").unwrap().news_evidence.clone().unwrap();
    assert_eq!((ev.source.as_str(), ev.form), ("benzinga", None));
}

#[test]
fn a_filing_never_counts_before_its_acceptance_time() {
    // Observed (seen in the feed) at 09:31:30, but stamped accepted at 09:31:00: from the first
    // barrier at or after the observation it passes; before it, it is invisible.
    let mut e = engine(Policy::v6());
    evidence(&mut e, "PITX");
    e.add_news(filing("PITX", "6-K", t(9, 31, 0), t(9, 31, 30)));
    trade_until(&mut e, "PITX", t(9, 31, 15));
    e.advance_to(UnixNanos(t(9, 31, 17)));
    assert_ne!(
        news_pillar(&e, "PITX").0,
        Tri::Pass,
        "not visible at 09:31:15"
    );
    print(&mut e, "PITX", t(9, 31, 40), 5_800_000, 1_000);
    e.advance_to(UnixNanos(t(9, 31, 47)));
    assert_eq!(news_pillar(&e, "PITX").0, Tri::Pass);

    // A record whose acceptance time is after the cutoff never passes, even if (wrongly) observed
    // earlier: the pillar's own `published_at <= cutoff` rule.
    let mut e = engine(Policy::v6());
    evidence(&mut e, "LATE");
    e.add_news(filing("LATE", "8-K", t(9, 40, 0), t(9, 20, 0)));
    trade_until(&mut e, "LATE", t(9, 33, 0));
    e.advance_to(UnixNanos(t(9, 33, 3)));
    assert_ne!(news_pillar(&e, "LATE").0, Tri::Pass);
}

#[test]
fn an_offering_filing_is_flagged_and_never_passes_the_news_pillar() {
    let mut e = engine(Policy::v6()).with_panel_policy(PanelPolicy::v2());
    evidence(&mut e, "DILU");
    e.add_news(filing("DILU", "424B5", t(8, 5, 0), t(8, 5, 50)));
    trade_until(&mut e, "DILU", t(9, 33, 0));
    e.advance_to(UnixNanos(t(9, 33, 3)));
    assert_ne!(news_pillar(&e, "DILU").0, Tri::Pass);
    let p = e
        .panel()
        .expect("news-optional admits it on the other four");
    assert!(p.news_optional && p.panel_policy.starts_with("ross-panel-v2@"));
    let r = row(p, "DILU").unwrap();
    assert!(r.news_unconfirmed);
    assert!(r.news_evidence.is_none());
    let o = r.offering.clone().expect("the offering flag");
    assert_eq!(o.form, "424B5");
    assert_eq!(o.published_at, Some(t(8, 5, 0)));
}

#[test]
fn the_news_switch_changes_membership_at_the_next_barrier() {
    let mut e = engine(Policy::v6());
    evidence(&mut e, "NONE");
    trade_until(&mut e, "NONE", t(9, 33, 0));
    e.advance_to(UnixNanos(t(9, 33, 3)));
    // News required (ross-panel-v1): four pillars pass, news is UNKNOWN (never searched): out.
    assert!(e.panel().is_none_or(|p| row(p, "NONE").is_none()));
    let d = e.current().find(|d| d.symbol == "NONE").unwrap();
    assert_eq!(d.counts.pass, 4);

    // Switched off: nothing changes until the next barrier decides again.
    assert!(e.set_panel_policy(PanelPolicy::v2()));
    assert!(
        !e.set_panel_policy(PanelPolicy::v2()),
        "the same policy is no change"
    );
    assert!(e.panel().is_none_or(|p| row(p, "NONE").is_none()));
    print(&mut e, "NONE", t(9, 33, 5), 5_800_000, 1_000);
    e.advance_to(UnixNanos(t(9, 33, 18)));
    let p = e.panel().expect("admitted at the next barrier").clone();
    assert!(p.news_optional);
    let r = row(&p, "NONE").unwrap();
    assert_eq!(r.membership.status, Some(MemberStatus::Qualified));
    assert!(r.news_unconfirmed);
    assert_eq!(r.missing.len(), 1, "news is still named");
    assert_eq!(r.missing[0].pillar, PillarId::News);
    let admitted = r.membership.active().unwrap().admitted_at;
    assert!(admitted > t(9, 33, 3) && admitted <= t(9, 33, 18));

    // Switched back on: v1 at the next barrier. The member stays at 4/5 (v1's retention), now
    // shown as retained with news missing, not "unconfirmed"; a new symbol cannot enter on four.
    evidence(&mut e, "NEWB");
    assert!(e.set_panel_policy(PanelPolicy::v1()));
    let mut at = t(9, 33, 20);
    while at < t(9, 35, 0) {
        print(&mut e, "NONE", at, 5_800_000, 1_000);
        print(&mut e, "NEWB", at + S, 5_800_000, 1_000);
        at += 5 * S;
    }
    e.advance_to(UnixNanos(t(9, 35, 3)));
    let p = e.panel().unwrap();
    assert!(!p.news_optional && p.panel_policy.starts_with("ross-panel-v1@"));
    let r = row(p, "NONE").unwrap();
    assert_eq!(r.membership.status, Some(MemberStatus::Retained));
    assert!(!r.news_unconfirmed);
    assert!(row(p, "NEWB").is_none(), "v1 admits at 5/5 only");
}
