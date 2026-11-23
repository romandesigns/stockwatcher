use super::*;
use serde_json::Value;

const T0: i64 = 1_763_993_400_000; // 2025-11-24 09:10 ET-ish, any weekday ms
const MIN: i64 = 60_000;

fn board() -> PendingBoard {
    let mut b = PendingBoard::new();
    b.enable((0.10, 25.0), 120);
    b
}

fn q(p: f64, c: f64) -> impl FnOnce() -> Quote {
    move || Some((p, Some(c)))
}

fn no_q() -> impl FnOnce() -> Quote {
    || None
}

fn hit(id: &str, at: i64, kind: HitKind) -> NewsHit {
    NewsHit {
        id: id.into(),
        source: "alpaca/benzinga".into(),
        headline: Some("XLAB announces FDA clearance".into()),
        url: Some("https://example.com/x".into()),
        at_ms: at,
        seen_ms: at + 2_000,
        kind,
    }
}

fn update(b: &mut PendingBoard) -> Value {
    serde_json::from_str(&b.take_update().expect("an update")).unwrap()
}

#[test]
fn t1_then_news_then_resumption_then_resumed() {
    let mut b = board();
    b.on_status("XLAB", StatusSignal::Halt, "T1", T0, q(2.76, 1.94), T0 + 50);
    let u = update(&mut b);
    assert_eq!(u["type"], "pending.update");
    assert_eq!(u["prev_gen"], 0);
    assert_eq!(u["gen"], 1);
    let c = &u["cards"][0];
    assert_eq!(c["id"], format!("XLAB-{T0}"));
    assert_eq!(c["phase"], "halted");
    assert_eq!(c["price"], 2.76);
    assert_eq!(c["chg_pct"], 42.27);
    assert_eq!(c["reason"], "News pending");
    assert_eq!(c["offering"], false);
    // Nothing changed: no frame (never per tick).
    b.tick(T0 + 1_000, |_| None);
    assert!(b.take_update().is_none());

    b.on_news(
        [(
            "XLAB".to_string(),
            hit("n1", T0 + 3 * MIN, HitKind::Catalyst),
        )],
        T0 + 4 * MIN,
    );
    let u = update(&mut b);
    assert_eq!(u["prev_gen"], 1);
    let c = &u["cards"][0];
    assert_eq!(c["phase"], "news");
    assert_eq!(c["news"][0]["label"], "Benzinga");
    assert_eq!(c["news"][0]["headline"], "XLAB announces FDA clearance");
    assert_eq!(c["news"][0]["kind"], "catalyst");
    // The same story again is not news.
    b.on_news(
        [(
            "XLAB".to_string(),
            hit("n1", T0 + 3 * MIN, HitKind::Catalyst),
        )],
        T0 + 5 * MIN,
    );
    assert!(b.take_update().is_none());

    // T2 then the quote resumption: a countdown to the estimated reopen.
    b.on_status(
        "XLAB",
        StatusSignal::Halt,
        "T2",
        T0 + 6 * MIN,
        no_q(),
        T0 + 6 * MIN,
    );
    b.on_status(
        "XLAB",
        StatusSignal::QuoteResume,
        "",
        T0 + 10 * MIN,
        no_q(),
        T0 + 10 * MIN,
    );
    let u = update(&mut b);
    let c = &u["cards"][0];
    assert_eq!(c["phase"], "resuming");
    assert_eq!(c["released_at"], T0 + 6 * MIN);
    assert_eq!(c["resume_at"], T0 + 15 * MIN);
    assert_eq!(c["resume_est"], true);
    // The halt kept its start and price through the later codes.
    assert_eq!(c["halted_at"], T0);
    assert_eq!(c["price"], 2.76);

    b.on_status(
        "XLAB",
        StatusSignal::Resume,
        "",
        T0 + 15 * MIN,
        no_q(),
        T0 + 15 * MIN,
    );
    let u = update(&mut b);
    assert_eq!(u["cards"][0]["phase"], "resumed");
    assert_eq!(u["cards"][0]["resumed_at"], T0 + 15 * MIN);
    // The reopening print, read on the tick.
    b.tick(T0 + 15 * MIN + 2_000, |s| {
        (s == "XLAB").then_some((3.40, T0 + 15 * MIN + 500))
    });
    let u = update(&mut b);
    assert_eq!(u["cards"][0]["reopen_price"], 3.4);
    // Grace over: removed.
    b.tick(T0 + 15 * MIN + 119_000, |_| None);
    assert!(b.take_update().is_none());
    b.tick(T0 + 15 * MIN + 120_000, |_| None);
    let u = update(&mut b);
    assert_eq!(u["cards"].as_array().unwrap().len(), 0);
    assert_eq!(u["removed"][0], format!("XLAB-{T0}"));
    assert!(b.card("XLAB").is_none());
}

