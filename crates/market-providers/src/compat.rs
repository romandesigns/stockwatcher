//! The daily-volume compatibility check (`docs/stock-context/daily-baseline-plan.md` §9): does
//! Alpaca's SIP daily-bar volume (`daily-bars/raw/`, the RVOL denominator of
//! `market_archive::baseline`) equal our own count of the same day (the pillar's numerator: SIP
//! prints 04:00 → session end, BOATS/overnight excluded, counted when the condition policy says
//! the print updates volume, `Trade::effect`)? Offline: reads only the local archive, makes no
//! request.
//!
//! **Only meaningful after the trade-identity fix (PR #51) has merged and the compared days were
//! imported with it.** Before it, distinct FINRA TRF prints sharing `(D, id)` are dropped as
//! duplicates, our count is low, and this check measures that bug instead of the definitions.
//!
//! Owner-accepted rule (2026-10-20): **compatible** when at least 95 % of the sampled symbol-days
//! are within ±2 % (`ours / provider`) and the median is within ±2 %. Otherwise the fallback is
//! the separately versioned `provider_daily_volume_v1` metric with a matching live numerator; the
//! strict metric stays unavailable.
//!
//! Sample: per trading date, the symbols that have both an imported (final) archive file and a
//! raw daily bar with a volume, stratified by price bucket (raw close < $1, $1–$5, $5–$25, other)
//! × volume tercile of that date, then taken round-robin across strata in a fixed hash order
//! (FNV-1a of `date|symbol`), so a re-run picks the same symbol-days. Reported by session (pre /
//! regular / after volume) and by condition code (volume counted and not counted by our policy).

use chrono::NaiveDate;
use market_archive::{Archive, DayData};
use market_events::{MarketEvent, SessionKind};
use serde::Serialize;

use std::collections::BTreeMap;

/// Owner-accepted tolerance (2026-10-20).
pub const TOLERANCE: f64 = 0.02;
/// Share of symbol-days that must be within [`TOLERANCE`].
pub const REQUIRED_SHARE: f64 = 0.95;
/// Below this many symbol-days the verdict is `insufficient_sample`.
pub const MIN_SAMPLE: usize = 100;

#[derive(Clone, Debug)]
pub struct CompatOptions {
    pub from: NaiveDate,
    pub to: NaiveDate,
    /// Symbol-days wanted in total (spread evenly over the dates).
    pub sample: usize,
    pub min_sample: usize,
    /// Fixture days are never market evidence; only tests set this.
    pub include_fixture: bool,
}

/// Volume by condition code within one symbol-day or the whole run.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ConditionTally {
    pub counted: u64,
    pub not_counted: u64,
    pub prints: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SymbolDay {
    pub date: NaiveDate,
    pub symbol: String,
    pub stratum: String,
    pub provider: u64,
    pub ours: u64,
    /// `ours / provider`.
    pub ratio: f64,
    pub by_session: BTreeMap<String, u64>,
    /// Prints outside pre/regular/after (overnight/BOATS, or no session): not counted.
    pub outside_window: u64,
    pub by_condition: BTreeMap<String, ConditionTally>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CompatReport {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub dates: Vec<NaiveDate>,
    pub note: &'static str,
    pub tolerance: f64,
    pub required_share: f64,
    pub symbol_days: usize,
    pub within_1pct: f64,
    pub within_2pct: f64,
    pub within_5pct: f64,
    pub median: Option<f64>,
    pub p5: Option<f64>,
    pub p95: Option<f64>,
    /// `compatible`, `incompatible` or `insufficient_sample`.
    pub verdict: &'static str,
    /// Our volume per session, summed over the sample, and the provider total.
    pub by_session: BTreeMap<String, u64>,
    pub provider_total: u64,
    pub ours_total: u64,
    pub by_condition: BTreeMap<String, ConditionTally>,
    /// Median ratio per stratum.
    pub by_stratum: BTreeMap<String, (usize, f64)>,
    pub rows: Vec<SymbolDay>,
}

const NOTE: &str = "valid only on days imported after the trade-identity fix (PR #51); before it, our count drops distinct FINRA TRF prints sharing (D, id)";

fn price_bucket(c: f64) -> &'static str {
    match c {
        c if c < 1.0 => "lt1",
        c if c < 5.0 => "1to5",
        c if c <= 25.0 => "5to25",
        _ => "other",
    }
}

