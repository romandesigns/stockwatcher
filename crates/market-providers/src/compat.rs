//! The daily-volume study (`docs/stock-context/daily-baseline-plan.md` §9): how far is Alpaca's
//! SIP daily-bar volume `Vp` (`daily-bars/raw/`, the RVOL denominator of
//! `market_archive::baseline`) from `Ve`, the sum of eligible trade sizes under the engine's
//! corrected policy (the pillar's numerator: SIP prints 04:00 → session end, BOATS/overnight
//! excluded, one print per `PrintId` (PR #51: `(source, exchange, id, event_ts)`), counted when
//! `Trade::effect()` updates volume)? Trade counts (`n` vs eligible prints) are compared
//! separately. Offline: reads only the local archive, makes no request.
//!
//! **What it needs.** Imported (final) archive days with every print stored, plus the raw daily
//! bars of the same dates. The VPS holds the rolling ~21 trading days of trades (and, after this
//! branch deploys, the raw daily bars); a development machine has only the acquisition pilot (12
//! symbol-days, no raw daily bars). Days must have been imported after the trade-identity fix
//! (PR #51) **or** be imports that stored every provider print (the nightly import always did:
//! the bug dropped prints in the engine, not in the archive, and this harness applies the
//! corrected rule itself). A live recording (provisional day) is excluded: if prints were
//! discarded before storage it would be incomplete, and it is replaced by the import anyway.
//!
//! **Verdict wording.** The owner-accepted ±2 % for ≥ 95 % of symbol-days (median within ±2 %)
//! is an engineering diagnostic, not a certification: `within_engineering_tolerance` /
//! `outside_engineering_tolerance` / `insufficient_sample`. Outside → the fallback is the
//! separately versioned `provider_daily_volume_v1` metric with a matching live numerator.

use chrono::NaiveDate;
use market_archive::{Archive, DayData};
use market_events::session::{DayKind, day_kind};
use market_events::{MarketEvent, SessionKind};
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};

/// Owner-accepted tolerance (2026-10-20).
pub const TOLERANCE: f64 = 0.02;
/// Share of symbol-days that must be within [`TOLERANCE`].
pub const REQUIRED_SHARE: f64 = 0.95;
/// Below this many comparable symbol-days the verdict is `insufficient_sample`.
pub const MIN_SAMPLE: usize = 100;
/// The RVOL threshold whose classifications are checked for flips (pillars-v1: 5.0×).
pub const RVOL_THRESHOLD: f64 = 5.0;

#[derive(Clone, Debug)]
pub struct CompatOptions {
    pub from: NaiveDate,
    pub to: NaiveDate,
    /// Symbol-days wanted in total (spread evenly over the dates).
    pub sample: usize,
    pub min_sample: usize,
    /// Sampling seed (fixed by default, so a re-run picks the same symbol-days).
    pub seed: u64,
    /// Symbols followed across every date for the denominator / RVOL-flip study.
    pub denominator_symbols: usize,
    /// Fewest sessions a symbol needs for the denominator study.
    pub min_sessions: usize,
    /// Fixture days are never market evidence; only tests set this.
    pub include_fixture: bool,
}

impl CompatOptions {
    pub fn new(from: NaiveDate, to: NaiveDate) -> Self {
        CompatOptions {
            from,
            to,
            sample: 500,
            min_sample: MIN_SAMPLE,
            seed: 20261020,
            denominator_symbols: 100,
            min_sessions: 10,
            include_fixture: false,
        }
    }
}

/// Volume by condition code.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ConditionTally {
    pub counted: u64,
    pub not_counted: u64,
    pub prints: u64,
}

/// Our side of one symbol-day.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Eligible {
    /// Ve: eligible shares.
    pub volume: u64,
    /// Eligible prints.
    pub prints: u64,
    /// Repeats of a `PrintId` (dropped, as the engine does).
    pub duplicates: u64,
    pub by_session: BTreeMap<String, u64>,
    /// Prints outside pre/regular/after (overnight/BOATS, no session): not counted.
    pub outside_window: u64,
    pub by_condition: BTreeMap<String, ConditionTally>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SymbolDay {
    pub date: NaiveDate,
    pub symbol: String,
    /// Every stratum the symbol-day belongs to (`price:…`, `activity:…`, …).
    pub strata: Vec<String>,
    /// Vp and the provider's trade count.
    pub provider_volume: u64,
    pub provider_trades: Option<u64>,
    pub ours: Eligible,
    /// `|Vp − Ve| / Ve` (Ve > 0).
    pub relative_error: Option<f64>,
    /// `(Vp − Ve) / Ve` (Ve > 0).
    pub signed_error: Option<f64>,
    pub absolute_error: u64,
}

