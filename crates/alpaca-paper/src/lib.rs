//! # alpaca-paper
//!
//! Chart trading routed to the owner's **Alpaca paper** account, behind the same messages as the
//! simulator (docs/architecture/chart-trading-sim.md; this route's contract and safety rules:
//! docs/architecture/alpaca-paper.md).
//!
//! ## Paper only, by construction
//!
//! - The trading host is the constant [`host::PAPER_BASE`] (`https://paper-api.alpaca.markets`)
//!   and the stream [`host::PAPER_STREAM`]. No setting, environment variable or argument can change
//!   them, and [`host::ensure_paper`] re-checks every URL right before a request leaves (REST and
//!   stream). `api.alpaca.markets`, the live-money host, is refused, with tests proving it.
//! - **Going live later must be an explicit code change** (a new constant, a new check, new tests,
//!   reviewed as such), never a configuration flip.
//! - The HTTP client follows no redirects, so the key headers can only ever reach the paper host.
//! - Credentials come from the server environment only ([`creds`]), preferably
//!   `ALPACA_PAPER_KEY` / `ALPACA_PAPER_SECRET`. They are never sent to a client, logged, or
//!   printed (`Debug` redacts them; Alpaca error bodies are scrubbed of them).
//!
//! ## Pieces
//!
//! - [`http`]: the HTTP seam (a trait, the reqwest client, scripted responses in tests) and the
//!   retry policy (429 for any method, transient failures only for GET/DELETE).
//! - [`alpaca`]: Trading API v2 shapes and request bodies.
//! - [`session`]: regular vs extended hours, and what Alpaca accepts in each.
//! - [`book`]: the account as `paper.state` / `paper.fill` frames (`simulated: false`,
//!   `broker: "alpaca_paper"`), bounded like the simulator.
//! - [`stream`]: the trade updates WebSocket, reconnecting with backoff.
//! - [`service`]: place / modify / cancel, the stream relay and the REST resyncs.

pub mod alpaca;
pub mod book;
pub mod creds;
pub mod host;
pub mod http;
pub mod service;
pub mod session;
pub mod stream;

pub use book::BROKER;
pub use creds::{CredSource, Credentials, CredsMissing};
pub use service::{BrokerError, LinkStatus, NOT_CONFIGURED, Publish, Service};

#[cfg(test)]
mod tests;
