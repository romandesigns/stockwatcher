//! Manual check against Alpaca's real stream service. Ignored by default (CI has no keys and
//! must not use the account's single stream connection). Run it with the keys in the
//! environment, never on the command line:
//!
//! ```sh
//! set -a; . ./.env; set +a
//! ALPACA_LIVE_SMOKE=1 cargo test -p market-providers --test alpaca_real -- --ignored --nocapture
//! ```
//!
//! It connects twice, one connection at a time, each closed before the next (Alpaca allows one
//! stream connection per account on most plans):
//! 1. `v2/test`, symbol `FAKEPACA`: Alpaca's always-on fake stream. Proves auth, subscribe and
//!    trade normalization against the real service on any day, market open or not.
//! 2. `v2/sip`, `*`: proves the account's SIP entitlement and that a whole-market subscription
//!    is acknowledged. Trades are only expected while a session is open.
//!
//! It prints counts and message kinds only. Credentials are never printed.

use market_events::MarketEvent;
use market_providers::alpaca_stream::{AlpacaStream, AlpacaStreamConfig, LiveSymbols, StreamFeed};
use market_providers::import::alpaca::AlpacaCredentials;
use market_providers::{LinkState, LiveFeed};
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::time::Instant;

async fn probe(
    creds: &AlpacaCredentials,
    feed: StreamFeed,
    symbols: LiveSymbols,
    listen: Duration,
    enough: usize,
) -> (bool, BTreeMap<&'static str, u64>) {
    let mut c = AlpacaStreamConfig::new(
        AlpacaStreamConfig::DEFAULT_BASE,
        feed,
        symbols,
        creds.clone(),
    );
    // One try only: a real refusal should be reported, not retried against the account.
    c.backoff.initial = Duration::from_secs(3600);
    c.backoff.max = Duration::from_secs(3600);
    let mut s = AlpacaStream::new(c);
    let stats = s.stats();
    let mut link = s.link().unwrap();
    let mut rx = Box::new(s).connect().unwrap();
    let mut up = false;
    let mut kinds = BTreeMap::new();
    let mut shown = 0;
    let deadline = Instant::now() + listen;
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => break,
            changed = link.changed() => {
                if changed.is_err() { break }
                let l = link.borrow_and_update().clone();
                println!("  link {:?}: {}", l.state, l.detail.unwrap_or_default());
                match l.state {
                    LinkState::Up => up = true,
                    LinkState::Down => break,
                    LinkState::Connecting => {}
                }
            }
            e = rx.recv() => {
                let Some(e) = e else { break };
                let k = match &e {
                    MarketEvent::Trade(_) => "trade",
                    MarketEvent::Quote(_) => "quote",
                    MarketEvent::Correction(_) => "correction",
                    MarketEvent::Cancel(_) => "cancel",
                    MarketEvent::Status(_) => "status",
                };
                *kinds.entry(k).or_default() += 1;
                if let MarketEvent::Trade(t) = &e && shown < 3 {
                    shown += 1;
                    println!(
                        "  trade {} {} x{} @{} ex={} tape={:?} cond={:?} session={:?} source={:?}",
                        t.symbol, t.price, t.size, t.event_ts.to_utc(), t.exchange, t.tape,
                        t.conditions, t.session.map(|s| s.to_string()), t.source
                    );
                }
                if kinds.values().sum::<u64>() as usize >= enough { break }
            }
        }
    }
    // Dropping the receiver makes the client send a close frame and stop for good.
    drop(rx);
    tokio::time::sleep(Duration::from_secs(2)).await;
    println!("  stats {:?}", stats.snapshot());
    (up, kinds)
}

#[tokio::test]
#[ignore = "needs real Alpaca keys and uses the account's stream connection"]
async fn alpaca_test_stream_and_sip_subscription() {
    if std::env::var("ALPACA_LIVE_SMOKE").ok().as_deref() != Some("1") {
        eprintln!("ALPACA_LIVE_SMOKE != 1: skipped");
        return;
    }
    let creds = AlpacaCredentials::from_lookup(|k| std::env::var(k).ok())
        .expect("ALPACA_API_KEY / ALPACA_API_SECRET not set");

    println!("v2/test FAKEPACA:");
    let (up, kinds) = probe(
        &creds,
        StreamFeed::Test,
        LiveSymbols::List(vec!["FAKEPACA".into()]),
        Duration::from_secs(30),
        20,
    )
    .await;
    println!("  up={up} events={kinds:?}");
    assert!(up, "test stream never subscribed");
    assert!(
        kinds.get("trade").copied().unwrap_or(0) > 0,
        "no FAKEPACA trades"
    );

    println!("v2/sip *:");
    let (up, kinds) = probe(
        &creds,
        StreamFeed::Sip,
        LiveSymbols::All,
        Duration::from_secs(8),
        usize::MAX,
    )
    .await;
    println!("  up={up} events={kinds:?}");
    assert!(up, "sip subscription with * was not acknowledged");
}