#[test]
fn an_offering_flags_the_card_at_once_and_is_never_pushed_off() {
    let mut b = board();
    b.on_status("DILU", StatusSignal::Halt, "T1", T0, q(4.0, 2.0), T0);
    b.take_update();
    b.on_news(
        [("DILU".to_string(), hit("o1", T0 + MIN, HitKind::Offering))],
        T0 + MIN,
    );
    let u = update(&mut b);
    assert_eq!(u["cards"][0]["offering"], true);
    assert_eq!(u["cards"][0]["phase"], "news");
    assert_eq!(u["cards"][0]["news"][0]["kind"], "offering");
    for i in 0..4 {
        b.on_news(
            [(
                "DILU".to_string(),
                hit(&format!("c{i}"), T0 + (2 + i) * MIN, HitKind::Catalyst),
            )],
            T0 + 6 * MIN,
        );
    }
    let c = b.card("DILU").unwrap();
    assert_eq!(c.news.len(), MAX_NEWS);
    assert!(c.offering(), "the offering stays on the card");
}

#[test]
fn a_halt_without_news_still_walks_to_resumed() {
    let mut b = board();
    b.on_status("QUIET", StatusSignal::Halt, "T1", T0, q(1.2, 1.0), T0);
    b.on_status(
        "QUIET",
        StatusSignal::Halt,
        "T3",
        T0 + 20 * MIN,
        no_q(),
        T0 + 20 * MIN,
    );
    let c = b.card("QUIET").unwrap();
    assert_eq!(c.phase, Phase::Resuming);
    assert!(c.news.is_empty());
    assert_eq!(c.resume_at, None, "T3 alone gives no time");
    b.on_status(
        "QUIET",
        StatusSignal::QuoteResume,
        "",
        T0 + 25 * MIN,
        no_q(),
        T0 + 25 * MIN,
    );
    assert_eq!(b.card("QUIET").unwrap().resume_at, Some(T0 + 30 * MIN));
    b.on_status(
        "QUIET",
        StatusSignal::Resume,
        "",
        T0 + 30 * MIN,
        no_q(),
        T0 + 30 * MIN,
    );
    assert_eq!(b.card("QUIET").unwrap().phase, Phase::Resumed);
}

#[test]
fn a_resume_without_t3_or_quote_resumption() {
    let mut b = board();
    b.on_status("JUMP", StatusSignal::Halt, "T1", T0, q(5.0, 4.0), T0);
    b.take_update();
    b.on_status(
        "JUMP",
        StatusSignal::Resume,
        "",
        T0 + 9 * MIN,
        no_q(),
        T0 + 9 * MIN,
    );
    let u = update(&mut b);
    let c = &u["cards"][0];
    assert_eq!(c["phase"], "resumed");
    assert_eq!(c["resume_at"], Value::Null);
    // A repeated resume is not news; neither is a resume of a stock never seen halting.
    b.on_status(
        "JUMP",
        StatusSignal::Resume,
        "",
        T0 + 9 * MIN + 1,
        no_q(),
        T0 + 9 * MIN,
    );
    b.on_status("NEVER", StatusSignal::Resume, "", T0, no_q(), T0);
    assert!(b.take_update().is_none());
}

