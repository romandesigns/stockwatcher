//! # Deterministic fixture days. FIXTURE DATA: NOT MARKET DATA.
//!
//! Generates a seeded synthetic trading day for any symbols, in **arrival order**, with every
//! event marked [`Source::Fixture`]. It exists to drive software tests and the simulator before
//! real archives exist, and must never be presented as market evidence.
//!
//! The price walk, timing and print mix are a port of the web app's former in-browser fixture
//! (`apps/web/src/fixtures/fixtureFeed.ts`, mulberry32, same constants, same order of random
//! draws; deleted 2026-10-31, in git history). This is now the ONLY fixture: the web app has no
//! offline stand-in, and development runs the server's replay over a day written here
//! (`SIM_AUTOFIXTURE=1`). Differences from the old browser copy, because this feeds the real
//! engine rather than imitating it:
//!
//! - restricted prints carry real SIP condition codes, and the engine decides how they count:
//!   odd lots get `I`, average-price prints `W`, the official open/close `Q`/`M`, extended-hours
//!   prints `T`;
//! - a "late" print is a real late arrival: its `event_ts` is 20 s before the print it follows,
//!   it carries `receive_ts`, and it sits at its arrival position in the stream, so the engine's
//!   settle-window rule excludes it;
//! - extras the browser fixture did not model, drawn from a **separate** generator so the main
//!   walk is unchanged: quotes, prints that arrive out of order inside the settle window, a trading
//!   halt/resume around the 10:05–10:06:30 silence, one correction (first regular print at or after
//!   11:00, published 30 s later) and one cancel (12:00, published 20 s later);
//! - prices below $1 use $0.0001 ticks.
//!
//! Symbol `i` (0-based) uses seed `seed + 101·i` and multiplies the price path by
//! `[1.0, 0.27, 6.2, 13.0][i % 4]` (so a default 4-symbol day spans ~$0.82 to ~$45 and exercises
//! the $0.10–$25 universe filter).

use crate::archive::Archive;
use crate::error::ArchiveError;
use crate::manifest::DayManifest;
use crate::writer::FsyncPolicy;
use chrono::NaiveDate;
use market_events::conditions::Tape;
use market_events::session::{SessionKind, et, sessions_for};
use market_events::{
    Cancel, CancelAction, Correction, MarketEvent, Price, Quote, Source, Status, StatusKind, Trade,
    UnixNanos,
};

pub const DEFAULT_SEED: u32 = 7;
pub const DEFAULT_SYMBOLS: [&str; 4] = ["SMPL", "FXLO", "FXMD", "FXHI"];
const SCALES: [f64; 4] = [1.0, 0.27, 6.2, 13.0];
const EXCHANGES: [&str; 7] = ["V", "P", "K", "Q", "Z", "N", "D"];

/// mulberry32, bit-identical to the TypeScript fixture's `rng`.
#[derive(Clone, Debug)]
pub struct Mulberry32(u32);

impl Mulberry32 {
    pub fn new(seed: u32) -> Self {
        Mulberry32(seed)
    }

    /// Uniform in `[0, 1)`.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6d2b_79f5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        f64::from(t ^ (t >> 14)) / 4_294_967_296.0
    }
}

struct Day {
    start: i64,
    end: i64,
    bands: Vec<(SessionKind, i64, i64)>,
    date: NaiveDate,
}

impl Day {
    fn new(date: NaiveDate) -> Result<Self, String> {
        let s = sessions_for(date);
        if s.is_empty() {
            return Err(format!("{date} is not a trading day"));
        }
        let bands: Vec<_> = s
            .iter()
            .map(|(id, a, b)| (id.kind, a.millis(), b.millis()))
            .collect();
        Ok(Day {
            start: bands[0].1,
            end: bands[bands.len() - 1].2,
            bands,
            date,
        })
    }

    fn et(&self, h: u32, m: u32, s: u32) -> i64 {
        et(self.date, h, m, s, 0).expect("valid ET time").millis()
    }

    fn kind(&self, t: i64) -> SessionKind {
        self.bands
            .iter()
            .find(|(_, a, b)| t >= *a && t < *b)
            .map_or(self.bands[self.bands.len() - 1].0, |b| b.0)
    }

    fn regular_end(&self) -> i64 {
        self.bands
            .iter()
            .find(|b| b.0 == SessionKind::Regular)
            .map_or(self.et(16, 0, 0), |b| b.2)
    }
}

