//! Frees large detached values on a background thread.
//!
//! At the 09:30 open every symbol drops its earlier date's candles and profiles
//! (`SymbolState::drop_prior_dates`), all in the same clock advance: on a whole-market tape that
//! is millions of buckets, each owning its own maps. Freed in place, that ran inside the server's
//! core lock and stopped its live ingest for 3-6 s (2026-11-23, measured by replaying the day:
//! `market_ingest_bench.rs`), past the feed reader's 2 s grace, so the open's events were dropped.
//! Detaching them is O(1) per series; the frees happen here instead, off every lock.
//!
//! One long-lived thread, started on first use; an unbounded queue (each item is one symbol's
//! detached day, gone within milliseconds). If the thread cannot start, or has gone, the value is
//! freed in place as before: slower, never lost or leaked.

use std::sync::OnceLock;
use std::sync::mpsc::{Sender, channel};

type Junk = Box<dyn Send>;

static QUEUE: OnceLock<Option<Sender<Junk>>> = OnceLock::new();

fn queue() -> Option<&'static Sender<Junk>> {
    QUEUE
        .get_or_init(|| {
            let (tx, rx) = channel::<Junk>();
            std::thread::Builder::new()
                .name("engine-drop".into())
                .spawn(move || {
                    for junk in rx {
                        drop(junk);
                    }
                })
                .ok()
                .map(|_| tx)
        })
        .as_ref()
}

/// Drops `value` on the background thread (in place if that thread is unavailable).
pub(crate) fn later<T: Send + 'static>(value: T) {
    match queue() {
        Some(tx) => {
            if let Err(back) = tx.send(Box::new(value)) {
                drop(back.0);
            }
        }
        None => drop(value),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    struct Flag(Arc<AtomicBool>);
    impl Drop for Flag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn a_deferred_value_is_still_dropped() {
        let dropped = Arc::new(AtomicBool::new(false));
        super::later(Flag(dropped.clone()));
        let until = Instant::now() + Duration::from_secs(5);
        while !dropped.load(Ordering::SeqCst) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(dropped.load(Ordering::SeqCst), "the background thread drops what it is given");
    }
}