#[test]
fn two_news_halts_the_same_day_make_two_cards_in_turn() {
    let mut b = board();
    b.on_status("TWICE", StatusSignal::Halt, "T1", T0, q(3.0, 2.0), T0);
    b.on_status(
        "TWICE",
        StatusSignal::Resume,
        "",
        T0 + 10 * MIN,
        no_q(),
        T0 + 10 * MIN,
    );
    b.take_update();
    // A LULD pause after it is not a news halt: no new card, the resumed one stays.
    b.on_status(
        "TWICE",
        StatusSignal::Halt,
        "LUDP",
        T0 + 20 * MIN,
        q(3.5, 2.0),
        T0 + 20 * MIN,
    );
    assert!(b.take_update().is_none());
    // ... which the exchange then turns into a T1: a new card from the pause's start.
    b.on_status(
        "TWICE",
        StatusSignal::Halt,
        "T1",
        T0 + 22 * MIN,
        no_q(),
        T0 + 22 * MIN,
    );
    let u = update(&mut b);
    assert_eq!(u["removed"][0], format!("TWICE-{T0}"));
    assert_eq!(u["cards"][0]["id"], format!("TWICE-{}", T0 + 20 * MIN));
    assert_eq!(u["cards"][0]["halted_at"], T0 + 20 * MIN);
    assert_eq!(u["cards"][0]["price"], 3.5);
    assert_eq!(u["cards"][0]["phase"], "halted");
    assert_eq!(b.cards().count(), 1);
}

#[test]
fn an_out_of_band_or_unpriced_symbol_is_ignored() {
    let mut b = board();
    b.on_status("BIG", StatusSignal::Halt, "T1", T0, q(40.0, 30.0), T0);
    b.on_status("PENNY", StatusSignal::Halt, "T1", T0, q(0.05, 0.04), T0);
    b.on_status("NOPX", StatusSignal::Halt, "T1", T0, no_q(), T0);
    b.on_status("EDGE", StatusSignal::Halt, "T1", T0, q(25.0, 20.0), T0);
    assert!(b.card("BIG").is_none() && b.card("PENNY").is_none() && b.card("NOPX").is_none());
    assert!(b.card("EDGE").is_some(), "the band is inclusive");
    // A non-news halt never makes a card.
    b.on_status("LULD", StatusSignal::Halt, "LUDP", T0, q(2.0, 1.0), T0);
    assert!(b.card("LULD").is_none());
}

#[test]
fn a_story_just_before_the_halt_is_its_story_an_older_one_is_not() {
    let mut b = board();
    b.on_news(
        [
            (
                "EARLY".to_string(),
                hit("e1", T0 - 2 * MIN, HitKind::Catalyst),
            ),
            (
                "EARLY".to_string(),
                hit("e0", T0 - 30 * MIN, HitKind::Catalyst),
            ),
        ],
        T0 - MIN,
    );
    b.on_status("EARLY", StatusSignal::Halt, "T1", T0, q(1.0, 0.5), T0);
    let c = b.card("EARLY").unwrap();
    assert_eq!(c.phase, Phase::News);
    assert_eq!(c.news.len(), 1);
    assert_eq!(c.news[0].id, "e1");
}

#[test]
fn nyse_phrases_and_codes() {
    assert_eq!(news_halt("T1"), Some(NewsHalt::Pending));
    assert_eq!(news_halt(" t2 "), Some(NewsHalt::Released));
    assert_eq!(news_halt("T3"), Some(NewsHalt::ResumeSet));
    assert_eq!(news_halt("News Pending"), Some(NewsHalt::Pending));
    assert_eq!(news_halt("News Dissemination"), Some(NewsHalt::Released));
    assert_eq!(news_halt("LUDP"), None);
    assert_eq!(news_halt("H10"), None);
    assert_eq!(
        signal(market_events::StatusKind::Other, "Q"),
        StatusSignal::QuoteResume
    );
    assert_eq!(
        signal(market_events::StatusKind::Halt, "H"),
        StatusSignal::Halt
    );
    assert_eq!(source_label("alpaca/benzinga"), "Benzinga");
    assert_eq!(source_label("sec_edgar/8-K"), "SEC 8-K");
    assert_eq!(source_label("press_wire/globenewswire"), "GlobeNewswire");
    assert_eq!(source_label("press_wire/somewire"), "Press wire");
}

