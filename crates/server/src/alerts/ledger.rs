//! The notification ledger (owner, 2026-10-26): one JSON line per notice the server decided, sent
//! or held and why, in `ARCHIVE_ROOT/notices/<ET date>.jsonl` (`crate::oplog`, 90 days). It is
//! what answers "why didn't I get this alert?" after the fact; the container's own log is erased
//! by every deploy.
//!
//! Lines (all `"v": 1`, times epoch ms unless named `_ns`):
//!
//! - `decision` — `{kind, v, decided_at, event_id, notice, symbol, price, at_ns, exec_ns, path,
//!   decision, held, devices, title, body, spread_bp}` for every Runners-pipeline notice (Runners paths,
//!   follow-ups, shelf, Ross/Signals crossing and trigger, pullback, halt), once for the desktop
//!   and web clients (`path: "desk"`) and once for the phones (`path: "phone"`). `notice` is the
//!   data kind the clients see (`runners.burst`, `ross.crossing`, …), `decision` `sent` | `held`,
//!   `held` the limit's code (`no_device`, `quiet_hours`, `stale`, `duplicate`) or `null`,
//!   `devices` how many devices or open clients wanted the kind, `spread_bp` the fire-time
//!   bid-ask spread in bp of the mid (`crate::spread`; `null` when no market quote qualified or
//!   on lines before 2026-11-19). Lines before 2026-10-30 may also
//!   hold `symbol_limit` or `merged` (with `merged_into` and `merged_devices`), the per-stock gap
//!   and merge since removed (`super::runners`, "Every distinct event is delivered").
//! - `alert` — `{kind, v, decided_at, event_id, alert_id, symbol, price, text, phone, held,
//!   devices}`: a price alert that fired, with its phone decision (`sent`, `off`, `held`).
//! - `pushed` — `{kind, v, event_id, notice, symbol, exec_ns, decided_ns, queued_ns, expo_ns,
//!   trade_to_board_ms, board_to_queue_ms, queue_to_expo_ms, trade_to_expo_ms}` when Expo took a
//!   Runners push (the latency the log line already reported).
//! - `failed` — `{kind, v, at, event_id, detail}` when Expo refused a push or its receipt failed.
//! - `confidence` — `{kind, v, decided_at, event_id, alert_id, symbol, notice, detector,
//!   detector_version, a_ns, t_ns, margin, score, tier, tagged, held, q_a, q_t}`: b-good-1's
//!   score of a B0-kind alert 30 s after it (`crate::bgood`), for every alert scored, tagged or
//!   not; `held` is `switched_off` when its runtime switch was off (decided, not shown), else
//!   `null`; `q_a` / `q_t` are the quotes at the alert and at the scoring instant (`{bid, ask,
//!   age_ms, flags}`, micro-dollars, the xsim flags), so the score can be recomputed.
//!
//! `decision` lines of research detectors also carry `detector` and `detector_version`
//! (`b-good-2`: the model's `score`, `margin` and the day's pick number `nth`), and every
//! `decision` line carries `session` (`pre` 04:00–09:30, `regular`, `after` 16:00–20:00 ET, from
//! `at_ns`; `null` outside them), all additive.
//!
//! Never a push token: the ledger only ever gets the device count, and Expo error details pass
//! through [`crate::oplog::redact_tokens`].

use serde_json::{Value, json};

use crate::oplog::{self, DailyLog};
use crate::rocket::Notice;

use super::runners::{self, Decision};

/// The ledger, or nothing (tests, a replay-only server), and the pushed notification log that
/// every line also goes to (`crate::notices::NoticeFeed`, `notices.v1`).
#[derive(Clone, Debug, Default)]
pub struct Ledger {
    log: Option<DailyLog>,
    feed: Option<crate::notices::NoticeFeed>,
}

impl Ledger {
    pub fn new(log: DailyLog) -> Self {
        Ledger {
            log: Some(log),
            feed: None,
        }
    }

    /// Every line written also goes to `feed`, the Insights log's pushed copy.
    pub fn with_feed(mut self, feed: crate::notices::NoticeFeed) -> Self {
        self.feed = Some(feed);
        self
    }

    /// The ledger under `archive_root` (module docs).
    pub fn start(archive_root: &std::path::Path) -> Self {
        Ledger::new(DailyLog::start(
            "notices",
            oplog::DailyLogConfig {
                dir: archive_root.join(oplog::NOTICES_DIR),
                prefix: "",
                retention_days: oplog::NOTICES_DAYS,
                cap_bytes: oplog::NOTICES_CAP,
            },
        ))
    }

