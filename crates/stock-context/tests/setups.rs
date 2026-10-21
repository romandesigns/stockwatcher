//! `bull_flag_v0` (EXPERIMENTAL, setups-v0), kept for research reproducibility only.
//!
//! Since the owner's decision of 2026-10-21 no Ross row uses it: the rows' cues come from the
//! detection policy's micro-pullback (`tests/ross_cues.rs`). The setup-comparison harness
//! (branch research/setup-compare) still steps [`BullFlag`] on the engine's own bars, so its
//! behaviour is pinned here, driven directly with the bars the engine would hand it. Synthetic
//! fixtures: one trading day, 2026-10-16.

use chrono::NaiveDate;
use market_events::session::et;
use std::collections::VecDeque;
use stock_context::setup::{
    Bar, BullFlag, SetupLogEntry, SetupPolicy, SetupState, SetupView, Step,
};
use stock_context::*;

const S: i64 = 1_000_000_000;

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 16).unwrap()
}
fn t(h: u32, m: u32, s: u32) -> i64 {
    et(day(), h, m, s, 0).unwrap().nanos()
}
/// End of the k-th 15 s bar after 09:30.
fn end(k: i64) -> i64 {
    t(9, 30, 0) + (k + 1) * 15 * S
}

/// The engine's per-barrier hand-off to `bull_flag_v0` (`Engine::publish_panel` before
/// 2026-10-21): completed bars, the latest folded price, the day's volume clock.
struct Flag {
    policy: SetupPolicy,
    bars: VecDeque<Bar>,
    flag: BullFlag,
    log: BoundedLog<SetupLogEntry>,
    last: Option<(i64, i64)>,
    day_volume: u64,
}

impl Flag {
    fn new() -> Flag {
        Flag {
            policy: SetupPolicy::v0(),
            bars: VecDeque::new(),
            flag: BullFlag::default(),
            log: BoundedLog::new(4096),
            last: None,
            day_volume: 0,
        }
    }

    /// Bar `k` exactly `[open, high, low, close]` (cents) with `vol` shares, then its barrier.
    fn bar(&mut self, k: i64, ohlc: [i64; 4], vol: u64) -> SetupView {
        let e0 = end(k) - 15 * S;
        let [o, h, l, c] = ohlc.map(|x| x * 10_000);
        let mut b = Bar::new(end(k));
        let q = vol / 4;
        for (off, p, sz) in [(1, o, q), (3, h, q), (5, l, q), (13, c, vol - 3 * q)] {
            b.add(e0 + off * S, p, sz, true, true);
            self.last = Some((e0 + off * S, p));
        }
        self.day_volume += vol;
        self.bars.push_back(b);
        self.flag.step(
            &self.policy,
            15,
            &Step {
                bar_end: end(k),
                cutoff: end(k) + 2 * S,
                bar_ns: 15 * S,
                bars: &self.bars,
                last_price: self.last,
                halted: false,
                day_volume: self.day_volume,
                day_slots: (k + 1) as u64,
                symbol: "FLAG",
                prefix: "fixture:2026-10-16:FLAG",
            },
            &mut self.log,
        )
    }

    /// Flat bars 0-3, the pole (bars 4-5, $5.00 → $5.30) and a two-bar flag (bars 6-7): at bar
    /// 7 the flag forms with trigger $5.25 and invalidation $5.17.
    fn pole_and_flag(&mut self) -> Vec<SetupView> {
        const BARS: [([i64; 4], u64); 8] = [
            ([500, 500, 500, 500], 400),
            ([500, 500, 500, 500], 400),
            ([500, 500, 500, 500], 400),
            ([500, 500, 500, 500], 400),
            ([500, 510, 500, 510], 2_000),
            ([510, 530, 510, 528], 2_000),
            ([525, 525, 518, 520], 500),
            ([520, 522, 517, 519], 500),
        ];
        BARS.into_iter()
            .enumerate()
            .map(|(k, (o, v))| self.bar(k as i64, o, v))
            .collect()
    }
}

#[test]
fn bull_flag_v0_still_walks_pole_flag_breakout_to_triggered() {
    let mut f = Flag::new();
    let views = f.pole_and_flag();
    let s5 = &views[5];
    assert_eq!(
        (s5.state, s5.reason),
        (SetupState::Watching, "impulse_awaiting_flag")
    );
    let pole = s5.pole.clone().unwrap();
    assert_eq!((pole.low, pole.high), (5_000_000, 5_300_000));
    let s7 = &views[7];
    assert_eq!(s7.state, SetupState::Forming);
    let trig1 = s7.trigger.clone().unwrap();
    assert_eq!(
        (trig1.price, s7.invalidation.as_ref().unwrap().price),
        (5_250_000, 5_170_000)
    );
    assert_eq!(
        trig1.version,
        "fixture:2026-10-16:FLAG:bull_flag_v0:1:trigger:1"
    );
    assert_eq!(s7.meter_milli, Some(250));
    assert_eq!(f.bar(8, [519, 521, 518, 521], 400).state, SetupState::Near);
    let s9 = f.bar(9, [521, 527, 520, 526], 300);
    assert_eq!(s9.state, SetupState::Crossing);
    let pend: Vec<_> = s9.pending.iter().map(|p| (p.condition, p.state)).collect();
    assert_eq!(
        pend,
        vec![("close_above_trigger", Tri::Pass), ("volume", Tri::Fail)]
    );
    let trig2 = s9.trigger.clone().unwrap();
    assert_eq!((trig2.price, trig2.reason), (5_270_000, "flag_high_raised"));
    let s10 = f.bar(10, [526, 532, 526, 531], 2_000);
    assert_eq!(s10.state, SetupState::Triggered);
    assert_eq!(s10.outcome.as_ref().unwrap().level, trig2);
    let mut last = s10;
    for k in 11..=18 {
        last = f.bar(k, [531, 531, 531, 531], 400);
        if k == 17 {
            assert_eq!(last.state, SetupState::Triggered, "held 120 s");
        }
    }
    assert_eq!(last.state, SetupState::Watching);
    let states: Vec<_> = f
        .log
        .take()
        .into_iter()
        .filter_map(|x| match x {
            SetupLogEntry::Transition(t) => Some(t.to),
            _ => None,
        })
        .collect();
    assert_eq!(
        states,
        vec![
            SetupState::Watching,
            SetupState::Forming,
            SetupState::Near,
            SetupState::Crossing,
            SetupState::Triggered,
            SetupState::Watching
        ]
    );
}

#[test]
fn bull_flag_v0_still_invalidates_below_the_flag_low() {
    let mut f = Flag::new();
    f.pole_and_flag();
    let s = f.bar(8, [519, 519, 512, 514], 400);
    assert_eq!(
        (s.state, s.reason),
        (SetupState::Invalidated, "closed_below_flag_low")
    );
    assert_eq!(s.outcome.unwrap().level.price, 5_170_000);
}
