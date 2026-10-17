use crate::config::ProviderConfig;
use crate::{ClockMode, MarketDataProvider, ProviderError, ProviderInfo, ProviderSession};
use market_archive::Archive;
use market_events::session::et_instant;
use market_replay::{ArchiveDaySource, EventSource, ReplayOptions};
use std::sync::Arc;

/// Archived events through the deterministic replay driver.
pub struct ReplayProvider {
    source: Arc<dyn EventSource>,
    opts: ReplayOptions,
}

impl ReplayProvider {
    pub fn new(source: Arc<dyn EventSource>, opts: ReplayOptions) -> Self {
        ReplayProvider { source, opts }
    }

    /// The configured day (or the latest archived one), starting at `REPLAY_START` ET.
    pub fn from_config(cfg: &ProviderConfig) -> Result<Self, ProviderError> {
        let archive = Archive::new(cfg.archive.clone());
        let day = match cfg.day {
            Some(d) => d,
            None => *archive
                .days()
                .map_err(|e| ProviderError::Failed(e.to_string()))?
                .last()
                .ok_or_else(|| {
                    ProviderError::NoData(format!(
                        "the archive at {} has no days (generate one with `swarchive fixture`, or SIM_AUTOFIXTURE=1)",
                        archive.root().display()
                    ))
                })?,
        };
        let source = ArchiveDaySource::new(archive, day).map_err(ProviderError::NoData)?;
        let start = cfg.start_et.and_then(|t| et_instant(day, t));
        Ok(ReplayProvider::new(
            Arc::new(source),
            ReplayOptions {
                start,
                speed: cfg.speed,
                autoplay: cfg.autoplay,
                ..ReplayOptions::default()
            },
        ))
    }

    pub fn source(&self) -> &Arc<dyn EventSource> {
        &self.source
    }
}

impl MarketDataProvider for ReplayProvider {
    fn mode(&self) -> ClockMode {
        ClockMode::Replay
    }

    fn start(self: Box<Self>) -> Result<ProviderSession, ProviderError> {
        let info = ProviderInfo {
            mode: ClockMode::Replay,
            label: self.source.label(),
            day: Some(self.source.day()),
            symbols: self.source.symbols(),
            fixture: self.source.fixture(),
        };
        let (handle, items) =
            market_replay::spawn(self.source, self.opts).map_err(ProviderError::Failed)?;
        Ok(ProviderSession {
            info,
            items,
            clock: handle.watch(),
            control: Some(handle),
        })
    }
}
