use crate::timeframe::Timeframe;
use market_events::Price;
use market_events::time::NANOS_PER_SEC;

/// Engine configuration. [`Default`] is the production default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineConfig {
    /// Maintained timeframes. Each must divide the largest one (true for 10s/15s/30s/1m/5m/1h).
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
    /// Once a symbol's latest trading date has reached its regular open (09:30 ET), drop its
    /// candles and profiles of earlier dates. Default `true`. The rings are counted in intervals,
    /// so without this they keep one earlier day of 15 s candles but up to twenty of 5 m, and an
    /// engine started before the open carries the whole previous session all day (several GB on
    /// a whole-market tape; see `SymbolState::drop_prior_dates`).
    pub drop_prior_dates_at_open: bool,
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
            drop_prior_dates_at_open: true,
        }
    }
}

impl EngineConfig {
    /// The ring capacity of `tf`: [`Self::max_intervals_per_timeframe`], scaled for 10 s by the
    /// ratio of its interval count to 15 s's, so every ring covers the same span of time.
    pub fn ring_cap(&self, tf: Timeframe) -> usize {
        ring_cap(self.max_intervals_per_timeframe, tf)
    }

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

/// [`EngineConfig::ring_cap`] from the configured capacity.
pub(crate) fn ring_cap(cap: usize, tf: Timeframe) -> usize {
    match tf {
        Timeframe::S10 => cap.saturating_mul(3) / 2,
        _ => cap,
    }
}
