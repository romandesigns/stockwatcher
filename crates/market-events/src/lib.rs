//! # market-events
//!
//! The normalized market-event contract shared by every provider (replay and live) and every
//! consumer (chart engine, archive), plus the two rule sets everything downstream must agree on:
//!
//! - [`session`]: the `America/New_York` session calendar (overnight/BOATS, pre, regular, after),
//!   DST, holidays and early closes for 2025–2027, and session ids such as `"2026-10-15/regular"`.
//! - [`conditions`]: the trade-condition policy (which prints update OHLC and/or volume), following
//!   Alpaca's documented minute-bar aggregation rules.
//!
//! Units: times are `i64` UTC nanoseconds ([`UnixNanos`]); prices are `i64` micro-dollars
//! ([`Price`]); sizes are `u64` shares. No floating point is used for anything that is compared,
//! keyed or summed.

pub mod conditions;
pub mod event;
pub mod price;
pub mod session;
pub mod time;

pub use conditions::{Effect, EffectReason, Rule, Tape, condition_rule, trade_effect};
pub use event::{
    CONTRACT_VERSION, Cancel, CancelAction, Correction, MarketEvent, Quote, Source, Status,
    StatusKind, Trade, TradeId, VersionedEvent,
};
pub use price::{PRICE_SCALE, Price};
pub use session::{
    DayKind, SessionId, SessionKind, classify, day_kind, is_trading_day, session_bounds,
};
pub use time::UnixNanos;
