use std::net::IpAddr;
use std::path::PathBuf;

use crate::market::MarketConfig;

pub const MIN_TOKEN_LEN: usize = 32;

#[derive(Clone, Debug)]
pub struct Config {
    pub host: IpAddr,
    pub port: u16,
    pub static_dir: PathBuf,
    pub data_dir: PathBuf,
    /// None = auth off (local dev only; logged loudly at startup).
    pub api_token: Option<String>,
    /// Market data: provider mode, archive, replay and simulator settings.
    pub market: MarketConfig,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Separate from `from_env` so tests don't have to mutate process env.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let var = |key: &str| get(key).filter(|v| !v.trim().is_empty());

        let host = match var("HOST") {
            Some(h) => h
                .parse()
                .map_err(|_| format!("HOST is not an IP address: {h}"))?,
            // Loopback by default so a dev run with auth off isn't on the LAN.
            // The image sets HOST=0.0.0.0.
            None => IpAddr::from([127, 0, 0, 1]),
        };
        let port = match var("PORT") {
            Some(p) => p
                .parse()
                .map_err(|_| format!("PORT is not a port number: {p}"))?,
            None => 8790,
        };
        let api_token = var("STOCKWATCHER_API_TOKEN").map(|t| t.trim().to_owned());
        if let Some(t) = &api_token
            && t.len() < MIN_TOKEN_LEN
        {
            // Never echo the value, only its length.
            return Err(format!(
                "STOCKWATCHER_API_TOKEN must be at least {MIN_TOKEN_LEN} characters (got {})",
                t.len()
            ));
        }

        Ok(Self {
            host,
            port,
            static_dir: var("STATIC_DIR")
                .unwrap_or_else(|| "./apps/web/dist".into())
                .into(),
            data_dir: var("DATA_DIR").unwrap_or_else(|| "./data".into()).into(),
            api_token,
            market: MarketConfig::from_lookup(&get)?,
        })
    }
}
