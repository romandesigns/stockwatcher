//! The Halt watcher bench (`halt_bench.rs`) per symbol and in parallel, for one day: each symbol
//! gets its own board, so the six-card cap and the swap margin between symbols are not modelled
//! (they only matter when more than six build-ups overlap). Also accepts a day still being
//! recorded (it reads the live files as they are). Ignored; run by hand:
//!
//! ```text
//! HALT_MOVERS_ARCHIVE=C:\research\stockwatcher\archive-vps HALT_MOVERS_DAY=2026-10-22 \
//! HALT_MOVERS_OUT=out.jsonl cargo test -p stockwatcher-server --release --test halt_movers \
//!   -- --ignored --nocapture
//! ```
//!
//! One JSON line per card episode (`card`) and per pause (`pause`, from halt statuses, or
//! inferred like the bench: a regular-session silence of 270 s+ after a print near the band).

use std::io::Write as _;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use chart_engine::ChartEngine;
use chart_engine::wire::ChartUpdate;
use market_archive::{Archive, ArchiveConfig};
use market_events::session::et;
use market_events::universe::PriceBand;
use market_events::{MarketEvent, Price, SessionKind, UnixNanos};
use market_replay::Merger;
use market_replay::source::EventStream;
use stockwatcher_server::pressure::{Band, PressureBoard};
use stockwatcher_server::quotes::Counted;

const SEC: i64 = 1_000_000_000;
const PAUSE_GAP: i64 = 270 * SEC;
const PAUSE_NEAR: f64 = 0.25;

