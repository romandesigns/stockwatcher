//! Alpaca's news stream frames → stories and control messages. Network-free, so every message
//! shape is unit-tested; the session state machine is in [`super::session`].
//!
//! Reference: `docs.alpaca.markets/us/docs/streaming-real-time-news` and
//! `/us/docs/streaming-market-data` (read 2026-10-21, public documentation, no probe). The news
//! socket carries `news` messages tagged `"T":"n"`, each with the article's `id`, `created_at`
//! and `updated_at`, and control messages share the market-data stream's shapes: `success`,
//! `subscription`, `error`.
//!
//! What this module does **not** decide:
//!
//! - **Whether a story is new.** That is the version index's job, and it is the same index the
//!   REST searches use ([`crate::import::alpaca_news::NewsVersions`]). A stream delivery goes
//!   through `propose` → append → `commit` exactly as a page does, so there is one durable
//!   owner of version state and never two dedup maps racing each other.
//! - **Whether a window has been searched.** A delivery says one story changed. It says nothing
//!   about what else did, so it never writes coverage and never advances the covered-through
//!   point. Coverage is a claim about having read a window in full, and only a completed search
//!   can make it.
//!
//! One bad message never discards its neighbours: each element of a frame is parsed on its own
//! and a failure becomes [`Inbound::Malformed`].

use crate::import::alpaca_news::RawStory;
use serde::Serialize;
use serde_json::Value;

/// One message off the news socket.
#[derive(Clone, Debug, PartialEq)]
pub enum Inbound {
    /// A news article, as the REST search's own story shape.
    Story(RawStory),
    /// `{"T":"success","msg":"connected"}` / `"authenticated"`.
    Success(String),
    /// `{"T":"subscription","news":[...]}`: what the server believes we are subscribed to.
    Subscribed(Vec<String>),
    /// An error the server reported, with its code where it gave one.
    Error { code: Option<i64>, msg: String },
    /// A message of a kind we never subscribe to: counted and ignored.
    Ignored(String),
    /// Unparsable, with why. Its neighbours in the frame are unaffected.
    Malformed(String),
}

/// What an error code means for the session, as far as the public documentation states.
///
/// The distinction that matters is whether retrying can help. A refused *entitlement* or an
/// exceeded connection limit will refuse again; a transient failure will not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Refusal {
    /// `401` not authenticated, `402` auth failed, `404` auth timeout, `405` already
    /// authenticated: the credentials or the handshake, not the subscription.
    Auth,
    /// `406` connection limit exceeded, `409` insufficient subscription: the account may not
    /// have what was asked for, and asking again will not change that.
    Entitlement,
    /// Anything else: treated as transient.
    Transient,
}

/// The documented control-message error codes
/// (`docs.alpaca.markets/us/docs/streaming-market-data`, read 2026-10-21).
pub fn refusal_of(code: Option<i64>) -> Refusal {
    match code {
        Some(401 | 402 | 404 | 405) => Refusal::Auth,
        Some(406 | 409) => Refusal::Entitlement,
        _ => Refusal::Transient,
    }
}

/// Parses one frame: a JSON array of messages, each tagged by `"T"`.
pub fn parse_frame(text: &str) -> Result<Vec<Inbound>, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("frame: {e}"))?;
    let Some(items) = v.as_array() else {
        return Err("frame: not an array".into());
    };
    Ok(items.iter().map(parse_message).collect())
}

fn parse_message(m: &Value) -> Inbound {
    let Some(kind) = m.get("T").and_then(Value::as_str) else {
        return Inbound::Malformed("message: no T".into());
    };
    match kind {
        "n" => match serde_json::from_value::<RawStory>(m.clone()) {
            Ok(s) => Inbound::Story(s),
            Err(e) => Inbound::Malformed(format!("news: {e}")),
        },
        "success" => Inbound::Success(
            m.get("msg")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        ),
        "subscription" => Inbound::Subscribed(
            m.get("news")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        ),
        "error" => Inbound::Error {
            code: m.get("code").and_then(Value::as_i64),
            msg: m
                .get("msg")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        },
        other => Inbound::Ignored(other.to_string()),
    }
}

/// The authentication frame. Never logged: [`super::session::Outbound`] carries it as a kind, and
/// the session's log line names the kind rather than the frame.
pub fn auth_frame(key: &str, secret: &str) -> String {
    serde_json::json!({ "action": "auth", "key": key, "secret": secret }).to_string()
}

