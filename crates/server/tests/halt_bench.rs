//! The Halt watcher's calibration bench (crates/server/src/pressure.rs, "Calibration"): replays
//! recorded days through the chart engine and the board exactly as the live ingest feeds them,
//! ranks once per market-clock second through the regular session, and reports how many cards the
//! board showed and, for every LULD pause in the data, how long before it the symbol entered the
//! board (lead time), plus the entries no pause followed (false alarms).
//!
//! Ignored (it needs recorded data, which the repo does not carry). Run it on any archive root:
//!
//! ```text
//! HALT_BENCH_ARCHIVE=/app/data/archive cargo test -p stockwatcher-server --release \
//!     --test halt_bench -- --ignored --nocapture
//! ```
//!
//! Optional: `HALT_BENCH_DAYS=2026-10-15,2026-10-16` (default: every day in the root).
//!
//! **Halts.** A live recording carries the provider's halt statuses and the engine turns them into
//! `chart.halt`; those are used as they are. A historical import is trades only (the importer
//! takes `/v2/stocks/{symbol}/trades`), so its pauses are **inferred**: a regular-session silence
//! of at least [`PAUSE_GAP`] whose last print before it sat within [`PAUSE_NEAR`] of the band's
//! half-width from the modelled band. That is what a LULD pause looks like on the tape (the price
//! held at the band for 15 s, then five minutes without prints); a thin stock's ordinary silence
//! happens away from its band. It is an inference from the modelled band, flagged as such in the
//! report.

use std::collections::{BTreeMap, HashMap};

use chart_engine::ChartEngine;
use chart_engine::wire::ChartUpdate;
use market_archive::{Archive, ArchiveConfig};
use market_events::session::et;
use market_events::universe::PriceBand;
use market_events::{MarketEvent, Price, SessionKind, UnixNanos};
use market_replay::{ArchiveDaySource, EventSource as _, Merger};
use stockwatcher_server::pressure::{Band, ENTER_CLOSE, ENTER_RISE, PIN, PressureBoard};
use stockwatcher_server::quotes::Counted;

const SEC: i64 = 1_000_000_000;
/// The silence that counts as an inferred pause: a LULD pause is 5 minutes; a little less allows
/// for the reopening auction printing early.
const PAUSE_GAP: i64 = 270 * SEC;
/// How close to the modelled band (share of its half-width) the last print before the silence
/// must be.
const PAUSE_NEAR: f64 = 0.25;
/// An entry counts as followed by a pause when the pause starts while the card is up or within
/// this long after it left.
const FOLLOW: i64 = 120 * SEC;

#[derive(Debug, Clone)]
struct Episode {
    symbol: String,
    start: i64,
    end: Option<i64>,
    risk_at_entry: u8,
    rise_at_entry: i16,
    to_band_at_entry: f64,
    peak: u8,
}

#[derive(Debug, Clone)]
struct Pause {
    symbol: String,
    /// The halt (status) or the last print before the silence (inferred).
    at: i64,
    inferred: bool,
    side_up: Option<bool>,
}

#[derive(Default)]
struct Sym {
    last: Option<(i64, Price, SessionKind)>,
    band: Option<Band>,
}

fn pct(v: &mut [f64], q: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * q).round() as usize]
}

fn hms(ns: i64) -> String {
    let t = chrono::DateTime::from_timestamp_nanos(ns).with_timezone(&chrono_tz::America::New_York);
    t.format("%H:%M:%S").to_string()
}

