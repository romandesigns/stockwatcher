//! Pullback-context study (docs/research/pullback-context/protocol.md): one row per armed MP5
//! episode with its levels, its crossing and the protocol's outcome, scored from the symbol-day's
//! chart-counted prints. Gates and budgets are applied later, offline.

use crate::replay::{Print, SEC, Track, in_band, session_of};
use serde::Serialize;

pub const HORIZON: i64 = 900 * SEC;

#[derive(Serialize)]
pub struct Row {
    pub day: String,
    pub sym: String,
    pub ep: String,
    pub armed_t: i64,
    pub session: &'static str,
    pub trigger: i64,
    pub stop: Option<i64>,
    pub impulse_bp: Option<i64>,
    pub depth_milli: Option<i64>,
    /// Counted shares in completed pre/regular bars at arming.
    pub armed_vol: Option<u64>,
    /// Latest OHLC print admitted by the arming.
    pub last_at_arm: Option<i64>,
    pub in_band: bool,
    pub cross_t: Option<i64>,
    pub cross_px: Option<i64>,
    /// `continuation`, `failure`, `timeout`, or `no_cross`.
    pub outcome: &'static str,
    pub t_res: Option<i64>,
    /// From the trigger, over 15 minutes from the crossing (or the arming if it never crossed).
    pub mfe_bp: Option<i64>,
    pub mae_bp: Option<i64>,
}

fn bp(p: i64, base: i64) -> i64 {
    ((p - base) as i128 * 10_000 / base as i128) as i64
}

pub fn rows(day: &str, sym: &str, tracks: &[(String, Track)], prints: &[Print]) -> Vec<Row> {
    let mut out = Vec::new();
    for (id, t) in tracks {
        let (Some(armed), Some(trig)) = (t.armed, t.trigger) else {
            continue;
        };
        if trig <= 0 {
            continue;
        }
        let last_at_arm = prints.iter().rev().find(|p| p.admit <= armed).map(|p| p.price);
        let from = t.cross.map_or(armed, |(c, _)| c);
        let up = trig as i128 * 105 / 100;
        let dn = trig as i128 * 97 / 100;
        let mut outcome = if t.cross.is_some() { "timeout" } else { "no_cross" };
        let mut t_res = None;
        let (mut hi, mut lo) = (i64::MIN, i64::MAX);
        // Prints that arrived after the decision, in execution order, within the horizon.
        for p in prints.iter().filter(|p| p.admit > from && p.exec <= from + HORIZON) {
            hi = hi.max(p.price);
            lo = lo.min(p.price);
            if t.cross.is_some() && t_res.is_none() {
                if p.price as i128 <= dn {
                    outcome = "failure";
                    t_res = Some(p.exec);
                } else if p.price as i128 >= up {
                    outcome = "continuation";
                    t_res = Some(p.exec);
                }
            }
        }
        out.push(Row {
            day: day.to_string(),
            sym: sym.to_string(),
            ep: id.clone(),
            armed_t: armed,
            session: session_of(armed),
            trigger: trig,
            stop: t.stop,
            impulse_bp: t.impulse_bp,
            depth_milli: t.depth_milli,
            armed_vol: t.armed_vol.map(|v| v.min(u64::MAX as u128) as u64),
            last_at_arm,
            in_band: in_band(trig),
            cross_t: t.cross.map(|c| c.0),
            cross_px: t.cross.map(|c| c.1),
            outcome,
            t_res,
            mfe_bp: (hi != i64::MIN).then(|| bp(hi, trig)),
            mae_bp: (lo != i64::MAX).then(|| bp(lo, trig)),
        });
    }
    out
}