/// The subscribe frame. `["*"]` is every symbol, which is what the collector wants: the pillar's
/// question is about any story tagging a watched symbol, and the watch list changes during a day.
pub fn subscribe_frame(symbols: &[String]) -> String {
    let news: Vec<&str> = if symbols.is_empty() {
        vec!["*"]
    } else {
        symbols.iter().map(String::as_str).collect()
    };
    serde_json::json!({ "action": "subscribe", "news": news }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_news_message_is_the_same_story_shape_the_search_returns() {
        let frame = r#"[{"T":"n","id":42,"headline":"ABCD Announces FDA Approval",
            "summary":"s","author":"Staff","created_at":"2026-10-15T13:00:00Z",
            "updated_at":"2026-10-15T13:05:00Z","symbols":["ABCD"],"source":"benzinga",
            "url":"https://example.invalid/42"}]"#;
        let got = parse_frame(frame).expect("a frame");
        assert_eq!(got.len(), 1);
        match &got[0] {
            Inbound::Story(s) => {
                assert_eq!(s.id, 42);
                assert_eq!(s.symbols, vec!["ABCD".to_string()]);
                assert_eq!(s.updated_at.as_deref(), Some("2026-10-15T13:05:00Z"));
                assert_eq!(s.created_at.as_deref(), Some("2026-10-15T13:00:00Z"));
            }
            other => panic!("expected a story, got {other:?}"),
        }
    }

    /// One bad message does not take its neighbours with it.
    #[test]
    fn a_malformed_message_does_not_discard_the_frame() {
        let frame = r#"[{"T":"n","id":"not a number"},
            {"T":"n","id":7,"headline":"h","summary":"","author":"a",
             "created_at":"2026-10-15T13:00:00Z","updated_at":"2026-10-15T13:00:00Z",
             "symbols":["ABCD"],"source":"benzinga","url":"u"},
            {"T":"q"}]"#;
        let got = parse_frame(frame).expect("a frame");
        assert_eq!(got.len(), 3);
        assert!(matches!(got[0], Inbound::Malformed(_)));
        assert!(matches!(&got[1], Inbound::Story(s) if s.id == 7));
        assert_eq!(got[2], Inbound::Ignored("q".into()));
    }

    #[test]
    fn control_messages_are_recognised() {
        let got = parse_frame(
            r#"[{"T":"success","msg":"authenticated"},
                {"T":"subscription","news":["*"]},
                {"T":"error","code":409,"msg":"insufficient subscription"}]"#,
        )
        .expect("a frame");
        assert_eq!(got[0], Inbound::Success("authenticated".into()));
        assert_eq!(got[1], Inbound::Subscribed(vec!["*".into()]));
        assert_eq!(
            got[2],
            Inbound::Error {
                code: Some(409),
                msg: "insufficient subscription".into()
            }
        );
    }

    /// Retrying helps for some refusals and cannot help for others, and they are named apart.
    #[test]
    fn refusals_are_told_apart_by_whether_retrying_could_help() {
        assert_eq!(refusal_of(Some(402)), Refusal::Auth);
        assert_eq!(refusal_of(Some(401)), Refusal::Auth);
        assert_eq!(refusal_of(Some(406)), Refusal::Entitlement);
        assert_eq!(refusal_of(Some(409)), Refusal::Entitlement);
        assert_eq!(refusal_of(Some(500)), Refusal::Transient);
        assert_eq!(refusal_of(None), Refusal::Transient);
    }

    /// The subscribe frame asks for every symbol by default: the watch list changes during a day
    /// and the pillar's question is about any story tagging a watched symbol.
    #[test]
    fn the_subscribe_frame_asks_for_everything_by_default() {
        assert_eq!(
            subscribe_frame(&[]),
            r#"{"action":"subscribe","news":["*"]}"#
        );
        assert_eq!(
            subscribe_frame(&["ABCD".to_string()]),
            r#"{"action":"subscribe","news":["ABCD"]}"#
        );
    }

    /// The auth frame carries the credentials, so nothing may log it.
    #[test]
    fn the_auth_frame_is_the_only_place_credentials_appear() {
        let f = auth_frame("my-key", "my-secret");
        assert!(f.contains("my-secret"), "it does carry them");
        assert_eq!(
            super::super::session::Outbound::Auth.describe(),
            "auth",
            "and what the session logs names the kind, not the frame"
        );
    }

    #[test]
    fn a_frame_that_is_not_an_array_is_refused_whole() {
        assert!(parse_frame(r#"{"T":"n"}"#).is_err());
        assert!(parse_frame("not json").is_err());
    }
}