/// A symbol-day that could not be compared, and why.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Excluded {
    pub date: NaiveDate,
    pub symbol: Option<String>,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Distribution {
    pub n: usize,
    pub median: Option<f64>,
    pub p95: Option<f64>,
    pub p99: Option<f64>,
    pub max: Option<f64>,
}

fn distribution(mut v: Vec<f64>) -> Distribution {
    v.sort_by(|a, b| a.total_cmp(b));
    Distribution {
        n: v.len(),
        median: quantile(&v, 0.5),
        p95: quantile(&v, 0.95),
        p99: quantile(&v, 0.99),
        max: v.last().copied(),
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct StratumStats {
    pub symbol_days: usize,
    pub relative_error: Distribution,
    pub mean_signed_error: Option<f64>,
    pub within_2pct: f64,
}

/// The 30-session denominator and 5× classification study (leave-one-out over the available
/// sessions of the followed symbols: each session's full-day `Ve` against the mean of the others,
/// once with our volumes and once with the provider's). Full-day numerators and the ~21 available
/// sessions are proxies for the intraday numerator and 30 sessions; the report says so.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct DenominatorStudy {
    pub symbols: usize,
    pub sessions: usize,
    /// `mean(Vp) / mean(Ve)` per symbol.
    pub denominator_ratio: Distribution,
    /// Sessions whose RVOL (our denominator) is within [4×, 6×].
    pub near_threshold: usize,
    /// Sessions classified differently at 5× by the two denominators.
    pub flips: usize,
    pub flips_near_threshold: usize,
    pub note: &'static str,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CompatReport {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub seed: u64,
    pub dates: Vec<NaiveDate>,
    pub requirements: &'static str,
    pub tolerance: f64,
    pub required_share: f64,
    /// Comparable symbol-days (Ve > 0 and Vp > 0).
    pub symbol_days: usize,
    /// Pairs where either side is zero, kept apart: `(Ve, Vp)` zero patterns.
    pub zero_volume_pairs: BTreeMap<String, usize>,
    pub relative_error: Distribution,
    pub signed_error_mean: Option<f64>,
    pub signed_error_median: Option<f64>,
    pub absolute_error: Distribution,
    pub within_1pct: f64,
    pub within_2pct: f64,
    pub within_5pct: f64,
    /// `n_p / n_e` (provider trade count over eligible prints).
    pub trade_count_ratio: Distribution,
    /// `within_engineering_tolerance`, `outside_engineering_tolerance`, `insufficient_sample`.
    pub verdict: &'static str,
    pub verdict_note: &'static str,
    pub by_stratum: BTreeMap<String, StratumStats>,
    pub by_session: BTreeMap<String, u64>,
    pub by_condition: BTreeMap<String, ConditionTally>,
    pub duplicates_dropped: u64,
    pub excluded: Vec<Excluded>,
    pub denominators: DenominatorStudy,
    pub rows: Vec<SymbolDay>,
}

const REQUIREMENTS: &str = "imported (final) archive days with every print stored and raw daily bars for the same dates; the VPS holds ~21 trading days, a development machine only the pilot; days imported after PR #51 or full-print imports (the engine rule is applied here); live recordings excluded";
const VERDICT_NOTE: &str = "engineering diagnostic against the owner-accepted tolerance (>=95% within +-2%, median within +-2%), not a certification of the provider metric";

fn quantile(sorted: &[f64], q: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let i = ((sorted.len() - 1) as f64 * q).round() as usize;
    Some(sorted[i])
}

/// FNV-1a 64 of `seed|date|symbol`: a fixed, platform-independent order.
fn hash_key(seed: u64, date: NaiveDate, symbol: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in format!("{seed}|{date}|{symbol}").bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Our eligible count for one archived symbol-day (module docs).
pub fn our_volume(archive: &Archive, date: NaiveDate, symbol: &str) -> Result<Eligible, String> {
    let reader = archive
        .reader(date, symbol)
        .map_err(|e| format!("archive_read_error: {e}"))?;
    let mut out = Eligible::default();
    let mut seen = HashSet::new();
    for ev in reader {
        let ev = ev.map_err(|e| format!("archive_read_error: {e}"))?;
        let MarketEvent::Trade(t) = ev.event else {
            continue;
        };
        if !seen.insert(t.print_id()) {
            out.duplicates += 1;
            continue;
        }
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
            let c = out.by_condition.entry(code).or_default();
            c.prints += 1;
            if counted {
                c.counted += t.size;
            } else {
                c.not_counted += t.size;
            }
        }
        match in_window {
            None => out.outside_window += t.size,
            Some(s) if counted => {
                out.volume += t.size;
                out.prints += 1;
                *out.by_session.entry(s.kind.to_string()).or_default() += t.size;
            }
            Some(_) => {}
        }
    }
    Ok(out)
}

fn price_bucket(c: f64) -> &'static str {
    match c {
        c if c < 1.0 => "lt1",
        c if c < 2.0 => "1to2",
        c if c <= 20.0 => "2to20",
        c if c <= 25.0 => "20to25",
        _ => "other",
    }
}

