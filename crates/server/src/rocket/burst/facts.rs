//! ADV30 for the burst features `radv30` and `rvol`, by the research's rule exactly
//! (`crates/setup-research/src/rocket.rs` `facts`, branch `research/burst-onset`): the mean of
//! the raw daily-bar volume over the last 30 raw-bar dates before the trading date, for symbols
//! with at least 10 of them.
//!
//! Reads up to 30 files of the archive's `daily-bars/raw/` once per trading day, off the ingest
//! path (the live market runs it on a blocking thread and hands the result to the detector).

use std::collections::HashMap;

use chrono::NaiveDate;
use market_archive::Archive;

pub fn adv30(archive: &Archive, day: NaiveDate) -> HashMap<String, f64> {
    let dates: Vec<NaiveDate> = archive
        .raw_bar_dates()
        .unwrap_or_default()
        .into_iter()
        .filter(|d| *d < day)
        .collect();
    let tail = &dates[dates.len().saturating_sub(30)..];
    let mut vols: HashMap<String, Vec<f64>> = HashMap::new();
    for d in tail {
        let Some(b) = archive.read_raw_bars(*d).ok().flatten() else {
            continue;
        };
        for (s, bar) in &b.bars {
            if let Some(v) = bar.v {
                vols.entry(s.clone()).or_default().push(v as f64);
            }
        }
    }
    vols.into_iter()
        .filter(|(_, v)| v.len() >= 10)
        .map(|(s, v)| (s, v.iter().sum::<f64>() / v.len() as f64))
        .collect()
}
