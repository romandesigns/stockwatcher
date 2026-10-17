use crate::timeframe::Timeframe;
use market_events::Price;
use market_events::time::NANOS_PER_SEC;

/// Engine configuration. [`Default`] is the production default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineConfig {
    /// Maintained timeframes. Each must divide the largest one (true for 15s/30s/1m/5m).
    pub timeframes: Vec<Timeframe>,
    /// How long after the end of a trade's **finest-timeframe** interval the trade is still
    /// accepted, measured against the symbol's watermark. Default 2 s. See the crate docs,
    /// "Late and out-of-order trades".
    pub settle_window_ns: i64,
    /// How long trade-level records are kept for exact recomputation (corrections, cancels,
    /// out-of-order inserts) and duplicate detection. Raised internally to at least the largest
    /// timeframe plus the settle window. Default 10 min.
    pub retention_ns: i64,
    /// Ring-buffer capacity per timeframe per symbol (intervals, candle or volume-only gap).
    /// Default 4000 (a full 04:00–20:00 day of 15 s intervals is 3840).
    pub max_intervals_per_timeframe: usize,
    /// Profile bin width used in increments. Snapshots can ask for any width. Default $0.01.
    pub profile_bin: Price,
    /// Sessions whose profile and data-quality counters are kept per symbol. Default 4
    /// (overnight, pre, regular, after of one trading date).
    pub sessions_retained: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            timeframes: Timeframe::ALL.to_vec(),
            settle_window_ns: 2 * NANOS_PER_SEC,
            retention_ns: 600 * NANOS_PER_SEC,
            max_intervals_per_timeframe: 4000,
            profile_bin: Price::from_cents(1),
            sessions_retained: 4,
        }
    }
}

impl EngineConfig {
    pub fn validate(&self) -> Result<(), String> {
        let mut tfs = self.timeframes.clone();
        tfs.sort();
        tfs.dedup();
        if tfs.len() != self.timeframes.len() || tfs.is_empty() {
            return Err("timeframes must be non-empty and distinct".into());
        }
        let max = *tfs.last().expect("non-empty");
        if let Some(bad) = tfs.iter().find(|t| max.nanos() % t.nanos() != 0) {
            return Err(format!("timeframe {bad} does not divide {max}"));
        }
        if self.settle_window_ns < 0 || self.retention_ns < 0 {
            return Err("settle window and retention must be non-negative".into());
        }
        if self.max_intervals_per_timeframe == 0 || self.sessions_retained == 0 {
            return Err("ring capacity and sessions_retained must be positive".into());
        }
        if !self.profile_bin.is_positive() {
            return Err("profile bin must be positive".into());
        }
        Ok(())
    }
}