    fn put(&self, at_ms: i64, line: Value) {
        let day = oplog::et_date_ms(at_ms);
        // The feed first: it regroups and pushes now, while the file write is queued for the
        // log's own thread (the feed's snapshots read its memory, never this file, for today).
        if let Some(feed) = &self.feed {
            feed.push(day, &line);
        }
        if let Some(log) = &self.log {
            log.send(day, move || line.to_string());
        }
    }

    /// A Runners-pipeline notice's decision on one path (module docs, `decision`).
    #[allow(clippy::too_many_arguments)]
    pub fn decision(
        &self,
        n: &Notice,
        event_id: &str,
        path: &'static str,
        decision: &Decision,
        devices: usize,
        title: &str,
        body: &str,
        now_ms: i64,
    ) {
        let line = decision_line(n, event_id, path, decision, devices, title, body, now_ms);
        self.put(now_ms, line);
    }

    /// A fired price alert and its phone decision (module docs, `alert`).
    #[allow(clippy::too_many_arguments)]
    pub fn alert(
        &self,
        event_id: &str,
        alert_id: &str,
        symbol: &str,
        price: Option<f64>,
        text: &str,
        phone: &str,
        held: Option<&str>,
        devices: usize,
        now_ms: i64,
    ) {
        self.put(
            now_ms,
            json!({
                "kind": "alert", "v": 1, "decided_at": now_ms, "event_id": event_id,
                "alert_id": alert_id, "symbol": symbol, "price": price, "text": text,
                "phone": phone, "held": held, "devices": devices,
            }),
        );
    }

    /// Expo took a Runners push (module docs, `pushed`).
    #[allow(clippy::too_many_arguments)]
    pub fn pushed(
        &self,
        event_id: &str,
        notice: &str,
        symbol: &str,
        exec_ns: Option<i64>,
        decided_ns: i64,
        queued_ns: i64,
        expo_ns: i64,
    ) {
        let ms = |a: i64, b: i64| (b - a) as f64 / 1e6;
        self.put(
            expo_ns.div_euclid(1_000_000),
            json!({
                "kind": "pushed", "v": 1, "event_id": event_id, "notice": notice, "symbol": symbol,
                "exec_ns": exec_ns, "decided_ns": decided_ns, "queued_ns": queued_ns, "expo_ns": expo_ns,
                "trade_to_board_ms": exec_ns.map(|e| ms(e, decided_ns)),
                "board_to_queue_ms": ms(decided_ns, queued_ns),
                "queue_to_expo_ms": ms(queued_ns, expo_ns),
                "trade_to_expo_ms": exec_ns.map(|e| ms(e, expo_ns)),
            }),
        );
    }

    /// A b-good-1 score (module docs, `confidence`). `on`: the detector's switch was on.
    pub fn confidence(&self, c: &crate::bgood::Confidence, on: bool, now_ms: i64) {
        let q = |q: &crate::bgood::logic::QuoteAt| json!({"bid": q.bid_u, "ask": q.ask_u, "age_ms": q.age_ms_f() as i64, "flags": q.flags});
        self.put(
            now_ms,
            json!({
                "kind": "confidence", "v": 1, "decided_at": now_ms, "event_id": c.event_id,
                "alert_id": c.alert_id, "symbol": c.symbol, "notice": runners::data_kind(c.kind),
                "detector": crate::bgood::TAG_ID,
                "detector_version": crate::bgood::version(crate::bgood::model1()),
                "a_ns": c.a_ns, "t_ns": c.t_ns, "margin": c.margin, "score": c.prob,
                "tier": c.tier(), "tagged": c.tagged,
                "held": if on { Value::Null } else { json!("switched_off") },
                "q_a": q(&c.q_a), "q_t": q(&c.q_t),
            }),
        );
    }

