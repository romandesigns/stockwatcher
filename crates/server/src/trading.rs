//! Which account the live market's chart orders go to (owner, 2026-10-29: "integrate my alpaca
//! paper trading account so I can place orders using alpaca paper trading"). Contract and safety
//! rules: docs/architecture/alpaca-paper.md.
//!
//! - `tradingAccount` (synced setting, written by clients): `{"live": "simulator" |
//!   "alpaca_paper"}`. Anything else, or nothing, is `simulator`, today's behaviour. It only ever
//!   applies to the LIVE market; the replay market is always the simulator
//!   ([`crate::Market::set_paper_route`] refuses anything else).
//! - `broker.status` (server-owned, like `alerts.state`): `{"alpacaPaper": {configured, state,
//!   message, keys}}`, so Settings can say "not configured" and why. Never a key, never account
//!   data.
//!
//! The Alpaca route exists even without keys ([`alpaca_paper::Service::unconfigured`]): choosing
//! it then rejects every order with "Alpaca paper is not configured on the server", visibly,
//! rather than silently falling back to the simulator.

use std::sync::Arc;
use std::time::Duration;

use alpaca_paper::{Credentials, CredsMissing, NOT_CONFIGURED, Service};
use serde_json::{Value, json};

use crate::market::Market;
use crate::settings::{Settings, SettingsOp};

/// Client-written: the live market's account.
pub const ACCOUNT_NS: &str = "tradingAccount";
/// Server-written: the Alpaca paper route's health.
pub const STATUS_NS: &str = "broker.status";

/// Namespaces only the server writes.
pub fn server_owned(ns: &str) -> bool {
    ns == STATUS_NS
}

/// The live market's account choice in the `tradingAccount` value.
pub fn wants_alpaca(v: Option<&Value>) -> bool {
    v.and_then(|v| v.get("live"))
        .and_then(Value::as_str)
        .is_some_and(|s| s == "alpaca_paper")
}

/// Builds the Alpaca paper route from the environment (unconfigured without keys) and follows
/// the setting. Its frames go out on the live market's broadcast while it is the route.
pub fn start(settings: Settings, live: Market, env: impl Fn(&str) -> Option<String>) -> Service {
    let publish_to = live.clone();
    let publish: alpaca_paper::Publish =
        Arc::new(move |frames| publish_to.publish_route_frames(frames));
    let svc = match Credentials::from_lookup(env) {
        Ok(c) => {
            tracing::info!(target: "alpaca_paper", keys = c.source.env_names(), "Alpaca paper route available");
            Service::start(c, publish)
        }
        Err(why) => {
            let reason = match why {
                CredsMissing::None => {
                    "set ALPACA_PAPER_KEY and ALPACA_PAPER_SECRET (paper-account keys) in the server's .env"
                }
                CredsMissing::SharedLooksLive => {
                    "ALPACA_API_KEY is a live-account key; set ALPACA_PAPER_KEY and ALPACA_PAPER_SECRET (paper-account keys) in the server's .env"
                }
            };
            tracing::info!(target: "alpaca_paper", reason, "Alpaca paper route not configured");
            Service::unconfigured(format!("{NOT_CONFIGURED}: {reason}"), publish)
        }
    };
    follow(settings, live, svc.clone());
    svc
}

/// Applies the setting now and on every change; writes `broker.status` on every status change.
pub fn follow(settings: Settings, live: Market, svc: Service) {
    let s = settings.clone();
    let route = svc.clone();
    tokio::spawn(async move {
        let mut rx = s.subscribe();
        let apply = |v: Option<Value>| {
            let alpaca = wants_alpaca(v.as_ref());
            if alpaca != live.paper_route().is_some() {
                tracing::info!(target: "alpaca_paper", account = if alpaca { "alpaca_paper" } else { "simulator" }, "live market account switched");
                live.set_paper_route(alpaca.then(|| route.clone()));
            }
        };
        apply(s.get(ACCOUNT_NS).await.map(|(_, v)| v));
        loop {
            match rx.recv().await {
                Ok(frame) => {
                    let ours = serde_json::from_str::<Value>(&frame)
                        .is_ok_and(|v| v["type"] == "settings.changed" && v["ns"] == ACCOUNT_NS);
                    if ours {
                        apply(s.get(ACCOUNT_NS).await.map(|(_, v)| v));
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    apply(s.get(ACCOUNT_NS).await.map(|(_, v)| v));
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    tokio::spawn(async move {
        let mut status = svc.status();
        let mut n = 0u64;
        loop {
            let value = json!({ "alpacaPaper": status.borrow_and_update().clone() });
            n += 1;
            let op = SettingsOp::Set {
                value,
                if_absent: false,
            };
            if let Err(e) = settings
                .apply(STATUS_NS, &format!("srv-broker-{n}"), op)
                .await
            {
                tracing::warn!(target: "alpaca_paper", code = e.code(), "broker.status not saved");
            }
            if status.changed().await.is_err() {
                break;
            }
            // A reconnect flaps through several states in a moment; write the one it settles on.
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_explicit_alpaca_paper_choice_routes() {
        assert!(wants_alpaca(Some(&json!({"live": "alpaca_paper"}))));
        for v in [
            json!({"live": "simulator"}),
            json!({"live": "alpaca"}),
            json!({"live": "alpaca_live"}),
            json!("alpaca_paper"),
            json!(null),
        ] {
            assert!(!wants_alpaca(Some(&v)), "{v}");
        }
        assert!(!wants_alpaca(None));
    }

    fn route() -> Service {
        Service::unconfigured(NOT_CONFIGURED.into(), Arc::new(|_| {}))
    }

    fn broker_of(frame: &str) -> String {
        serde_json::from_str::<Value>(frame).unwrap()["broker"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    #[test]
    fn the_replay_market_is_never_routed_to_alpaca() {
        let replay = Market::idle(market_providers::ClockMode::Replay, "idle", None);
        assert!(!replay.set_paper_route(Some(route())));
        assert!(replay.paper_route().is_none());
        assert_eq!(broker_of(&replay.paper_state_frame()), "simulator");
    }

    #[test]
    fn the_live_market_switches_its_state_and_back() {
        let live = Market::idle(market_providers::ClockMode::Live, "idle", None);
        let mut rx = live.subscribe();
        assert_eq!(broker_of(&live.paper_state_frame()), "simulator");
        assert!(live.set_paper_route(Some(route())));
        assert_eq!(broker_of(&live.paper_state_frame()), "alpaca_paper");
        // Every connection was sent the new state.
        let crate::market::OutKind::Paper(f) = &rx.try_recv().unwrap().kind else {
            panic!("not a paper frame")
        };
        assert_eq!(broker_of(f), "alpaca_paper");
        // The simulator stays quiet while routed: its frames would overwrite Alpaca's.
        live.paper_reset();
        assert!(rx.try_recv().is_err());
        assert!(live.set_paper_route(None));
        let crate::market::OutKind::Paper(f) = &rx.try_recv().unwrap().kind else {
            panic!("not a paper frame")
        };
        assert_eq!(broker_of(f), "simulator");
    }

    #[test]
    fn the_status_namespace_is_server_owned() {
        assert!(server_owned(STATUS_NS));
        assert!(!server_owned(ACCOUNT_NS));
    }
}