#[test]
fn off_does_nothing_and_the_snapshot_says_so() {
    let mut b = PendingBoard::new();
    b.on_status("XLAB", StatusSignal::Halt, "T1", T0, q(2.0, 1.0), T0);
    assert!(b.card("XLAB").is_none());
    let s: Value = serde_json::from_str(&b.snapshot_frame()).unwrap();
    assert_eq!(s["type"], "pending.snapshot");
    assert_eq!(s["enabled"], false);
    assert_eq!(s["cards"].as_array().unwrap().len(), 0);
}

#[test]
fn the_seam_reads_a_news_item() {
    let mut item = stock_context::NewsItem {
        id: "news:alpaca:1".into(),
        version: 1,
        instrument: stock_context::InstrumentId::symbol("XLAB"),
        event_type: "financing".into(),
        source: "alpaca/benzinga".into(),
        credible_source: true,
        relevant: true,
        substantive: true,
        authenticity: stock_context::Authenticity::IssuerAnnounced,
        favorability: stock_context::Favorability::Undetermined,
        published_at: Some(T0 * 1_000_000),
        observed_at: (T0 + 5_000) * 1_000_000,
        tag: stock_context::Tag::Present,
        content_key: String::new(),
        revision_at: None,
        rules: "news-rules-v2".into(),
        url: Some("https://example.test/story".into()),
        headline: Some("XLAB announces trial results".into()),
        halt: None,
    };
    let (sym, h) = hit_from_item(&item).unwrap();
    assert_eq!(sym, "XLAB");
    assert_eq!(h.kind, HitKind::Offering);
    assert_eq!(h.at_ms, T0);
    assert_eq!(h.seen_ms, T0 + 5_000);
    // The seam's headline and url reach the card.
    assert_eq!(h.headline.as_deref(), Some("XLAB announces trial results"));
    assert_eq!(h.url.as_deref(), Some("https://example.test/story"));
    item.event_type = "clinical_result".into();
    assert_eq!(hit_from_item(&item).unwrap().1.kind, HitKind::Catalyst);
    // An offering headline flags the card even when the class does not.
    item.headline = Some("XLAB Announces Pricing of $5 Million Registered Direct Offering".into());
    assert_eq!(hit_from_item(&item).unwrap().1.kind, HitKind::Offering);
    item.headline = Some("XLAB announces trial results".into());
    item.source = "halt_t1/T1".into();
    assert!(hit_from_item(&item).is_none(), "our own T1 echo");
    item.source = "alpaca/benzinga".into();
    item.tag = stock_context::Tag::Removed;
    assert!(hit_from_item(&item).is_none());
}

/// FMP news recorded for the audit only (record-only `shadow_*`, noise, third-party) never
/// reaches a card; a counting FMP catalyst or offering does (market_providers::import::fmp_news).
#[test]
fn record_only_fmp_news_never_reaches_a_card() {
    use market_providers::import::fmp_news::RULES;
    let rec = |kind: &str, event_type: Option<&str>| market_archive::SourceNewsRecord {
        v: 1,
        provider: "press_wire".into(),
        source: "fmp/globenewswire".into(),
        id: "fmp:abc".into(),
        version: 1,
        symbols: vec!["XLAB".into()],
        published_at: T0 * 1_000_000,
        first_observed_at: (T0 + 5_000) * 1_000_000,
        headline: Some("XLAB Announces Positive Topline Results".into()),
        url: Some("https://example.test/r".into()),
        kind: kind.into(),
        event_type: event_type.map(str::to_string),
        rules: RULES.into(),
    };
    let item = |kind: &str, et: Option<&str>| {
        market_archive::evidence_load::source_news_items(&[rec(kind, et)], None).remove(0)
    };
    for et in ["shadow_catalyst", "shadow_offering", "noise", "third_party"] {
        assert!(hit_from_item(&item("recorded", Some(et))).is_none(), "{et}");
    }
    assert_eq!(
        hit_from_item(&item("catalyst", None)).unwrap().1.kind,
        HitKind::Catalyst
    );
    assert_eq!(
        hit_from_item(&item("offering", None)).unwrap().1.kind,
        HitKind::Offering
    );
    // A recorded item from any other source is unchanged (an SEC 8-K/A still shows).
    let mut other = rec("recorded", None);
    other.rules = "wire-rules-v1".into();
    let n = market_archive::evidence_load::source_news_items(&[other], None).remove(0);
    assert!(hit_from_item(&n).is_some());
}