struct Candidate {
    symbol: String,
    close: f64,
    v: u64,
    n: Option<u64>,
    prev_close: Option<f64>,
}

/// Runs the study (module docs).
pub fn compare(archive: &Archive, opt: &CompatOptions) -> Result<CompatReport, String> {
    let mut excluded = Vec::new();
    let raw_dates: Vec<NaiveDate> = archive
        .raw_bar_dates()
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|d| *d >= opt.from && *d <= opt.to)
        .collect();
    let mut dates = Vec::new();
    for d in raw_dates {
        match archive.day_data(d) {
            Some(DayData::Final) => dates.push(d),
            Some(DayData::Provisional { .. }) => excluded.push(Excluded {
                date: d,
                symbol: None,
                reason: "provisional_live_recording".into(),
            }),
            None => excluded.push(Excluded {
                date: d,
                symbol: None,
                reason: "no_archived_trades".into(),
            }),
        }
    }
    // Symbols with a corporate action effective in the range (split/rename examples).
    let ledger = archive.read_action_ledger().unwrap_or_default();
    let mut action_symbols = std::collections::BTreeSet::new();
    for e in ledger.values() {
        let a = &e.versions.last().expect("non-empty").2;
        if a.effective_date
            .is_some_and(|x| x >= opt.from && x <= opt.to)
        {
            action_symbols.extend(a.symbol.clone());
            action_symbols.extend(a.new_symbol.clone());
        }
    }
    let per_date = opt.sample.div_ceil(dates.len().max(1));
    let mut rows = Vec::new();
    let mut zero_pairs: BTreeMap<String, usize> = BTreeMap::new();
    // Per symbol, per date: (Ve, Vp) for the denominator study.
    let mut series: BTreeMap<String, BTreeMap<NaiveDate, (u64, u64)>> = BTreeMap::new();
    let mut followed: Vec<String> = Vec::new();
    for (di, &date) in dates.iter().enumerate() {
        let Some(manifest) = archive.manifest(date).map_err(|e| e.to_string())? else {
            continue;
        };
        if manifest.fixture && !opt.include_fixture {
            excluded.push(Excluded {
                date,
                symbol: None,
                reason: "fixture_day".into(),
            });
            continue;
        }
        let Some(raw) = archive.read_raw_bars(date).map_err(|e| e.to_string())? else {
            continue;
        };
        let prev = dates
            .get(di.wrapping_sub(1))
            .and_then(|p| archive.read_raw_bars(*p).ok().flatten());
        let mut cands = Vec::new();
        for (s, entry) in &manifest.symbols {
            if entry.import.as_ref().is_some_and(|i| !i.complete) {
                excluded.push(Excluded {
                    date,
                    symbol: Some(s.clone()),
                    reason: "import_incomplete".into(),
                });
                continue;
            }
            let Some(b) = raw.bars.get(s) else {
                excluded.push(Excluded {
                    date,
                    symbol: Some(s.clone()),
                    reason: if raw.no_bar.contains(s) {
                        "provider_returned_no_bar".into()
                    } else {
                        "not_in_raw_bars".into()
                    },
                });
                continue;
            };
            let Some(v) = b.v else {
                excluded.push(Excluded {
                    date,
                    symbol: Some(s.clone()),
                    reason: "provider_bar_without_volume".into(),
                });
                continue;
            };
            cands.push(Candidate {
                symbol: s.clone(),
                close: b.c,
                v,
                n: b.n,
                prev_close: prev.as_ref().and_then(|p| p.bars.get(s)).map(|b| b.c),
            });
        }
        let mut vols: Vec<u64> = cands.iter().map(|c| c.v).collect();
        vols.sort_unstable();
        let activity = |v: u64| match (vols.partition_point(|x| *x < v) * 3) / vols.len().max(1) {
            0 => "low",
            1 => "mid",
            _ => "high",
        };
        // Sampling strata: price × activity; round-robin in seeded hash order.
        let mut strata: BTreeMap<String, Vec<&Candidate>> = BTreeMap::new();
        let mut order: Vec<&Candidate> = cands.iter().collect();
        order.sort_by_key(|c| hash_key(opt.seed, date, &c.symbol));
        if di == 0 {
            followed = order
                .iter()
                .take(opt.denominator_symbols)
                .map(|c| c.symbol.clone())
                .collect();
        }
        for c in order {
            strata
                .entry(format!("{}/{}", price_bucket(c.close), activity(c.v)))
                .or_default()
                .push(c);
        }
        let mut picked: Vec<&Candidate> = Vec::new();
        let mut round = 0;
        while picked.len() < per_date {
            let mut any = false;
            for list in strata.values() {
                if picked.len() >= per_date {
                    break;
                }
                if let Some(c) = list.get(round) {
                    picked.push(c);
                    any = true;
                }
            }
            if !any {
                break;
            }
            round += 1;
        }
        let early = matches!(day_kind(date), DayKind::EarlyClose(_));
        for c in cands.iter() {
            let in_sample = picked.iter().any(|p| p.symbol == c.symbol);
            let follow = followed.contains(&c.symbol);
            if !in_sample && !follow {
                continue;
            }
            let ours = match our_volume(archive, date, &c.symbol) {
                Ok(o) => o,
                Err(e) => {
                    excluded.push(Excluded {
                        date,
                        symbol: Some(c.symbol.clone()),
                        reason: e,
                    });
                    continue;
                }
            };
            if follow {
                series
                    .entry(c.symbol.clone())
                    .or_default()
                    .insert(date, (ours.volume, c.v));
            }
            if !in_sample {
                continue;
            }
            let ve = ours.volume;
            if ve == 0 || c.v == 0 {
                let key = format!("ve_{}_vp_{}", ve.min(1), c.v.min(1));
                *zero_pairs.entry(key).or_default() += 1;
                continue;
            }
            let mut strata = vec![
                format!("price:{}", price_bucket(c.close)),
                format!("activity:{}", activity(c.v)),
            ];
            let gainer = c.prev_close.is_some_and(|p| p > 0.0 && c.close / p >= 1.10);
            if gainer {
                strata.push("relevance:gainer_10pct".into());
                if (2.0..=20.0).contains(&c.close) {
                    // Float/market cap are not in the archive: in the pillars' price band and up
                    // 10 % stands in for "small-cap gainer".
                    strata.push("relevance:pillar_band_gainer".into());
                }
            }
            let ext: u64 = ours
                .by_session
                .iter()
                .filter(|(k, _)| k.as_str() != "regular")
                .map(|(_, v)| *v)
                .sum();
            strata.push(
                if ext as f64 > 0.25 * ve as f64 {
                    "sessions:extended_heavy"
                } else {
                    "sessions:regular_heavy"
                }
                .into(),
            );
            if ours.prints < 100 {
                strata.push("trading:sparse_lt100_prints".into());
            }
            if early {
                strata.push("calendar:early_close".into());
            }
            if action_symbols.contains(&c.symbol) {
                strata.push("corporate_action:split_or_rename".into());
            }
            let diff = c.v as f64 - ve as f64;
            rows.push(SymbolDay {
                date,
                symbol: c.symbol.clone(),
                strata,
                provider_volume: c.v,
                provider_trades: c.n,
                relative_error: Some(diff.abs() / ve as f64),
                signed_error: Some(diff / ve as f64),
                absolute_error: diff.abs() as u64,
                ours,
            });
        }
    }

    let n = rows.len();
    let rel: Vec<f64> = rows.iter().filter_map(|r| r.relative_error).collect();
    let signed: Vec<f64> = rows.iter().filter_map(|r| r.signed_error).collect();
    let share = |tol: f64| {
        if n == 0 {
            0.0
        } else {
            rel.iter().filter(|e| **e <= tol).count() as f64 / n as f64
        }
    };
    let within_2pct = share(TOLERANCE);
    let mut signed_sorted = signed.clone();
    signed_sorted.sort_by(|a, b| a.total_cmp(b));
    let signed_median = quantile(&signed_sorted, 0.5);
    let verdict = if n < opt.min_sample {
        "insufficient_sample"
    } else if within_2pct >= REQUIRED_SHARE && signed_median.is_some_and(|m| m.abs() <= TOLERANCE) {
        "within_engineering_tolerance"
    } else {
        "outside_engineering_tolerance"
    };
    let mut by_stratum: BTreeMap<String, Vec<&SymbolDay>> = BTreeMap::new();
    let mut by_session = BTreeMap::new();
    let mut by_condition: BTreeMap<String, ConditionTally> = BTreeMap::new();
    let mut duplicates = 0;
    for r in &rows {
        for s in &r.strata {
            by_stratum.entry(s.clone()).or_default().push(r);
        }
        for (k, v) in &r.ours.by_session {
            *by_session.entry(k.clone()).or_insert(0u64) += v;
        }
        for (k, t) in &r.ours.by_condition {
            let e = by_condition.entry(k.clone()).or_default();
            e.counted += t.counted;
            e.not_counted += t.not_counted;
            e.prints += t.prints;
        }
        duplicates += r.ours.duplicates;
    }
    let by_stratum = by_stratum
        .into_iter()
        .map(|(k, rs)| {
            let rel: Vec<f64> = rs.iter().filter_map(|r| r.relative_error).collect();
            let within =
                rel.iter().filter(|e| **e <= TOLERANCE).count() as f64 / rel.len().max(1) as f64;
            let signed: Vec<f64> = rs.iter().filter_map(|r| r.signed_error).collect();
            let mean =
                (!signed.is_empty()).then(|| signed.iter().sum::<f64>() / signed.len() as f64);
            (
                k,
                StratumStats {
                    symbol_days: rs.len(),
                    relative_error: distribution(rel),
                    mean_signed_error: mean,
                    within_2pct: within,
                },
            )
        })
        .collect();

    Ok(CompatReport {
        from: opt.from,
        to: opt.to,
        seed: opt.seed,
        dates,
        requirements: REQUIREMENTS,
        tolerance: TOLERANCE,
        required_share: REQUIRED_SHARE,
        symbol_days: n,
        zero_volume_pairs: zero_pairs,
        relative_error: distribution(rel.clone()),
        signed_error_mean: (!signed.is_empty())
            .then(|| signed.iter().sum::<f64>() / signed.len() as f64),
        signed_error_median: signed_median,
        absolute_error: distribution(rows.iter().map(|r| r.absolute_error as f64).collect()),
        within_1pct: share(0.01),
        within_2pct,
        within_5pct: share(0.05),
        trade_count_ratio: distribution(
            rows.iter()
                .filter_map(|r| {
                    let n = r.provider_trades? as f64;
                    (r.ours.prints > 0).then(|| n / r.ours.prints as f64)
                })
                .collect(),
        ),
        verdict,
        verdict_note: VERDICT_NOTE,
        by_stratum,
        by_session,
        by_condition,
        duplicates_dropped: duplicates,
        excluded,
        denominators: denominator_study(&series, opt.min_sessions),
        rows,
    })
}

