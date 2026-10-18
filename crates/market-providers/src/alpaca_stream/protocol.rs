//! Alpaca market-data stream (v2 protocol) frames → the event contract. Network-free, so every
//! message shape is unit-tested; the socket side is in [`super::client`].
//!
//! Reference: docs.alpaca.markets "Real-time Stock Data" and "Streaming Market Data", read
//! 2026-10-18; `crates/provider-sim` serves the same shapes locally.
//!
//! - Frames are JSON arrays of messages, each tagged by `"T"`. Several messages (of different
//!   kinds) may share a frame. JSON, not MessagePack: the client asks for it with
//!   `Content-Type: application/json` on the upgrade, and a binary frame is refused rather than
//!   guessed at.
//! - `t` trade → [`Trade`] through the archive importer's own [`to_trade`], so a print arrives as
//!   the same contract event live and from history (the stream message is the historical trade
//!   object plus `T`/`S`).
//! - `c` correction → [`Correction`], `x` cancel/error → [`Cancel`]. Alpaca sends both with the
//!   trade subscription; they are never subscribed separately.
//! - `s` trading status → [`Status`] with the code (`sc`) and reason (`rc`, else `rm`) kept
//!   verbatim and the kind derived from the tape's status-code table (see [`status_kind`]).
//! - `success` / `subscription` / `error` are control messages for the session state machine.
//! - Anything else (`q`, `b`, `u`, `d`, `l`, imbalances) is counted and ignored: we never
//!   subscribe to it, so it can only appear if the provider adds messages.
//!
//! One bad message never discards its neighbours: each element of a frame is normalized on its
//! own, and a failure becomes [`Inbound::Malformed`].

use crate::import::alpaca::{RawTrade, parse_price, parse_time, to_trade};
use market_events::{
    Cancel, CancelAction, Correction, MarketEvent, Source, Status, StatusKind, Tape,
};
use serde::Deserialize;
use serde_json::Value;

/// Which Alpaca stream. `Test` is Alpaca's always-on fake stream (symbol `FAKEPACA`), labelled
/// [`Source::Fixture`] so it can never pose as market evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamFeed {
    Sip,
    Iex,
    Boats,
    Overnight,
    Test,
}

impl StreamFeed {
    pub fn parse(s: &str) -> Result<StreamFeed, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "sip" => Ok(StreamFeed::Sip),
            "iex" => Ok(StreamFeed::Iex),
            "boats" => Ok(StreamFeed::Boats),
            "overnight" => Ok(StreamFeed::Overnight),
            "test" => Ok(StreamFeed::Test),
            other => Err(format!(
                "unknown stream feed {other:?} (sip, iex, boats, overnight, test)"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            StreamFeed::Sip => "sip",
            StreamFeed::Iex => "iex",
            StreamFeed::Boats => "boats",
            StreamFeed::Overnight => "overnight",
            StreamFeed::Test => "test",
        }
    }

    /// Path under the stream base URL. BOATS and overnight are still `v1beta1` at Alpaca.
    pub fn path(self) -> &'static str {
        match self {
            StreamFeed::Sip => "v2/sip",
            StreamFeed::Iex => "v2/iex",
            StreamFeed::Boats => "v1beta1/boats",
            StreamFeed::Overnight => "v1beta1/overnight",
            StreamFeed::Test => "v2/test",
        }
    }

    /// Same mapping as the importer's `Feed::source`, plus `Test` → fixture.
    pub fn source(self) -> Source {
        match self {
            StreamFeed::Sip => Source::AlpacaSip,
            StreamFeed::Iex => Source::AlpacaIex,
            StreamFeed::Boats | StreamFeed::Overnight => Source::AlpacaBoats,
            StreamFeed::Test => Source::Fixture,
        }
    }
}

/// The subscription lists the server echoed (`"T":"subscription"`). Only the lists this client
/// uses are kept.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subscription {
    #[serde(default)]
    pub trades: Vec<String>,
    #[serde(default)]
    pub statuses: Vec<String>,
    #[serde(default)]
    pub corrections: Vec<String>,
    #[serde(default)]
    pub cancel_errors: Vec<String>,
}

