//! Alpaca's real-time news stream, **off by default**.
//!
//! - [`protocol`]: frames → stories and control messages (pure, unit-tested).
//! - [`session`]: the handshake, the backoff, the counters and when to stop trying (pure,
//!   unit-tested).
//! - [`runner`]: the socket seam and the bounded pump the news worker calls, exercised against an
//!   injected fake transport.
//! - [`socket`]: the real WebSocket behind that seam.
//!
//! # What it changes, and what it does not
//!
//! A stream is *intended* to reduce the delay before an edit is seen — an edit the REST searches
//! would find anyway. Whether it does, and by how much, is not established by anything here: the
//! tests are offline protocol and runner tests, and no end-to-end delivery delay has been
//! measured. It is not coverage, and it is not permitted to act like it:
//!
//! - a delivery never writes a coverage record and never advances the covered-through point,
//!   because receiving one message says one story changed and nothing about what else did;
//! - the bounded repair search keeps running whether the socket is connected or not
//!   ([`crate::import::evidence_collect::news_repair_window`]);
//! - stories reach the archive through the same durable path a REST page uses — propose, append,
//!   commit ([`crate::import::alpaca_news::NewsVersions`]) — so one owner holds the version
//!   state and two dedup maps never race each other;
//! - no pillar threshold, rule version or label changes because it is on.
//!
//! It is off unless `EVIDENCE_NEWS_STREAM=1`, and everything here is tested offline: no
//! authenticated probe and no live acquisition.

pub mod protocol;
pub mod runner;
pub mod session;
pub mod socket;

pub use protocol::{Inbound, Refusal};
pub use runner::{Dial, Recv, Runner, Transport};
pub use session::{NewsStreamConfig, NewsStreamStats, Phase, Session, Step};
pub use socket::WebSocketDial;