#[test]
#[ignore]
fn halt_watcher_bench() {
    let Ok(root) = std::env::var("HALT_BENCH_ARCHIVE") else {
        eprintln!("HALT_BENCH_ARCHIVE not set: nothing to replay");
        return;
    };
    let archive = Archive::new(ArchiveConfig {
        root: root.clone().into(),
        retention_days: 0,
        min_free_bytes: 0,
    });
    let days: Vec<chrono::NaiveDate> = match std::env::var("HALT_BENCH_DAYS") {
        Ok(s) => s.split(',').map(|d| d.trim().parse().unwrap()).collect(),
        Err(_) => archive.days().unwrap(),
    };
    let band = PriceBand::DEFAULT;
    let mut all_eps: Vec<Episode> = Vec::new();
    let mut all_pauses: Vec<Pause> = Vec::new();
    let mut card_secs = 0u64;
    let mut session_secs = 0u64;
    let mut concurrent: BTreeMap<usize, u64> = BTreeMap::new();
    let mut card_risks: Vec<f64> = Vec::new();
    let mut symbols_seen = 0usize;
    let mut all_approach: Vec<(f64, bool)> = Vec::new();
    for day in &days {
        let src = ArchiveDaySource::new(archive.clone(), *day).unwrap();
        symbols_seen += src.symbols().len();
        let fixture = src.info().fixture;
        let mut merger = Merger::new(src.open().unwrap());
        let mut engine = ChartEngine::default();
        let mut board = PressureBoard::new();
        let mut last_price: HashMap<String, Price> = HashMap::new();
        let mut syms: HashMap<String, Sym> = HashMap::new();
        let open = et(*day, 9, 30, 0, 0).unwrap().0;
        let close = et(*day, 16, 0, 0, 0).unwrap().0;
        let mut mark = i64::MIN;
        let mut ticked = open - SEC;
        let mut eps: Vec<Episode> = Vec::new();
        let mut live: HashMap<String, usize> = HashMap::new();
        let mut pauses: Vec<Pause> = Vec::new();
        let mut approach: HashMap<String, (f64, bool)> = HashMap::new();

        let mut rank = |now: i64,
                        board: &mut PressureBoard,
                        last_price: &HashMap<String, Price>,
                        eps: &mut Vec<Episode>,
                        live: &mut HashMap<String, usize>| {
            let cards = board.tick(now, |s| {
                last_price.get(s).is_some_and(|p| band.contains(*p))
            });
            session_secs += 1;
            *concurrent.entry(cards.len()).or_default() += 1;
            if !cards.is_empty() {
                card_secs += 1;
            }
            // Every 10 s, how close each in-band symbol came to a band (share of the half-width
            // covered) and whether it ever had an entry-size rise while past half-way: what the
            // entry gates leave out.
            if (now - open) % (10 * SEC) == 0 {
                for (s, p) in last_price {
                    if !band.contains(*p) {
                        continue;
                    }
                    if let Some(r) = board.risk_of(s, now) {
                        let a = approach.entry(s.clone()).or_insert((0.0, false));
                        let covered = 1.0 - r.to_band / r.half_width;
                        a.0 = f64::max(a.0, covered);
                        a.1 |= covered >= ENTER_CLOSE && r.rise >= ENTER_RISE;
                    }
                }
            }
            let on: Vec<String> = cards.iter().map(|c| c.symbol.to_string()).collect();
            for c in &cards {
                card_risks.push(c.risk.risk as f64);
                let s = c.symbol.to_string();
                match live.get(&s) {
                    Some(&k) => eps[k].peak = eps[k].peak.max(c.risk.risk),
                    None => {
                        live.insert(s.clone(), eps.len());
                        eps.push(Episode {
                            symbol: s,
                            start: now,
                            end: None,
                            risk_at_entry: c.risk.risk,
                            rise_at_entry: c.risk.rise,
                            to_band_at_entry: c.risk.to_band,
                            peak: c.risk.risk,
                        });
                    }
                }
            }
            live.retain(|s, k| {
                let stay = on.contains(s);
                if !stay {
                    eps[*k].end = Some(now);
                }
                stay
            });
        };

        while let Some((d, e)) = merger.pop() {
            let m = d.div_euclid(SEC) * SEC;
            if m > mark {
                // Rank every whole second the clock passed inside the regular session, before
                // the events of the new second (the live quotes task ranks between events too).
                while ticked + SEC <= m.min(close) {
                    ticked += SEC;
                    if ticked >= open {
                        rank(ticked, &mut board, &last_price, &mut eps, &mut live);
                    }
                }
                mark = m;
                for u in engine.advance_clock(UnixNanos(m)) {
                    if let ChartUpdate::Luld(l) = &u {
                        board.apply_luld(&l.symbol, &l.point);
                        syms.entry(l.symbol.clone()).or_default().band = Band::from_wire(&l.point);
                    }
                }
            }
            let (ups, _) = engine.apply_revising(&e);
            if let MarketEvent::Trade(t) = &e
                && let Some(ChartUpdate::Tape(row)) = ups.first()
            {
                let counted = Counted::from_tape(&row.counted);
                board.apply_trade(t, counted);
                if counted.ohlc {
                    last_price.insert(t.symbol.clone(), t.price);
                    let st = syms.entry(t.symbol.clone()).or_default();
                    let kind = t.session.map(|s| s.kind);
                    if let (Some((lt, lp, SessionKind::Regular)), Some(SessionKind::Regular)) =
                        (st.last, kind)
                        && t.event_ts.0 - lt >= PAUSE_GAP
                        && let Some(b) = st.band
                    {
                        let p = lp.micros() as f64;
                        let r = b.reference.micros() as f64;
                        let (up, down) = (b.up.micros() as f64, b.down.micros() as f64);
                        let hu = (up - r) / r;
                        let hd = (r - down) / r;
                        let near_up = (up - p) / p <= PAUSE_NEAR * hu;
                        let near_down = (p - down) / p <= PAUSE_NEAR * hd;
                        if near_up || near_down {
                            pauses.push(Pause {
                                symbol: t.symbol.clone(),
                                at: lt,
                                inferred: true,
                                side_up: Some(near_up),
                            });
                        }
                    }
                    if let Some(k) = kind {
                        st.last = Some((t.event_ts.0, t.price, k));
                    }
                }
            }
            for u in &ups {
                match u {
                    ChartUpdate::Halt(h) => {
                        board.apply_halt(&h.symbol, &h.halt);
                        if h.halt.end.is_none() {
                            pauses.push(Pause {
                                symbol: h.symbol.clone(),
                                at: h.halt.start * 1_000_000,
                                inferred: false,
                                side_up: None,
                            });
                        }
                    }
                    ChartUpdate::Luld(l) => {
                        board.apply_luld(&l.symbol, &l.point);
                        syms.entry(l.symbol.clone()).or_default().band = Band::from_wire(&l.point);
                    }
                    _ => {}
                }
            }
        }
        while ticked + SEC <= close {
            ticked += SEC;
            if ticked >= open {
                rank(ticked, &mut board, &last_price, &mut eps, &mut live);
            }
        }
        for k in live.values() {
            eps[*k].end = Some(close);
        }
        for (s, (covered, rose)) in &approach {
            if *covered >= ENTER_CLOSE {
                println!(
                    "  {s:6} came {:.0}% of the way to a band{}",
                    covered * 100.0,
                    if *rose {
                        ", with an entry-size rise there"
                    } else {
                        ""
                    }
                );
            }
        }
        all_approach.extend(approach.into_values());
        println!(
            "\n== {day}{} : {} episodes, {} pauses ({} inferred)",
            if fixture {
                " (FIXTURE: synthetic, not market data)"
            } else {
                ""
            },
            eps.len(),
            pauses.len(),
            pauses.iter().filter(|p| p.inferred).count()
        );
        for ep in &eps {
            println!(
                "  card {:6} {} → {} entry risk {} rise {:+} {:.2}% to band, peak {}",
                ep.symbol,
                hms(ep.start),
                ep.end.map(hms).unwrap_or_default(),
                ep.risk_at_entry,
                ep.rise_at_entry,
                ep.to_band_at_entry * 100.0,
                ep.peak
            );
        }
        for p in &pauses {
            println!(
                "  pause {:6} {} {}{}",
                p.symbol,
                hms(p.at),
                if p.inferred { "inferred" } else { "status" },
                match p.side_up {
                    Some(true) => " (at the upper band)",
                    Some(false) => " (at the lower band)",
                    None => "",
                }
            );
        }
        all_eps.extend(eps);
        all_pauses.extend(pauses);
    }

    // Lead times: for each pause, the episode of that symbol up at the pause or ended within
    // FOLLOW before it, the earliest such; its start to the pause.
    let mut leads = Vec::new();
    let mut missed = 0;
    for p in &all_pauses {
        let ep = all_eps
            .iter()
            .filter(|e| {
                e.symbol == p.symbol && e.start <= p.at && e.end.is_none_or(|x| x + FOLLOW >= p.at)
            })
            .min_by_key(|e| e.start);
        match ep {
            Some(e) => leads.push((p.at - e.start) as f64 / SEC as f64),
            None => missed += 1,
        }
    }
    let followed = |e: &Episode| {
        all_pauses.iter().any(|p| {
            p.symbol == e.symbol && p.at >= e.start && e.end.is_none_or(|x| p.at <= x + FOLLOW)
        })
    };
    let false_alarms = all_eps.iter().filter(|e| !followed(e)).count();
    let mut durs: Vec<f64> = all_eps
        .iter()
        .filter_map(|e| e.end.map(|x| (x - e.start) as f64 / SEC as f64))
        .collect();
    let mut entry: Vec<f64> = all_eps.iter().map(|e| e.risk_at_entry as f64).collect();
    let mut peaks: Vec<f64> = all_eps.iter().map(|e| e.peak as f64).collect();
    println!(
        "\n== summary ({root}, {} days, {symbols_seen} symbol-days)",
        days.len()
    );
    println!(
        "regular-session seconds ranked: {session_secs}; with at least one card: {card_secs} ({:.2}%)",
        100.0 * card_secs as f64 / session_secs.max(1) as f64
    );
    println!("cards shown per second (count: seconds): {concurrent:?}");
    let mut covered: Vec<f64> = all_approach.iter().map(|a| a.0).collect();
    println!(
        "symbol-days with a band and a score: {}; closest approach (share of the half-width covered) p50/90/99/max {:.2}/{:.2}/{:.2}/{:.2}; past half-way {}, pinned ({}+) {}, entry-size rise past half-way {}",
        all_approach.len(),
        pct(&mut covered, 0.5),
        pct(&mut covered, 0.9),
        pct(&mut covered, 0.99),
        pct(&mut covered, 1.0),
        all_approach.iter().filter(|a| a.0 >= ENTER_CLOSE).count(),
        1.0 - PIN,
        all_approach.iter().filter(|a| a.0 >= 1.0 - PIN).count(),
        all_approach.iter().filter(|a| a.1).count()
    );
    println!(
        "episodes: {} ({} symbols); duration s p50 {:.0} p90 {:.0} max {:.0}",
        all_eps.len(),
        all_eps
            .iter()
            .map(|e| &e.symbol)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        pct(&mut durs, 0.5),
        pct(&mut durs, 0.9),
        pct(&mut durs, 1.0)
    );
    println!(
        "risk at entry p10/50/90 {:.0}/{:.0}/{:.0}; peak p10/50/90 {:.0}/{:.0}/{:.0}; every card-second p10/50/90 {:.0}/{:.0}/{:.0}",
        pct(&mut entry, 0.1),
        pct(&mut entry, 0.5),
        pct(&mut entry, 0.9),
        pct(&mut peaks, 0.1),
        pct(&mut peaks, 0.5),
        pct(&mut peaks, 0.9),
        pct(&mut card_risks, 0.1),
        pct(&mut card_risks, 0.5),
        pct(&mut card_risks, 0.9)
    );
    println!(
        "pauses: {} ({} inferred); caught {} missed {missed}; lead s p10/50/90 {:.0}/{:.0}/{:.0}",
        all_pauses.len(),
        all_pauses.iter().filter(|p| p.inferred).count(),
        leads.len(),
        pct(&mut leads.clone(), 0.1),
        pct(&mut leads.clone(), 0.5),
        pct(&mut leads, 0.9)
    );
    println!(
        "false alarms (entries no pause followed within {}s): {false_alarms} of {}",
        FOLLOW / SEC,
        all_eps.len()
    );
}

