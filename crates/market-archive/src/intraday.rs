//! Intraday cumulative-volume curves derived from already-archived prints: the **diagnostic**
//! denominator for the same-time and recent-window RVOL measures.
//!
//! # What this is for, and what it is not
//!
//! `stock_context::IntradayBaseline` has existed and been tested since the pillars went in, and
//! nothing in production ever produced one, so `RvolDiagnostics::same_time_milli` and
//! `recent_milli` were permanently `None` and the panel could only ever report the
//! daily-denominator measure (the audit's F5/F10, `daily-baseline-plan.md` "increment 2b"). This
//! module is the missing producer.
//!
//! It is **diagnostic-only and stays so**. The RVOL pillar keeps the owner's 30-session
//! daily-denominator rule, byte for byte; nothing here changes a threshold, a minimum number of
//! sessions, or what the Ross Panel admits. A curve is a measurement reported beside the pillar,
//! never an input to it.
//!
//! # Where the volume comes from
//!
//! One source only: the day's **own archived prints**, counted under the same volume-condition
//! policy the live numerator uses (`Trade::effect().updates_volume`), deduplicated by
//! [`market_events::PrintId`], with corrections and cancels applied where the reference resolves
//! to exactly one retained print. Nothing is interpolated, extrapolated, back-filled or carried
//! across symbols. A session with no archived file simply has no curve, and the engine then
//! reports `insufficient_sessions` rather than a number.
//!
//! A provider's one-minute bars are a **separate, coarser series** with their own name and
//! provenance ([`MinuteVolumeSeries`]). There is deliberately no conversion from them into an
//! [`IntradayCurveRecord`]: spreading a minute's volume over four fifteen-second points would
//! invent data the provider never gave, and labelling it as the same kind of curve would hide
//! that. `asserts_no_provider_bar_curve` pins it.
//!
//! # Provenance on every record
//!
//! Each record carries the file it was derived from and that file's SHA-256 from the day
//! manifest, the record count, the sources, a fidelity label, the rules version, when the
//! derivation ran (`observed_at`), and a coverage block with what was counted, excluded and left
//! unresolved. `complete` is only ever true for a **final (imported)** day: a live recording is
//! provisional and the recorder's own gaps are not provable from the file, so those sessions are
//! published with `complete: false` and the engine leaves them out of the diagnostic. A covered
//! zero is a valid zero; a gap is not.

use crate::{Archive, ArchiveError, DayManifest, SymbolEntry, valid_symbol};
use chrono::NaiveDate;
use market_events::conditions::trade_effect;
use market_events::session::{et, sessions_for};
use market_events::{MarketEvent, Price, Resolution, Source, TradeId, TradeRef};
use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;

pub const INTRADAY_DIR: &str = "intraday-curves";

/// The rules this module derives a curve under. A changed rule is a new version string; the
/// stored prints do not change, so any version can be re-derived over the same history.
pub const CURVE_RULES_VERSION: &str = "intraday-curves-v1";

/// Grid step of a curve's points, seconds. The engine's own bar grid: a curve read at a decision
/// boundary needs no interpolation.
pub const STEP_SECS: i64 = 15;

/// How far back a correction or cancel may reach for the print it names.
///
/// Revisions arrive close behind their print, and holding a whole hot symbol's day in a map to
/// serve a late one is how an off-path job becomes an unbounded one. A reference older than this
/// is counted in [`CurveCoverage::unresolved_revisions`] rather than guessed at.
pub const REVISION_WINDOW_SECS: i64 = 3_600;

/// What a curve was derived from. Deliberately only the recorded-print kinds: there is no
/// provider-bar variant, because this module never derives a curve from provider bars.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CurveFidelity {
    /// A final (imported) day's prints: the day as the provider restates it.
    ImportedHistory,
    /// The live recording's own prints, provisional until the nightly import replaces the day.
    RecordedLive,
    /// Fixture data. **Never market evidence**; carried so a reader can refuse it.
    SyntheticFixture,
}

impl CurveFidelity {
    fn of(entry: &SymbolEntry, live_day: bool) -> Self {
        if entry.is_fixture() {
            CurveFidelity::SyntheticFixture
        } else if entry.import.is_some() && !live_day {
            CurveFidelity::ImportedHistory
        } else {
            CurveFidelity::RecordedLive
        }
    }

