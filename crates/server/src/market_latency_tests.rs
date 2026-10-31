//! How long after a Runners decision becomes possible the market actually makes it, measured on
//! the real `Market` ingest and its background tasks, in real time.
//!
//! Every Runners path decides on whole seconds, so the earliest a fast move can be decided is
//! the end of the second its qualifying prints are in. These tests replay a fast move (+15 % in
//! one second on 12 prints, `crate::rocket::fast`'s 15 s arm) into a market whose clock runs at
//! wall speed, at several points inside the second, and measure from the end of that second to
//! the decision: the phone notice leaving the board (live) or the Runners row appearing
//! (replay). Before the rocket task (`Market::spawn_rocket_task`) the decision rode the 1 Hz
//! broadcast tick, 250 ms past each wall second, so this was never under 250 ms; the numbers
//! for that version were taken by running this file against it (commit message).
//!
//! The provider is simulated the way it behaves: a live clock sampled from the wall once a
//! second on a free-running timer, a replay clock every 250 ms (`ReplayOptions::clock_every`),
//! and a busy market's prints (a flat $5 heartbeat symbol every 20 ms, in band) so the Runners
//! tape's newest print time moves on as it does in a session. With the heartbeat out of band
//! (a quiet market) the old tick decided 1.25 s after the second, waiting for the stale clock.

use super::*;
use std::sync::atomic::AtomicU64;

const SEC: i64 = 1_000_000_000;
const TRIALS: usize = 8;

fn wall() -> i64 {
    crate::observe::wall_now()
}

fn clock(mode: ClockMode, now: i64) -> ClockState {
    ClockState {
        mode,
        now: UnixNanos(now),
        speed: market_providers::Speed::Times(1),
        playing: true,
        day: None,
        start: None,
        end: None,
        finished: false,
        events: 0,
    }
}

/// Per trial: (end of the deciding second → decision, last qualifying print applied →
/// decision), both in ms of wall time.
async fn measure(mode: ClockMode) -> Vec<(f64, f64)> {
    let market = Market::idle(mode, "test", None);
    market.set_rocket_watch(true, crate::rocket::Policy::default());
    let (sink, _inputs, mut notices) = crate::alerts::AlertSink::for_test();
    market.attach_alerts(sink);
    // Market time = wall time + a whole number of seconds, so market and wall seconds share
    // their boundaries (the broadcast tick is phase-locked to the wall's).
    let day = NaiveDate::from_ymd_opt(2026, 10, 15).unwrap();
    let open = market_events::session::et(day, 10, 0, 0, 0).unwrap().0;
    let offset = open - wall().div_euclid(SEC) * SEC;
    let mnow = move || wall() + offset;
    let seq = Arc::new(AtomicU64::new(0));
    let trade = {
        let seq = seq.clone();
        move |sym: &str, ts: i64, dollars: f64, size: u64| {
            FeedItem::Event(MarketEvent::Trade(market_events::Trade::new(
                sym,
                UnixNanos(ts),
                Price::from_micros((dollars * 1e6).round() as i64),
                size,
                seq.fetch_add(1, Ordering::Relaxed) + 1,
            )))
        }
    };
    let syms: Vec<String> = (0..TRIALS)
        .map(|k| format!("LT{}", (b'A' + k as u8) as char))
        .collect();
    market.apply(FeedItem::Clock(clock(mode, mnow())));
    // The provider's clock: live, a wall sample once a second on a timer started at whatever
    // phase the process happened to start at; replay, every 250 ms.
    let every = if mode == ClockMode::Live {
        Duration::from_secs(1)
    } else {
        Duration::from_millis(250)
    };
    let ticker = {
        let market = market.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(370)).await;
            let mut tick = tokio::time::interval(every);
            loop {
                tick.tick().await;
                market.apply(FeedItem::Advance(UnixNanos(mnow())));
                market.apply(FeedItem::Clock(clock(mode, mnow())));
            }
        })
    };
    let heartbeat = {
        let market = market.clone();
        let trade = trade.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(20)).await;
                market.apply(trade("HBT", mnow(), 5.0, 100));
            }
        })
    };
    market.spawn_quotes_task();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    // Each trial symbol's standing price, $2.00: the 15 s window's low is carried from it
    // (`FastTape::advance`). After the board's first tick, whose roll into the day clears the
    // fast tapes.
    for s in &syms {
        market.apply(trade(s, mnow(), 2.00, 100));
    }

    let mut out = Vec::new();
    for (k, sym) in syms.iter().enumerate() {
        // The burst lands 5–85 % of the way into its second.
        let phase = 50 + (800 * k as i64) / (TRIALS as i64 - 1);
        let now = mnow();
        let next = (now.div_euclid(SEC) + 1) * SEC + phase * 1_000_000;
        tokio::time::sleep(Duration::from_nanos((next - now) as u64)).await;
        let t = mnow();
        let s = t.div_euclid(SEC);
        for i in 0..12 {
            market.apply(trade(sym, t + i * 1_000, 2.30, 1_000));
        }
        let printed = wall();
        assert!(
            mnow().div_euclid(SEC) == s,
            "the burst stayed in its second"
        );
        let decided = loop {
            if mode == ClockMode::Live {
                let n = tokio::time::timeout(Duration::from_secs(3), notices.recv())
                    .await
                    .expect("a notice within 3 s")
                    .expect("sink open");
                if n.symbol == *sym {
                    assert_eq!(n.kind, crate::rocket::NoticeKind::Fast, "{n:?}");
                    break wall();
                }
            } else {
                let flagged = market
                    .0
                    .core
                    .lock()
                    .expect("core lock")
                    .rocket
                    .symbol_context(sym, None)
                    .is_some();
                if flagged {
                    break wall();
                }
                assert!(wall() - printed < 3 * SEC, "a row within 3 s");
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        };
        let end = (s + 1) * SEC - offset;
        let ms = |a: i64, b: i64| (b - a) as f64 / 1e6;
        out.push((ms(end, decided), ms(printed, decided)));
    }
    ticker.abort();
    heartbeat.abort();
    out
}

