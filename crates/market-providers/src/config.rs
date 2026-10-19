use crate::alpaca_stream::{AlpacaStreamConfig, LiveSymbols, StreamFeed};
use crate::import::alpaca::AlpacaCredentials;
use crate::recorder::RecorderConfig;
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
/// | `REPLAY_DAY` | latest ready archived day | trading date to replay, `YYYY-MM-DD` (unset on the VPS, so a restart lands on the newest day; a day still being imported is skipped) |
/// | `REPLAY_START` | `10:40` | ET wall time the clock starts at, `HH:MM[:SS]` (same as the browser fixture) |
/// | `REPLAY_SPEED` | `10` | `1`, `2`, `10`, `60`, … or `max` |
/// | `REPLAY_AUTOPLAY` | `1` | start playing immediately |
/// | `ALPACA_API_KEY` / `ALPACA_API_SECRET` | unset | live only; Alpaca's `APCA_API_KEY_ID` / `APCA_API_SECRET_KEY` also work (the importer reads the same names). Without them live mode is "not configured" |
/// | `ALPACA_STREAM_BASE` | `wss://stream.data.alpaca.markets` | live only; provider-sim is `ws://127.0.0.1:8791` |
/// | `ALPACA_STREAM_FEED` | `sip` | live only: `sip`, `iex`, `boats`, `overnight` or `test` (Alpaca's fake `FAKEPACA` stream) |
/// | `ALPACA_STREAM_NIGHT_FEED` | `boats` | live only, with the `sip` day feed: the feed during the overnight session (20:00–04:00 ET before a trading day; SIP carries nothing then), `boats` or `overnight`; `none` streams the day feed around the clock |
/// | `LIVE_SYMBOLS` | `*` | live only: `*` (whole market) or a comma list |
/// | `LIVE_RECORD` | `1` | live only: record what the stream delivers into the archive (`ARCHIVE_ROOT/<day>/live/`, `crate::recorder`); `0` turns recording off |
/// | `LIVE_RECORD_DAYS` | `5` | live recordings kept when no nightly import replaced them (at least 2; never the day being recorded) |
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderConfig {
    pub mode: MarketDataMode,
    pub archive: ArchiveConfig,
    pub day: Option<NaiveDate>,
    pub start_et: Option<NaiveTime>,
    pub speed: Speed,
    pub autoplay: bool,
    pub live: LiveConfig,
}

/// Live-mode settings. Read in every mode (so a typo fails at startup, not at the first switch
/// to live), used only in live mode.
#[derive(Clone, Debug, PartialEq)]
pub struct LiveConfig {
    pub stream_base: String,
    pub feed: StreamFeed,
    /// The overnight session's feed (`ALPACA_STREAM_NIGHT_FEED`); used only with the `sip` day feed.
    pub night_feed: Option<StreamFeed>,
    pub symbols: LiveSymbols,
    /// `None` until both halves are set; `Debug` redacts them.
    pub credentials: Option<AlpacaCredentials>,
    /// Recording into the archive (`crate::recorder`); `None` with `LIVE_RECORD=0`.
    pub record: Option<RecorderConfig>,
}

impl Default for LiveConfig {
    fn default() -> Self {
        LiveConfig {
            stream_base: AlpacaStreamConfig::DEFAULT_BASE.to_string(),
            feed: StreamFeed::Sip,
            night_feed: Some(StreamFeed::Boats),
            symbols: LiveSymbols::All,
            credentials: None,
            record: Some(RecorderConfig::new(ArchiveConfig::default())),
        }
    }
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
            live: LiveConfig::default(),
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
        if let Some(b) = var("ALPACA_STREAM_BASE") {
            if !(b.starts_with("wss://") || b.starts_with("ws://")) {
                return Err(format!(
                    "ALPACA_STREAM_BASE must be a ws:// or wss:// URL, got {b:?}"
                ));
            }
            c.live.stream_base = b;
        }
        if let Some(f) = var("ALPACA_STREAM_FEED") {
            c.live.feed = StreamFeed::parse(&f).map_err(|e| format!("ALPACA_STREAM_FEED: {e}"))?;
        }
        if let Some(f) = var("ALPACA_STREAM_NIGHT_FEED") {
            c.live.night_feed = match f.to_ascii_lowercase().as_str() {
                "none" | "off" | "0" => None,
                _ => match StreamFeed::parse(&f) {
                    Ok(n @ (StreamFeed::Boats | StreamFeed::Overnight)) => Some(n),
                    Ok(other) => {
                        return Err(format!(
                            "ALPACA_STREAM_NIGHT_FEED must be boats, overnight or none, got {:?}",
                            other.as_str()
                        ));
                    }
                    Err(e) => return Err(format!("ALPACA_STREAM_NIGHT_FEED: {e}")),
                },
            };
        }
        if let Some(s) = var("LIVE_SYMBOLS") {
            c.live.symbols = LiveSymbols::parse(&s).map_err(|e| format!("LIVE_SYMBOLS: {e}"))?;
        }
        c.live.credentials = AlpacaCredentials::from_lookup(&get);
        c.live.record = if var("LIVE_RECORD").is_some_and(|v| !truthy(&v)) {
            None
        } else {
            // Into the same archive replay and the importer use: one store.
            let mut r = RecorderConfig::new(c.archive.clone());
            if let Some(n) = var("LIVE_RECORD_DAYS") {
                r.keep_days = n
                    .parse::<usize>()
                    .ok()
                    .filter(|n| *n >= 2)
                    .ok_or_else(|| format!("LIVE_RECORD_DAYS must be a whole number of at least 2 (today and the day before), got {n:?}"))?;
            }
            Some(r)
        };
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

