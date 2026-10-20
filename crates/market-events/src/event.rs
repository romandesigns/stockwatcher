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

use crate::conditions::{Effect, EffectReason, Tape, trade_effect};
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

/// The provider's reference to a print: `(source, exchange, source_seq)`, i.e. Alpaca's `i` on venue
/// `x`. This is what a [`Correction`] or [`Cancel`] names.
///
/// It is **not** a unique identity (docs/architecture/trade-identity.md): FINRA's trade reporting
/// facilities all report under venue `D` with independent id sequences, and Alpaca's message has
/// no field telling them apart, so distinct `D` prints share a `TradeId` (1,941 of 72,036 SIP
/// prints in the 2026-10 pilot). Deduplicate on [`PrintId`]; resolve a revision's target with
/// [`TradeRef::resolve`].
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TradeId {
    pub source: Source,
    pub exchange: String,
    pub seq: u64,
}

/// The identity of a print: its [`TradeId`] plus its execution time.
///
/// A redelivery (reconnect replay, a recording read twice) repeats the message, so it repeats the
/// execution time to the nanosecond and is still a duplicate. Distinct TRF prints that share an id
/// were never observed at the same nanosecond (0 of 2,076 colliding pairs in the pilot). Every
/// archived trade already carries `event_ts`, so this needs no new field and no format change.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PrintId {
    pub id: TradeId,
    pub event_ts: UnixNanos,
}

/// What a correction or cancel says about the print it revises: the provider reference plus the
/// original's price and size, which Alpaca sends with both (`op`/`os`, `p`/`s`). It carries no
/// execution time, so on venue `D` the reference alone can match several retained prints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TradeRef {
    pub id: TradeId,
    pub price: Price,
    pub size: u64,
}

/// Which retained print a [`TradeRef`] names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution<T> {
    /// Exactly one print fits: apply the revision to it.
    Found(T),
    /// No print with that reference is held (never seen, or no longer retained).
    NotFound,
    /// Prints with that reference are held but not exactly one of them fits: several match the
    /// price and size, or none does (the named print may be one that is no longer held while a
    /// different TRF's print with the same id is). Applying it would be a guess.
    Ambiguous,
}

