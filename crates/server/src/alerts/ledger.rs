//! The notification ledger (owner, 2026-10-26): one JSON line per notice the server decided, sent
//! or held and why, in `ARCHIVE_ROOT/notices/<ET date>.jsonl` (`crate::oplog`, 90 days). It is
//! what answers "why didn't I get this alert?" after the fact; the container's own log is erased
//! by every deploy.
//!
//! Lines (all `"v": 1`, times epoch ms unless named `_ns`):
//!
//! - `decision` — `{kind, v, decided_at, event_id, notice, symbol, price, at_ns, exec_ns, path,
//!   decision, held, devices, title, body}` for every Runners-pipeline notice (Runners paths,
//!   follow-ups, shelf, Ross/Signals crossing and trigger, pullback, halt), once for the desktop
//!   and web clients (`path: "desk"`) and once for the phones (`path: "phone"`). `notice` is the
//!   data kind the clients see (`runners.burst`, `ross.crossing`, …), `decision` `sent` | `held`,
//!   `held` the limit's code (`no_device`, `quiet_hours`, `stale`, `symbol_limit`, `duplicate`)
//!   or `null`, `devices` how many devices or open clients wanted the kind.
//! - `alert` — `{kind, v, decided_at, event_id, alert_id, symbol, price, text, phone, held,
//!   devices}`: a price alert that fired, with its phone decision (`sent`, `off`, `held`).
//! - `pushed` — `{kind, v, event_id, notice, symbol, exec_ns, decided_ns, queued_ns, expo_ns,
//!   trade_to_board_ms, board_to_queue_ms, queue_to_expo_ms, trade_to_expo_ms}` when Expo took a
//!   Runners push (the latency the log line already reported).
//! - `failed` — `{kind, v, at, event_id, detail}` when Expo refused a push or its receipt failed.
//!
//! Never a push token: the ledger only ever gets the device count, and Expo error details pass
//! through [`crate::oplog::redact_tokens`].

use serde_json::{Value, json};

use crate::oplog::{self, DailyLog};
use crate::rocket::Notice;

use super::runners::{self, Decision};

/// The ledger, or nothing (tests, a replay-only server).
#[derive(Clone, Debug, Default)]
pub struct Ledger(Option<DailyLog>);

impl Ledger {
    pub fn new(log: DailyLog) -> Self {
        Ledger(Some(log))
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
        if let Some(log) = &self.0 {
            log.send(oplog::et_date_ms(at_ms), move || line.to_string());
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
        merged: Option<&runners::Merged>,
        title: &str,
        body: &str,
        now_ms: i64,
    ) {
        let mut line = decision_line(n, event_id, path, decision, devices, title, body, now_ms);
        if let Some(m) = merged {
            // What it was folded into, for the Insights log's "merged into the Running alert 1 s
            // earlier" (super::runners, "Merging"); `merged_devices` had that one already.
            line["merged_into"] =
                json!({"event_id": m.into_event, "notice": m.into_notice, "secs": m.secs});
            line["merged_devices"] = json!(m.covered);
        }
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
    });
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
        for decision in [Decision::Send, Decision::Held(runners::Hold::SymbolLimit)] {
            let line = decision_line(&n, "b1", "phone", &decision, 1, "t", "b", 1_793_055_061_000);
            assert_eq!(line["breakout"], n.source_evidence().unwrap());
            assert_eq!(line["entry_context"], json!(n.reasons));
            assert_eq!(line["at_ns"], n.at_ns);
            assert_eq!(line["decided_at"], 1_793_055_061_000i64);
        }
    }

    fn notice() -> Notice {
        Notice {
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
        let held = Decision::Held(runners::Hold::SymbolLimit);
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
        assert_eq!(v["held"], "symbol_limit");
        assert_eq!(v["devices"], 2);
        assert_eq!(v["path"], "phone");
        let sent = decision_line(&notice(), "ev-1", "desk", &Decision::Send, 1, "t", "b", 0);
        assert_eq!(sent["decision"], "sent");
        assert!(sent["held"].is_null());
        // The fields are fixed: nothing a device credential could ride in.
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert!(!keys.iter().any(|k| k.contains("token")), "{keys:?}");
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
            None,
            "t",
            "b",
            1_793_017_427_256,
        );
        // A merged one says what it was folded into (runners, "Merging").
        ledger.decision(
            &notice(),
            "ev-3",
            "phone",
            &Decision::Held(runners::Hold::Merged),
            2,
            Some(&runners::Merged {
                into_event: "runners-running-WFF-1".into(),
                into_notice: "runners.running".into(),
                secs: 1.2,
                covered: 2,
            }),
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
        assert!(!lines[0].contains("merged_into"), "{}", lines[0]);
        let merged: Value = serde_json::from_str(&lines[1]).unwrap();
        assert_eq!(merged["held"], "merged");
        assert_eq!(merged["merged_into"]["notice"], "runners.running");
        assert_eq!(merged["merged_devices"], 2);
        assert!(!lines[2].contains("secret"), "{}", lines[2]);
        assert!(lines[2].contains("[push token]"));
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
            for decision in [Decision::Send, Decision::Held(runners::Hold::SymbolLimit)] {
                let v = decision_line(&n, "ev-1", "phone", &decision, 1, "t", "b", 0);
                assert_eq!(v["mp5_wick"]["status"], expected);
                assert_eq!(v["mp5_wick"], serde_json::to_value(&wick).unwrap());
            }
        }
    }
}
