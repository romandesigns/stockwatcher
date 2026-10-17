use chrono::{NaiveDate, NaiveTime};
use market_archive::ArchiveConfig;
use market_replay::Speed;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarketDataMode {
    Replay,
    Live,
}

/// Provider configuration from the environment:
///
/// | Variable | Default | Meaning |
/// |---|---|---|
/// | `MARKET_DATA_MODE` | `replay` | `replay` or `live` |
/// | `ARCHIVE_ROOT`, `ARCHIVE_RETENTION_DAYS`, `ARCHIVE_MIN_FREE_MB` | see `market-archive` | |
/// | `REPLAY_DAY` | latest archived day | trading date to replay, `YYYY-MM-DD` |
/// | `REPLAY_START` | `10:40` | ET wall time the clock starts at, `HH:MM[:SS]` (same as the browser fixture) |
/// | `REPLAY_SPEED` | `10` | `1`, `2`, `10`, `60`, … or `max` |
/// | `REPLAY_AUTOPLAY` | `1` | start playing immediately |
/// | `LIVE_PROVIDER_ENABLED` | `0` | live mode only; the Alpaca client does not exist yet, so this still fails as "not configured" |
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderConfig {
    pub mode: MarketDataMode,
    pub archive: ArchiveConfig,
    pub day: Option<NaiveDate>,
    pub start_et: Option<NaiveTime>,
    pub speed: Speed,
    pub autoplay: bool,
    pub live_enabled: bool,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        ProviderConfig {
            mode: MarketDataMode::Replay,
            archive: ArchiveConfig::default(),
            day: None,
            start_et: NaiveTime::from_hms_opt(10, 40, 0),
            speed: Speed::Times(10),
            autoplay: true,
            live_enabled: false,
        }
    }
}

pub(crate) fn truthy(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

impl ProviderConfig {
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let var = |k: &str| {
            get(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let mut c = ProviderConfig {
            archive: ArchiveConfig::from_lookup(&get)?,
            ..ProviderConfig::default()
        };
        if let Some(m) = var("MARKET_DATA_MODE") {
            c.mode = match m.to_ascii_lowercase().as_str() {
                "replay" | "sim" | "simulation" => MarketDataMode::Replay,
                "live" => MarketDataMode::Live,
                _ => {
                    return Err(format!(
                        "MARKET_DATA_MODE must be replay or live, got {m:?}"
                    ));
                }
            };
        }
        if let Some(d) = var("REPLAY_DAY") {
            c.day = Some(
                NaiveDate::parse_from_str(&d, "%Y-%m-%d")
                    .map_err(|_| format!("REPLAY_DAY is not YYYY-MM-DD: {d:?}"))?,
            );
        }
        if let Some(s) = var("REPLAY_START") {
            c.start_et = Some(
                NaiveTime::parse_from_str(&s, "%H:%M:%S")
                    .or_else(|_| NaiveTime::parse_from_str(&s, "%H:%M"))
                    .map_err(|_| format!("REPLAY_START is not HH:MM[:SS]: {s:?}"))?,
            );
        }
        if let Some(s) = var("REPLAY_SPEED") {
            c.speed = Speed::parse(&s).map_err(|e| format!("REPLAY_SPEED: {e}"))?;
        }
        if let Some(a) = var("REPLAY_AUTOPLAY") {
            c.autoplay = truthy(&a);
        }
        if let Some(l) = var("LIVE_PROVIDER_ENABLED") {
            c.live_enabled = truthy(&l);
        }
        Ok(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_replay_and_parses_overrides() {
        let c = ProviderConfig::from_lookup(|_| None).unwrap();
        assert_eq!(c.mode, MarketDataMode::Replay);
        assert_eq!(c.speed, Speed::Times(10));
        let c = ProviderConfig::from_lookup(|k| {
            match k {
                "MARKET_DATA_MODE" => Some("live"),
                "REPLAY_SPEED" => Some("max"),
                "REPLAY_START" => Some("09:30"),
                "REPLAY_DAY" => Some("2026-10-15"),
                _ => None,
            }
            .map(String::from)
        })
        .unwrap();
        assert_eq!(c.mode, MarketDataMode::Live);
        assert_eq!(c.speed, Speed::Max);
        assert_eq!(c.start_et, NaiveTime::from_hms_opt(9, 30, 0));
        assert!(
            ProviderConfig::from_lookup(|k| (k == "MARKET_DATA_MODE").then(|| "paper".into()))
                .is_err()
        );
    }
}
