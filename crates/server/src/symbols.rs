//! The price-band candidate list pushed over the socket (`symbols.v1`): `symbols.snapshot` once
//! per subscribe, then `symbols.update` with what entered and left whenever the set changes.
//!
//! Why: the web and phone symbol pickers polled `GET /api/symbols` every 10 s per client, so a
//! stock that entered the band showed up 0–10 s late and an idle client re-downloaded the whole
//! list (thousands of symbols live) six times a minute. The server already knows when the set
//! changes; the 1 Hz broadcast tick now checks and pushes only the difference. `/api/symbols`
//! stays, as the clients' first probe (it also tells a client which source it is on before the
//! socket opens) and for older builds, which keep polling: they never send `symbols.subscribe`.
//!
//! Frames (all fields of `/api/symbols` besides `symbols` are the "meta": `mode`, `status`,
//! `source`, `day`, `fixture`, `priceBand`, `screened`, `session`):
//!
//! - `symbols.snapshot {rev, symbols, ...meta}`: the whole sorted list. Replaces the client's.
//! - `symbols.update {rev, added, removed, ...meta}`: `added`/`removed` sorted; the meta whole.
//!   Sent only when the set or the meta changed. Apply to a list at `rev - 1`; any other `rev`
//!   means a frame was missed: subscribe again (the answer is a fresh snapshot).
//!
//! One published baseline per market, shared by every connection, so `rev` is a single counter:
//! a subscribe takes the snapshot from the same baseline the updates are diffed against (after
//! bringing it up to date, which may itself publish an update), and a connection forwards only
//! updates with a larger `rev` than its snapshot. A seek or a lag resends the snapshot.

use serde_json::{Map, Value, json};

/// The feature a client must see in `welcome.features` before it sends `symbols.subscribe`.
pub const FEATURE: &str = "symbols.v1";

/// The ticks between reads of the live `session` (it reads the archive's manifest for the day,
/// file IO kept off the 1 Hz path; it changes a few times a day). 10 s, the old poll's period.
pub const SESSION_EVERY_TICKS: u64 = 10;

/// What the last frame published (module docs).
#[derive(Debug, Default)]
pub struct Published {
    pub rev: u64,
    /// Sorted.
    pub symbols: Vec<String>,
    /// `/api/symbols` without `symbols`; `Value::Null` before the first refresh.
    pub meta: Value,
}

/// What entered (`added`) and left (`removed`) between two sorted lists, both sorted.
pub fn diff(old: &[String], new: &[String]) -> (Vec<String>, Vec<String>) {
    let (mut added, mut removed) = (Vec::new(), Vec::new());
    let (mut i, mut j) = (0, 0);
    while i < old.len() || j < new.len() {
        match (old.get(i), new.get(j)) {
            (Some(a), Some(b)) if a == b => {
                i += 1;
                j += 1;
            }
            (Some(a), Some(b)) if a < b => {
                removed.push(a.clone());
                i += 1;
            }
            (Some(_), Some(b)) | (None, Some(b)) => {
                added.push(b.clone());
                j += 1;
            }
            (Some(a), None) => {
                removed.push(a.clone());
                i += 1;
            }
            (None, None) => break,
        }
    }
    (added, removed)
}

/// `type` goes first, as in every other frame: the web feed routes frames by their leading
/// `{"type":"symbols.` without parsing them (serde_json's map would sort it after `day`).
fn frame(kind: &str, rev: u64, meta: &Value, extra: Vec<(&str, Value)>) -> String {
    let mut m = Map::new();
    m.insert("rev".into(), json!(rev));
    for (k, v) in extra {
        m.insert(k.into(), v);
    }
    if let Value::Object(meta) = meta {
        for (k, v) in meta {
            m.insert(k.clone(), v.clone());
        }
    }
    let rest = Value::Object(m).to_string();
    // `rest` is `{"day":...}`: never empty, it always holds `rev`.
    format!("{{\"type\":{},{}", json!(kind), &rest[1..])
}

/// `symbols.snapshot` of `p`.
pub fn snapshot_frame(p: &Published) -> String {
    frame(
        "symbols.snapshot",
        p.rev,
        &p.meta,
        vec![("symbols", json!(p.symbols))],
    )
}

/// Brings `p` to (`symbols`, `meta`); the `symbols.update` frame when anything changed.
pub fn advance(p: &mut Published, symbols: Vec<String>, meta: Value) -> Option<String> {
    if symbols == p.symbols && meta == p.meta {
        return None;
    }
    let (added, removed) = diff(&p.symbols, &symbols);
    p.rev += 1;
    p.symbols = symbols;
    p.meta = meta;
    Some(frame(
        "symbols.update",
        p.rev,
        &p.meta,
        vec![("added", json!(added)), ("removed", json!(removed))],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn diff_of_sorted_lists() {
        let (a, r) = diff(&v(&["A", "C", "D"]), &v(&["B", "C", "E"]));
        assert_eq!((a, r), (v(&["B", "E"]), v(&["A", "D"])));
        assert_eq!(diff(&[], &v(&["A"])), (v(&["A"]), vec![]));
        assert_eq!(diff(&v(&["A"]), &[]), (vec![], v(&["A"])));
        assert_eq!(diff(&v(&["A"]), &v(&["A"])), (vec![], vec![]));
    }

    #[test]
    fn updates_only_on_change_and_apply_to_the_previous_rev() {
        let mut p = Published::default();
        let meta = json!({"mode": "replay", "screened": {"total": 3, "candidates": 2}});
        let f = advance(&mut p, v(&["A", "B"]), meta.clone()).unwrap();
        let f: Value = serde_json::from_str(&f).unwrap();
        assert_eq!(f["type"], "symbols.update");
        assert_eq!(f["rev"], 1);
        assert!(snapshot_frame(&p).starts_with(r#"{"type":"symbols.snapshot","#));
        assert_eq!(f["added"], json!(["A", "B"]));
        assert_eq!(f["mode"], "replay");
        assert!(advance(&mut p, v(&["A", "B"]), meta.clone()).is_none());
        // A meta change alone (a new symbol screened out of band) is still an update.
        let meta2 = json!({"mode": "replay", "screened": {"total": 4, "candidates": 2}});
        let f: Value =
            serde_json::from_str(&advance(&mut p, v(&["A", "B"]), meta2).unwrap()).unwrap();
        assert_eq!(
            (f["rev"].as_u64(), f["added"].clone()),
            (Some(2), json!([]))
        );
        let s: Value = serde_json::from_str(&snapshot_frame(&p)).unwrap();
        assert_eq!(s["type"], "symbols.snapshot");
        assert_eq!(s["rev"], 2);
        assert_eq!(s["symbols"], json!(["A", "B"]));
        assert_eq!(s["screened"]["total"], 4);
        assert!(s.get("removed").is_none());
    }
}
