//! The profile figures on chart increments (`total`, `poc`, `valueArea`;
//! chart_engine::wire::ProfileDerivedWire), filled in per bin as a frame goes out, and the cache
//! that keeps that to one engine read per chart change rather than one per connection.
//!
//! Why not in the engine: the engine emits an increment for every trade in the whole market, and
//! the POC and value area need every row of the session's profile (O(distinct prices), thousands
//! on a busy name). Only the few symbols someone is watching need them, so they are read here, on
//! the way out, for frames a connection is actually about to send.
//!
//! Two caches, both small and bounded (a handful of watched charts at a time):
//!
//! - **Reads** per (symbol, session, bin): the whole profile's rows at that bin plus its
//!   [`Derived`], and the core seq they were read at. A read taken at seq S is current for any
//!   frame with seq <= S (the rows are absolute totals, read no earlier than the frame's change),
//!   so a burst of frames queued behind one another shares one read, and every connection on
//!   the same chart shares it too. A frame newer than the read reads again. The last frame of a
//!   burst always reads at or after its own change, so what a client ends up with is current.
//! - **Frames** per (broadcast item, bin): the finished JSON, so a second connection on the same
//!   chart and bin sends the first one's text. Keyed by a `Weak` to the item: it pins the
//!   allocation (not the contents), so the address cannot be reused by a newer item while the
//!   entry exists, and an entry whose item is gone can never match again.
//!
//! This replaced `Market::rebin`, which read the whole profile under the core lock once per
//! connection per frame, at every bin but the default, and sent the default bin's frame without
//! POC or value area, so each client recomputed them after every increment (store.ts
//! `withDerived`).

use std::collections::VecDeque;
use std::sync::{Arc, Weak};

use chart_engine::profile::Derived;
use chart_engine::wire::{ChartUpdate, ProfileDerivedWire, ProfileRow, ValueArea};
use market_events::{Price, SessionId};

/// Entries kept in each cache. Each is a few watched charts' worth: a miss only costs a read.
const CAP: usize = 32;

/// One profile read (module docs).
#[derive(Clone, Debug)]
pub struct ProfileRead {
    /// The core seq the rows were read at.
    pub seq: u64,
    /// Non-empty bins, ascending.
    pub rows: Arc<Vec<(Price, u64)>>,
    pub derived: Derived,
}

#[derive(Default)]
pub struct FrameCache {
    reads: VecDeque<(String, SessionId, Price, ProfileRead)>,
    frames: VecDeque<(Weak<crate::market::Outbound>, Price, String)>,
}

impl FrameCache {
    /// A read of (symbol, session, bin) current for a frame at `seq`, if one is held.
    pub fn read(
        &self,
        symbol: &str,
        session: SessionId,
        bin: Price,
        seq: u64,
    ) -> Option<ProfileRead> {
        self.reads
            .iter()
            .find(|(s, sess, b, r)| s == symbol && *sess == session && *b == bin && r.seq >= seq)
            .map(|(_, _, _, r)| r.clone())
    }

    pub fn put_read(&mut self, symbol: &str, session: SessionId, bin: Price, read: ProfileRead) {
        self.reads
            .retain(|(s, sess, b, _)| !(s == symbol && *sess == session && *b == bin));
        if self.reads.len() >= CAP {
            self.reads.pop_front();
        }
        self.reads
            .push_back((symbol.to_string(), session, bin, read));
    }

    pub fn frame(&self, item: &Arc<crate::market::Outbound>, bin: Price) -> Option<String> {
        let w = Arc::downgrade(item);
        self.frames
            .iter()
            .find(|(i, b, _)| *b == bin && Weak::ptr_eq(i, &w))
            .map(|(_, _, json)| json.clone())
    }

    pub fn put_frame(&mut self, item: &Arc<crate::market::Outbound>, bin: Price, json: String) {
        if self.frames.len() >= CAP {
            self.frames.pop_front();
        }
        self.frames.push_back((Arc::downgrade(item), bin, json));
    }
}