fn target(day: &Day, t: i64, scale: f64) -> f64 {
    let pts = [
        (day.start, 3.02),
        (day.et(4, 0, 0), 3.06),
        (day.et(9, 29, 0), 3.16),
        (day.et(10, 15, 0), 3.21),
        (day.et(10, 34, 0), 3.47),
        (day.et(11, 30, 0), 3.39),
        (day.et(16, 0, 0), 3.36),
        (day.end, 3.33),
    ];
    for i in 1..pts.len() {
        let (t1, p1) = pts[i];
        let (t0, p0) = pts[i - 1];
        if t <= t1 {
            return scale * (p0 + ((p1 - p0) * (t - t0) as f64) / (t1 - t0) as f64);
        }
    }
    scale * pts[pts.len() - 1].1
}

/// Rounds a dollar float to the tick ($0.01, or $0.0001 below $1) as an exact price.
fn tick_price(p: f64) -> Price {
    let tick_micros: i64 = if p < 1.0 { 100 } else { 10_000 };
    let ticks = (p * 1_000_000.0 / tick_micros as f64).round() as i64;
    Price::from_micros(ticks.max(1) * tick_micros)
}

fn ns(ms: i64) -> UnixNanos {
    UnixNanos::from_millis(ms)
}

/// One symbol's fixture events, in arrival order.
pub fn fixture_symbol(
    date: NaiveDate,
    symbol: &str,
    seed: u32,
    scale: f64,
) -> Result<Vec<MarketEvent>, String> {
    let day = Day::new(date)?;
    let mut r = Mulberry32::new(seed);
    let mut x = Mulberry32::new(seed ^ 0x9e37_79b9);
    let mean_gap = |k: SessionKind| match k {
        SessionKind::Overnight => 55.0,
        SessionKind::Pre => 7.0,
        SessionKind::Regular => 1.1,
        SessionKind::After => 9.0,
    };
    let silence = (day.et(10, 5, 0), day.et(10, 6, 30));
    let burst_range = (day.et(10, 18, 0), day.et(10, 36, 0));
    let mut out: Vec<MarketEvent> = Vec::new();
    let mut t = day.start + 1_000;
    let mut p = 3.02 * scale;
    let mut seq: u64 = 0;
    let mut last_ohlc_15s: i64 = -1;
    let mut officials: Vec<(i64, &str)> = vec![(day.et(9, 30, 0), "Q"), (day.regular_end(), "M")];
    let mut halted = false;

    let trade = |t: i64, price: Price, size: u64, seq: u64, exch: &str, conds: &[&str]| {
        Trade::new(symbol, ns(t), price, size, seq)
            .with_exchange(exch)
            .with_tape(Tape::C)
            .with_source(Source::Fixture)
            .with_conditions(conds.iter().copied())
    };
    let pick = |r: &mut Mulberry32| EXCHANGES[(r.next() * EXCHANGES.len() as f64) as usize];

    while t < day.end {
        let kind = day.kind(t);
        let burst = t >= burst_range.0 && t < burst_range.1;
        let gap =
            (-(1.0 - r.next()).ln() * mean_gap(kind) * if burst { 0.35 } else { 1.0 } * 1000.0)
                .round() as i64;
        t += gap.max(1);
        if t >= silence.0 && t < silence.1 {
            if !halted {
                halted = true;
                out.push(status(
                    symbol,
                    silence.0,
                    StatusKind::Halt,
                    "H",
                    "Trading Halt",
                ));
                out.push(status(
                    symbol,
                    silence.1,
                    StatusKind::Resume,
                    "T",
                    "Trading Resumption",
                ));
            }
            t = silence.1 + (r.next() * 800.0).round() as i64;
        }
        if t >= day.end {
            break;
        }
        while let Some(&(ot, code)) = officials.first()
            && ot <= t
        {
            officials.remove(0);
            let size = 100 * (50 + (r.next() * 300.0) as u64);
            seq += 1;
            let ex = pick(&mut r);
            out.push(trade(ot, tick_price(p), size, seq, ex, &[code]).into());
        }
        let vol = if kind == SessionKind::Regular {
            0.004
        } else {
            0.0025
        };
        p += (target(&day, t, scale) - p) * 0.03
            + (r.next() - 0.5) * vol * p * if burst { 1.8 } else { 1.0 };
        let price = tick_price(p);
        let base: &[&'static str] = if kind == SessionKind::Regular {
            &["@"]
        } else {
            &["@", "T"]
        };
        let with = |extra: &'static str| -> Vec<&'static str> {
            let mut v = base.to_vec();
            v.push(extra);
            v
        };
        let u = r.next();
        let after_ohlc = t.div_euclid(15_000) == last_ohlc_15s;
        if u < 0.12 && after_ohlc {
            let size = 1 + (r.next() * 99.0) as u64;
            seq += 1;
            let ex = pick(&mut r);
            out.push(trade(t, price, size, seq, ex, &with("I")).into());
        } else if u < 0.13 && after_ohlc {
            let wp = tick_price(p + (r.next() - 0.5) * 0.06 * scale);
            let size = 100 * (5 + (r.next() * 20.0) as u64);
            seq += 1;
            let ex = pick(&mut r);
            out.push(trade(t, wp, size, seq, ex, &["W"]).into());
        } else if u < 0.145 {
            // A late arrival: executed 20 s earlier, delivered now (after the print at `t`'s
            // predecessor), so the settle window has passed.
            let lp = tick_price(p - (0.02 + r.next() * 0.04) * scale);
            let size = 100 * (1 + (r.next() * 9.0) as u64);
            seq += 1;
            let ex = pick(&mut r);
            let exec = (t - 20_000).max(day.start);
            out.push(
                trade(exec, lp, size, seq, ex, base)
                    .with_receive_ts(ns(t))
                    .into(),
            );
        } else {
            let lots = (r.next().powi(3) * if burst { 60.0 } else { 25.0 }).ceil() as u64;
            seq += 1;
            let ex = pick(&mut r);
            let tr = trade(t, price, lots.max(1) * 100, seq, ex, base);
            out.push(tr.clone().into());
            last_ohlc_15s = t.div_euclid(15_000);
            if r.next() < 0.006 {
                out.push(tr.clone().into()); // the feed repeats a print: same identity
            }
            // Extras from the separate generator (do not disturb the walk above).
            let q = x.next();
            if q < 0.35 {
                let tick = if p < 1.0 { 100 } else { 10_000 };
                out.push(MarketEvent::Quote(Quote {
                    symbol: symbol.to_string(),
                    event_ts: ns(t),
                    receive_ts: None,
                    bid_price: Price::from_micros(tr.price.micros() - tick),
                    bid_size: 1 + (x.next() * 20.0) as u64,
                    bid_exchange: pick(&mut x).to_string(),
                    ask_price: Price::from_micros(tr.price.micros() + tick),
                    ask_size: 1 + (x.next() * 20.0) as u64,
                    ask_exchange: pick(&mut x).to_string(),
                    conditions: vec!["R".into()],
                    tape: Tape::C,
                    source: Source::Fixture,
                }));
            } else if q < 0.37 && t - 300 > day.start {
                // Delivered after the print at `t` but executed 300 ms before it: inside the
                // settle window, so the engine applies it in key order.
                seq += 1;
                let ex = pick(&mut x);
                out.push(
                    trade(t - 300, price, 100, seq, ex, base)
                        .with_receive_ts(ns(t + 1))
                        .into(),
                );
            }
        }
    }
    add_correction_and_cancel(&mut out, &day, symbol, &mut seq);
    Ok(out)
}

