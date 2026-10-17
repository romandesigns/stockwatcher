use serde::{Deserialize, Serialize};

/// Data-quality counters, kept per symbol per session (plus one bucket for trades outside every
/// session). Every trade the engine does not count normally is counted here, so nothing is
/// dropped silently.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataQuality {
    /// Same `(source, exchange, source_seq)` seen again within retention; ignored.
    pub duplicates: u64,
    /// Arrived after the settle window; excluded from candles and profile.
    pub late_excluded: u64,
    /// Arrived after its finest interval ended but within the settle window; applied.
    pub late_applied: u64,
    /// Arrived with an ordering key below one already applied; applied in key order.
    pub out_of_order_reordered: u64,
    pub corrections_applied: u64,
    /// Correction whose original print is not (or no longer) retained.
    pub corrections_unapplied: u64,
    pub cancels_applied: u64,
    pub cancels_unapplied: u64,
    /// Prints whose conditions update nothing (`M`, `Q`, `9`).
    pub condition_excluded: u64,
    /// Prints counted for volume only (odd lots, average price, …).
    pub volume_only: u64,
    /// Prints carrying a condition code not in Alpaca's table.
    pub unrecognized_conditions: u64,
    /// Prints outside every session (weekend, holiday, closed evening); excluded.
    pub outside_session: u64,
}
