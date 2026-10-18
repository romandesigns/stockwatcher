//! The Alpaca market-data stream as a [`crate::LiveFeed`]: trades (with their corrections and
//! cancel/errors) and trading statuses, normalized into the event contract.
//!
//! - [`protocol`]: frames → [`market_events::MarketEvent`] (pure, unit-tested).
//! - [`client`]: the socket: connect, auth, subscribe, reconnect, backpressure.
//!
//! Nothing downstream knows this exists: [`crate::LiveProvider`] turns its events into the same
//! [`crate::FeedItem`] stream replay produces.

pub mod client;
pub mod protocol;

pub use client::{
    AlpacaStream, AlpacaStreamConfig, Backoff, LiveSymbols, ReceiveTime, StatsSnapshot, StreamStats,
};
pub use protocol::{Inbound, StreamFeed, Subscription, parse_frame, status_kind};