/// One element of a frame.
#[derive(Clone, Debug, PartialEq)]
pub enum Inbound {
    Event(MarketEvent),
    /// `{"T":"success","msg":"connected"}`
    Connected,
    /// `{"T":"success","msg":"authenticated"}`
    Authenticated,
    Subscription(Subscription),
    Error {
        code: u16,
        msg: String,
    },
    /// A well-formed message of a kind this client does not use (its `T`).
    Ignored(String),
    /// Could not be normalized; the reason names the field, never echoes the whole frame.
    Malformed(String),
}

/// Parses one text frame. `Err` only when the frame itself is not a JSON array/object; a bad
/// element becomes [`Inbound::Malformed`] and the rest of the frame still counts.
pub fn parse_frame(text: &str, source: Source) -> Result<Vec<Inbound>, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("frame is not JSON: {e}"))?;
    Ok(match v {
        Value::Array(items) => items.iter().map(|m| parse_message(m, source)).collect(),
        // Alpaca always sends arrays; accept a bare object rather than drop it.
        obj @ Value::Object(_) => vec![parse_message(&obj, source)],
        _ => return Err("frame is neither an array nor an object".into()),
    })
}

fn str_field<'a>(m: &'a Value, k: &str) -> Option<&'a str> {
    m.get(k).and_then(Value::as_str)
}

/// One message → [`Inbound`].
pub fn parse_message(m: &Value, source: Source) -> Inbound {
    let Some(kind) = str_field(m, "T") else {
        return Inbound::Malformed("message without \"T\"".into());
    };
    let event = |r: Result<MarketEvent, String>| match r {
        Ok(e) => Inbound::Event(e),
        Err(e) => Inbound::Malformed(format!("{kind:?} message: {e}")),
    };
    match kind {
        "t" => event(trade(m, source)),
        "c" => event(correction(m, source)),
        "x" => event(cancel(m, source)),
        "s" => event(status(m, source)),
        "success" => match str_field(m, "msg") {
            Some("connected") => Inbound::Connected,
            Some("authenticated") => Inbound::Authenticated,
            other => Inbound::Ignored(format!("success {other:?}")),
        },
        "subscription" => match Subscription::deserialize(m) {
            Ok(s) => Inbound::Subscription(s),
            Err(e) => Inbound::Malformed(format!("subscription: {e}")),
        },
        "error" => Inbound::Error {
            code: m
                .get("code")
                .and_then(Value::as_u64)
                .and_then(|c| u16::try_from(c).ok())
                .unwrap_or(0),
            msg: str_field(m, "msg").unwrap_or("").to_string(),
        },
        other => Inbound::Ignored(other.to_string()),
    }
}

fn symbol(m: &Value) -> Result<&str, String> {
    str_field(m, "S")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "missing \"S\"".into())
}

fn tape(m: &Value) -> Tape {
    str_field(m, "z").map_or(Tape::Unknown, Tape::from_code)
}

fn trade(m: &Value, source: Source) -> Result<MarketEvent, String> {
    let sym = symbol(m)?;
    let raw = RawTrade::deserialize(m).map_err(|e| e.to_string())?;
    to_trade(&raw, sym, source).map(MarketEvent::Trade)
}

fn price(n: &serde_json::Number, field: &str) -> Result<market_events::Price, String> {
    parse_price(n).ok_or_else(|| format!("bad {field} {n}"))
}

#[derive(Deserialize)]
struct RawCorrection {
    t: String,
    #[serde(default)]
    x: String,
    oi: u64,
    op: serde_json::Number,
    os: u64,
    #[serde(default)]
    oc: Option<Vec<String>>,
    ci: u64,
    cp: serde_json::Number,
    cs: u64,
    #[serde(default)]
    cc: Option<Vec<String>>,
}