    /// Whether a session derived from this is a comparable prior session for the diagnostic.
    pub fn usable_as_history(self) -> bool {
        self == CurveFidelity::ImportedHistory
    }
}

/// The file a curve came from, and what the manifest says about it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurveSource {
    pub fidelity: CurveFidelity,
    /// File name inside the day directory, e.g. `"SMPL.swa"`.
    pub file: String,
    /// The manifest's SHA-256 of that file: the curve is reproducible from exactly these bytes.
    pub sha256: String,
    /// Records the manifest counted in it.
    pub events: u64,
    pub sources: Vec<Source>,
}

/// What the derivation saw. Nothing here is inferred; every number is counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurveCoverage {
    /// Prints whose conditions let them update volume, duplicates excluded.
    pub counted: u64,
    /// Prints the volume-condition policy excluded.
    pub excluded: u64,
    /// Redeliveries of a print already counted (same [`market_events::PrintId`]).
    pub duplicates: u64,
    /// Corrections and cancels applied to exactly one retained print.
    pub revisions_applied: u64,
    /// Corrections and cancels whose reference matched no retained print, or more than one, or
    /// reached further back than [`REVISION_WINDOW_SECS`]. Never guessed at.
    pub unresolved_revisions: u64,
    /// Counted prints outside the session window (they are not in the curve).
    pub outside_window: u64,
    /// First and last counted execution time (UTC ns).
    pub first_ts: Option<i64>,
    pub last_ts: Option<i64>,
}

/// One session's cumulative eligible-volume curve for one symbol, with its provenance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntradayCurveRecord {
    pub v: u32,
    pub symbol: String,
    pub date: NaiveDate,
    pub step_secs: i64,
    /// `(seconds since 04:00 ET, cumulative eligible shares)`, ascending, starting at `(0, 0)`.
    /// A point is emitted only where the cumulative changed, so a lookup takes the last point at
    /// or before the offset it asks about (`stock_context::IntradayCurve::at`).
    pub cum: Vec<(i64, u64)>,
    /// The whole 04:00 → session end window was collected. Only ever true for a final day.
    pub complete: bool,
    pub source: CurveSource,
    pub coverage: CurveCoverage,
    /// `intraday-curves-v1`.
    pub rules: String,
    /// When the derivation ran (UTC ns). Never a market time.
    pub observed_at: i64,
}

impl IntradayCurveRecord {
    /// Cumulative eligible volume at `offset` seconds since 04:00 (the last point at or before).
    pub fn at(&self, offset: i64) -> u64 {
        let i = self.cum.partition_point(|&(s, _)| s <= offset);
        if i == 0 { 0 } else { self.cum[i - 1].1 }
    }

    /// The session's total eligible volume inside the window.
    pub fn total(&self) -> u64 {
        self.cum.last().map(|&(_, c)| c).unwrap_or(0)
    }

    /// Usable as a prior session for the diagnostic: a final day, collected whole.
    pub fn usable_as_history(&self) -> bool {
        self.complete && self.source.fidelity.usable_as_history()
    }
}

/// A provider's one-minute volume series for a session.
///
/// A **separate, coarser** series: its own name, its own provenance, and no conversion into an
/// [`IntradayCurveRecord`] anywhere in this crate. Spreading a minute's volume across four
/// fifteen-second points would invent data the provider never gave, and giving the result the
/// same type as a print-derived curve would hide that it was invented. It is also not
/// interchangeable with the strict RVOL numerator, which counts condition-filtered prints: using
/// one for the other needs a compatibility study, not a cast.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MinuteVolumeSeries {
    pub symbol: String,
    pub date: NaiveDate,
    pub provider: String,
    /// `(seconds since 04:00 ET, volume in that minute)`, ascending.
    pub minutes: Vec<(i64, u64)>,
    /// When the bars were retrieved (UTC ns).
    pub observed_at: i64,
}

/// A print retained for a revision to name.
struct Held {
    exec: i64,
    price: Price,
    size: u64,
    counted: bool,
}

