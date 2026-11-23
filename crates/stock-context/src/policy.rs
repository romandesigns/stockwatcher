//! The versioned pillar policy (`policy/pillars-v1.json`), embedded in the binary and identified by
//! its id and the SHA-256 of its exact bytes, so every decision names the rules that made it.
//! A changed rule is a new file and id (`pillars-v2`), never an edit of v1.
//!
//! - `pillars-v1`: the documented public reference (`ross_public_reference_v1`, price $2–$20).
//! - `pillars-v2`: StockWatcher's own price band (`stockwatcher_band_v1`, price $0.10–$25
//!   inclusive, owner decision 2026-10-20), with 120 s news coverage tolerance.
//! - `pillars-v3`: v2 with discovery eligible from $0.10 (owner, 2026-10-26).
//! - `pillars-v4`: v3 with a float record usable for 4 days instead of 1 (owner, 2026-10-31).
//! - `pillars-v5`: v4 with the float pillar passing on a shares-outstanding figure below the float
//!   maximum when no usable float is visible (owner, 2026-11-22).
//! - `pillars-v6`: v5 with SEC EDGAR current reports (8-K, 6-K) as a second news source beside
//!   Benzinga (owner, 2026-11-22; `news.sources`).
//!   [`Policy::current`] returns v6; callers can still select a frozen historical policy.

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The v1 policy file, byte for byte.
pub const PILLARS_V1: &str = include_str!("../policy/pillars-v1.json");

/// The v2 policy file (price pillar on the app's own $0.10–$25 band), byte for byte.
pub const PILLARS_V2: &str = include_str!("../policy/pillars-v2.json");
/// v3 lowers discovery eligibility to $0.10; v1 and v2 remain frozen.
pub const PILLARS_V3: &str = include_str!("../policy/pillars-v3.json");
/// v4 keeps a float record for 4 days; v1, v2 and v3 remain frozen.
pub const PILLARS_V4: &str = include_str!("../policy/pillars-v4.json");
/// v5 lets a shares-outstanding figure below the float maximum pass the float pillar when no
/// usable float is visible; v1 to v4 remain frozen.
pub const PILLARS_V5: &str = include_str!("../policy/pillars-v5.json");
/// v6 reads SEC EDGAR current reports as news beside Benzinga; v1 to v5 remain frozen.
pub const PILLARS_V6: &str = include_str!("../policy/pillars-v6.json");

/// The news provider every policy before v6 read, and the one a policy without `news.sources`
/// still reads: Alpaca's news API (Benzinga stories).
pub const NEWS_PROVIDER_ALPACA: &str = "alpaca";
/// SEC EDGAR filings (`market_archive::evidence_load::filing_items`), read from pillars-v6.
pub const NEWS_PROVIDER_SEC_EDGAR: &str = "sec_edgar";
/// Issuer press releases from the public wire feeds (`SourceNews`), read from pillars-v6.
pub const NEWS_PROVIDER_PRESS_WIRE: &str = "press_wire";
/// A T1 "news pending" trading halt (exchange-issued evidence of a coming catalyst), read from
/// pillars-v6.
pub const NEWS_PROVIDER_HALT_T1: &str = "halt_t1";

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
    /// Whether a shares-outstanding figure below the float maximum may pass the pillar where no
    /// usable float is visible (`pillars-v5`). Absent from v1..v4, so their bytes, hashes and
    /// decisions are unchanged. The figure obeys the same `stale_after_secs`.
    #[serde(default)]
    pub outstanding_fallback: bool,
}