fn status(symbol: &str, t: i64, kind: StatusKind, code: &str, _msg: &str) -> MarketEvent {
    MarketEvent::Status(Status {
        symbol: Some(symbol.to_string()),
        event_ts: ns(t),
        receive_ts: None,
        source: Source::Fixture,
        kind,
        code: code.to_string(),
        reason: "LUDP".to_string(),
        tape: Tape::C,
    })
}

/// Inserts a correction (11:00 print, +30 s) and a cancel (12:00 print, +20 s) at their
/// arrival positions.
fn add_correction_and_cancel(out: &mut Vec<MarketEvent>, day: &Day, symbol: &str, seq: &mut u64) {
    let regular_print = |out: &[MarketEvent], from: i64| {
        out.iter().find_map(|e| match e {
            MarketEvent::Trade(t)
                if t.event_ts.millis() >= from
                    && t.receive_ts.is_none()
                    && t.conditions == ["@"] =>
            {
                Some(t.clone())
            }
            _ => None,
        })
    };
    let insert_at = |out: &mut Vec<MarketEvent>, at: i64, ev: MarketEvent| {
        let pos = out
            .iter()
            .position(|e| e.event_ts().millis() > at)
            .unwrap_or(out.len());
        out.insert(pos, ev);
    };
    if let Some(orig) = regular_print(out, day.et(11, 0, 0)) {
        *seq += 1;
        let at = orig.event_ts.millis() + 30_000;
        let cp = Price::from_micros(orig.price.micros() + 50_000);
        insert_at(
            out,
            at,
            MarketEvent::Correction(Correction {
                symbol: symbol.to_string(),
                event_ts: ns(at),
                receive_ts: None,
                source: Source::Fixture,
                exchange: orig.exchange.clone(),
                tape: Tape::C,
                original_id: orig.source_seq,
                original_price: orig.price,
                original_size: orig.size,
                original_conditions: orig.conditions.clone(),
                corrected_id: *seq,
                corrected_price: cp,
                corrected_size: orig.size,
                corrected_conditions: orig.conditions.clone(),
            }),
        );
    }
    if let Some(orig) = regular_print(out, day.et(12, 0, 0)) {
        let at = orig.event_ts.millis() + 20_000;
        insert_at(
            out,
            at,
            MarketEvent::Cancel(Cancel {
                symbol: symbol.to_string(),
                event_ts: ns(at),
                receive_ts: None,
                source: Source::Fixture,
                exchange: orig.exchange.clone(),
                tape: Tape::C,
                trade_id: orig.source_seq,
                price: orig.price,
                size: orig.size,
                action: CancelAction::Cancel,
            }),
        );
    }
}

