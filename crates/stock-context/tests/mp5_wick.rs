use stock_context::detect::fixture::{self, Script, cutoff, end};
use stock_context::{DetectionTransition, Family, State, WickStatus};

fn prefix(close: i64) -> Script {
    let mut s = Script::new();
    for k in 0..6 {
        s.bar("WICK", k, [300; 4], 100);
    }
    s.bar("WICK", 6, [300, 302, 300, 302], 200);
    s.bar("WICK", 7, [302, 310, 302, 310], 300);
    s.bar("WICK", 8, [310, 325, 310, 325], 300);
    s.bar("WICK", 9, [325, 340, 325, close], 400);
    s.bar("WICK", 10, [330, 332, 329, 330], 150);
    s.advance(cutoff(10));
    s
}

fn micro(ts: Vec<DetectionTransition>) -> Vec<DetectionTransition> {
    ts.into_iter()
        .filter(|t| t.family == Family::MicroPullback)
        .collect()
}

#[test]
fn flagged_and_clear_candles_keep_the_same_arming_and_levels() {
    let run = |close| {
        let mut s = prefix(close);
        s.bar("WICK", 11, [330, 331, 328, 329], 120);
        s.advance(cutoff(11));
        let mut e = fixture::engine("wick-test");
        micro(fixture::drive(&mut e, &s.steps))
    };
    let flagged = run(328);
    let clear = run(338);
    for (ts, status) in [(&flagged, WickStatus::Flagged), (&clear, WickStatus::Clear)] {
        let armed = ts
            .iter()
            .find(|t| t.to == State::Armed)
            .expect("still arms");
        let a = armed.anchors.as_ref().unwrap();
        let w = a.wick.as_ref().unwrap();
        assert_eq!(w.status, status);
        assert_eq!(w.bar_end, end(9));
        assert_eq!(w.known_at, cutoff(10));
        assert_eq!(a.frozen_at, w.known_at);
        assert_eq!(w.ohlc.unwrap()[1], a.h0);
    }
    let signature = |ts: &[DetectionTransition]| {
        ts.iter()
            .map(|t| (t.to, t.observed_at, t.level.as_ref().map(|l| l.price)))
            .collect::<Vec<_>>()
    };
    assert_eq!(signature(&flagged), signature(&clear));
}

#[test]
fn a_future_spike_does_not_change_the_frozen_candle_or_logged_prefix() {
    let mut s = prefix(328);
    let mut e = fixture::engine("wick-test");
    let first = micro(fixture::drive(&mut e, &s.steps));
    let frozen = first
        .first()
        .unwrap()
        .anchors
        .as_ref()
        .unwrap()
        .wick
        .clone();
    s.steps.clear();
    s.bar("WICK", 11, [330, 331, 328, 329], 120);
    s.advance(cutoff(11));
    s.bar("WICK", 12, [329, 500, 329, 490], 500);
    s.advance(cutoff(12));
    let next = micro(fixture::drive(&mut e, &s.steps));
    for t in next.iter().filter(|t| t.episode_id == first[0].episode_id) {
        assert_eq!(t.anchors.as_ref().unwrap().wick, frozen);
    }
    assert_eq!(first[0].anchors.as_ref().unwrap().wick, frozen);
    assert_eq!(frozen.as_ref().unwrap().status, WickStatus::Flagged);
}

#[test]
fn a_revision_after_freezing_does_not_substitute_a_different_h0_candle() {
    let mut s = prefix(328);
    let mut e = fixture::engine("wick-test");
    let first = micro(fixture::drive(&mut e, &s.steps));
    let frozen = first[0].anchors.as_ref().unwrap().wick.clone();
    s.steps.clear();
    // The high print is seq 38: nine preceding bars, then the second H0 print.
    s.cancel("WICK", 38, 340, 400, cutoff(10) + 1_000_000_000);
    s.advance(cutoff(11));
    let next = micro(fixture::drive(&mut e, &s.steps));
    for t in next.iter().filter(|t| t.episode_id == first[0].episode_id) {
        assert_eq!(t.anchors.as_ref().unwrap().wick, frozen);
    }
    let snap = e.detection_snapshot().unwrap();
    let ep = snap
        .active
        .iter()
        .chain(&snap.recent)
        .find(|ep| ep.episode_id == first[0].episode_id)
        .unwrap();
    assert_eq!(ep.anchors.as_ref().unwrap().wick, frozen);
}