/// How a news-coverage record may be read. Absent from `pillars-v1.json`, so v1 keeps its exact
/// bytes, hash and behaviour (a lag of 0).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct NewsRules {
    /// How far a cutoff may run past a coverage record's `covered_through` and still count as
    /// searched. Every real source is behind the clock: a poll covers through its request time and
    /// is observed after it, so with 0 (v1) a forward run never sees `covered_through >= cutoff`
    /// from an honest record, and the news pillar can only PASS or be UNKNOWN. A policy that lets
    /// polled coverage FAIL the pillar states its tolerance here
    /// (`docs/stock-context/float-news-plan.md` section 7). That is a new policy id, not an edit
    /// of v1.
    #[serde(default)]
    pub coverage_max_lag_secs: i64,
    /// The providers whose items may PASS the pillar (`NewsItem::source` is `<provider>/<source>`).
    ///
    /// Absent from v1..v5, which read every provider except the ones pillars-v6 introduced
    /// ([`NEWS_PROVIDER_SEC_EDGAR`], [`NEWS_PROVIDER_PRESS_WIRE`], [`NEWS_PROVIDER_HALT_T1`]):
    /// they were frozen before those were collected (production only ever had
    /// [`NEWS_PROVIDER_ALPACA`]), so such an item loaded beside them must not change a decision
    /// they name. pillars-v6 lists `alpaca` and `sec_edgar`. A FAIL is not
    /// affected: it rests on coverage records, and only the market-wide Benzinga searches write
    /// those for the engine (EDGAR's own coverage stays with its collector).
    #[serde(default)]
    pub sources: Option<Vec<String>>,
}