/// Leave-one-out 5× classification with our vs the provider's denominator (see
/// [`DenominatorStudy`]).
pub fn denominator_study(
    series: &BTreeMap<String, BTreeMap<NaiveDate, (u64, u64)>>,
    min_sessions: usize,
) -> DenominatorStudy {
    let mut out = DenominatorStudy {
        note: "full-day numerators and the available sessions (<=21 on the VPS) stand in for the intraday numerator and 30 sessions",
        ..DenominatorStudy::default()
    };
    let mut ratios = Vec::new();
    for days in series.values() {
        let v: Vec<(u64, u64)> = days.values().copied().filter(|(e, _)| *e > 0).collect();
        if v.len() < min_sessions.max(2) {
            continue;
        }
        out.symbols += 1;
        let (se, sp): (u64, u64) = v.iter().fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
        if se > 0 {
            ratios.push(sp as f64 / se as f64);
        }
        for (i, (ve, _)) in v.iter().enumerate() {
            let k = (v.len() - 1) as f64;
            let mean_e = (se - v[i].0) as f64 / k;
            let mean_p = (sp - v[i].1) as f64 / k;
            if mean_e <= 0.0 || mean_p <= 0.0 {
                continue;
            }
            out.sessions += 1;
            let (re, rp) = (*ve as f64 / mean_e, *ve as f64 / mean_p);
            let near = (4.0..=6.0).contains(&re);
            if near {
                out.near_threshold += 1;
            }
            if (re >= RVOL_THRESHOLD) != (rp >= RVOL_THRESHOLD) {
                out.flips += 1;
                if near {
                    out.flips_near_threshold += 1;
                }
            }
        }
    }
    out.denominator_ratio = distribution(ratios);
    out
}

