//! Practice notices: what the REPLAY market's detectors decided, as in-app toasts on the practice
//! page only (owner, 2026-11-23: practice alerts are in-app only and never a phone push, ntfy or
//! a desktop OS notification).
//!
//! # Where they go, and where they cannot
//!
//! [`crate::market::Market`] routes every notice a detector offers through one function
//! (`offer_notices`). On the live market that hands them to the alerts task
//! (`crate::alerts::runners`: phone push, ntfy, the `runners.notice` desk frames, the notices
//! ledger and the Insights log). On the replay market it calls `offer_practice` instead, which
//! takes no alert sink and reaches none: it serializes each notice with [`frame`] and broadcasts
//! it on the replay market's own channel (`OutKind::Practice`). Only connections that said
//! `hello.source: "replay"` read that channel, so a live connection can never receive one, and
//! nothing on the alerts side ever sees them.
//!
//! - Not while a seek or day switch rebuilds the day: what the re-read decides is the past, not
//!   news (the same reason a rebuild sends no `runners.event`).
//! - Never in a snapshot: a toast is a moment. A reconnect does not replay them.
//! - The text is the live push's own ([`crate::alerts::runners::message`]), so a practice toast
//!   reads exactly as the real alert would have, with `practice: true` and the replay day beside
//!   it.
//! - `kind` is the same vocabulary as `runners.notice` (`crate::alerts::runners::data_kind`), so a
//!   client can apply its own per-kind switches. A client must show these as in-app toasts with a
//!   "Practice" label and never as a system notification.

use chrono::NaiveDate;
use serde_json::json;

use crate::rocket::Notice;

/// The `welcome.features` entry saying this server sends [`frame`]s to replay connections.
pub const FEATURE: &str = "practice.notice.v1";

/// The `practice.notice` frame for one notice the replay market decided at `day`.
pub fn frame(n: &Notice, day: Option<NaiveDate>) -> String {
    let (title, body, data) = crate::alerts::runners::message(n);
    json!({
        "type": "practice.notice",
        "practice": true,
        "id": format!("practice-{}", n.event_id()),
        "alertId": n.alert_id,
        "symbol": n.symbol,
        "kind": crate::alerts::runners::data_kind(n.kind),
        "title": title,
        "body": body,
        "data": data,
        // The replay clock's time of the decision (epoch ms), never the wall clock.
        "at": n.at_ns.div_euclid(1_000_000),
        "day": day.map(|d| d.to_string()),
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rocket::NoticeKind;
    use serde_json::Value;

    #[test]
    fn the_practice_frame_is_its_own_type_with_the_live_text() {
        let mut n = Notice::test(NoticeKind::New, "CJMB");
        n.at_ns = 1_761_200_000_123_000_000;
        let day = NaiveDate::from_ymd_opt(2026, 11, 17);
        let f: Value = serde_json::from_str(&frame(&n, day)).unwrap();
        let (title, body, _) = crate::alerts::runners::message(&n);
        // Never `runners.notice`: no client's live notifier may take it for a real alert.
        assert_eq!(f["type"], "practice.notice");
        assert_eq!(f["practice"], true);
        assert_eq!(f["title"], title.as_str());
        assert_eq!(f["body"], body.as_str());
        assert_eq!(f["symbol"], "CJMB");
        assert_eq!(
            f["kind"],
            crate::alerts::runners::data_kind(NoticeKind::New)
        );
        assert_eq!(f["at"], 1_761_200_000_123_i64);
        assert_eq!(f["day"], "2026-11-17");
        assert!(f["id"].as_str().unwrap().starts_with("practice-"));
    }
}