fn correction(m: &Value, source: Source) -> Result<MarketEvent, String> {
    let sym = symbol(m)?;
    let r = RawCorrection::deserialize(m).map_err(|e| e.to_string())?;
    Ok(MarketEvent::Correction(Correction {
        symbol: sym.to_string(),
        event_ts: parse_time(&r.t)?,
        receive_ts: None,
        source,
        exchange: r.x,
        tape: tape(m),
        original_id: r.oi,
        original_price: price(&r.op, "op")?,
        original_size: r.os,
        original_conditions: r.oc.unwrap_or_default(),
        corrected_id: r.ci,
        corrected_price: price(&r.cp, "cp")?,
        corrected_size: r.cs,
        corrected_conditions: r.cc.unwrap_or_default(),
    }))
}

#[derive(Deserialize)]
struct RawCancel {
    t: String,
    #[serde(default)]
    x: String,
    i: u64,
    p: serde_json::Number,
    s: u64,
    a: String,
}

fn cancel(m: &Value, source: Source) -> Result<MarketEvent, String> {
    let sym = symbol(m)?;
    let r = RawCancel::deserialize(m).map_err(|e| e.to_string())?;
    // Both remove the print; an unknown action is refused rather than assumed to be either.
    let action = match r.a.trim() {
        "C" => CancelAction::Cancel,
        "E" => CancelAction::Error,
        other => return Err(format!("unknown cancel action {other:?}")),
    };
    Ok(MarketEvent::Cancel(Cancel {
        symbol: sym.to_string(),
        event_ts: parse_time(&r.t)?,
        receive_ts: None,
        source,
        exchange: r.x,
        tape: tape(m),
        trade_id: r.i,
        price: price(&r.p, "p")?,
        size: r.s,
        action,
    }))
}

#[derive(Deserialize)]
struct RawStatus {
    t: String,
    #[serde(default)]
    sc: String,
    #[serde(default)]
    rc: String,
    #[serde(default)]
    rm: String,
}

/// Alpaca's trading-status code table (docs "Real-time Stock Data", read 2026-10-18):
///
/// - CTA (tapes A, B): `2` Trading Halt, `3` Resume; `5`–`F` are price/range indications,
///   imbalances, short-sale restriction and the LULD indicator.
/// - UTP (tapes C, O): `H` Trading Halt, `P` Volatility Trading Pause (the LULD pause), `T`
///   Trading Resumption, `Q` Quotation Resumption.
///
/// Only a code that starts or ends *trading* moves the chart's halt: `Q` is quoting resuming
/// ahead of trading (UTP sends `H` → `Q` → `T`), so it is `Other`, or the halt would close early.
/// Codes are case-sensitive, like the table.
pub fn status_kind(code: &str) -> StatusKind {
    match code.trim() {
        "2" | "H" | "P" => StatusKind::Halt,
        "3" | "T" => StatusKind::Resume,
        _ => StatusKind::Other,
    }
}

