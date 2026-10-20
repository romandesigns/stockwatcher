//! The versioned pillar policy (`policy/pillars-v1.json`), embedded in the binary and identified by
//! its id and the SHA-256 of its exact bytes, so every decision names the rules that made it.
//! A changed rule is a new file and id (`pillars-v2`), never an edit of v1.

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The v1 policy file, byte for byte.
pub const PILLARS_V1: &str = include_str!("../policy/pillars-v1.json");

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ReferenceProfile {
    pub id: String,
    /// Inclusive bounds on the last qualifying price, micro-dollars.
    pub price_min_micros: i64,
    pub price_max_micros: i64,
    /// Day gain threshold, basis points (1000 = 10 %).
    pub gain_min_bp: i64,
    /// Daily-denominator RVOL threshold, thousandths (5000 = 5.0x).
    pub rvol_daily_min_milli: u64,
    /// Prior sessions the strict daily denominator needs, all covered.
    pub rvol_daily_sessions: usize,
    /// Float must be strictly below this.
    pub float_max_exclusive_shares: u64,
    pub news_freshness_secs: i64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Band {
    pub min_micros: i64,
    pub max_micros: i64,
    pub source: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Grid {
    pub bar_secs: i64,
    pub settle_ms: i64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct PriceStaleness {
    pub regular_secs: i64,
    pub extended_secs: i64,
    pub source: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct VolumeRules {
    pub day_start: String,
    pub exclude_overnight_boats: bool,
    pub counted: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Diagnostics {
    pub validated: bool,
    pub same_time_min_sessions: usize,
    pub recent_window_secs: i64,
    pub recent_min_sessions: usize,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct FloatRules {
    pub stale_after_secs: i64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct PolicyBody {
    pub schema: u32,
    pub id: String,
    pub note: String,
    pub reference_profile: ReferenceProfile,
    pub discovery_band: Band,
    pub grid: Grid,
    pub price_staleness: PriceStaleness,
    pub volume: VolumeRules,
    pub diagnostics: Diagnostics,
    pub float: FloatRules,
}

/// A parsed policy and its identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    pub body: PolicyBody,
    /// Hex SHA-256 of the file's exact bytes.
    pub hash: String,
}

impl Policy {
    pub fn parse(text: &str) -> Result<Policy, String> {
        let body: PolicyBody =
            serde_json::from_str(text).map_err(|e| format!("pillar policy: {e}"))?;
        if body.schema != 1 {
            return Err(format!("pillar policy: unsupported schema {}", body.schema));
        }
        if body.grid.bar_secs <= 0 || body.grid.settle_ms < 0 {
            return Err("pillar policy: grid must be positive".into());
        }
        let p = &body.reference_profile;
        if p.price_min_micros <= 0
            || p.price_max_micros < p.price_min_micros
            || p.rvol_daily_sessions == 0
        {
            return Err("pillar policy: invalid reference profile".into());
        }
        let hash = Sha256::digest(text.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        Ok(Policy { body, hash })
    }

    /// The embedded v1 policy.
    pub fn v1() -> Policy {
        Policy::parse(PILLARS_V1).expect("embedded pillars-v1.json parses")
    }

    /// `pillars-v1@<first 12 hex of the hash>`, the tag every decision carries.
    pub fn tag(&self) -> String {
        format!("{}@{}", self.body.id, &self.hash[..12])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_parses_and_its_hash_follows_the_bytes() {
        let p = Policy::v1();
        assert_eq!(p.body.id, "pillars-v1");
        assert_eq!(p.body.reference_profile.id, "ross_public_reference_v1");
        assert_eq!(p.body.reference_profile.price_min_micros, 2_000_000);
        assert_eq!(
            p.body.reference_profile.float_max_exclusive_shares,
            20_000_000
        );
        assert_eq!(p.hash.len(), 64);
        // One changed byte is a different policy identity.
        let edited = PILLARS_V1.replacen("\"gain_min_bp\": 1000", "\"gain_min_bp\": 1001", 1);
        assert_ne!(Policy::parse(&edited).unwrap().hash, p.hash);
        assert!(p.tag().starts_with("pillars-v1@"));
    }

    #[test]
    fn invalid_policies_are_refused() {
        assert!(Policy::parse("{}").is_err());
        let bad = PILLARS_V1.replacen("\"bar_secs\": 15", "\"bar_secs\": 0", 1);
        assert!(Policy::parse(&bad).is_err());
    }
}
