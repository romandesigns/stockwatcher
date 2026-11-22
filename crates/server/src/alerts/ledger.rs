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
//!   `held` the limit's code (`no_device`, `quiet_hours`, `stale`, `duplicate`,
//!   `switched_off`) or `null`,
//!   `devices` how many devices or open clients wanted the kind, `spread_bp` the fire-time
//!   bid-ask spread in bp of the mid (`crate::spread`; `null` when no market quote qualified or
//!   on lines before 2026-11-19). Since 2026-11-22 (research v2 DEPLOY-PLAN, "Per-alert
//!   ledger") every line also has `detector` (the switchable detector's id, `super::detectors`,
//!   else the notice kind), `detector_version` (its frozen definition, `null` for a kind with no
//!   detectors entry), `switch` (`on` | `off`, `null` likewise) and `session` (`pre` | `regular`
//!   | `after` | `overnight` of `at_ns`, ET; `null` outside any). A switched-off detector's lines
//!   are `held: "switched_off"`, its would-be alerts (the `run` cut by default). Per item:
//!   x-float-1 (`v2.xfloat`) adds `arrival_ns` (the close print's arrival at the server) and
//!   `x_float` (`float`, `float_source`, `shares`, `rotation`, `c1`, `gain`, `prior_high`,
//!   `emitted_ns`); x-gap-1 / x-ctx-1 (`v2.xgap` / `v2.xctx`, `crate::gap_ctx`) add
//!   `arrival_ns` (`null` when the feed did not stamp it), `emitted_ns` (the server's decision)
//!   and `facts` (the trigger's numbers).
//!   A Signals crossing (rw-mp5-1, `crate::mp5_gate`) has `detector` `rw-mp5-1`, its
//!   `detector_version`, `decided_ns` (the gate's decision instant), its own `session` (the
//!   research's `pre` | `regular` | `after` from 04:00 ET) and `gate` (`{chg, rvol_day, runs90,
//!   completed_buckets, why, pass}`; non-finite numbers as `"inf"` / `"nan"`), sent or held
//!   (`held`: `mp5_gate`, `mp5_dedup`, `switched_off`). Lines before 2026-10-30 may also
//!   hold `symbol_limit` or `merged` (with `merged_into` and `merged_devices`), the per-stock gap
//!   and merge since removed (`super::runners`, "Every distinct event is delivered").
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
        self.decision_ext(
            n, event_id, path, decision, devices, title, body, now_ms, None,
        );
    }

    /// [`Ledger::decision`] with a detector's own fields merged into the line (module docs).
    #[allow(clippy::too_many_arguments)]
    pub fn decision_ext(
        &self,
        n: &Notice,
        event_id: &str,
        path: &'static str,
        decision: &Decision,
        devices: usize,
        title: &str,
        body: &str,
        now_ms: i64,
        extra: Option<&Value>,
    ) {
        let mut line = decision_line(n, event_id, path, decision, devices, title, body, now_ms);
        if let (Some(Value::Object(x)), Value::Object(l)) = (extra, &mut line) {
            for (k, v) in x {
                l.insert(k.clone(), v.clone());
            }
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
        // The fire-time spread (`crate::spread`), `null` when none qualified: kept with the
        // decision so the cost of each alert can be studied later (board rows lose theirs on a
        // restart; this line does not).
        "spread_bp": n.spread_bp,
        "session": session_of(n.at_ns),
    });
    // Research v2's per-alert ledger: which frozen definition decided it, whether its switch was
    // on, and its session (the daily checklist reads these per detector and session).
    let det = super::detectors::of(n.kind);
    line["detector"] = json!(det.map_or(n.kind.as_str(), |d| d.id));
    line["detector_version"] = json!(det.map(|d| d.version));
    line["switch"] = json!(det.map(|_| {
        if matches!(decision, Decision::Held(runners::Hold::SwitchedOff)) {
            "off"
        } else {
            "on"
        }
    }));
    if let Some(crate::rocket::NoticeDetail::XFloat(x)) = &n.detail {
        // The facts it was judged on, held or sent: the daily checklist compares them with the
        // research's (float source and rotation especially; crate::xfloat_live docs).
        line["arrival_ns"] = json!(x.arrival_ns);
        line["x_float"] = json!({
            "float": x.float, "float_source": x.float_source, "shares": x.shares,
            "rotation": x.rotation, "c1": x.c1, "gain": x.gain, "prior_high": x.prior_high,
            "emitted_ns": x.emitted_ns,
        });
    }
    if let Some(crate::rocket::NoticeDetail::Pullback(p)) = &n.detail {
        // Persist the frozen observation even when delivery was held. None remains unknown.
        line["mp5_wick"] = json!(p.wick);
    }
    if let Some(crate::rocket::NoticeDetail::GapCtx(x)) = &n.detail {
        // The deploy plan's per-alert ledger: who decided it, on what, when it arrived and when
        // it was decided, whether delivered or held.
        line["detector"] = json!(x.detector);
        line["detector_version"] = json!(x.version);
        line["arrival_ns"] = json!(x.arrival_ns);
        line["emitted_ns"] = json!(x.emitted_ns);
        line["session"] = json!(x.session);
        line["facts"] = json!({
            "gap": x.gap, "cum": x.cum, "trail": x.trail, "rvol": x.rvol,
            "base_kind": x.base_kind, "level": x.level, "c1": x.c1, "prev_run": x.prev_run,
        });
    }
    if let Some(evidence) = n.source_evidence() {
        line["breakout"] = evidence;
        line["entry_context"] = json!(n.reasons);
    }
    line
}