/// The increment's session and touched rows, or `None` for a frame with no profile (tape, halt,
/// LULD).
pub fn profile_of(update: &ChartUpdate) -> Option<(&str, &[ProfileRow])> {
    match update {
        ChartUpdate::Candle(c) => Some((&c.session, &c.profile)),
        ChartUpdate::Remove(c) => Some((&c.session, &c.profile)),
        ChartUpdate::Gap(c) => Some((&c.session, &c.profile)),
        ChartUpdate::Tape(_) | ChartUpdate::Halt(_) | ChartUpdate::Luld(_) => None,
    }
}

/// `update` at `bin`: its touched bins re-binned with absolute totals from `read` (None: the
/// session has no profile any more, every bin 0), and the whole profile's figures attached.
/// With no touched rows (a finalization, an empty gap) only the bin changes and nothing is
/// attached: those frames change no row, so there is nothing for a client to recompute.
pub fn build(update: &ChartUpdate, bin: Price, read: Option<&ProfileRead>) -> ChartUpdate {
    let mut u = update.clone();
    let Some((_, rows)) = profile_of(update) else {
        return u;
    };
    let mut touched: Vec<Price> = rows
        .iter()
        .filter_map(|r| Price::from_f64_rounded(r.p))
        .map(|p| p.floor_to(bin))
        .collect();
    touched.sort();
    touched.dedup();
    let all: &[(Price, u64)] = read.map_or(&[], |r| r.rows.as_slice());
    let new_rows: Vec<ProfileRow> = touched
        .iter()
        .map(|b| ProfileRow {
            p: b.to_f64(),
            v: all
                .binary_search_by_key(b, |(p, _)| *p)
                .map_or(0, |i| all[i].1),
        })
        .collect();
    let derived = (!new_rows.is_empty()).then(|| {
        let d = read.map(|r| r.derived).unwrap_or(Derived {
            total: 0,
            poc: None,
            value_area: None,
        });
        ProfileDerivedWire {
            total: d.total,
            poc: d.poc.map(Price::to_f64),
            value_area: d.value_area.map(|(l, h, v)| ValueArea {
                low: l.to_f64(),
                high: h.to_f64(),
                v,
            }),
        }
    });
    let set = |profile_bin: &mut f64,
               profile: &mut Vec<ProfileRow>,
               d: &mut Option<ProfileDerivedWire>| {
        *profile_bin = bin.to_f64();
        *profile = new_rows.clone();
        *d = derived.clone();
    };
    match &mut u {
        ChartUpdate::Candle(c) => set(&mut c.profile_bin, &mut c.profile, &mut c.derived),
        ChartUpdate::Remove(c) => set(&mut c.profile_bin, &mut c.profile, &mut c.derived),
        ChartUpdate::Gap(c) => set(&mut c.profile_bin, &mut c.profile, &mut c.derived),
        ChartUpdate::Tape(_) | ChartUpdate::Halt(_) | ChartUpdate::Luld(_) => {}
    }
    u
}

#[cfg(test)]
mod tests {
    use super::*;
    use chart_engine::Timeframe;
    use chart_engine::profile::derive;
    use chart_engine::wire::{CandleUpdate, CandleWire};

    fn c(cents: i64) -> Price {
        Price::from_cents(cents)
    }

    fn update(rows: &[(f64, u64)]) -> ChartUpdate {
        ChartUpdate::Candle(CandleUpdate {
            symbol: "SMPL".into(),
            timeframe: Timeframe::M1,
            session: "2026-10-15/regular".into(),
            candle: CandleWire {
                t: 0,
                o: 1.0,
                h: 1.0,
                l: 1.0,
                c: 1.0,
                v: 1,
                n: 1,
                is_final: false,
            },
            profile_bin: 0.01,
            profile: rows.iter().map(|&(p, v)| ProfileRow { p, v }).collect(),
            derived: None,
        })
    }

    fn read(rows: Vec<(Price, u64)>, seq: u64) -> ProfileRead {
        ProfileRead {
            seq,
            derived: derive(&rows, 70),
            rows: Arc::new(rows),
        }
    }