/// The board in a live market: on with the band, its snapshot from the core, and an update
/// broadcast only while someone subscribes (one frame per change).
#[tokio::test]
async fn a_live_market_snapshots_and_broadcasts_to_subscribers_only() {
    use crate::market::{Market, OutKind};
    let live = Market::idle(market_providers::ClockMode::Live, "idle", None);
    live.enable_pending(90);
    let mut rx = live.subscribe();
    let push = |sym: &str| {
        let frame = {
            let mut core = live.0.core.lock().unwrap();
            core.pending
                .on_status(sym, StatusSignal::Halt, "T1", T0, q(2.0, 1.0), T0);
            core.pending.take_update()
        };
        live.pending_publish(frame);
    };
    // Nobody subscribed: no frame on the shared feed.
    push("AAA");
    let s: Value = serde_json::from_str(&live.pending_snapshot()).unwrap();
    assert_eq!(s["enabled"], true);
    assert_eq!(s["grace_secs"], 90);
    assert_eq!(s["gen"], 1);
    assert_eq!(s["cards"][0]["sym"], "AAA");
    let guard = live.pending_subscribe();
    push("BBB");
    let mut got = None;
    while let Ok(o) = rx.try_recv() {
        if let OutKind::Pending(f) = &o.kind {
            assert!(got.is_none(), "one frame per change");
            got = Some(serde_json::from_str::<Value>(f).unwrap());
        }
    }
    let u = got.expect("a pending.update");
    assert_eq!(u["prev_gen"], 1);
    assert_eq!(u["cards"].as_array().unwrap().len(), 1);
    assert_eq!(u["cards"][0]["sym"], "BBB");
    drop(guard);
    push("CCC");
    while let Ok(o) = rx.try_recv() {
        assert!(
            !matches!(o.kind, OutKind::Pending(_)),
            "unsubscribed: nothing"
        );
    }
}

/// The scheduled corporate-action halt (19:50:00 ET, labelled news pending, the evening before a
/// reverse split) never makes a card; a real T1 a minute later does, tagged after hours.
#[test]
fn the_1950_corporate_action_halt_is_not_news() {
    let at_1950 = T0 + 10 * 60 * MIN + 40 * MIN; // 2025-11-24 19:50:00 ET
    assert!(scheduled_action_halt(at_1950));
    assert!(scheduled_action_halt(at_1950 + 999));
    assert!(!scheduled_action_halt(at_1950 + 1_000));
    assert!(!scheduled_action_halt(T0));
    let mut b = board();
    b.on_status(
        "SPLT",
        StatusSignal::Halt,
        "T1",
        at_1950 + 120,
        q(1.0, 0.9),
        at_1950,
    );
    assert!(b.card("SPLT").is_none());
    assert!(b.take_update().is_none());
    // Its later codes do not make one either (the episode began at 19:50:00).
    b.on_status(
        "SPLT",
        StatusSignal::Halt,
        "T3",
        at_1950 + 60_000,
        no_q(),
        at_1950,
    );
    assert!(b.card("SPLT").is_none());
    b.on_status(
        "NEWS",
        StatusSignal::Halt,
        "T1",
        at_1950 + MIN,
        q(1.0, 0.9),
        at_1950,
    );
    let u = update(&mut b);
    assert_eq!(u["cards"][0]["session"], "after");
    b.on_status("EARLY", StatusSignal::Halt, "T1", T0, q(1.0, 0.9), T0);
    assert_eq!(update(&mut b)["cards"][0]["session"], "pre");
}
