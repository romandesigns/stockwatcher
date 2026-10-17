//! The normalized event contract, version [`CONTRACT_VERSION`].
//!
//! Every provider (archived replay, Alpaca SIP live, BOATS overnight, fixtures) normalizes into
//! [`MarketEvent`] once; everything downstream (chart engine, archive, replay) sees only this.
//!
//! # Serialized form (v1)
//!
//! JSON via serde, internally tagged by `"type"` (`trade`, `quote`, `correction`, `cancel`,
//! `status`), snake_case field names. [`VersionedEvent`] adds `"v": 1` at the top level for
//! anything persisted or sent across a process boundary; a reader refuses other versions. Units:
//!
//! - times: `i64` nanoseconds since the Unix epoch, UTC ([`UnixNanos`]);
//! - prices: `i64` micro-dollars ([`Price`]) — **not** dollars;
//! - sizes: `u64` shares;
//! - `session`: `"2026-10-15/regular"` or `null` outside every session ([`crate::session`]).
//!
//! Changing a field's meaning or unit requires bumping [`CONTRACT_VERSION`]; adding an optional
//! field with `#[serde(default)]` does not.

use crate::conditions::Tape;
use crate::price::Price;
use crate::session::{SessionId, classify};
use crate::time::UnixNanos;
use serde::{Deserialize, Serialize};

pub const CONTRACT_VERSION: u16 = 1;

/// Where an event came from. Part of a trade's identity: the same `source_seq` from two different
/// feeds is two different trades.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Alpaca SIP (consolidated CTA + UTP), live or historical.
    AlpacaSip,
    /// Alpaca's Blue Ocean ATS overnight feed.
    AlpacaBoats,
    /// Alpaca IEX-only feed (not consolidated; kept distinct so it can never pose as SIP).
    AlpacaIex,
    /// Deterministic test fixtures. Never market evidence.
    #[default]
    Fixture,
}

/// The identity of a trade print: `(source, exchange, source_seq)`.
///
/// The exchange is included because SIP trade ids are assigned per reporting venue; two venues may
/// reuse an id for different prints. Including it cannot merge two distinct trades, and a genuine
/// redelivery repeats the exchange, so duplicates are still detected.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TradeId {
    pub source: Source,
    pub exchange: String,
    pub seq: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trade {
    pub symbol: String,
    /// Exchange/SIP timestamp of the execution.
    pub event_ts: UnixNanos,
    /// When our process received it, if the provider path knows.
    #[serde(default)]
    pub receive_ts: Option<UnixNanos>,
    pub price: Price,
    pub size: u64,
    /// Computed from `event_ts` by [`classify`] at construction.
    pub session: Option<SessionId>,
    /// Provider sequence / trade id (Alpaca `i`). Orders same-timestamp trades.
    pub source_seq: u64,
    /// Reporting venue code (Alpaca `x`), e.g. `"V"`, `"Q"`, `"D"`.
    pub exchange: String,
    #[serde(default)]
    pub tape: Tape,
    pub source: Source,
    /// Sale conditions as delivered (Alpaca `c`), e.g. `["@", "I"]`.
    #[serde(default)]
    pub conditions: Vec<String>,
}

impl Trade {
    /// A trade with the session computed, no conditions (regular sale), fixture source, empty
    /// exchange and unknown tape. Use the `with_*` methods for the rest.
    pub fn new(
        symbol: impl Into<String>,
        event_ts: UnixNanos,
        price: Price,
        size: u64,
        source_seq: u64,
    ) -> Self {
        Trade {
            symbol: symbol.into(),
            event_ts,
            receive_ts: None,
            price,
            size,
            session: classify(event_ts),
            source_seq,
            exchange: String::new(),
            tape: Tape::Unknown,
            source: Source::Fixture,
            conditions: Vec::new(),
        }
    }