#[test]
#[ignore]
fn halt_movers() {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let root = env("HALT_MOVERS_ARCHIVE").expect("HALT_MOVERS_ARCHIVE");
    let day: chrono::NaiveDate = env("HALT_MOVERS_DAY").unwrap().parse().unwrap();
    let out = env("HALT_MOVERS_OUT").unwrap();
    let threads: usize = env("HALT_MOVERS_THREADS").map_or(3, |v| v.parse().unwrap());
    let archive = Archive::new(ArchiveConfig {
        root: root.into(),
        retention_days: 0,
        min_free_bytes: 0,
    });
    let m = archive
        .live_manifest(day)
        .ok()
        .flatten()
        .or_else(|| archive.manifest(day).ok().flatten())
        .unwrap();
    let live = archive.live_manifest(day).ok().flatten().is_some() && !archive.is_final(day);
    let syms: Vec<String> = match env("HALT_MOVERS_SYMBOLS") {
        Some(v) => v.split(',').map(str::to_string).collect(),
        None => m.symbols.keys().cloned().collect(),
    };
    let band = PriceBand::DEFAULT;
    let open = et(day, 9, 30, 0, 0).unwrap().0;
    let close = et(day, 16, 0, 0, 0).unwrap().0;
    let next = AtomicUsize::new(0);
    let lines = Mutex::new(Vec::<String>::new());
    let started = std::time::Instant::now();
    std::thread::scope(|sc| {
        for _ in 0..threads {
            sc.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(sym) = syms.get(i) else { break };
                    if i.is_multiple_of(1000) {
                        eprintln!("  {i} symbols, {:.0} s", started.elapsed().as_secs_f64());
                    }
                    let r = if live {
                        archive.live_reader(day, sym, None)
                    } else {
                        archive.reader(day, sym)
                    };
                    let Ok(r) = r else { continue };
                    let stream: EventStream =
                        Box::new(r.map(|x| x.map(|v| v.event).map_err(|e| e.to_string())));
                    let mut merger = Merger::new(vec![stream]);
                    let mut engine = ChartEngine::default();
                    let mut board = PressureBoard::new();
                    let mut last_price: Option<Price> = None;
                    let mut last: Option<(i64, Price, SessionKind)> = None;
                    let mut sband: Option<Band> = None;
                    let mut mark = i64::MIN;
                    let mut ticked = open - SEC;
                    // (start, end, risk at entry, peak, price at entry)
                    let mut eps: Vec<(i64, Option<i64>, u8, u8, f64)> = Vec::new();
                    let mut on = false;
                    let mut mine = Vec::new();
                    let rank = |now: i64,
                                    board: &mut PressureBoard,
                                    lp: Option<Price>,
                                    eps: &mut Vec<(i64, Option<i64>, u8, u8, f64)>,
                                    on: &mut bool| {
                        let cards = board.tick(now, |_| lp.is_some_and(|p| band.contains(p)));
                        if let Some(c) = cards.first() {
                            if *on {
                                let e = eps.last_mut().unwrap();
                                e.3 = e.3.max(c.risk.risk);
                            } else {
                                *on = true;
                                eps.push((
                                    now,
                                    None,
                                    c.risk.risk,
                                    c.risk.risk,
                                    lp.map_or(f64::NAN, |p| p.micros() as f64 / 1e6),
                                ));
                            }
                        } else if *on {
                            *on = false;
                            eps.last_mut().unwrap().1 = Some(now);
                        }
                    };
                    while let Some((d, e)) = merger.pop() {
                        let mm = d.div_euclid(SEC) * SEC;
                        if mm > mark {
                            while ticked + SEC <= mm.min(close) {
                                ticked += SEC;
                                if ticked >= open {
                                    rank(ticked, &mut board, last_price, &mut eps, &mut on);
                                }
                            }
                            mark = mm;
                            for u in engine.advance_clock(UnixNanos(mm)) {
                                if let ChartUpdate::Luld(l) = &u {
                                    board.apply_luld(&l.symbol, &l.point);
                                    sband = Band::from_wire(&l.point);
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
                                last_price = Some(t.price);
                                let kind = t.session.map(|s| s.kind);
                                if let (
                                    Some((lt, lp, SessionKind::Regular)),
                                    Some(SessionKind::Regular),
                                ) = (last, kind)
                                    && t.event_ts.0 - lt >= PAUSE_GAP
                                    && let Some(b) = sband
                                {
                                    let p = lp.micros() as f64;
                                    let r = b.reference.micros() as f64;
                                    let (up, down) = (b.up.micros() as f64, b.down.micros() as f64);
                                    let hu = (up - r) / r;
                                    let hd = (r - down) / r;
                                    let near_up = (up - p) / p <= PAUSE_NEAR * hu;
                                    let near_down = (p - down) / p <= PAUSE_NEAR * hd;
                                    if near_up || near_down {
                                        mine.push(
                                            serde_json::json!({"kind": "pause", "s": sym,
                                                "at": lt, "src": "inferred", "up": near_up,
                                                "price": p / 1e6, "resume": t.event_ts.0})
                                            .to_string(),
                                        );
                                    }
                                }
                                if let Some(k) = kind {
                                    last = Some((t.event_ts.0, t.price, k));
                                }
                            }
                        }
                        for u in &ups {
                            match u {
                                ChartUpdate::Halt(h) => {
                                    board.apply_halt(&h.symbol, &h.halt);
                                    if h.halt.end.is_none() {
                                        mine.push(
                                            serde_json::json!({"kind": "pause", "s": sym,
                                                "at": h.halt.start * 1_000_000, "src": "status",
                                                "price": last_price.map(|p| p.micros() as f64 / 1e6),
                                                "halt": serde_json::to_value(&h.halt).ok()})
                                            .to_string(),
                                        );
                                    } else {
                                        mine.push(
                                            serde_json::json!({"kind": "resume", "s": sym,
                                                "halt": serde_json::to_value(&h.halt).ok()})
                                            .to_string(),
                                        );
                                    }
                                }
                                ChartUpdate::Luld(l) => {
                                    board.apply_luld(&l.symbol, &l.point);
                                    sband = Band::from_wire(&l.point);
                                }
                                _ => {}
                            }
                        }
                    }
                    while ticked + SEC <= close {
                        ticked += SEC;
                        if ticked >= open {
                            rank(ticked, &mut board, last_price, &mut eps, &mut on);
                        }
                    }
                    if on {
                        eps.last_mut().unwrap().1 = Some(close);
                    }
                    for e in eps {
                        mine.push(
                            serde_json::json!({"kind": "card", "s": sym, "start": e.0,
                                "end": e.1, "risk": e.2, "peak": e.3, "price": e.4})
                            .to_string(),
                        );
                    }
                    lines.lock().unwrap().extend(mine);
                }
            });
        }
    });
    let lines = lines.into_inner().unwrap();
    let mut f = std::fs::File::create(&out).unwrap();
    for l in &lines {
        writeln!(f, "{l}").unwrap();
    }
    eprintln!(
        "{day}: {} symbols, {} lines, {:.0} s",
        syms.len(),
        lines.len(),
        started.elapsed().as_secs_f64()
    );
}