fn status(m: &Value, source: Source) -> Result<MarketEvent, String> {
    let sym = symbol(m)?;
    let r = RawStatus::deserialize(m).map_err(|e| e.to_string())?;
    let reason = if r.rc.trim().is_empty() { r.rm } else { r.rc };
    Ok(MarketEvent::Status(Status {
        symbol: Some(sym.to_string()),
        event_ts: parse_time(&r.t)?,
        receive_ts: None,
        source,
        kind: status_kind(&r.sc),
        code: r.sc,
        reason,
        tape: tape(m),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::alpaca::RawTrade;
    use market_events::{Price, Trade, UnixNanos};
    use serde_json::json;

    const SIP: Source = Source::AlpacaSip;

    fn one(v: Value) -> Inbound {
        let mut got = parse_frame(&Value::Array(vec![v]).to_string(), SIP).unwrap();
        assert_eq!(got.len(), 1);
        got.remove(0)
    }

    fn ts(s: &str) -> UnixNanos {
        parse_time(s).unwrap()
    }

    #[test]
    fn trade_is_the_importers_trade() {
        let msg = json!({"T":"t","S":"SMPL","i":52983525029461u64,"x":"V","p":3.1825,"s":100,
            "c":["@","I"],"t":"2026-10-15T14:00:01.123456789Z","z":"C"});
        let Inbound::Event(MarketEvent::Trade(live)) = one(msg.clone()) else {
            panic!("not a trade")
        };
        // The REST object is the stream message without T/S: same function, same event.
        let mut rest = msg.clone();
        rest.as_object_mut().unwrap().remove("T");
        rest.as_object_mut().unwrap().remove("S");
        let raw: RawTrade = serde_json::from_value(rest).unwrap();
        let hist = to_trade(&raw, "SMPL", SIP).unwrap();
        assert_eq!(live, hist);
        assert_eq!(
            serde_json::to_string(&live).unwrap(),
            serde_json::to_string(&hist).unwrap(),
            "byte-identical serialized form"
        );
        let want = Trade::new(
            "SMPL",
            ts("2026-10-15T14:00:01.123456789Z"),
            "3.1825".parse::<Price>().unwrap(),
            100,
            52983525029461,
        )
        .with_exchange("V")
        .with_tape(Tape::C)
        .with_source(SIP)
        .with_conditions(["@", "I"]);
        assert_eq!(live, want);
        assert_eq!(
            live.receive_ts, None,
            "the client, not the parser, stamps arrival"
        );
    }

    #[test]
    fn correction_and_cancel_error() {
        let c = one(
            json!({"T":"c","S":"SMPL","x":"K","oi":10,"op":3.21,"os":200,"oc":["@"],
            "ci":11,"cp":3.22,"cs":200,"cc":["@"],"t":"2026-10-15T14:01:40Z","z":"C"}),
        );
        assert_eq!(
            c,
            Inbound::Event(MarketEvent::Correction(Correction {
                symbol: "SMPL".into(),
                event_ts: ts("2026-10-15T14:01:40Z"),
                receive_ts: None,
                source: SIP,
                exchange: "K".into(),
                tape: Tape::C,
                original_id: 10,
                original_price: Price::from_cents(321),
                original_size: 200,
                original_conditions: vec!["@".into()],
                corrected_id: 11,
                corrected_price: Price::from_cents(322),
                corrected_size: 200,
                corrected_conditions: vec!["@".into()],
            }))
        );
        for (a, action) in [("C", CancelAction::Cancel), ("E", CancelAction::Error)] {
            let x = one(
                json!({"T":"x","S":"SMPL","i":9,"x":"Z","p":3.19,"s":300,"a":a,
                "t":"2026-10-15T14:01:20Z","z":"C"}),
            );
            assert_eq!(
                x,
                Inbound::Event(MarketEvent::Cancel(Cancel {
                    symbol: "SMPL".into(),
                    event_ts: ts("2026-10-15T14:01:20Z"),
                    receive_ts: None,
                    source: SIP,
                    exchange: "Z".into(),
                    tape: Tape::C,
                    trade_id: 9,
                    price: Price::from_cents(319),
                    size: 300,
                    action,
                }))
            );
        }
        let bad = one(
            json!({"T":"x","S":"SMPL","i":9,"x":"Z","p":3.19,"s":300,"a":"Q",
            "t":"2026-10-15T14:01:20Z","z":"C"}),
        );
        assert!(matches!(bad, Inbound::Malformed(m) if m.contains("cancel action")));
    }

    #[test]
    fn statuses_map_both_tapes_and_keep_codes_verbatim() {
        let s = |sc: &str, rc: &str, rm: &str, z: &str| {
            let Inbound::Event(MarketEvent::Status(s)) = one(json!({"T":"s","S":"SMPL",
                "sc":sc,"sm":"","rc":rc,"rm":rm,"t":"2026-10-15T14:02:00Z","z":z}))
            else {
                panic!("not a status")
            };
            s
        };
        let h = s("H", "LUDP", "Volatility Trading Pause", "C");
        assert_eq!(
            (h.kind, h.code.as_str(), h.reason.as_str(), h.tape),
            (StatusKind::Halt, "H", "LUDP", Tape::C)
        );
        assert_eq!(h.symbol.as_deref(), Some("SMPL"));
        assert_eq!(s("P", "LUDP", "", "C").kind, StatusKind::Halt);
        assert_eq!(s("T", "", "", "C").kind, StatusKind::Resume);
        assert_eq!(s("Q", "", "", "C").kind, StatusKind::Other, "quotes only");
        assert_eq!(s("2", "T1", "", "A").kind, StatusKind::Halt);
        assert_eq!(s("3", "", "", "B").kind, StatusKind::Resume);
        for other in ["5", "6", "7", "8", "9", "A", "C", "D", "E", "F", "h"] {
            assert_eq!(s(other, "", "", "A").kind, StatusKind::Other, "{other}");
        }
        // No reason code: the reason message is kept instead.
        assert_eq!(
            s("H", "", "Halt News Pending", "C").reason,
            "Halt News Pending"
        );
    }

    #[test]
    fn control_messages() {
        assert_eq!(
            one(json!({"T":"success","msg":"connected"})),
            Inbound::Connected
        );
        assert_eq!(
            one(json!({"T":"success","msg":"authenticated"})),
            Inbound::Authenticated
        );
        assert_eq!(
            one(
                json!({"T":"subscription","trades":["*"],"quotes":[],"bars":[],
                "updatedBars":[],"dailyBars":[],"statuses":["*"],"lulds":[],
                "corrections":["*"],"cancelErrors":["*"]})
            ),
            Inbound::Subscription(Subscription {
                trades: vec!["*".into()],
                statuses: vec!["*".into()],
                corrections: vec!["*".into()],
                cancel_errors: vec!["*".into()],
            })
        );
        for (code, msg) in [
            (402, "auth failed"),
            (405, "symbol limit exceeded"),
            (406, "connection limit exceeded"),
            (407, "slow client"),
            (409, "insufficient subscription"),
        ] {
            assert_eq!(
                one(json!({"T":"error","code":code,"msg":msg})),
                Inbound::Error {
                    code,
                    msg: msg.into()
                }
            );
        }
    }

    #[test]
    fn mixed_frame_keeps_good_messages_around_a_bad_one() {
        let frame = json!([
            {"T":"q","S":"SMPL","bp":1.0},
            {"T":"t","S":"SMPL","i":1,"x":"V","p":"oops","s":1,"t":"2026-10-15T14:00:00Z"},
            {"T":"t","S":"SMPL","i":2,"x":"V","p":3.1,"s":1,"t":"2026-10-15T14:00:00Z","z":"C"},
            {"T":"t","i":3,"x":"V","p":3.1,"s":1,"t":"2026-10-15T14:00:00Z"},
            {"T":"t","S":"SMPL","i":4,"x":"V","p":3.1,"s":1,"t":"yesterday"},
            {"no":"tag"}
        ]);
        let got = parse_frame(&frame.to_string(), SIP).unwrap();
        assert_eq!(got[0], Inbound::Ignored("q".into()));
        assert!(matches!(&got[1], Inbound::Malformed(_)), "{:?}", got[1]);
        assert!(matches!(&got[2], Inbound::Event(MarketEvent::Trade(t)) if t.source_seq == 2));
        assert!(matches!(&got[3], Inbound::Malformed(m) if m.contains("\"S\"")));
        assert!(matches!(&got[4], Inbound::Malformed(m) if m.contains("time")));
        assert!(matches!(&got[5], Inbound::Malformed(_)));
        assert!(parse_frame("not json", SIP).is_err());
        assert!(parse_frame("42", SIP).is_err());
        // A bare object is accepted.
        assert_eq!(
            parse_frame(r#"{"T":"success","msg":"connected"}"#, SIP).unwrap(),
            vec![Inbound::Connected]
        );
    }

    #[test]
    fn feeds() {
        assert_eq!(StreamFeed::parse("SIP").unwrap().path(), "v2/sip");
        assert_eq!(StreamFeed::parse("boats").unwrap().path(), "v1beta1/boats");
        assert_eq!(StreamFeed::Test.source(), Source::Fixture);
        assert_eq!(StreamFeed::Overnight.source(), Source::AlpacaBoats);
        assert!(StreamFeed::parse("delayed").is_err());
    }
}