    #[test]
    fn live_settings_default_to_the_whole_sip_market() {
        let c = ProviderConfig::from_lookup(|_| None).unwrap();
        assert_eq!(c.live, LiveConfig::default());
        assert_eq!(c.live.stream_base, "wss://stream.data.alpaca.markets");
        assert_eq!(c.live.symbols, LiveSymbols::All);
        let get = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        let c = ProviderConfig::from_lookup(get(&[
            ("ALPACA_STREAM_BASE", "ws://127.0.0.1:8791"),
            ("ALPACA_STREAM_FEED", "test"),
            ("LIVE_SYMBOLS", "fakepaca"),
            ("APCA_API_KEY_ID", "id"),
            ("APCA_API_SECRET_KEY", "secret"),
        ]))
        .unwrap();
        assert_eq!(c.live.feed, StreamFeed::Test);
        assert_eq!(LiveConfig::default().night_feed, Some(StreamFeed::Boats));
        let night = |v: &'static str| {
            ProviderConfig::from_lookup(move |k| {
                (k == "ALPACA_STREAM_NIGHT_FEED").then(|| v.to_string())
            })
        };
        assert_eq!(
            night("overnight").unwrap().live.night_feed,
            Some(StreamFeed::Overnight)
        );
        assert_eq!(night("none").unwrap().live.night_feed, None);
        assert!(
            night("iex").is_err(),
            "only an overnight feed can be the night feed"
        );
        assert_eq!(c.live.symbols, LiveSymbols::List(vec!["FAKEPACA".into()]));
        assert_eq!(
            c.live.credentials,
            Some(AlpacaCredentials {
                key_id: "id".into(),
                secret: "secret".into()
            })
        );
        // Half a key pair is no key pair.
        let c = ProviderConfig::from_lookup(get(&[("ALPACA_API_KEY", "id")])).unwrap();
        assert_eq!(c.live.credentials, None);
        assert!(ProviderConfig::from_lookup(get(&[("ALPACA_STREAM_BASE", "https://x")])).is_err());
        assert!(ProviderConfig::from_lookup(get(&[("ALPACA_STREAM_FEED", "delayed")])).is_err());
        assert!(ProviderConfig::from_lookup(get(&[("LIVE_SYMBOLS", "A B")])).is_err());
    }

    #[test]
    fn live_recording_goes_into_the_archive_and_can_be_turned_off() {
        let c = ProviderConfig::from_lookup(|k| match k {
            "ARCHIVE_ROOT" => Some("/app/data/archive".into()),
            "LIVE_RECORD_DAYS" => Some("7".into()),
            _ => None,
        })
        .unwrap();
        let r = c.live.record.unwrap();
        assert_eq!(r.archive, c.archive, "one store: the archive replay reads");
        assert_eq!(r.keep_days, 7);
        let off = ProviderConfig::from_lookup(|k| (k == "LIVE_RECORD").then(|| "0".into()));
        assert_eq!(off.unwrap().live.record, None);
        assert!(
            ProviderConfig::from_lookup(|k| (k == "LIVE_RECORD_DAYS").then(|| "1".into())).is_err()
        );
    }
}
