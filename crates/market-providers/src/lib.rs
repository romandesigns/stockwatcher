//! # market-providers
//!
//! The provider boundary. Everything downstream (chart engine, server, provider-sim) consumes a
//! [`ProviderSession`]: a bounded stream of [`FeedItem`]s (normalized events, engine-clock marks,
//! display-clock ticks, resets), plus an optional replay control handle. Which provider produced
//! it is configuration only (`MARKET_DATA_MODE=replay|live`), and nothing downstream branches on
//! it except for labels and for refusing replay controls in live mode.
//!
//! - [`ReplayProvider`]: an archive day through `market-replay` (deterministic, controllable).
//! - [`LiveProvider`]: a [`LiveFeed`] on the same engine-clock rule as replay, which is what the
//!   parity tests rely on. In production the feed is [`alpaca_stream::AlpacaStream`] (Alpaca's
//!   market-data WebSocket: trades with their corrections and cancels, and trading statuses, for
//!   `LIVE_SYMBOLS`, default the whole market); tests also use [`ChannelFeed`]. Without
//!   credentials it fails with [`ProviderError::NotConfigured`] naming the missing variables.
//!   Connection changes reach consumers as [`FeedItem::Link`].
//!
//! Live mode **records** what it receives into the archive itself ([`recorder`],
//! `market_archive::live`): a provisional day per trading date that the server loads back after
//! a restart and replay can open, until the archiver's nightly SIP import replaces it with the
//! complete, corrected day.

pub mod alpaca_stream;
/// Offline daily-volume compatibility check (plan §9); reads only the local archive.
pub mod compat;
mod config;
pub mod import;
mod live;
pub mod recorder;
mod replay;
/// Non-blocking trigger for warming up intraday band entrants (server API, not wired yet).
pub mod warmup;

pub use config::{LiveConfig, MarketDataMode, ProviderConfig};
pub use live::{ChannelFeed, LiveFeed, LiveOptions, LiveProvider};
pub use market_replay::{
    ClockMode, ClockState, Command, CommandError, FeedItem, Link, LinkState, ReplayHandle,
    SourceInfo, Speed,
};
pub use recorder::{Recorder, RecorderConfig};
pub use replay::ReplayProvider;

use chrono::NaiveDate;
use serde::Serialize;
use std::fmt;
use tokio::sync::{mpsc, watch};

/// Static description of a running provider.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProviderInfo {
    pub mode: ClockMode,
    /// Human label, e.g. `"archive 2026-10-15 (fixture)"`.
    pub label: String,
    pub day: Option<NaiveDate>,
    pub symbols: Vec<String>,
    /// Every event is fixture data (never market evidence).
    pub fixture: bool,
}

impl ProviderInfo {
    /// The description of a replay after a day switch ([`FeedItem::Source`]).
    pub fn replay(src: &SourceInfo) -> Self {
        ProviderInfo {
            mode: ClockMode::Replay,
            label: src.label.clone(),
            day: Some(src.day),
            symbols: src.symbols.clone(),
            fixture: src.fixture,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProviderError {
    /// The provider cannot run with the current configuration (e.g. live mode in this build).
    NotConfigured(String),
    /// No data to replay (empty archive, unknown day).
    NoData(String),
    Failed(String),
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProviderError::NotConfigured(m) => write!(f, "not configured: {m}"),
            ProviderError::NoData(m) => write!(f, "no data: {m}"),
            ProviderError::Failed(m) => write!(f, "failed: {m}"),
        }
    }
}

impl std::error::Error for ProviderError {}

/// A running provider.
pub struct ProviderSession {
    pub info: ProviderInfo,
    pub items: mpsc::Receiver<FeedItem>,
    pub clock: watch::Receiver<ClockState>,
    /// `Some` only in replay mode.
    pub control: Option<ReplayHandle>,
    /// `Some` only in live mode with recording on: where the events go, for barriers.
    pub recorder: Option<Recorder>,
}

/// A market-data provider: starts and hands over its session. Object-safe so the server can pick
/// one at runtime from configuration.
pub trait MarketDataProvider: Send {
    fn mode(&self) -> ClockMode;
    fn start(self: Box<Self>) -> Result<ProviderSession, ProviderError>;
}

/// Builds the configured provider (must be called inside a tokio runtime).
pub fn from_config(cfg: &ProviderConfig) -> Result<Box<dyn MarketDataProvider>, ProviderError> {
    match cfg.mode {
        MarketDataMode::Replay => Ok(Box::new(ReplayProvider::from_config(cfg)?)),
        MarketDataMode::Live => Ok(Box::new(LiveProvider::from_config(cfg))),
    }
}
