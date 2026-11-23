//! Provider history into the archive (spec 01, build step 4): Alpaca historical trades as the
//! trade-faithful replay source, FMP 1-minute bars as a separate comparison dataset.
//!
//! The logic here is plain and network-free (an [`http::Http`] trait carries every request), so
//! it is unit-tested with scripted responses and never needs credentials. The real HTTP client
//! and the `swimport` command live behind the `import` cargo feature.

pub mod alpaca;
pub mod alpaca_news;
pub mod batch;
pub mod check;
pub mod corporate;
pub mod edgar;
pub mod evidence_collect;
pub mod fmp;
pub mod fmp_float;
pub mod fmp_news;
pub mod http;
pub mod maintain;
pub mod scorecard;

#[cfg(feature = "import")]
pub mod client;
