use std::collections::HashSet;

use market_events::{Price, Trade, UnixNanos};
use serde_json::Value;

use super::*;

const BOTH: Counted = Counted {
    ohlc: true,
    volume: true,
};

fn trade(sym: &str, exec: i64, p: f64, size: u64) -> Trade {
    Trade::new(
        sym,
        UnixNanos(exec),
        Price::from_micros((p * 1e6).round() as i64),
        size,
        1,
    )
}

/// One record of the research's flat tape (`zones_export.rs`): i64 exec, f64 price, f64 size,
/// u8 ohlc, u8 volume.
struct Rec {
    sym: &'static str,
    t: i64,
    p: f64,
    s: f64,
    o: bool,
    v: bool,
}

fn tape(sym: &'static str, bytes: &[u8]) -> Vec<Rec> {
    let mut out: Vec<Rec> = bytes
        .as_chunks::<26>()
        .0
        .iter()
        .map(|c| Rec {
            sym,
            t: i64::from_le_bytes(c[0..8].try_into().unwrap()),
            p: f64::from_le_bytes(c[8..16].try_into().unwrap()),
            s: f64::from_le_bytes(c[16..24].try_into().unwrap()),
            o: c[24] == 1,
            v: c[25] == 1,
        })
        .collect();
    // The research sorts by execution time, stable.
    out.sort_by_key(|r| r.t);
    out
}

fn dev_day() -> Vec<Rec> {
    let mut all = Vec::new();
    all.extend(tape("GENK", include_bytes!("fixtures/2026-04-01-GENK.bin")));
    all.extend(tape("ROMA", include_bytes!("fixtures/2026-04-01-ROMA.bin")));
    all.extend(tape("AMOD", include_bytes!("fixtures/2026-04-01-AMOD.bin")));
    all.sort_by_key(|r| r.t);
    all
}

fn feed(b: &mut ZoneBoard, r: &Rec) {
    // The clock reaches a print's execution time before the print is applied (live: the
    // quotes task ticks once a second at the market clock).
    b.tick(r.t);
    let t = trade(r.sym, r.t, r.p, r.s as u64);
    assert_eq!(t.price.to_f64(), r.p, "price round trip");
    b.apply_trade(
        &t,
        Counted {
            ohlc: r.o,
            volume: r.v,
        },
    );
}

fn end_of_day(b: &mut ZoneBoard) {
    let t0 = b.t0;
    b.tick(t0 + NSLOT * BAR_NS + 2 * SEC_NS);
}

fn expected() -> Value {
    serde_json::from_str(include_str!("fixtures/2026-04-01-expected.json")).unwrap()
}

fn res_of(s: State) -> &'static str {
    match s {
        State::Held => "HELD",
        State::Lost => "LOST",
        State::Expired => "NEITHER",
        State::InShelf => "OPEN",
    }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(1.0)
}