/// Every 10 s of the regular session, each symbol whose rise or risk stands out: the readings
/// behind the board (pressure, rise, distance to the band, move, time to band), for tuning by eye.
/// Same environment as the bench; `HALT_BENCH_DAYS` is not read (every day in the root).
#[test]
#[ignore]
fn halt_watcher_trace() {
    let root = std::env::var("HALT_BENCH_ARCHIVE").unwrap();
    let archive = Archive::new(ArchiveConfig {
        root: root.into(),
        retention_days: 0,
        min_free_bytes: 0,
    });
    for day in archive.days().unwrap() {
        let src = ArchiveDaySource::new(archive.clone(), day).unwrap();
        let mut merger = Merger::new(src.open().unwrap());
        let mut engine = ChartEngine::default();
        let mut board = PressureBoard::new();
        let mut mark = i64::MIN;
        let open = et(day, 9, 30, 0, 0).unwrap().0;
        let mut next = open + 10 * SEC;
        let mut syms: Vec<String> = src.symbols();
        syms.sort();
        while let Some((d, e)) = merger.pop() {
            let m = d.div_euclid(SEC) * SEC;
            if m > mark {
                mark = m;
                for u in engine.advance_clock(UnixNanos(m)) {
                    if let ChartUpdate::Luld(l) = &u {
                        board.apply_luld(&l.symbol, &l.point);
                    }
                }
                while next <= m && next < open + 390 * 60 * SEC {
                    for s in &syms {
                        let sc = board.score_of(s, next);
                        if let Some(r) = board.risk_of(s, next)
                            && (r.rise.abs() >= 20 || r.risk >= 30)
                        {
                            println!(
                                "{} {s} p={:?} risk {} rise {:+} to {:.1}% half {:.1}% moved {:.2}% T {:?}",
                                hms(next),
                                sc.map(|x| (x.pressure, x.bull())),
                                r.risk,
                                r.rise,
                                r.to_band * 100.0,
                                r.half_width * 100.0,
                                r.moved * 100.0,
                                r.secs_to_band.map(|x| x.round())
                            );
                        }
                    }
                    next += 10 * SEC;
                }
            }
            let (ups, _) = engine.apply_revising(&e);
            if let MarketEvent::Trade(t) = &e
                && let Some(ChartUpdate::Tape(row)) = ups.first()
            {
                board.apply_trade(t, Counted::from_tape(&row.counted));
            }
            for u in &ups {
                match u {
                    ChartUpdate::Halt(h) => board.apply_halt(&h.symbol, &h.halt),
                    ChartUpdate::Luld(l) => board.apply_luld(&l.symbol, &l.point),
                    _ => {}
                }
            }
        }
    }
}
