//! Provider history into the archive (spec 01, build step 4): Alpaca historical trades as the
//! trade-faithful replay source, FMP 1-minute bars as a separate comparison dataset; and, for
//! research acquisition only (`swimport research-*`), quotes, raw reference data and a bounded,
//! checkpointed pilot driver (`quotes`, `reference`, `pilot`, `pacing`).
//!
//! The logic here is plain and network-free (an [`http::Http`] trait carries every request), so
//! it is unit-tested with scripted responses and never needs credentials. The real HTTP client
//! and the `swimport` command live behind the `import` cargo feature.

pub mod alpaca;
pub mod batch;
pub mod check;
pub mod fmp;
pub mod http;
pub mod maintain;
pub mod pacing;
pub mod pilot;
pub mod quotes;
pub mod reference;

#[cfg(feature = "import")]
pub mod client;
#[cfg(feature = "import")]
pub mod paced_client;