/// A research DEV day (2026-04-01, three runner symbols: $1.50, $3.20 and a sub-dollar $0.358
/// run): the runs equal the `events-g20` labels, the events equal variant C's DEV output field
/// for field, and the live outcomes equal the research's contract outcomes (HELD, HELD,
/// NEITHER, LOST).
#[test]
fn a_research_dev_day_is_reproduced() {
    let mut b = ZoneBoard::new(true);
    for r in dev_day() {
        feed(&mut b, &r);
    }
    end_of_day(&mut b);
    let exp = expected();
    for run in exp["runs"].as_array().unwrap() {
        let sym = run["symbol"].as_str().unwrap();
        let sh = b.shelf(sym).unwrap_or_else(|| panic!("{sym}: no run"));
        let got = sh.runs[0];
        assert_eq!(got.start_ns, run["start_ns"].as_i64().unwrap(), "{sym}");
        assert_eq!(got.plus20_ns, run["plus20_ns"].as_i64().unwrap(), "{sym}");
        assert!(close(got.start_price, run["start_price"].as_f64().unwrap()));
        assert_eq!(sh.runs.len(), 1, "{sym}: {:?}", sh.runs);
    }
    let mut got: Vec<(String, Event)> = Vec::new();
    for sym in ["GENK", "ROMA", "AMOD"] {
        for e in &b.shelf(sym).unwrap().events {
            got.push((sym.to_string(), *e));
        }
    }
    got.sort_by_key(|(_, e)| e.t_ns);
    let want = exp["events"].as_array().unwrap();
    assert_eq!(got.len(), want.len(), "{got:?}");
    for ((sym, e), w) in got.iter().zip(want) {
        assert_eq!(sym, w["symbol"].as_str().unwrap());
        assert_eq!(e.t_ns, w["t_ns"].as_i64().unwrap(), "{sym}");
        assert!(close(e.entry, w["entry_price"].as_f64().unwrap()), "{sym}");
        assert!(close(e.lo, w["zone_lo"].as_f64().unwrap()), "{sym} {e:?}");
        assert!(close(e.hi, w["zone_hi"].as_f64().unwrap()), "{sym} {e:?}");
        assert!(close(e.peak, w["peak_price"].as_f64().unwrap()), "{sym}");
        assert_eq!(e.run_start_ns, w["run_start_ns"].as_i64().unwrap(), "{sym}");
        assert!(close(round4(e.retr), w["score"].as_f64().unwrap()), "{sym}");
        assert_eq!(res_of(e.state), w["_res"].as_str().unwrap(), "{sym} {e:?}");
    }
}

