use serde_json::{Value, json};
use stockwatcher_server::{
    alerts::runners,
    notices,
    rocket::{BreakoutNotice, Notice, NoticeDetail, NoticeKind},
};

fn notice() -> Notice {
    let evidence: BreakoutNotice =
        serde_json::from_str(include_str!("fixtures/breakout-notice-v1.json")).unwrap();
    Notice {
        spread_bp: None,
        no_quote: false,
        kind: NoticeKind::Breakout,
        alert_id: "2026-10-26-AUDIT-1".into(),
        symbol: "AUDIT".into(),
        at_ns: 1_793_055_060_000_000_000,
        price: 1.81,
        from_flag_pct: Some(-1.1),
        reasons: vec![
            "+21% in 7 min off $1.51".into(),
            "25+ trades and $25k+ in the leg".into(),
        ],
        since_start_min: None,
        exec_ns: None,
        detail: Some(NoticeDetail::Breakout(Box::new(evidence))),
    }
}

#[test]
fn current_measurements_reach_phone_desk_and_history_without_reinterpreting_clocks() {
    let n = notice();
    let (title, body, data) = runners::message(&n);
    assert!(title.contains("15s new high since alert"));
    assert!(body.contains("High $1.90 > $1.83"));
    assert!(body.contains("22000 shares/15s"));
    assert!(!body.contains("+21%"));
    assert!(body.ends_with("Alert $1.81 · +5% = $1.90"));
    assert!(body.chars().count() <= runners::MAX_BODY_CHARS);
    assert_eq!(data["event_id"], n.event_id());
    assert_eq!(data["entry_context"], json!(n.reasons));
    assert_eq!(data["breakout"], n.source_evidence().unwrap());
    let desk: Value = serde_json::from_str(&runners::desk_frame(&n)).unwrap();
    assert_eq!(desk["data"], data);
    assert_eq!(desk["at"], 1_793_055_060_000i64);
    let line = json!({"kind":"decision","event_id":n.event_id(),"notice":"runners.breakout","symbol":"AUDIT",
        "decided_at":1_793_055_061_000i64,"path":"desk","decision":"sent","price":n.price,
        "title":title,"body":body,"breakout":data["breakout"],"entry_context":data["entry_context"]});
    let (items, _) = notices::group(&line.to_string());
    assert_eq!(items[0]["at"], 1_793_055_061_000i64);
    assert_eq!(items[0]["breakout"]["event_at_ms"], 1_793_055_060_000i64);
    assert_eq!(
        items[0]["breakout"]["evaluated_at_ms"],
        1_793_055_060_250i64
    );
    assert_eq!(items[0]["breakout"]["flag_known_ns"], "1793055048250000000");
    assert_eq!(items[0]["entry_context"], data["entry_context"]);
}

#[test]
fn micro_price_increments_and_zero_base_fallback_are_described_honestly() {
    let mut n = notice();
    let Some(NoticeDetail::Breakout(e)) = &mut n.detail else {
        unreachable!()
    };
    e.reference_price_micros = 16_000_001;
    e.high_price_micros = 16_000_002;
    e.volume_ratio = None;
    e.pace_slot = 0.0;
    e.zero_base_fallback = true;
    let (_, body, _) = runners::message(&n);
    assert!(body.contains("$16.000002 > $16.000001"));
    assert!(body.contains("prior pace unavailable") || body.contains("unavail"));
    assert!(!body.contains("× prior pace"));
    let legacy = json!({"kind":"decision","event_id":"old","notice":"runners.breakout","decided_at":1000,
        "title":"Original title","body":"Original context","path":"phone","decision":"held"});
    let (items, _) = notices::group(&legacy.to_string());
    assert_eq!(items[0]["title"], "Original title");
    assert!(items[0].get("breakout").is_none());
}