    pub fn with_conditions<S: Into<String>>(mut self, c: impl IntoIterator<Item = S>) -> Self {
        self.conditions = c.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_exchange(mut self, x: impl Into<String>) -> Self {
        self.exchange = x.into();
        self
    }

    pub fn with_tape(mut self, tape: Tape) -> Self {
        self.tape = tape;
        self
    }

    pub fn with_source(mut self, source: Source) -> Self {
        self.source = source;
        self
    }

    pub fn with_receive_ts(mut self, ts: UnixNanos) -> Self {
        self.receive_ts = Some(ts);
        self
    }

    /// Re-derives `session` from `event_ts` (for events read from an untrusted serialized form).
    pub fn recompute_session(&mut self) {
        self.session = classify(self.event_ts);
    }

    pub fn id(&self) -> TradeId {
        TradeId {
            source: self.source,
            exchange: self.exchange.clone(),
            seq: self.source_seq,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quote {
    pub symbol: String,
    pub event_ts: UnixNanos,
    #[serde(default)]
    pub receive_ts: Option<UnixNanos>,
    pub bid_price: Price,
    pub bid_size: u64,
    pub bid_exchange: String,
    pub ask_price: Price,
    pub ask_size: u64,
    pub ask_exchange: String,
    #[serde(default)]
    pub conditions: Vec<String>,
    #[serde(default)]
    pub tape: Tape,
    pub source: Source,
}

/// A correction of an earlier print (Alpaca `"T":"c"`). The original's identity is
/// `(source, exchange, original_id)`; the corrected print keeps the original's execution time and
/// takes `corrected_id`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Correction {
    pub symbol: String,
    /// When the correction was published.
    pub event_ts: UnixNanos,
    #[serde(default)]
    pub receive_ts: Option<UnixNanos>,
    pub source: Source,
    pub exchange: String,
    #[serde(default)]
    pub tape: Tape,
    pub original_id: u64,
    pub original_price: Price,
    pub original_size: u64,
    #[serde(default)]
    pub original_conditions: Vec<String>,
    pub corrected_id: u64,
    pub corrected_price: Price,
    pub corrected_size: u64,
    #[serde(default)]
    pub corrected_conditions: Vec<String>,
}

impl Correction {
    pub fn original_trade_id(&self) -> TradeId {
        TradeId {
            source: self.source,
            exchange: self.exchange.clone(),
            seq: self.original_id,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelAction {
    /// Trade cancelled.
    Cancel,
    /// Trade reported in error.
    Error,
}

/// A cancel/error of an earlier print (Alpaca `"T":"x"`). Either way the print is removed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cancel {
    pub symbol: String,
    pub event_ts: UnixNanos,
    #[serde(default)]
    pub receive_ts: Option<UnixNanos>,
    pub source: Source,
    pub exchange: String,
    #[serde(default)]
    pub tape: Tape,
    pub trade_id: u64,
    pub price: Price,
    pub size: u64,
    pub action: CancelAction,
}

impl Cancel {
    pub fn target_trade_id(&self) -> TradeId {
        TradeId {
            source: self.source,
            exchange: self.exchange.clone(),
            seq: self.trade_id,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusKind {
    Halt,
    Resume,
    MarketOpen,
    MarketClose,
    Other,
}

/// Trading status (Alpaca `"T":"s"`) or market-wide open/close. `symbol` is `None` for
/// market-wide events.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub symbol: Option<String>,
    pub event_ts: UnixNanos,
    #[serde(default)]
    pub receive_ts: Option<UnixNanos>,
    pub source: Source,
    pub kind: StatusKind,
    /// Provider status code as delivered (e.g. Alpaca `sc`).
    #[serde(default)]
    pub code: String,
    /// Provider reason code/text (e.g. Alpaca `rc` / `rm`).
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub tape: Tape,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MarketEvent {
    Trade(Trade),
    Quote(Quote),
    Correction(Correction),
    Cancel(Cancel),
    Status(Status),
}

impl MarketEvent {
    pub fn symbol(&self) -> Option<&str> {
        match self {
            MarketEvent::Trade(e) => Some(&e.symbol),
            MarketEvent::Quote(e) => Some(&e.symbol),
            MarketEvent::Correction(e) => Some(&e.symbol),
            MarketEvent::Cancel(e) => Some(&e.symbol),
            MarketEvent::Status(e) => e.symbol.as_deref(),
        }
    }

    pub fn event_ts(&self) -> UnixNanos {
        match self {
            MarketEvent::Trade(e) => e.event_ts,
            MarketEvent::Quote(e) => e.event_ts,
            MarketEvent::Correction(e) => e.event_ts,
            MarketEvent::Cancel(e) => e.event_ts,
            MarketEvent::Status(e) => e.event_ts,
        }
    }

    pub fn versioned(self) -> VersionedEvent {
        VersionedEvent {
            v: CONTRACT_VERSION,
            event: self,
        }
    }
}

impl From<Trade> for MarketEvent {
    fn from(t: Trade) -> Self {
        MarketEvent::Trade(t)
    }
}

/// `{"v":1,"type":"trade",...}`. Deserializing any other `v` fails.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawVersioned")]
pub struct VersionedEvent {
    pub v: u16,
    #[serde(flatten)]
    pub event: MarketEvent,
}

#[derive(Deserialize)]
struct RawVersioned {
    v: u16,
    #[serde(flatten)]
    event: MarketEvent,
}

impl TryFrom<RawVersioned> for VersionedEvent {
    type Error = String;

    fn try_from(r: RawVersioned) -> Result<Self, Self::Error> {
        if r.v != CONTRACT_VERSION {
            return Err(format!(
                "unsupported market-event contract version {} (expected {CONTRACT_VERSION})",
                r.v
            ));
        }
        Ok(VersionedEvent {
            v: r.v,
            event: r.event,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::et;
    use chrono::NaiveDate;

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 15).unwrap()
    }

    #[test]
    fn trade_json_is_stable() {
        let t = Trade::new(
            "SMPL",
            et(day(), 10, 0, 0, 0).unwrap(),
            "3.18".parse().unwrap(),
            100,
            42,
        )
        .with_exchange("V")
        .with_tape(Tape::C)
        .with_source(Source::AlpacaSip)
        .with_conditions(["@", "F"]);
        let json = serde_json::to_string(&MarketEvent::Trade(t.clone()).versioned()).unwrap();
        assert_eq!(
            json,
            r#"{"v":1,"type":"trade","symbol":"SMPL","event_ts":1792072800000000000,"receive_ts":null,"price":3180000,"size":100,"session":"2026-10-15/regular","source_seq":42,"exchange":"V","tape":"C","source":"alpaca_sip","conditions":["@","F"]}"#
        );
        let back: VersionedEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.event, MarketEvent::Trade(t));
    }

    #[test]
    fn rejects_other_versions() {
        let json = r#"{"v":2,"type":"cancel","symbol":"A","event_ts":0,"source":"fixture","exchange":"V","trade_id":1,"price":1,"size":1,"action":"cancel"}"#;
        let err = serde_json::from_str::<VersionedEvent>(json)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("unsupported market-event contract version 2"),
            "{err}"
        );
    }

    #[test]
    fn every_variant_round_trips() {
        let ts = et(day(), 10, 0, 0, 0).unwrap();
        let p: Price = "3.18".parse().unwrap();
        let events = vec![
            MarketEvent::Quote(Quote {
                symbol: "SMPL".into(),
                event_ts: ts,
                receive_ts: Some(ts.add_nanos(5)),
                bid_price: p,
                bid_size: 3,
                bid_exchange: "Q".into(),
                ask_price: Price::from_cents(319),
                ask_size: 5,
                ask_exchange: "V".into(),
                conditions: vec!["R".into()],
                tape: Tape::C,
                source: Source::AlpacaSip,
            }),
            MarketEvent::Correction(Correction {
                symbol: "SMPL".into(),
                event_ts: ts,
                receive_ts: None,
                source: Source::AlpacaSip,
                exchange: "V".into(),
                tape: Tape::C,
                original_id: 1,
                original_price: p,
                original_size: 100,
                original_conditions: vec!["@".into()],
                corrected_id: 2,
                corrected_price: Price::from_cents(317),
                corrected_size: 100,
                corrected_conditions: vec!["@".into()],
            }),
            MarketEvent::Cancel(Cancel {
                symbol: "SMPL".into(),
                event_ts: ts,
                receive_ts: None,
                source: Source::AlpacaSip,
                exchange: "V".into(),
                tape: Tape::C,
                trade_id: 2,
                price: p,
                size: 100,
                action: CancelAction::Error,
            }),
            MarketEvent::Status(Status {
                symbol: Some("SMPL".into()),
                event_ts: ts,
                receive_ts: None,
                source: Source::AlpacaSip,
                kind: StatusKind::Halt,
                code: "H".into(),
                reason: "T1".into(),
                tape: Tape::C,
            }),
        ];
        for e in events {
            let json = serde_json::to_string(&e.clone().versioned()).unwrap();
            let back: VersionedEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(back.event, e, "{json}");
        }
    }

    #[test]
    fn trade_session_and_identity() {
        let t = Trade::new(
            "SMPL",
            et(day(), 3, 59, 59, 999).unwrap(),
            Price::from_cents(100),
            1,
            7,
        )
        .with_exchange("D");
        assert_eq!(t.session.unwrap().to_string(), "2026-10-15/overnight");
        assert_eq!(
            t.id(),
            TradeId {
                source: Source::Fixture,
                exchange: "D".into(),
                seq: 7
            }
        );
    }
}