/// Every print executed at or after a slot's end is invisible to that slot's decision, even
/// when it arrives before the slot is decided: a huge print 1 ns after each symbol's first
/// event's slot end (arriving before the settle) leaves that event exactly as it was. (Later
/// decisions may change: for them the print is in the past.)
#[test]
fn a_decision_never_sees_a_print_from_after_its_slot() {
    let day = dev_day();
    let mut base = ZoneBoard::new(true);
    for r in &day {
        feed(&mut base, r);
    }
    end_of_day(&mut base);
    let syms: [&'static str; 3] = ["GENK", "ROMA", "AMOD"];
    let mut extra: Vec<Rec> = Vec::new();
    for sym in syms {
        let e = base.shelf(sym).unwrap().events[0];
        // Far below the shelf, with a huge size: leaked into the decision it would move the
        // shelf (and so the event's edges).
        extra.push(Rec {
            sym,
            t: e.t_ns + 1,
            p: ((e.lo * 0.9) * 1e6).round() / 1e6,
            s: 5_000_000.0,
            o: false,
            v: true,
        });
    }
    let mut all: Vec<&Rec> = day.iter().chain(extra.iter()).collect();
    all.sort_by_key(|r| r.t);
    let mut poisoned = ZoneBoard::new(true);
    for r in all {
        feed(&mut poisoned, r);
    }
    end_of_day(&mut poisoned);
    for sym in syms {
        let a = base.shelf(sym).unwrap().events[0];
        let b = poisoned.shelf(sym).unwrap().events[0];
        assert_eq!(
            (a.t_ns, a.entry, a.lo, a.hi, a.peak),
            (b.t_ns, b.entry, b.lo, b.hi, b.peak)
        );
    }
    // And the poison did reach the profile afterwards (the test would be vacuous otherwise).
    let p = |b: &ZoneBoard| b.shelf("GENK").unwrap().profile.zones();
    assert_ne!(p(&base), p(&poisoned));
}

/// A synthetic leg: liquid $1.00 prints from 09:30, then a climb to `top` over `steps` slots.
fn leg(sym: &str, t0: i64, top: f64, trades_per_slot: u64, size: u64) -> Vec<Trade> {
    let mut v = Vec::new();
    let start = t0 + 22_200 * SEC_NS; // 10:10 ET
    for k in 0..40 {
        let p = if k < 20 {
            1.00
        } else {
            1.00 + (top - 1.00) * (k - 19) as f64 / 20.0
        };
        for n in 0..trades_per_slot {
            v.push(trade(
                sym,
                start + k * BAR_NS + (n as i64 + 1) * 100_000_000,
                p,
                size,
            ));
        }
    }
    v
}

fn day_t0() -> i64 {
    let d = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
    market_events::session::et(d, 4, 0, 0, 0).unwrap().0
}

fn run_board(ts: &[Trade]) -> ZoneBoard {
    let mut b = ZoneBoard::new(true);
    for t in ts {
        b.tick(t.event_ts.0);
        b.apply_trade(t, BOTH);
    }
    b
}

/// A run is known only from the END of its mark slot plus the settle, never before; the start
/// price is the leg's low.
#[test]
fn a_run_is_confirmed_only_after_its_mark_slot_ends() {
    let t0 = day_t0();
    let ts = leg("LEGX", t0, 1.30, 3, 1_000);
    let mut b = ZoneBoard::new(true);
    let mut confirmed_at = None;
    for t in &ts {
        b.tick(t.event_ts.0);
        if confirmed_at.is_none() && b.shelf("LEGX").is_some() {
            confirmed_at = Some(t.event_ts.0);
        }
        b.apply_trade(t, BOTH);
    }
    let sh = b.shelf("LEGX").expect("a +20 % leg confirms");
    let run = sh.runs[0];
    assert_eq!(run.start_price, 1.00);
    // The first close >= 1.20: step k-19 with 0.30*(k-19)/20 >= 0.20 -> k = 33.
    let start = t0 + 22_200 * SEC_NS;
    assert_eq!(run.plus20_ns, start + 34 * BAR_NS);
    // The latest slot at the low is slot 19.
    assert_eq!(run.start_ns, start + 20 * BAR_NS);
    let at = confirmed_at.expect("confirmed while printing");
    assert!(
        at >= run.plus20_ns + SETTLE_NS,
        "seen at {at}, mark end {}",
        run.plus20_ns
    );
}

/// The liquidity floor (25 trades and $25k from the start slot through the mark), the band on
/// the start price and the 30-minute refractory all hold.
#[test]
fn the_trigger_keeps_the_labels_floor_band_and_refractory() {
    let t0 = day_t0();
    // 1 trade a slot from slot 19 to the mark at 33: 15 trades < 25.
    let b = run_board(&leg("THIN", t0, 1.30, 1, 10_000));
    assert!(b.shelf("THIN").is_none(), "under 25 trades");
    // 3 trades of 50 shares at ~$1.1: ~45 x $55 < $25k.
    let b = run_board(&leg("POOR", t0, 1.30, 3, 50));
    assert!(b.shelf("POOR").is_none(), "under $25k");
    // Liquid but $0.12 start: under the band.
    let ts: Vec<Trade> = leg("PENNY", t0, 1.30, 3, 1_000_000)
        .into_iter()
        .map(|t| {
            let p = t.price.to_f64() * 0.12;
            trade("PENNY", t.event_ts.0, p, t.size)
        })
        .collect();
    let b = run_board(&ts);
    assert!(b.shelf("PENNY").is_none(), "start under $0.15");
    // Two legs 20 minutes apart: the second is inside the refractory.
    let mut ts = leg("TWICE", t0, 1.30, 3, 1_000);
    let later: Vec<Trade> = leg("TWICE", t0, 1.30, 3, 1_000)
        .into_iter()
        .map(|t| {
            trade(
                "TWICE",
                t.event_ts.0 + 20 * MIN_NS,
                t.price.to_f64() * 1.3,
                t.size,
            )
        })
        .collect();
    ts.extend(later);
    let b = run_board(&ts);
    assert_eq!(b.shelf("TWICE").unwrap().runs.len(), 1);
}

/// No shelf is shown, and no event can fire, before the symbol's first run is confirmed.
#[test]
fn nothing_is_decided_before_the_first_run() {
    let mut b = ZoneBoard::new(true);
    let day = dev_day();
    let first = expected()["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["symbol"] == "GENK")
        .unwrap()["plus20_ns"]
        .as_i64()
        .unwrap();
    for r in day.iter().filter(|r| r.sym == "GENK") {
        if r.t >= first + SETTLE_NS {
            break;
        }
        feed(&mut b, r);
        assert!(b.shelf("GENK").is_none(), "confirmed at {}", r.t);
        assert_eq!(b.json(r.t), serde_json::json!({}));
    }
}

/// After a restart the history `Builder` rebuilds the symbol's day (a rebuilder fed the
/// recorded prints, ticked at each clock advance) and the live board adopts it: everything after
/// the adoption equals the uninterrupted run, and the forward log writes only what the previous
/// process had not.
#[test]
fn a_restart_rebuilds_the_day_and_logs_nothing_twice() {
    let day = dev_day();
    // The uninterrupted run.
    let mut full = ZoneBoard::new(true);
    full.enable_log(HashSet::new());
    let mut full_lines = Vec::new();
    for r in &day {
        feed(&mut full, r);
        full_lines.extend(full.take_log());
    }
    end_of_day(&mut full);
    full_lines.extend(full.take_log());
    // GENK's first event is at 1775050650 (13:37:30 UTC); restart between its two events,
    // while the first one's outcome is still open (it resolves later).
    let restart = 1_775_050_700_000_000_000;
    let adopt_at = restart + 90 * SEC_NS;
    let mut before = ZoneBoard::new(true);
    before.enable_log(HashSet::new());
    let mut written: Vec<String> = Vec::new();
    for r in day.iter().filter(|r| r.t < restart) {
        feed(&mut before, r);
        written.extend(before.take_log().into_iter().map(|(_, l)| l));
    }
    let keys = |lines: &[String]| -> HashSet<String> {
        lines
            .iter()
            .map(|l| {
                let v: Value = serde_json::from_str(l).unwrap();
                let t = if v["kind"] == "run" {
                    v["plus20_ns"].as_i64()
                } else {
                    v["t_ns"].as_i64()
                };
                key(
                    v["kind"].as_str().unwrap(),
                    v["symbol"].as_str().unwrap(),
                    t.unwrap(),
                )
            })
            .collect()
    };
    // The new process: today's file read back, then every symbol rebuilt and adopted.
    let mut live = ZoneBoard::new(true);
    live.enable_log(keys(&written));
    for sym in ["GENK", "ROMA", "AMOD"] {
        let mut rb = ZoneBoard::rebuilder();
        for r in day.iter().filter(|r| r.sym == sym && r.t < adopt_at) {
            feed(&mut rb, r);
        }
        live.adopt(sym, rb, adopt_at);
    }
    for r in day.iter().filter(|r| r.t >= adopt_at) {
        feed(&mut live, r);
    }
    end_of_day(&mut live);
    let after: Vec<String> = live.take_log().into_iter().map(|(_, l)| l).collect();
    for sym in ["GENK", "ROMA", "AMOD"] {
        let a = &full.shelf(sym).unwrap().events;
        let b = &live.shelf(sym).unwrap().events;
        assert_eq!(a.len(), b.len(), "{sym}");
        for (x, y) in a.iter().zip(b) {
            assert_eq!(
                (x.t_ns, x.state, x.resolved_ns),
                (y.t_ns, y.state, y.resolved_ns)
            );
            assert_eq!((x.lo, x.hi, x.entry), (y.lo, y.hi, y.entry));
        }
        assert_eq!(full.shelf(sym).unwrap().runs, live.shelf(sym).unwrap().runs);
    }
    let (kw, ka) = (keys(&written), keys(&after));
    assert!(
        kw.is_disjoint(&ka),
        "logged twice: {:?}",
        kw.intersection(&ka)
    );
    let all: HashSet<String> = kw.union(&ka).cloned().collect();
    let full_keys = keys(
        &full_lines
            .into_iter()
            .map(|(_, l)| l)
            .collect::<Vec<String>>(),
    );
    assert_eq!(all, full_keys);
    // The open outcome at the restart is logged by the new process.
    assert!(ka.contains(&key("outcome", "GENK", 1_775_050_650_000_000_000)));
}

/// The frame shows qualifying shelves while they exist and the event with its outcome for 10
/// minutes after it resolves.
#[test]
fn the_frame_shows_bands_and_the_event_for_a_while() {
    let mut b = ZoneBoard::new(true);
    let day = dev_day();
    let ev = 1_775_050_650_000_000_000;
    let mut seen_in_shelf = false;
    for r in &day {
        feed(&mut b, r);
        if r.t > ev + SETTLE_NS && r.t < ev + 30 * SEC_NS {
            let v = b.json(r.t);
            if v["GENK"]["event"]["state"] == "in_shelf" {
                seen_in_shelf = true;
                assert!(!v["GENK"]["bands"].as_array().unwrap().is_empty());
            }
        }
    }
    assert!(seen_in_shelf);
    let e = b.shelf("GENK").unwrap().events[0];
    let resolved = e.resolved_ns.unwrap();
    // Not the latest event any more by the end of the day (a second one fired), so check the
    // window rule on a board stopped right after the resolution.
    let mut c = ZoneBoard::new(true);
    for r in day.iter().filter(|r| r.t <= resolved + SEC_NS) {
        feed(&mut c, r);
    }
    c.tick(resolved + 20 * SEC_NS);
    assert_eq!(
        c.json(resolved + 20 * SEC_NS)["GENK"]["event"]["state"],
        "held"
    );
    let later = c.json(resolved + SHOW_RESOLVED_NS + SEC_NS);
    assert!(later["GENK"]["event"].is_null() || later.get("GENK").is_none());
}

/// Cost: a busy synthetic session (2,000 symbols in band, ~500 k prints) measured per print,
/// and the memory held while they trade and after a run confirms.
#[test]
fn cost() {
    let t0 = day_t0();
    let start = t0 + 19_800 * SEC_NS; // 09:30 ET
    let mut b = ZoneBoard::new(true);
    let n_sym = 2_000;
    let names: Vec<String> = (0..n_sym).map(|i| format!("S{i:04}")).collect();
    let mut trades = Vec::new();
    // 20 minutes, each symbol printing ~13 times a slot at a few prices.
    for k in 0..80i64 {
        for (i, s) in names.iter().enumerate() {
            for n in 0..3 {
                let p = 2.0 + ((i as i64 + k + n) % 7) as f64 * 0.01;
                trades.push(trade(s, start + k * BAR_NS + n * 1_000_000_000, p, 100));
            }
        }
    }
    let t = std::time::Instant::now();
    for tr in &trades {
        b.tick(tr.event_ts.0);
        b.apply_trade(tr, BOTH);
    }
    let el = t.elapsed();
    let (syms, conf, bytes) = b.cost();
    let ns = el.as_nanos() as f64 / trades.len() as f64;
    eprintln!(
        "zones cost: {} prints, {ns:.0} ns/print incl. ticks, {syms} symbols, \
         {conf} confirmed, {:.1} MB held ({:.0} B/symbol)",
        trades.len(),
        bytes as f64 / 1e6,
        bytes as f64 / syms as f64
    );
    assert!(bytes / syms < 16_384, "{bytes} B for {syms} symbols");
    // A confirmed symbol's profile from the DEV day.
    let mut d = ZoneBoard::new(true);
    for r in dev_day() {
        feed(&mut d, &r);
    }
    for s in ["GENK", "ROMA", "AMOD"] {
        let p = d.shelf(s).unwrap().profile.bytes();
        eprintln!("zones cost: {s} profile {p} B");
        assert!(p < 16_384);
    }
}