impl TradeRef {
    /// Resolves the reference among the held prints that share its [`TradeId`], given as
    /// `(handle, price, size)`. The rule (docs/architecture/trade-identity.md):
    ///
    /// - none held → [`Resolution::NotFound`];
    /// - a reference with a size (every real provider message) must match price **and** size: the
    ///   one print that does is the target; zero or several → [`Resolution::Ambiguous`]. This
    ///   holds for a single held print too, because on venue `D` the named print may be gone and
    ///   the held one another TRF's print with the same id;
    /// - a reference without a payload (size 0, test fixtures only) is resolved by id alone, so
    ///   only when exactly one print is held.
    pub fn resolve<T>(&self, held: impl IntoIterator<Item = (T, Price, u64)>) -> Resolution<T> {
        let mut any = false;
        let mut found = None;
        let mut fits = 0usize;
        for (h, price, size) in held {
            any = true;
            if self.size == 0 || (price == self.price && size == self.size) {
                fits += 1;
                found = Some(h);
            }
        }
        match (any, fits) {
            (false, _) => Resolution::NotFound,
            (true, 1) => Resolution::Found(found.expect("one fit")),
            _ => Resolution::Ambiguous,
        }
    }
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
    /// The provider's later verdict on this print, as delivered (Alpaca historical `u`:
    /// `"canceled"`, `"incorrect"` or `"corrected"`). Historical responses carry it on the print
    /// itself, with no publication time and no link from a corrected print to the one it
    /// replaces, so it cannot become a [`Correction`] or [`Cancel`] without guessing. It is kept
    /// raw instead; [`Trade::effect`] excludes `canceled` and `incorrect` prints. Absent for live
    /// prints and fixtures (live corrections arrive as their own events).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<String>,
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
            update: None,
        }
    }

    /// The provider's update marker (see [`Trade::update`]).
    pub fn with_update(mut self, update: impl Into<String>) -> Self {
        self.update = Some(update.into());
        self
    }

    /// What this print may update: the condition table ([`trade_effect`]), except that a print
    /// the provider has since marked `canceled` or `incorrect` updates nothing. Every consumer that
    /// counts trades (chart engine, paper broker, simulator bars) should use this, not the
    /// condition table alone, so a withdrawn print never moves a candle or fills an order.
    pub fn effect(&self) -> Effect {
        match self.update.as_deref().map(str::trim) {
            Some(u @ ("canceled" | "cancelled" | "incorrect")) => Effect {
                updates_ohlc: false,
                updates_volume: false,
                reason: EffectReason::Withdrawn {
                    update: u.to_string(),
                },
                unrecognized: Vec::new(),
            },
            _ => trade_effect(&self.conditions, self.tape),
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

    /// The provider reference (not unique on venue `D`; see [`TradeId`]).
    pub fn id(&self) -> TradeId {
        TradeId {
            source: self.source,
            exchange: self.exchange.clone(),
            seq: self.source_seq,
        }
    }

    /// The print's identity, for deduplication (see [`PrintId`]).
    pub fn print_id(&self) -> PrintId {
        PrintId {
            id: self.id(),
            event_ts: self.event_ts,
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

/// A correction of an earlier print (Alpaca `"T":"c"`). It names the original by
/// `(source, exchange, original_id)` plus its price and size ([`Correction::target`]); the
/// corrected print keeps the original's execution time and takes `corrected_id`.
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

    /// The original print as this correction names it (resolve with [`TradeRef::resolve`]).
    pub fn target(&self) -> TradeRef {
        TradeRef {
            id: self.original_trade_id(),
            price: self.original_price,
            size: self.original_size,
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

    /// The print as this cancel names it (resolve with [`TradeRef::resolve`]).
    pub fn target(&self) -> TradeRef {
        TradeRef {
            id: self.target_trade_id(),
            price: self.price,
            size: self.size,
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
    fn provider_update_is_kept_raw_and_withdrawn_prints_count_for_nothing() {
        let ts = et(day(), 10, 0, 0, 0).unwrap();
        let t = Trade::new("SMPL", ts, Price::from_cents(318), 100, 1).with_conditions(["@"]);
        // Absent: the serialized form is unchanged (no "update" key).
        assert!(!serde_json::to_string(&t).unwrap().contains("update"));
        assert!(t.effect().updates_ohlc);
        let c = t.clone().with_update("canceled");
        let json = serde_json::to_string(&MarketEvent::Trade(c.clone()).versioned()).unwrap();
        assert!(json.ends_with(r#""update":"canceled"}"#), "{json}");
        let back: VersionedEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.event, MarketEvent::Trade(c.clone()));
        for u in ["canceled", "incorrect"] {
            let e = t.clone().with_update(u).effect();
            assert!(!e.updates_ohlc && !e.updates_volume, "{u}");
            assert_eq!(e.reason_code(), Some(format!("withdrawn_{u}")));
        }
        // A corrected print is a real execution: it counts like any other.
        assert_eq!(t.clone().with_update("corrected").effect(), t.effect());
        // An odd lot keeps its condition rule.
        let odd = Trade::new("SMPL", ts, Price::from_cents(318), 37, 2).with_conditions(["@", "I"]);
        assert!(!odd.effect().updates_ohlc && odd.effect().updates_volume);
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
        assert_eq!(t.print_id().event_ts, t.event_ts);
    }

    /// Two FINRA TRF prints with the same `D` id (CONL 2026-10-12, ids 1-3 appear twice in the
    /// first seconds of premarket) are two prints; a redelivery is the same print.
    #[test]
    fn print_identity_separates_trf_prints_and_keeps_redelivery_equal() {
        let ts = et(day(), 4, 0, 2, 0).unwrap();
        let a = Trade::new("CONL", ts, Price::from_cents(551), 70, 1).with_exchange("D");
        let b = Trade::new(
            "CONL",
            ts.add_nanos(24_387_263),
            Price::from_cents(548),
            1,
            1,
        )
        .with_exchange("D");
        assert_eq!(a.id(), b.id(), "the provider reference collides");
        assert_ne!(a.print_id(), b.print_id(), "the prints do not");
        assert_eq!(a.print_id(), a.clone().print_id());
    }

    #[test]
    fn a_reference_resolves_only_to_exactly_one_fitting_print() {
        let id = TradeId {
            source: Source::AlpacaSip,
            exchange: "D".into(),
            seq: 1,
        };
        let r = |cents, size| TradeRef {
            id: id.clone(),
            price: Price::from_cents(cents),
            size,
        };
        let held = [
            ("a", Price::from_cents(551), 70),
            ("b", Price::from_cents(548), 1),
        ];
        assert_eq!(r(548, 1).resolve(held), Resolution::Found("b"));
        assert_eq!(r(551, 70).resolve(held), Resolution::Found("a"));
        // Fits none: the named print may be gone while another TRF's print is held.
        assert_eq!(r(500, 70).resolve(held), Resolution::Ambiguous);
        assert_eq!(r(500, 70).resolve([held[0]]), Resolution::Ambiguous);
        // Fits several: identical price and size on different TRFs.
        let twins = [held[1], ("c", Price::from_cents(548), 1)];
        assert_eq!(r(548, 1).resolve(twins), Resolution::Ambiguous);
        assert_eq!(r(548, 1).resolve::<&str>([]), Resolution::NotFound);
        // No payload (fixtures): by id alone, only when unique.
        assert_eq!(r(0, 0).resolve([held[0]]), Resolution::Found("a"));
        assert_eq!(r(0, 0).resolve(held), Resolution::Ambiguous);
    }
}