fn report(mode: &str, v: &[(f64, f64)]) -> (f64, f64) {
    let mut end: Vec<f64> = v.iter().map(|x| x.0).collect();
    end.sort_by(f64::total_cmp);
    let mut print: Vec<f64> = v.iter().map(|x| x.1).collect();
    print.sort_by(f64::total_cmp);
    let med = |x: &[f64]| x[x.len() / 2];
    eprintln!(
        "runners decision latency, {mode}: second end -> decision ms {end:.0?} (median {:.0}); \
         last print -> decision ms {print:.0?} (median {:.0})",
        med(&end),
        med(&print),
    );
    (med(&end), *end.last().unwrap())
}

/// Live: the phone notice leaves the board within [`ROCKET_SETTLE`] plus scheduling of the
/// second ending. The bounds are loose for a loaded CI machine but still well under the 250 ms
/// the broadcast tick could never beat.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_fast_move_is_decided_just_after_its_second() {
    let v = measure(ClockMode::Live).await;
    let (median, max) = report("live", &v);
    assert!(
        v.iter().all(|x| x.0 >= 0.0),
        "never before the second ended: {v:?}"
    );
    assert!(median < 200.0, "median {median:.0} ms");
    assert!(max < 600.0, "max {max:.0} ms");
}

/// Replay: no grace for late prints; the first print of the next second wakes the decision.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replay_fast_move_is_decided_just_after_its_second() {
    let v = measure(ClockMode::Replay).await;
    let (median, max) = report("replay", &v);
    assert!(
        v.iter().all(|x| x.0 >= 0.0),
        "never before the second ended: {v:?}"
    );
    assert!(median < 200.0, "median {median:.0} ms");
    assert!(max < 600.0, "max {max:.0} ms");
}

fn paused(mut c: ClockState) -> ClockState {
    c.playing = false;
    c
}

/// Spins until `f` holds or `within` passes; returns whether it held.
async fn eventually(within: Duration, mut f: impl FnMut() -> bool) -> bool {
    let until = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < until {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    f()
}

/// A seek into a paused replay: the rebuild's prints wake the rocket task while every pass is
/// refused (`core.rebuilding`), and the closing clock lands on the second those prints already
/// made due. The board must still decide at the target time once the rebuild is over, as the
/// 1 Hz tick did; it used to stay undecided until the replay was stepped or played, because a
/// refused pass marked its second decided.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_seek_into_a_paused_replay_still_decides_the_board() {
    let market = Market::idle(ClockMode::Replay, "test", None);
    market.set_rocket_watch(true, crate::rocket::Policy::default());
    let day = NaiveDate::from_ymd_opt(2026, 10, 15).unwrap();
    let at = |h, m, s, ms| market_events::session::et(day, h, m, s, ms).unwrap().0;
    let trade = |sym: &str, ts: i64, seq: u64| {
        FeedItem::Event(MarketEvent::Trade(market_events::Trade::new(
            sym,
            UnixNanos(ts),
            Price::from_micros(5_000_000),
            100,
            seq,
        )))
    };
    market.apply(FeedItem::Clock(clock(ClockMode::Replay, at(10, 0, 0, 500))));
    market.spawn_quotes_task();
    let decision = |m: &Market| m.0.core.lock().expect("core lock").rocket.decision_ns();
    assert!(
        eventually(Duration::from_secs(2), || decision(&market) > 0).await,
        "the board decides before the seek"
    );

    let target = at(10, 5, 1, 700);
    market.apply(FeedItem::Reset(paused(clock(ClockMode::Replay, target))));
    market.apply(trade("HBT", at(10, 5, 0, 200), 1));
    market.apply(trade("HBT", at(10, 5, 1, 100), 2));
    // Long enough for the task to wake on those prints and have its passes refused.
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(decision(&market), 0, "nothing decided mid-rebuild");
    market.apply(FeedItem::Clock(paused(clock(ClockMode::Replay, target))));
    assert!(
        eventually(Duration::from_secs(1), || decision(&market) == target).await,
        "decided at the target time after the rebuild: {}",
        decision(&market)
    );
}

/// A replay plays a recorded day's late prints at their arrival positions (about 9 % of live
/// prints land after their second ended). A late print must not move the rocket task's "now"
/// back, or a pass in between decides at the older second; one far back (over a minute) is a
/// move in time and is followed.
#[test]
fn a_late_replay_print_does_not_move_the_decision_clock_back() {
    let market = Market::idle(ClockMode::Replay, "test", None);
    let sec = |s: i64| s * SEC;
    market.wake_rocket(sec(1_000));
    market.wake_rocket(sec(997) + 5);
    assert_eq!(market.0.rocket_print_sec.load(Ordering::Relaxed), 1_000);
    market.wake_rocket(sec(1_001));
    assert_eq!(market.0.rocket_print_sec.load(Ordering::Relaxed), 1_001);
    market.wake_rocket(sec(900));
    assert_eq!(market.0.rocket_print_sec.load(Ordering::Relaxed), 900);
    // Live keeps every print's second (the stalled-clock fallback), but never wakes the task.
    let live = Market::idle(ClockMode::Live, "test", None);
    live.wake_rocket(sec(1_000));
    live.wake_rocket(sec(999));
    assert_eq!(live.0.rocket_print_sec.load(Ordering::Relaxed), 999);
}