    /// A push Expo refused (module docs, `failed`).
    pub fn failed(&self, event_id: &str, detail: &str, now_ms: i64) {
        self.put(
            now_ms,
            json!({
                "kind": "failed", "v": 1, "at": now_ms, "event_id": event_id,
                "detail": oplog::redact_tokens(detail),
            }),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn decision_line(
    n: &Notice,
    event_id: &str,
    path: &'static str,
    decision: &Decision,
    devices: usize,
    title: &str,
    body: &str,
    now_ms: i64,
) -> Value {
    let (word, held) = match decision {
        Decision::Send => ("sent", None),
        Decision::Held(h) => ("held", Some(h.code())),
    };
    let mut line = json!({
        "kind": "decision", "v": 1, "decided_at": now_ms, "event_id": event_id,
        "notice": runners::data_kind(n.kind), "symbol": n.symbol, "price": n.price,
        "at_ns": n.at_ns, "exec_ns": n.exec_ns, "path": path, "decision": word, "held": held,
        "devices": devices, "title": title, "body": body,
        // The fire-time spread (`crate::spread`), `null` when none qualified: kept with the
        // decision so the cost of each alert can be studied later (board rows lose theirs on a
        // restart; this line does not).
        "spread_bp": n.spread_bp,
        "session": session_of(n.at_ns),
    });
    if let Some(crate::rocket::NoticeDetail::BGood(b)) = &n.detail {
        line["detector"] = json!(crate::bgood::PICK_ID);
        line["detector_version"] = json!(b.version);
        line["score"] = json!(b.prob);
        line["margin"] = json!(b.margin);
        line["nth"] = json!(b.nth);
    }
    if let Some(crate::rocket::NoticeDetail::Pullback(p)) = &n.detail {
        // Persist the frozen observation even when delivery was held. None remains unknown.
        line["mp5_wick"] = json!(p.wick);
    }
    if let Some(evidence) = n.source_evidence() {
        line["breakout"] = evidence;
        line["entry_context"] = json!(n.reasons);
    }
    line
}

/// `pre` / `regular` / `after` for an instant (the trading calendar's sessions), `None` overnight
/// or on a closed day.
fn session_of(at_ns: i64) -> Option<&'static str> {
    use market_events::session::SessionKind;
    let s = market_events::session::classify(market_events::UnixNanos(at_ns))?;
    match s.kind {
        SessionKind::Pre => Some("pre"),
        SessionKind::Regular => Some("regular"),
        SessionKind::After => Some("after"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rocket::NoticeKind;

    #[test]
    fn sent_and_held_breakouts_keep_the_same_frozen_cause_and_entry_context() {
        let mut n = notice();
        n.kind = NoticeKind::Breakout;
        n.reasons = vec!["Earlier Run +20% context".into()];
        n.detail = Some(crate::rocket::NoticeDetail::Breakout(
            serde_json::from_str(include_str!("../../tests/fixtures/breakout-notice-v1.json"))
                .unwrap(),
        ));
        for decision in [Decision::Send, Decision::Held(runners::Hold::QuietHours)] {
            let line = decision_line(&n, "b1", "phone", &decision, 1, "t", "b", 1_793_055_061_000);
            assert_eq!(line["breakout"], n.source_evidence().unwrap());
            assert_eq!(line["entry_context"], json!(n.reasons));
            assert_eq!(line["at_ns"], n.at_ns);
            assert_eq!(line["decided_at"], 1_793_055_061_000i64);
        }
    }

    fn notice() -> Notice {
        Notice {
            spread_bp: None,
            kind: NoticeKind::Pullback,
            alert_id: "e1".into(),
            symbol: "SRTM".into(),
            at_ns: 1_793_017_427_000_000_000,
            price: 0.55,
            from_flag_pct: None,
            reasons: Vec::new(),
            since_start_min: None,
            exec_ns: None,
            detail: None,
        }
    }

    #[test]
    fn a_decision_line_names_the_limit_and_carries_no_token() {
        let held = Decision::Held(runners::Hold::QuietHours);
        let v = decision_line(
            &notice(),
            "ev-1",
            "phone",
            &held,
            2,
            "SRTM · Pullback",
            "Trigger $0.55",
            1_793_017_427_256,
        );
        assert_eq!(v["kind"], "decision");
        assert_eq!(v["v"], 1);
        assert_eq!(v["notice"], "runners.pullback");
        assert_eq!(v["decision"], "held");
        assert_eq!(v["held"], "quiet_hours");
        assert_eq!(v["devices"], 2);
        assert_eq!(v["path"], "phone");
        let sent = decision_line(&notice(), "ev-1", "desk", &Decision::Send, 1, "t", "b", 0);
        assert_eq!(sent["decision"], "sent");
        assert!(sent["held"].is_null());
        // The fields are fixed: nothing a device credential could ride in.
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert!(!keys.iter().any(|k| k.contains("token")), "{keys:?}");
    }

    /// research v2 b-good: a pick's decision line names its detector and version; every line
    /// carries its session; a switched-off detector's hold is ledgered with its code.
    #[test]
    fn a_research_detector_line_names_detector_version_and_session() {
        let mut n = notice();
        n.kind = NoticeKind::BGood;
        n.detail = Some(crate::rocket::NoticeDetail::BGood(
            crate::bgood::BGoodNotice {
                margin: -0.3,
                prob: 0.43,
                nth: 2,
                version: "e4273799d70e".into(),
                emitted_ns: n.at_ns,
            },
        ));
        let held = Decision::Held(runners::Hold::SwitchedOff);
        let v = decision_line(&n, "e", "desk", &held, 1, "t", "b", 0);
        assert_eq!(v["notice"], "model.pick");
        assert_eq!(v["held"], "switched_off");
        assert_eq!(v["detector"], "b-good-2");
        assert_eq!(v["detector_version"], "e4273799d70e");
        assert_eq!(v["nth"], 2);
        // 1_793_017_427 s = 2026-10-26 12:23:47 UTC = 08:23:47 ET (pre-market).
        assert_eq!(v["session"], "pre");
        let plain = decision_line(&notice(), "e", "desk", &Decision::Send, 1, "t", "b", 0);
        assert!(plain.get("detector").is_none());
    }

    #[test]
    fn the_ledger_writes_lines_by_et_date() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::start(dir.path());
        ledger.decision(
            &notice(),
            "ev-1",
            "desk",
            &Decision::Send,
            1,
            "t",
            "b",
            1_793_017_427_256,
        );
        ledger.decision(
            &notice(),
            "ev-3",
            "phone",
            &Decision::Held(runners::Hold::QuietHours),
            2,
            "t",
            "b",
            1_793_017_427_300,
        );
        ledger.failed(
            "ev-2",
            "DeviceNotRegistered ExponentPushToken[secret]",
            1_793_017_428_000,
        );
        let path = dir.path().join("notices").join("2026-10-26.jsonl");
        let mut lines = Vec::new();
        for _ in 0..200 {
            lines = std::fs::read_to_string(&path)
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect();
            if lines.len() >= 3 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(lines[0].contains("\"decision\":\"sent\""));
        let held: Value = serde_json::from_str(&lines[1]).unwrap();
        assert_eq!(held["held"], "quiet_hours");
        assert_eq!(held["devices"], 2);
        assert!(!lines[2].contains("secret"), "{}", lines[2]);
        assert!(lines[2].contains("[push token]"));
    }

    /// Every line also reaches the pushed log at once, token-redacted like the file.
    #[test]
    fn the_ledger_feeds_the_pushed_log() {
        let day = oplog::et_date_ms(1_793_017_427_256);
        let feed = crate::notices::NoticeFeed::open(None, day);
        let mut rx = feed.subscribe();
        let ledger = Ledger::default().with_feed(feed.clone());
        ledger.decision(
            &notice(),
            "ev-1",
            "desk",
            &Decision::Send,
            1,
            "t",
            "b",
            1_793_017_427_256,
        );
        ledger.failed("ev-1", "ExponentPushToken[secret]", 1_793_017_427_300);
        let a = rx.try_recv().unwrap();
        assert_eq!(a.day, day);
        let b = rx.try_recv().unwrap();
        assert!(b.seq > a.seq);
        let v: Value = serde_json::from_str(&b.frame).unwrap();
        assert_eq!(v["items"][0]["desk"]["decision"], "sent");
        assert!(!b.frame.contains("secret"));
    }

    #[test]
    fn pullback_diagnostics_survive_sent_held_and_unknown_decisions() {
        for (candle, expected) in [
            (Some([105, 120, 100, 108]), "flagged"),
            (Some([100, 120, 100, 120]), "clear"),
            (None, "unknown"),
        ] {
            let wick = stock_context::MicroWick::measure(candle, 15, 32);
            let mut n = notice();
            n.detail = Some(crate::rocket::NoticeDetail::Pullback(
                crate::rocket::PullbackNotice {
                    episode_id: "e1".into(),
                    trigger: Some(0.12),
                    stop: Some(0.11),
                    dip_pct: None,
                    retraced_pct: None,
                    dip_volume_ratio: None,
                    wick: Some(wick.clone()),
                    also_signals: false,
                },
            ));
            for decision in [Decision::Send, Decision::Held(runners::Hold::QuietHours)] {
                let v = decision_line(&n, "ev-1", "phone", &decision, 1, "t", "b", 0);
                assert_eq!(v["mp5_wick"]["status"], expected);
                assert_eq!(v["mp5_wick"], serde_json::to_value(&wick).unwrap());
            }
        }
    }
}