impl NewsRules {
    /// Whether an item from `source` (`<provider>/<source>`) may pass the pillar.
    pub fn reads(&self, source: &str) -> bool {
        let provider = source.split('/').next().unwrap_or("");
        match &self.sources {
            Some(list) => list.iter().any(|p| p == provider),
            None => ![
                NEWS_PROVIDER_SEC_EDGAR,
                NEWS_PROVIDER_PRESS_WIRE,
                NEWS_PROVIDER_HALT_T1,
            ]
            .contains(&provider),
        }
    }
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
    #[serde(default)]
    pub news: NewsRules,
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
        if body.news.coverage_max_lag_secs < 0 {
            return Err("pillar policy: news.coverage_max_lag_secs must not be negative".into());
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

    /// The embedded v2 policy: v1 with the price pillar on StockWatcher's own band.
    pub fn v2() -> Policy {
        Policy::parse(PILLARS_V2).expect("embedded pillars-v2.json parses")
    }

    /// The embedded v3 policy: v2 with discovery eligible from $0.10.
    pub fn v3() -> Policy {
        Policy::parse(PILLARS_V3).expect("embedded pillars-v3.json parses")
    }

    /// The embedded v4 policy: v3 with a float record usable for 4 days.
    pub fn v4() -> Policy {
        Policy::parse(PILLARS_V4).expect("embedded pillars-v4.json parses")
    }

    /// The embedded v5 policy: v4 with the shares-outstanding float fallback.
    pub fn v5() -> Policy {
        Policy::parse(PILLARS_V5).expect("embedded pillars-v5.json parses")
    }

    /// The embedded v6 policy: v5 with SEC EDGAR current reports as news.
    pub fn v6() -> Policy {
        Policy::parse(PILLARS_V6).expect("embedded pillars-v6.json parses")
    }

    /// The policy the product runs today (v6). v1 stays parseable and taggable as the documented
    /// public reference ($2–$20); decisions made under it keep naming it.
    pub fn current() -> Policy {
        Policy::v6()
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
    fn v2_changes_only_the_price_band_and_the_news_lag() {
        let (v1, v2) = (Policy::v1(), Policy::v2());
        assert_eq!(v2.body.id, "pillars-v2");
        assert_eq!(v2.body.reference_profile.id, "stockwatcher_band_v1");
        let prof = &v2.body.reference_profile;
        assert_eq!(
            (prof.price_min_micros, prof.price_max_micros),
            (100_000, 25_000_000)
        );
        // Everything else is v1's.
        let mut same = v2.body.clone();
        same.id = v1.body.id.clone();
        same.note = v1.body.note.clone();
        same.reference_profile.id = v1.body.reference_profile.id.clone();
        same.reference_profile.price_min_micros = 2_000_000;
        same.reference_profile.price_max_micros = 20_000_000;
        assert_eq!(same.news.coverage_max_lag_secs, 120);
        same.news.coverage_max_lag_secs = 0; // v1 has no field: the strict rule
        assert_eq!(same, v1.body);
        assert_ne!(v1.hash, v2.hash);
        assert!(v2.tag().starts_with("pillars-v2@"));
        assert_eq!(v2.body.discovery_band.min_micros, 150_000);
    }

    #[test]
    fn v3_only_lowers_discovery_eligibility_to_ten_cents() {
        let v2 = Policy::v2();
        let v3 = Policy::v3();
        let mut same = v3.body.clone();
        same.id = v2.body.id.clone();
        same.note = v2.body.note.clone();
        same.discovery_band.source = v2.body.discovery_band.source.clone();
        assert_eq!(same.discovery_band.min_micros, 100_000);
        same.discovery_band.min_micros = 150_000;
        assert_eq!(same, v2.body);
        assert_ne!(v2.hash, v3.hash);
        assert!(v3.tag().starts_with("pillars-v3@"));
        assert_eq!(v3.body.float.stale_after_secs, 86_400);
    }

    #[test]
    fn v4_only_keeps_a_float_for_four_days() {
        let v3 = Policy::v3();
        let v4 = Policy::v4();
        let mut same = v4.body.clone();
        same.id = v3.body.id.clone();
        same.note = v3.body.note.clone();
        assert_eq!(same.float.stale_after_secs, 4 * 86_400);
        same.float.stale_after_secs = 86_400;
        assert_eq!(same, v3.body);
        assert_ne!(v3.hash, v4.hash);
        assert!(v4.tag().starts_with("pillars-v4@"));
        assert!(!v4.body.float.outstanding_fallback);
    }

    #[test]
    fn v5_only_adds_the_outstanding_fallback() {
        let v4 = Policy::v4();
        let v5 = Policy::v5();
        let mut same = v5.body.clone();
        same.id = v4.body.id.clone();
        same.note = v4.body.note.clone();
        assert!(same.float.outstanding_fallback);
        same.float.outstanding_fallback = false;
        assert_eq!(same, v4.body);
        assert_ne!(v4.hash, v5.hash);
        assert!(v5.tag().starts_with("pillars-v5@"));
        // v1..v4 never had the field: their bytes still parse to "no fallback".
        for p in [Policy::v1(), Policy::v2(), Policy::v3(), Policy::v4()] {
            assert!(!p.body.float.outstanding_fallback, "{}", p.body.id);
        }
    }

    #[test]
    fn v6_only_adds_sec_edgar_as_a_news_source() {
        let v5 = Policy::v5();
        let v6 = Policy::v6();
        let mut same = v6.body.clone();
        same.id = v5.body.id.clone();
        same.note = v5.body.note.clone();
        assert_eq!(
            same.news.sources.as_deref(),
            Some(
                &[
                    "alpaca".to_string(),
                    "sec_edgar".to_string(),
                    "press_wire".to_string(),
                    "halt_t1".to_string()
                ][..]
            )
        );
        same.news.sources = None;
        assert_eq!(same, v5.body);
        assert_ne!(v5.hash, v6.hash);
        assert!(v6.tag().starts_with("pillars-v6@"));
        assert_eq!(Policy::current(), v6);
        for src in [
            "alpaca/benzinga",
            "sec_edgar/8-K",
            "press_wire/globenewswire",
            "halt_t1/T1",
        ] {
            assert!(v6.body.news.reads(src), "{src}");
        }
        // v1..v5 never had the field: Benzinga only, so an EDGAR item cannot reach their decisions.
        for p in [Policy::v1(), Policy::v2(), Policy::v3(), Policy::v4(), v5] {
            assert!(p.body.news.sources.is_none(), "{}", p.body.id);
            assert!(p.body.news.reads("alpaca/benzinga"), "{}", p.body.id);
            for src in ["sec_edgar/8-K", "press_wire/globenewswire", "halt_t1/T1"] {
                assert!(!p.body.news.reads(src), "{} {src}", p.body.id);
            }
        }
    }

    #[test]
    fn invalid_policies_are_refused() {
        assert!(Policy::parse("{}").is_err());
        let bad = PILLARS_V1.replacen("\"bar_secs\": 15", "\"bar_secs\": 0", 1);
        assert!(Policy::parse(&bad).is_err());
    }
}
