//! The confidence stage: b-good-1's tags leave the board without their quote group (the board has
//! no quotes, `crate::spread` module docs), so this task fetches the NBBO at the alert instant
//! `a` and at the scoring instant `t = a + 30 s`, scores the tag ([`super::score_tag`]) and hands
//! the result to the alerts task, which ledgers it and shows it ([`crate::alerts`], "Confidence").
//!
//! # The quotes, as the research's xsim rule (`cachelib.quote_at`)
//!
//! The last VALID quote (bid > 0, ask > bid) at or before the instant, flagged stale past 60 s and
//! non-market past a 50 % spread. The request asks Alpaca's historical quotes endpoint (the spread
//! stage's source, same keys) for the newest [`crate::spread::QUOTE_LIMIT`] quotes in the
//! [`LOOKBACK_NS`] before the instant, so a quote older than 60 s is still found and flagged
//! stale, as in the research. A failed request is "no quote data" ([`logic::QF_NO_FILE`], the
//! research's "no quote file"); an answer with no valid quote is [`logic::QF_NO_QUOTE`]. Without a
//! quote source (no keys, `ALERT_SPREAD=0`, tests) every tag is scored with no quote data, which is
//! a distribution the model saw (about 13 % of Train alerts had no quote file) but not the live
//! one: the stage logs it once.
//!
//! Requests come at most a few hundred a day (one per B0 alert); each pair is fetched
//! concurrently and bounded by the spread stage's timeout, so a tag lands about 30.5 s after its
//! alert.

use tokio::sync::mpsc;

use super::logic::{self, QuoteAt};
use super::{Confidence, TagRequest, score_tag};
use crate::spread::QuoteSource;

/// How far back a quote is looked for (module docs).
pub const LOOKBACK_NS: i64 = 3_600 * logic::SEC;

/// The quote rule at `at_ns` over one fetched page (`None` = the request failed).
pub fn quote_from(page: Option<&[crate::spread::Nbbo]>, at_ns: i64) -> QuoteAt {
    let Some(p) = page else {
        return QuoteAt::NO_DATA;
    };
    let mut book: Vec<(i64, i64, i64)> = p
        .iter()
        .map(|q| {
            (
                q.at_ns,
                (q.bid * 1e6).round() as i64,
                (q.ask * 1e6).round() as i64,
            )
        })
        .collect();
    book.sort_by_key(|q| q.0);
    QuoteAt::at(Some(&book), at_ns)
}

async fn quote(src: Option<&QuoteSource>, symbol: &str, at_ns: i64) -> QuoteAt {
    let Some(src) = src else {
        return QuoteAt::NO_DATA;
    };
    let page = src.quotes_before(symbol, at_ns, LOOKBACK_NS).await;
    quote_from(page.as_deref(), at_ns)
}

/// Starts the stage: tag requests in from the market, scored tags out to the alerts task.
pub fn spawn(
    source: Option<QuoteSource>,
    mut rx: mpsc::Receiver<TagRequest>,
    tx: mpsc::Sender<Confidence>,
) {
    if source.is_none() {
        tracing::warn!("b-good-1: no quote source; tags are scored without quotes (no quote data)");
    }
    tokio::spawn(async move {
        while let Some(r) = rx.recv().await {
            let src = source.clone();
            let tx = tx.clone();
            tokio::spawn(async move {
                let (q_a, q_t) = tokio::join!(
                    quote(src.as_ref(), &r.symbol, r.a_ns),
                    quote(src.as_ref(), &r.symbol, r.t_ns)
                );
                let now = crate::history::wall_now().0;
                let c = score_tag(&r, q_a, q_t, now);
                if tx.send(c).await.is_err() {
                    tracing::debug!("b-good-1: the alerts task is gone; tag dropped");
                }
            });
        }
    });
}