    #[test]
    fn rebins_the_touched_rows_and_attaches_the_whole_profiles_figures() {
        // The whole profile at 5 cents: 1.00 (40), 1.05 (100), 1.10 (10).
        let r = read(vec![(c(100), 40), (c(105), 100), (c(110), 10)], 7);
        let u = build(&update(&[(1.07, 30)]), c(5), Some(&r));
        let ChartUpdate::Candle(cu) = &u else {
            panic!()
        };
        assert_eq!(cu.profile_bin, 0.05);
        assert_eq!(cu.profile, vec![ProfileRow { p: 1.05, v: 100 }]);
        let d = cu.derived.as_ref().unwrap();
        assert_eq!(d.total, 150);
        assert_eq!(d.poc, Some(1.05));
        // 70 % of 150 = 105: 100 at the POC, then 40 below beats 10 above.
        assert_eq!(
            d.value_area,
            Some(ValueArea {
                low: 1.0,
                high: 1.05,
                v: 140
            })
        );
        let v = serde_json::to_value(&u).unwrap();
        assert_eq!(v["total"], 150);
        assert_eq!(v["poc"], 1.05);
        assert_eq!(v["valueArea"]["low"], 1.0);
    }

    #[test]
    fn no_touched_rows_attaches_nothing_and_an_emptied_profile_sends_nulls() {
        let r = read(vec![(c(100), 40)], 1);
        let quiet = build(&update(&[]), c(1), Some(&r));
        let v = serde_json::to_value(&quiet).unwrap();
        assert!(v.get("total").is_none() && v.get("poc").is_none());
        // Cancels emptied the session: the touched bin reads 0, POC and value area null.
        let gone = build(&update(&[(1.0, 0)]), c(1), None);
        let v = serde_json::to_value(&gone).unwrap();
        assert_eq!(v["profile"][0]["v"], 0);
        assert_eq!(v["total"], 0);
        assert!(v["poc"].is_null() && v["valueArea"].is_null());
        // And a client of an older protocol reads the same frame back without the figures.
        let back: ChartUpdate = serde_json::from_value(v).unwrap();
        let ChartUpdate::Candle(cu) = back else {
            panic!()
        };
        assert_eq!(cu.derived.unwrap().total, 0);
    }

    #[test]
    fn a_read_serves_every_frame_up_to_its_seq_only() {
        let mut cache = FrameCache::default();
        let s: SessionId = "2026-10-15/regular".parse().unwrap();
        cache.put_read("SMPL", s, c(1), read(vec![(c(100), 1)], 10));
        assert!(cache.read("SMPL", s, c(1), 10).is_some());
        assert!(cache.read("SMPL", s, c(1), 3).is_some());
        assert!(
            cache.read("SMPL", s, c(1), 11).is_none(),
            "a newer frame reads again"
        );
        assert!(
            cache.read("SMPL", s, c(5), 3).is_none(),
            "another bin is another read"
        );
        assert!(cache.read("OTHR", s, c(1), 3).is_none());
        // A newer read replaces the entry rather than piling up.
        cache.put_read("SMPL", s, c(1), read(vec![(c(100), 2)], 12));
        assert_eq!(cache.reads.len(), 1);
        for i in 0..(CAP + 5) {
            cache.put_read(&format!("S{i}"), s, c(1), read(vec![], 1));
        }
        assert_eq!(cache.reads.len(), CAP);
    }

    #[test]
    fn a_frame_is_reused_for_its_own_item_and_bin_only() {
        use crate::market::{OutKind, Outbound};
        let mut cache = FrameCache::default();
        let a = Arc::new(Outbound {
            seq: 1,
            kind: OutKind::Resync,
        });
        let b = Arc::new(Outbound {
            seq: 1,
            kind: OutKind::Resync,
        });
        cache.put_frame(&a, c(1), "A".into());
        assert_eq!(cache.frame(&a, c(1)).as_deref(), Some("A"));
        assert!(cache.frame(&a, c(5)).is_none());
        assert!(cache.frame(&b, c(1)).is_none(), "same seq, another item");
    }
}