impl CompatReport {
    pub fn summary(&self) -> String {
        let f = |x: Option<f64>| x.map_or("-".into(), |v| format!("{v:.4}"));
        format!(
            "volume study {}..{} (seed {}, {} dates, {} comparable symbol-days, {} zero-volume pairs, {} excluded): relative error median {} p95 {} p99 {} max {}; signed bias mean {} median {}; within ±1% {:.1}%, ±2% {:.1}%, ±5% {:.1}%; denominators: {} symbols, {} sessions, {} near 5x, {} flips. Verdict: {} ({}). Needs: {}",
            self.from,
            self.to,
            self.seed,
            self.dates.len(),
            self.symbol_days,
            self.zero_volume_pairs.values().sum::<usize>(),
            self.excluded.len(),
            f(self.relative_error.median),
            f(self.relative_error.p95),
            f(self.relative_error.p99),
            f(self.relative_error.max),
            f(self.signed_error_mean),
            f(self.signed_error_median),
            self.within_1pct * 100.0,
            self.within_2pct * 100.0,
            self.within_5pct * 100.0,
            self.denominators.symbols,
            self.denominators.sessions,
            self.denominators.near_threshold,
            self.denominators.flips,
            self.verdict,
            self.verdict_note,
            self.requirements
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_provider_denominator_10pct_high_flips_sessions_just_above_5x() {
        // 12 sessions of 100k (ours) / 110k (provider) and one session at 520k: 5.2x on our
        // denominator, 4.73x on the provider's → one flip, near the threshold.
        let mut days = BTreeMap::new();
        for i in 0..12 {
            days.insert(
                NaiveDate::from_ymd_opt(2026, 9, 1 + i).unwrap(),
                (100_000, 110_000),
            );
        }
        days.insert(
            NaiveDate::from_ymd_opt(2026, 9, 20).unwrap(),
            (520_000, 572_000),
        );
        let s = denominator_study(&BTreeMap::from([("AAA".to_string(), days)]), 10);
        assert_eq!((s.symbols, s.sessions), (1, 13));
        assert_eq!(
            (s.near_threshold, s.flips, s.flips_near_threshold),
            (1, 1, 1)
        );
        let r = s.denominator_ratio.median.unwrap();
        assert!((r - 1.1).abs() < 1e-9);
    }
}