/// FNV-1a 64 of `date|symbol`: a fixed, platform-independent order (not a security hash).
fn hash_key(date: NaiveDate, symbol: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in format!("{date}|{symbol}").bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn quantile(sorted: &[f64], q: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let i = ((sorted.len() - 1) as f64 * q).round() as usize;
    Some(sorted[i])
}

/// Our count for one archived symbol-day (module docs).
pub fn our_volume(archive: &Archive, date: NaiveDate, symbol: &str) -> Result<SymbolDay, String> {
    let reader = archive
        .reader(date, symbol)
        .map_err(|e| format!("{date} {symbol}: {e}"))?;
    let mut row = SymbolDay {
        date,
        symbol: symbol.to_string(),
        stratum: String::new(),
        provider: 0,
        ours: 0,
        ratio: 0.0,
        by_session: BTreeMap::new(),
        outside_window: 0,
        by_condition: BTreeMap::new(),
    };
    for ev in reader {
        let ev = ev.map_err(|e| format!("{date} {symbol}: {e}"))?;
        let MarketEvent::Trade(t) = ev.event else {
            continue;
        };
        let in_window = t.session.filter(|s| {
            s.date == date
                && matches!(
                    s.kind,
                    SessionKind::Pre | SessionKind::Regular | SessionKind::After
                )
        });
        let counted = in_window.is_some() && t.effect().updates_volume;
        let codes: Vec<String> = if t.conditions.is_empty() {
            vec!["@".into()]
        } else {
            t.conditions.iter().map(|c| c.trim().to_string()).collect()
        };
        for code in codes {
            let c = row.by_condition.entry(code).or_default();
            c.prints += 1;
            if counted {
                c.counted += t.size;
            } else {
                c.not_counted += t.size;
            }
        }
        match in_window {
            None => row.outside_window += t.size,
            Some(s) if counted => {
                row.ours += t.size;
                *row.by_session.entry(s.kind.to_string()).or_default() += t.size;
            }
            Some(_) => {}
        }
    }
    Ok(row)
}

/// Runs the comparison (module docs).
pub fn compare(archive: &Archive, opt: &CompatOptions) -> Result<CompatReport, String> {
    let dates: Vec<NaiveDate> = archive
        .raw_bar_dates()
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|d| *d >= opt.from && *d <= opt.to)
        .filter(|d| matches!(archive.day_data(*d), Some(DayData::Final)))
        .collect();
    let per_date = opt.sample.div_ceil(dates.len().max(1));
    let mut rows = Vec::new();
    for &date in &dates {
        let Some(manifest) = archive.manifest(date).map_err(|e| e.to_string())? else {
            continue;
        };
        if manifest.fixture && !opt.include_fixture {
            continue;
        }
        let Some(raw) = archive.read_raw_bars(date).map_err(|e| e.to_string())? else {
            continue;
        };
        // Candidates: archived symbols with a provider volume.
        let mut cands: Vec<(String, f64, u64)> = manifest
            .symbols
            .keys()
            .filter_map(|s| {
                let b = raw.bars.get(s)?;
                Some((s.clone(), b.c, b.v?))
            })
            .collect();
        let mut vols: Vec<u64> = cands.iter().map(|c| c.2).collect();
        vols.sort_unstable();
        let tercile = |v: u64| {
            let i = vols.partition_point(|x| *x < v);
            match (i * 3) / vols.len().max(1) {
                0 => "vlow",
                1 => "vmid",
                _ => "vhigh",
            }
        };
        let mut strata: BTreeMap<String, Vec<(String, f64, u64)>> = BTreeMap::new();
        cands.sort_by_key(|c| hash_key(date, &c.0));
        for c in cands {
            let key = format!("{}/{}", price_bucket(c.1), tercile(c.2));
            strata.entry(key).or_default().push(c);
        }
        let mut taken = 0;
        let mut round = 0;
        while taken < per_date {
            let mut any = false;
            for (key, list) in &strata {
                if taken >= per_date {
                    break;
                }
                let Some((sym, _, v)) = list.get(round) else {
                    continue;
                };
                any = true;
                let mut row = our_volume(archive, date, sym)?;
                row.stratum = key.clone();
                row.provider = *v;
                row.ratio = if *v == 0 {
                    if row.ours == 0 { 1.0 } else { f64::INFINITY }
                } else {
                    row.ours as f64 / *v as f64
                };
                rows.push(row);
                taken += 1;
            }
            if !any {
                break;
            }
            round += 1;
        }
    }
    let n = rows.len();
    let share = |tol: f64| {
        if n == 0 {
            0.0
        } else {
            rows.iter().filter(|r| (r.ratio - 1.0).abs() <= tol).count() as f64 / n as f64
        }
    };
    let mut ratios: Vec<f64> = rows.iter().map(|r| r.ratio).collect();
    ratios.sort_by(|a, b| a.total_cmp(b));
    let median = quantile(&ratios, 0.5);
    let within_2pct = share(TOLERANCE);
    let verdict = if n < opt.min_sample {
        "insufficient_sample"
    } else if within_2pct >= REQUIRED_SHARE && median.is_some_and(|m| (m - 1.0).abs() <= TOLERANCE)
    {
        "compatible"
    } else {
        "incompatible"
    };
    let mut by_session = BTreeMap::new();
    let mut by_condition: BTreeMap<String, ConditionTally> = BTreeMap::new();
    let mut by_stratum_ratios: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for r in &rows {
        for (k, v) in &r.by_session {
            *by_session.entry(k.clone()).or_insert(0u64) += v;
        }
        for (k, t) in &r.by_condition {
            let e = by_condition.entry(k.clone()).or_default();
            e.counted += t.counted;
            e.not_counted += t.not_counted;
            e.prints += t.prints;
        }
        by_stratum_ratios
            .entry(r.stratum.clone())
            .or_default()
            .push(r.ratio);
    }
    let by_stratum = by_stratum_ratios
        .into_iter()
        .map(|(k, mut v)| {
            v.sort_by(|a, b| a.total_cmp(b));
            let m = quantile(&v, 0.5).unwrap_or(f64::NAN);
            (k, (v.len(), m))
        })
        .collect();
    Ok(CompatReport {
        from: opt.from,
        to: opt.to,
        dates,
        note: NOTE,
        tolerance: TOLERANCE,
        required_share: REQUIRED_SHARE,
        symbol_days: n,
        within_1pct: share(0.01),
        within_2pct,
        within_5pct: share(0.05),
        median,
        p5: quantile(&ratios, 0.05),
        p95: quantile(&ratios, 0.95),
        verdict,
        by_session,
        provider_total: rows.iter().map(|r| r.provider).sum(),
        ours_total: rows.iter().map(|r| r.ours).sum(),
        by_condition,
        by_stratum,
        rows,
    })
}

impl CompatReport {
    pub fn summary(&self) -> String {
        let f = |x: Option<f64>| x.map_or("-".into(), |v| format!("{v:.4}"));
        format!(
            "volume compatibility {}..{} ({} dates, {} symbol-days): within ±1% {:.1}%, ±2% {:.1}%, ±5% {:.1}%; median {} p5 {} p95 {}; verdict {} (needs ≥{:.0}% within ±{:.0}%). {}",
            self.from,
            self.to,
            self.dates.len(),
            self.symbol_days,
            self.within_1pct * 100.0,
            self.within_2pct * 100.0,
            self.within_5pct * 100.0,
            f(self.median),
            f(self.p5),
            f(self.p95),
            self.verdict,
            self.required_share * 100.0,
            self.tolerance * 100.0,
            self.note
        )
    }
}