/// The session `at_ns` falls in (`market_events::session::classify`), as the ledger spells it.
fn session_of(at_ns: i64) -> Option<&'static str> {
    use market_events::session::SessionKind as K;
    market_events::session::classify(market_events::UnixNanos(at_ns)).map(|s| match s.kind {
        K::Overnight => "overnight",
        K::Pre => "pre",
        K::Regular => "regular",
        K::After => "after",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rocket::NoticeKind;

    /// Research v2's per-alert ledger fields: a switched-off detector's would-be alert (the
    /// `run` cut) is a held line with its detector, version and switch; a kind with no
    /// detectors entry carries its kind and no version; every line carries its session.
    #[test]
    fn decisions_carry_the_detector_switch_and_session() {
        let mut n = notice();
        n.kind = NoticeKind::Run;
        // 2025-10-22 10:00 ET (regular session).
        n.at_ns = 1_761_141_600_000_000_000;
        let off = Decision::Held(runners::Hold::SwitchedOff);
        let line = decision_line(&n, "e", "phone", &off, 1, "t", "b", 1);
        assert_eq!(line["decision"], "held");
        assert_eq!(line["held"], "switched_off");
        assert_eq!(line["detector"], "run");
        assert_eq!(
            line["detector_version"],
            super::super::detectors::RUN.version
        );
        assert_eq!(line["switch"], "off");
        assert_eq!(line["session"], "regular");
        n.kind = NoticeKind::ResumedUp;
        let line = decision_line(&n, "e", "desk", &Decision::Send, 1, "t", "b", 1);
        assert_eq!(line["detector"], "x-halt-1");
        assert_eq!(line["detector_version"], crate::xhalt_live::logic::VERSION);
        assert_eq!(line["switch"], "on");
        n.kind = NoticeKind::FollowUp;
        n.at_ns = 1_761_120_000_000_000_000; // 04:00 ET: pre-market
        let line = decision_line(&n, "e", "desk", &Decision::Send, 1, "t", "b", 1);
        assert_eq!(line["detector"], "followup");
        assert!(line["detector_version"].is_null() && line["switch"].is_null());
        assert_eq!(line["session"], "pre");
    }

    /// A research-v2 detector's line names the detector and its version, the session, the close
    /// print's arrival and the facts; a switched-off one is held with its own code.
    #[test]
    fn an_x_float_line_carries_the_deploy_plan_fields() {
        let mut n = notice();
        n.kind = NoticeKind::XFloat;
        n.detail = Some(crate::rocket::NoticeDetail::XFloat(
            crate::xfloat_live::XFloatNotice {
                float: 8.2e6,
                float_source: "fmp",
                shares: 2_570_000,
                rotation: 0.3134,
                c1: 2.36,
                gain: 0.148,
                prior_high: 2.69,
                arrival_ns: Some(n.at_ns - 40_000_000),
                emitted_ns: n.at_ns + 1_000_000,
            },
        ));
        let held = Decision::Held(runners::Hold::SwitchedOff);
        let v = decision_line(&n, "x1", "phone", &held, 1, "t", "b", 1_793_055_061_000);
        assert_eq!(v["detector"], "x-float-1");
        assert_eq!(v["detector_version"], crate::xfloat_live::logic::VERSION);
        assert_eq!(v["held"], "switched_off");
        assert_eq!(v["notice"], "v2.xfloat");
        assert_eq!(v["arrival_ns"], n.at_ns - 40_000_000);
        assert_eq!(v["x_float"]["shares"], 2_570_000);
        assert_eq!(v["x_float"]["float_source"], "fmp");
        // 2026-10-26 08:23:47 ET.
        assert_eq!(v["session"], "pre");
        // A kind with no runtime switch: its kind as the detector, no version, no facts.
        let p = decision_line(&notice(), "p1", "phone", &held, 1, "t", "b", 0);
        assert_eq!(p["detector"], notice().kind.as_str());
        assert!(p["detector_version"].is_null() && p.get("x_float").is_none());
    }

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

    /// The deploy plan's per-alert fields on a v2 detector's line (`crate::gap_ctx`), sent or
    /// held by the `detectors` switch alike.
    #[test]
    fn a_v2_detector_line_names_its_detector_arrival_session_and_facts() {
        let mut n = notice();
        n.kind = NoticeKind::XGap;
        n.detail = Some(crate::rocket::NoticeDetail::GapCtx(
            crate::gap_ctx::GapCtxNotice {
                detector: "x-gap-1",
                version: crate::gap_ctx::logic::XGAP_VERSION,
                gap: 0.2,
                cum: 130_000.0,
                trail: 120_000.0,
                rvol: None,
                base_kind: 20,
                level: 1.05,
                c1: Some(1.0),
                prev_run: None,
                session: "pre",
                arrival_ns: Some(n.at_ns - 30_000_000),
                emitted_ns: n.at_ns + 10_000_000,
            },
        ));
        for d in [Decision::Send, Decision::Held(runners::Hold::SwitchedOff)] {
            let line = decision_line(&n, "e", "phone", &d, 1, "t", "b", 1);
            assert_eq!(line["notice"], "v2.xgap");
            assert_eq!(line["detector"], "x-gap-1");
            assert_eq!(
                line["detector_version"],
                crate::gap_ctx::logic::XGAP_VERSION
            );
            assert_eq!(line["arrival_ns"], n.at_ns - 30_000_000);
            assert_eq!(line["emitted_ns"], n.at_ns + 10_000_000);
            assert_eq!(line["session"], "pre");
            assert_eq!(line["facts"]["rvol"], Value::Null, "a zero baseline");
            assert_eq!(line["facts"]["cum"], 130_000.0);
        }
        let held = decision_line(
            &n,
            "e",
            "desk",
            &Decision::Held(runners::Hold::SwitchedOff),
            0,
            "t",
            "b",
            1,
        );
        assert_eq!(held["held"], "switched_off");
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