/// Seconds since 04:00 ET of `date`, and the session window's last offset.
fn window(date: NaiveDate) -> Option<(i64, i64)> {
    let start = et(date, 4, 0, 0, 0)?.nanos();
    let end = sessions_for(date).last().map(|s| s.2.nanos())?;
    Some((start, (end - start) / 1_000_000_000))
}

/// Derives `symbol`'s curve for `date` from the archived prints, or `None` when the day has no
/// file for it (missing, not zero).
///
/// `observed_at` is when the derivation ran. Bounded: one symbol's prints are streamed, and only
/// the last [`REVISION_WINDOW_SECS`] of them are retained for revisions to name.
pub fn derive_curve(
    archive: &Archive,
    manifest: &DayManifest,
    symbol: &str,
    date: NaiveDate,
    observed_at: i64,
) -> Result<Option<IntradayCurveRecord>, ArchiveError> {
    if !valid_symbol(symbol) {
        return Err(ArchiveError::InvalidSymbol(symbol.to_string()));
    }
    let Some(entry) = manifest.symbols.get(symbol) else {
        return Ok(None);
    };
    let Some((start, last_offset)) = window(date) else {
        return Ok(None);
    };
    let live_day = manifest.live.is_some();
    let fidelity = CurveFidelity::of(entry, live_day);
    let mut cov = CurveCoverage::default();
    // Cumulative eligible volume at each grid step that changed, and the running total.
    let mut cum: Vec<(i64, u64)> = vec![(0, 0)];
    let mut total: u64 = 0;
    // Bounded to the revision window: the prints a correction or cancel may still name, and the
    // identities a redelivery may still repeat. A reconnect replay repeats a print within
    // seconds, so a window an hour wide is a generous bound and not a guess — anything older is
    // counted as unresolved rather than matched.
    let mut held: VecDeque<(TradeId, Held)> = VecDeque::new();
    let mut ids: HashSet<(TradeId, i64)> = HashSet::new();

    let mut push = |offset: i64, total: u64| {
        let step = offset.div_euclid(STEP_SECS) * STEP_SECS + STEP_SECS;
        let step = step.min(last_offset);
        match cum.last_mut() {
            Some(last) if last.0 == step => last.1 = total,
            Some(last) if last.0 > step => {}
            _ => cum.push((step, total)),
        }
    };

    for rec in archive.reader(date, symbol)? {
        let ev = rec?.event;
        let exec = ev.event_ts().nanos();
        // Keep the retention window bounded: anything older than the newest print by more than
        // the revision window can no longer be named.
        while held
            .front()
            .is_some_and(|(_, h)| exec - h.exec > REVISION_WINDOW_SECS * 1_000_000_000)
        {
            if let Some((id, h)) = held.pop_front() {
                ids.remove(&(id, h.exec));
            }
        }
        match ev {
            MarketEvent::Trade(t) => {
                let counted = t.effect().updates_volume;
                if !ids.insert((t.id(), exec)) {
                    // Same identity to the nanosecond: a redelivery, not a second print
                    // (`market_events::PrintId`).
                    cov.duplicates += 1;
                    continue;
                }
                held.push_back((
                    t.id(),
                    Held {
                        exec,
                        price: t.price,
                        size: t.size,
                        counted,
                    },
                ));
                if !counted {
                    cov.excluded += 1;
                    continue;
                }
                let offset = (exec - start) / 1_000_000_000;
                if offset < 0 || offset > last_offset {
                    cov.outside_window += 1;
                    continue;
                }
                cov.counted += 1;
                cov.first_ts = Some(cov.first_ts.map_or(exec, |x: i64| x.min(exec)));
                cov.last_ts = Some(cov.last_ts.map_or(exec, |x: i64| x.max(exec)));
                total += t.size;
                push(offset, total);
            }
            MarketEvent::Correction(c) => {
                let at = ((c.event_ts.nanos() - start) / 1_000_000_000).clamp(0, last_offset);
                match resolve(&held, &c.target()) {
                    Some(i) => {
                        cov.revisions_applied += 1;
                        let was = if held[i].1.counted { held[i].1.size } else { 0 };
                        let counts = trade_effect(&c.corrected_conditions, c.tape).updates_volume;
                        let h = &mut held[i].1;
                        h.price = c.corrected_price;
                        h.size = c.corrected_size;
                        h.counted = counts;
                        let now = if counts { c.corrected_size } else { 0 };
                        total = total.saturating_sub(was) + now;
                        push(at, total);
                    }
                    None => cov.unresolved_revisions += 1,
                }
            }
            MarketEvent::Cancel(c) => {
                let at = ((c.event_ts.nanos() - start) / 1_000_000_000).clamp(0, last_offset);
                match resolve(&held, &c.target()) {
                    Some(i) => {
                        cov.revisions_applied += 1;
                        let was = if held[i].1.counted { held[i].1.size } else { 0 };
                        held[i].1.counted = false;
                        total = total.saturating_sub(was);
                        push(at, total);
                    }
                    None => cov.unresolved_revisions += 1,
                }
            }
            MarketEvent::Quote(_) | MarketEvent::Status(_) => {}
        }
    }
    // Completeness is the import's own answer, not an assumption. A live recording is
    // provisional and its own gaps are not provable from the file; a final day claims the window
    // only when every page was fetched and the provider returned no further token
    // (`ImportProvenance::complete`). A covered zero is a valid zero; a gap is not.
    let complete = fidelity == CurveFidelity::ImportedHistory
        && entry.import.as_ref().is_some_and(|i| i.complete);
    Ok(Some(IntradayCurveRecord {
        v: 1,
        symbol: symbol.to_string(),
        date,
        step_secs: STEP_SECS,
        cum,
        complete,
        source: CurveSource {
            fidelity,
            file: entry.file.clone(),
            sha256: entry.sha256.clone(),
            events: entry.events,
            sources: entry.sources.clone(),
        },
        coverage: cov,
        rules: CURVE_RULES_VERSION.to_string(),
        observed_at,
    }))
}

