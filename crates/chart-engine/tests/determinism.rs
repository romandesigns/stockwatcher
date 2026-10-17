//! Property tests with a hand-rolled seeded generator (no external crate):
//! 1. any arrival order the policy permits inside the settle window yields identical finalized
//!    candles, gaps and profile;
//! 2. replaying the same event list twice is byte-identical, increments included.
mod common;

use chart_engine::{ChartEngine, Timeframe};
use common::*;
use market_events::{MarketEvent, Trade, UnixNanos};

/// xorshift64*: tiny, deterministic, good enough for shuffling fixtures.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = self.below(i as u64 + 1) as usize;
            v.swap(i, j);
        }
    }
}

const CONDITIONS: &[&[&str]] = &[
    &["@"],
    &["@"],
    &["@"],
    &["@", "F"],
    &["@", "I"],
    &["W"],
    &["M"],
    &["@", "T"],
    &["4"],
];

/// Synthetic prints from 09:28 (pre-market) well into the regular session, with bursts,
/// same-timestamp groups, sub-penny prices and quiet stretches (gaps).
fn generate(seed: u64, n: usize) -> Vec<Trade> {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let start = at(9, 28, 0, 0).nanos();
    let mut ts = start;
    let mut out = Vec::with_capacity(n);
    for seq in 1..=n as u64 {
        // Mostly small steps, sometimes a same-timestamp print, occasionally a long silence.
        ts += match rng.below(20) {
            0..=2 => 0,
            3 => 20_000_000_000 + rng.below(40_000_000_000) as i64,
            _ => rng.below(1_500_000_000) as i64,
        };
        let micros = 3_000_000
            + rng.below(250) as i64 * 1_000
            + if rng.below(5) == 0 {
                rng.below(10) as i64 * 100
            } else {
                0
            };
        let conds = CONDITIONS[rng.below(CONDITIONS.len() as u64) as usize];
        let ex = ["V", "Q", "D"][rng.below(3) as usize];
        out.push(
            Trade::new(
                SYM,
                UnixNanos(ts),
                market_events::Price::from_micros(micros),
                1 + rng.below(500),
                seq,
            )
            .with_exchange(ex)
            .with_tape(market_events::Tape::C)
            .with_conditions(conds.iter().copied()),
        );
    }
    out
}

/// A permitted arrival order: shuffle within 1-second windows (settle window is 2 s), and inject
/// duplicates of earlier prints from the same window.
fn permute(trades: &[Trade], rng: &mut Rng) -> Vec<MarketEvent> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < trades.len() {
        let window = trades[i].event_ts.nanos().div_euclid(1_000_000_000);
        let mut j = i;
        while j < trades.len() && trades[j].event_ts.nanos().div_euclid(1_000_000_000) == window {
            j += 1;
        }
        let mut chunk: Vec<Trade> = trades[i..j].to_vec();
        if rng.below(4) == 0 {
            let dup = chunk[rng.below(chunk.len() as u64) as usize].clone();
            chunk.push(dup);
        }
        rng.shuffle(&mut chunk);
        out.extend(chunk.into_iter().map(ev));
        i = j;
    }
    out
}

fn finalized_state(events: &[MarketEvent]) -> String {
    let mut e = ChartEngine::default();
    for ev in events {
        e.apply(ev);
    }
    e.advance_clock(at(11, 0, 0, 0));
    let mut out = String::new();
    for tf in Timeframe::ALL {
        let mut s = e.snapshot(SYM, tf, None).unwrap();
        // Counters legitimately depend on arrival order (how many prints were reordered).
        s.quality = Default::default();
        assert!(s.candles.iter().all(|c| c.is_final));
        out.push_str(&serde_json::to_string(&s).unwrap());
    }
    for sess in e.sessions(SYM) {
        out.push_str(
            &serde_json::to_string(&e.profile(
                SYM,
                sess,
                Some(market_events::Price::from_micros(100)),
            ))
            .unwrap(),
        );
    }
    out
}

#[test]
fn permitted_reorderings_give_identical_finalized_candles() {
    for seed in 1..=25u64 {
        let trades = generate(seed, 600);
        let in_order: Vec<_> = trades.iter().cloned().map(ev).collect();
        let reference = finalized_state(&in_order);
        let mut rng = Rng(seed ^ 0xDEAD_BEEF);
        for round in 0..4 {
            let permuted = permute(&trades, &mut rng);
            assert_eq!(
                finalized_state(&permuted),
                reference,
                "seed {seed} round {round}"
            );
        }
    }
}

#[test]
fn generator_exercises_the_interesting_cases() {
    let trades = generate(7, 600);
    let mut e = ChartEngine::default();
    let ups: Vec<_> = trades
        .iter()
        .cloned()
        .flat_map(|t| e.apply(&ev(t)))
        .collect();
    let counted = tape_counted(&ups);
    for kind in ["ohlc_and_volume", "volume_only", "excluded:condition_M"] {
        assert!(counted.iter().any(|c| c == kind), "{kind}");
    }
    let snap = e.snapshot(SYM, Timeframe::S15, None).unwrap();
    assert!(!snap.gaps.is_empty());
    assert!(e.sessions(SYM).len() == 2, "spans pre and regular");
    assert!(trades.windows(2).any(|w| w[0].event_ts == w[1].event_ts));
}

#[test]
fn replaying_the_same_events_is_byte_identical() {
    let trades = generate(42, 800);
    let mut rng = Rng(99);
    let mut events = permute(&trades, &mut rng);
    // Mix in corrections and cancels of retained prints.
    // (The fixture helpers address exchange "V".)
    let on_v: Vec<&Trade> = trades
        .iter()
        .rev()
        .filter(|t| t.exchange == "V")
        .take(2)
        .collect();
    events.push(correction(
        on_v[1].event_ts,
        on_v[1].source_seq,
        10_000,
        "3.33",
        77,
        &["@"],
    ));
    events.push(cancel(on_v[0].event_ts, on_v[0].source_seq));
    let run = || {
        let mut e = ChartEngine::default();
        let mut log = String::new();
        for ev in &events {
            for u in e.apply(ev) {
                log.push_str(&serde_json::to_string(&u).unwrap());
                log.push('\n');
            }
        }
        for u in e.advance_clock(at(11, 0, 0, 0)) {
            log.push_str(&serde_json::to_string(&u).unwrap());
        }
        for tf in Timeframe::ALL {
            log.push_str(&serde_json::to_string(&e.snapshot(SYM, tf, None)).unwrap());
        }
        let q = e.quality(SYM);
        assert_eq!(q.iter().map(|(_, q)| q.corrections_applied).sum::<u64>(), 1);
        assert_eq!(q.iter().map(|(_, q)| q.cancels_applied).sum::<u64>(), 1);
        log
    };
    let a = run();
    assert_eq!(a, run());
    assert!(a.len() > 10_000);
}