/// Seed and price scale of a fixture symbol. A default symbol keeps its own slot whatever its
/// position in the list, so FXLO is always the sub-dollar one and FXHI always trades above $25
/// (it exists to exercise the price band); other symbols use their position.
pub fn symbol_params(base_seed: u32, i: usize, symbol: &str) -> (u32, f64) {
    let slot = DEFAULT_SYMBOLS
        .iter()
        .position(|d| *d == symbol)
        .unwrap_or(i);
    (
        base_seed.wrapping_add(101 * slot as u32),
        SCALES[slot % SCALES.len()] * (1.0 + (slot / SCALES.len()) as f64 * 0.1),
    )
}

/// Writes a full fixture day for `symbols` and returns its manifest.
pub fn write_fixture_day(
    archive: &Archive,
    date: NaiveDate,
    symbols: &[String],
    base_seed: u32,
) -> Result<DayManifest, ArchiveError> {
    let mut manifest = None;
    for (i, sym) in symbols.iter().enumerate() {
        let (seed, scale) = symbol_params(base_seed, i, sym);
        let events = fixture_symbol(date, sym, seed, scale).map_err(|m| ArchiveError::Format {
            path: archive.day_dir(date),
            message: m,
        })?;
        let mut w = archive.writer(date, sym, FsyncPolicy::OnFinalize)?;
        for e in &events {
            w.append(e)?;
        }
        manifest = Some(archive.commit(date, w.finalize()?)?);
    }
    manifest.ok_or_else(|| ArchiveError::InvalidSymbol("(no symbols)".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mulberry32_matches_the_typescript_generator() {
        // rng(7) in the former apps/web/src/fixtures/fixtureFeed.ts, first three draws (computed
        // with Bun). Kept as constants: the generator must not drift now that it is the only one.
        let mut r = Mulberry32::new(7);
        assert_eq!(r.next(), 0.011704753153026104);
        assert_eq!(r.next(), 0.06195825757458806);
        assert_eq!(r.next(), 0.97690763277933);
    }

    #[test]
    fn first_prints_match_the_typescript_fixture() {
        // generateTrades(7) starts: 3.02×900 @Q, 3.01×1000 @P, 3.02×200 @K (all ["@","T"]).
        let ev = fixture_symbol(
            NaiveDate::from_ymd_opt(2026, 10, 15).unwrap(),
            "SMPL",
            7,
            1.0,
        )
        .unwrap();
        let trades: Vec<_> = ev
            .iter()
            .filter_map(|e| match e {
                MarketEvent::Trade(t) => Some(t),
                _ => None,
            })
            .take(3)
            .collect();
        let got: Vec<_> = trades
            .iter()
            .map(|t| (t.event_ts.millis(), t.price, t.size, t.exchange.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                (1792022401648, Price::from_cents(302), 900, "Q"),
                (1792022436177, Price::from_cents(301), 1000, "P"),
                (1792022515614, Price::from_cents(302), 200, "K"),
            ]
        );
        assert_eq!(trades[0].conditions, ["@", "T"]);
    }

    #[test]
    fn tick_rounding() {
        assert_eq!(tick_price(3.184_9), Price::from_cents(318));
        assert_eq!(tick_price(0.936_76), Price::from_micros(936_800));
    }
}