/// The retained print a reference names, by index, or `None` when none or several fit.
///
/// [`TradeRef::resolve`]'s own rule, over the prints this pass still holds: a reference that fits
/// more than one is `Ambiguous` and applying it would be a guess, so it is counted instead
/// ([`CurveCoverage::unresolved_revisions`]).
fn resolve(held: &VecDeque<(TradeId, Held)>, target: &TradeRef) -> Option<usize> {
    let candidates: Vec<(usize, Price, u64)> = held
        .iter()
        .enumerate()
        .filter(|(_, (id, _))| *id == target.id)
        .map(|(i, (_, h))| (i, h.price, h.size))
        .collect();
    match target.resolve(candidates) {
        Resolution::Found(i) => Some(i),
        Resolution::NotFound | Resolution::Ambiguous => None,
    }
}

impl Archive {
    fn intraday_dir(&self, date: NaiveDate) -> PathBuf {
        self.root()
            .join(INTRADAY_DIR)
            .join(date.format("%Y-%m-%d").to_string())
    }

    /// Writes a derived curve, replacing any earlier derivation of the same `(date, symbol)`
    /// (a re-derivation under the same rules is the same record; under new rules it is a new
    /// one, and the rules version on it says which).
    pub fn write_intraday_curve(&self, r: &IntradayCurveRecord) -> Result<(), ArchiveError> {
        if !valid_symbol(&r.symbol) {
            return Err(ArchiveError::InvalidSymbol(r.symbol.clone()));
        }
        let dir = self.intraday_dir(r.date);
        std::fs::create_dir_all(&dir).map_err(|e| ArchiveError::io(&dir, e))?;
        let path = dir.join(format!("{}.json", r.symbol));
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(r).expect("serializes"))
            .map_err(|e| ArchiveError::io(&tmp, e))?;
        std::fs::rename(&tmp, &path).map_err(|e| ArchiveError::io(&path, e))?;
        Ok(())
    }

    /// The stored curve for `(date, symbol)`, or `None` when none was derived.
    pub fn read_intraday_curve(
        &self,
        date: NaiveDate,
        symbol: &str,
    ) -> Result<Option<IntradayCurveRecord>, ArchiveError> {
        if !valid_symbol(symbol) {
            return Err(ArchiveError::InvalidSymbol(symbol.to_string()));
        }
        let path = self.intraday_dir(date).join(format!("{symbol}.json"));
        match std::fs::read(&path) {
            Ok(b) => Ok(Some(
                serde_json::from_slice(&b)
                    .map_err(|e| ArchiveError::format(&path, e.to_string()))?,
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(ArchiveError::io(&path, e)),
        }
    }

    /// Curves stored for `symbol` over `dates`, oldest first. Missing dates are simply absent.
    pub fn read_intraday_curves(
        &self,
        symbol: &str,
        dates: &[NaiveDate],
    ) -> Result<Vec<IntradayCurveRecord>, ArchiveError> {
        let mut out = Vec::new();
        for d in dates {
            if let Some(r) = self.read_intraday_curve(*d, symbol)? {
                out.push(r);
            }
        }
        out.sort_by_key(|r| r.date);
        Ok(out)
    }
}

/// The engine's diagnostic baseline from stored curves, or `None` when none are usable.
///
/// Only curves a prior **final** day was collected whole for are offered
/// ([`IntradayCurveRecord::usable_as_history`]); a provisional live recording and a fixture are
/// carried as `complete: false` so the engine leaves them out, and the minimum number of
/// sessions the diagnostic needs is the policy's, unchanged. `observed_at` is the latest
/// derivation time, so the records join decisions from when they existed and not before.
pub fn intraday_baseline(
    records: &[IntradayCurveRecord],
    symbol: &str,
) -> Option<stock_context::IntradayBaseline> {
    let observed_at = records.iter().map(|r| r.observed_at).max()?;
    let sessions: Vec<stock_context::IntradayCurve> = records
        .iter()
        .map(|r| stock_context::IntradayCurve {
            date: r.date,
            cum: r.cum.clone(),
            complete: r.usable_as_history(),
        })
        .collect();
    if sessions.is_empty() {
        return None;
    }
    Some(stock_context::IntradayBaseline {
        id: format!("{CURVE_RULES_VERSION}:intraday:{symbol}"),
        version: 1,
        instrument: stock_context::InstrumentId::symbol(symbol),
        sessions,
        observed_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fidelity label has no provider-bar variant, and nothing in this module converts a
    /// [`MinuteVolumeSeries`] into a curve. Interpolating minute bars into fifteen-second points
    /// would invent data the provider never gave.
    #[test]
    fn asserts_no_provider_bar_curve() {
        for f in [
            CurveFidelity::ImportedHistory,
            CurveFidelity::RecordedLive,
            CurveFidelity::SyntheticFixture,
        ] {
            let json = serde_json::to_string(&f).expect("serializes");
            assert!(
                !json.contains("bar") && !json.contains("minute"),
                "a curve is never derived from bars: {json}"
            );
        }
        // Only a final day's whole-window collection is prior-session history.
        assert!(CurveFidelity::ImportedHistory.usable_as_history());
        assert!(!CurveFidelity::RecordedLive.usable_as_history());
        assert!(!CurveFidelity::SyntheticFixture.usable_as_history());
    }

    #[test]
    fn a_curve_reads_the_last_point_at_or_before_an_offset() {
        let r = IntradayCurveRecord {
            v: 1,
            symbol: "AAA".into(),
            date: NaiveDate::from_ymd_opt(2026, 10, 15).expect("valid"),
            step_secs: STEP_SECS,
            cum: vec![(0, 0), (20_100, 10), (20_400, 30)],
            complete: true,
            source: CurveSource {
                fidelity: CurveFidelity::ImportedHistory,
                file: "AAA.swa".into(),
                sha256: "00".into(),
                events: 2,
                sources: vec![],
            },
            coverage: CurveCoverage::default(),
            rules: CURVE_RULES_VERSION.into(),
            observed_at: 1,
        };
        assert_eq!((r.at(0), r.at(20_099), r.at(20_100)), (0, 0, 10));
        assert_eq!((r.at(20_399), r.at(20_400), r.at(99_999)), (10, 30, 30));
        assert_eq!(r.total(), 30);
        assert!(r.usable_as_history());
    }
}
