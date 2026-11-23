//! The HOD Momentum scanner's rules, with no I/O: the per-symbol day state fed by prints
//! ([`Sym::apply`]), the categories a new high of day is checked against ([`evaluate`]), and the
//! policy they read ([`Policy`], `policy-hod-v1.json`). The module docs of `crate::hod` say what
//! is live and what is not; every threshold is in the policy file, none is here.

use serde::Deserialize;

pub const SEC: i64 = 1_000_000_000;
pub const MIN: i64 = 60 * SEC;
/// Clock minutes of history a symbol keeps (the longest window read is 10 minutes, and "change
/// 10 min" looks at the minute 10 minutes back, so 16 leaves room for a quiet minute or five).
pub const RING: usize = 16;

/// The scanner's id in the ledger and the runtime switch (`detectors` settings, `"hod-1"`).
pub const DETECTOR: &str = "hod-1";
/// The policy as shipped: compiled in, sent to every client in the snapshot.
pub const POLICY_JSON: &str = include_str!("policy-hod-v1.json");

/// Category ids in the policy's order; a row's `cats` is a bit mask over this list.
pub const CATS: [&str; 10] = [
    "lf_momo",
    "lf_hrv",
    "lf_hrv_20",
    "lf_mrv",
    "mf_hrv_20",
    "mf_hrv_u20",
    "mf_mrv_20",
    "sq_10_10",
    "sq_5_5",
    "sq_52wk",
];

pub fn cat_bit(id: &str) -> u16 {
    CATS.iter().position(|c| *c == id).map_or(0, |i| 1u16 << i)
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Band {
    pub min: f64,
    pub max: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct FloatCuts {
    /// Low float: strictly under this many shares.
    pub low_max: u64,
    /// Medium float: from `low_max` up to (not including) this.
    pub medium_max: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct RvolCuts {
    /// High rel vol: at least this multiple of the daily rate.
    pub high_min: f64,
    /// Medium rel vol: at least this, under `high_min`.
    pub medium_min: f64,
    /// Prior sessions in the daily denominator (the stock-context engine's 30).
    pub sessions: usize,
    /// What the categories without a rel vol band of their own (former momo, the squeezes)
    /// still need: a stock that has not yet traded a normal day's volume is not a mover.
    pub base_min: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Momo {
    /// A prior session's high at least this far above its previous close (0.3 = +30 %).
    pub run_min: f64,
    pub sessions: usize,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct CategoryDef {
    pub id: String,
    pub name: String,
    pub default_on: bool,
    #[serde(default)]
    pub pct: Option<f64>,
    #[serde(default)]
    pub minutes: Option<i64>,
    #[serde(default)]
    pub needs_history: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Policy {
    pub id: String,
    pub version: u32,
    pub band: Band,
    pub cooldown_secs: i64,
    /// A row needs a day gain strictly above this (0 = up on the day).
    pub min_day_gain: f64,
    pub float: FloatCuts,
    pub rvol: RvolCuts,
    /// "$20+": at least this price (the band's top is $25).
    pub price_high_min: f64,
    pub momo: Momo,
    /// Daily bars the 52-week high needs; fewer and the category is unavailable.
    pub hi52_sessions: usize,
    pub categories: Vec<CategoryDef>,
}

impl Policy {
    pub fn shipped() -> Policy {
        serde_json::from_str(POLICY_JSON).expect("policy-hod-v1.json parses")
    }
}

/// A symbol's daily facts, read once a day from the archive's daily bars (`crate::hod::load_table`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Facts {
    /// The previous close on the day's share basis (a fallback: the quote board's comes first).
    pub c1: Option<f64>,
    /// The mean daily volume of the `rvol.sessions` prior sessions, restated onto the day's share
    /// basis; `None` with fewer sessions (as the stock-context engine's strict denominator).
    pub vol_base: Option<f64>,
    /// The biggest one-session run in the prior `momo.sessions`: max(high / previous close) − 1.
    pub runmax: Option<f64>,
    /// The 52-week high on the day's basis; `None` without `hi52_sessions` daily bars.
    pub hi52: Option<f64>,
}

/// What the market hands the decision, looked up only for a symbol with a new high pending.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Inputs {
    /// The quote board's split-guarded previous close (what the rest of the app's "% up" uses).
    pub c1: Option<f64>,
    /// The stock-context engine's latest public float (shares).
    pub float: Option<u64>,
    /// The normal volume of the 5-minute bucket of the day holding the decision (rw-mp5-1's
    /// 20-session time-of-day baseline), for "rel vol 5 min".
    pub tod5_base: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct MinBar {
    m: i64,
    low: f64,
    close: f64,
    vol: u64,
}

const EMPTY_BAR: MinBar = MinBar {
    m: i64::MIN,
    low: f64::INFINITY,
    close: f64::NAN,
    vol: 0,
};

/// One symbol's day since 04:00 ET.
#[derive(Clone, Debug, PartialEq)]
pub struct Sym {
    /// The session high so far (OHLC-counted prints only).
    pub high: Option<f64>,
    pub first: Option<f64>,
    /// The regular session's first OHLC print.
    pub open: Option<f64>,
    /// Volume-counted shares since 04:00.
    pub vol: u64,
    ring: [MinBar; RING],
    /// When this symbol's last row was emitted (the cooldown).
    pub last_row: Option<i64>,
    /// A new high not decided yet: (ns, price). Only the latest is kept.
    pub pending: Option<(i64, f64)>,
}

impl Default for Sym {
    fn default() -> Self {
        Sym {
            high: None,
            first: None,
            open: None,
            vol: 0,
            ring: [EMPTY_BAR; RING],
            last_row: None,
            pending: None,
        }
    }
}

pub fn minute(t0: i64, ns: i64) -> i64 {
    (ns - t0).div_euclid(MIN)
}

impl Sym {
    /// One print. Returns true when it made a new high of day that is now pending (strictly above
    /// every earlier OHLC print of the day, the symbol's first print never counts, and not within
    /// `cooldown_ns` of the symbol's last row).
    #[allow(clippy::too_many_arguments)]
    pub fn apply(
        &mut self,
        t0: i64,
        ns: i64,
        px: f64,
        size: u64,
        ohlc: bool,
        volume: bool,
        regular: bool,
        cooldown_ns: i64,
    ) -> bool {
        if !ohlc && !volume {
            return false;
        }
        let m = minute(t0, ns);
        let slot = m.rem_euclid(RING as i64) as usize;
        if self.ring[slot].m != m {
            if self.ring[slot].m > m {
                // A late print for a minute the ring has already moved past: it still counts for
                // the day's volume and high, not for the windows.
                if volume {
                    self.vol = self.vol.saturating_add(size);
                }
                if ohlc {
                    self.high = Some(self.high.map_or(px, |h| h.max(px)));
                }
                return false;
            }
            self.ring[slot] = MinBar { m, ..EMPTY_BAR };
        }
        let bar = &mut self.ring[slot];
        if volume {
            self.vol = self.vol.saturating_add(size);
            bar.vol = bar.vol.saturating_add(size);
        }
        if !ohlc {
            return false;
        }
        bar.low = bar.low.min(px);
        bar.close = px;
        self.first.get_or_insert(px);
        if regular && self.open.is_none() {
            self.open = Some(px);
        }
        let mut fired = false;
        if let Some(h) = self.high
            && px > h
            && self.last_row.is_none_or(|l| ns - l >= cooldown_ns)
        {
            fired = self.pending.is_none();
            self.pending = Some((ns, px));
        }
        self.high = Some(self.high.map_or(px, |h| h.max(px)));
        fired
    }

    fn bars(&self, m_now: i64, minutes: i64) -> impl Iterator<Item = &MinBar> {
        self.ring
            .iter()
            .filter(move |b| b.m != i64::MIN && b.m <= m_now && b.m > m_now - minutes)
    }

    /// The lowest OHLC print in the current and previous `minutes − 1` clock minutes.
    pub fn low_within(&self, m_now: i64, minutes: i64) -> Option<f64> {
        let l = self
            .bars(m_now, minutes)
            .map(|b| b.low)
            .fold(f64::INFINITY, f64::min);
        l.is_finite().then_some(l)
    }

    /// Volume-counted shares in the current and previous `minutes − 1` clock minutes.
    pub fn vol_within(&self, m_now: i64, minutes: i64) -> u64 {
        self.bars(m_now, minutes).map(|b| b.vol).sum()
    }

    /// The last price at the end of clock minute `m` (or the latest earlier minute the ring
    /// still holds that traded).
    pub fn close_at(&self, m_now: i64, m: i64) -> Option<f64> {
        self.ring
            .iter()
            .filter(|b| {
                b.m != i64::MIN && b.m <= m && b.m > m_now - RING as i64 && b.close.is_finite()
            })
            .max_by_key(|b| b.m)
            .map(|b| b.close)
    }
}

/// One scanner row.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: u64,
    pub ns: i64,
    pub sym: String,
    pub price: f64,
    /// Shares since 04:00.
    pub vol: u64,
    pub float: Option<u64>,
    /// Rel vol, daily rate: `vol` over the mean daily volume.
    pub rvol: Option<f64>,
    /// Rel vol 5 min: the last 5 minutes' shares over the normal shares of this 5-minute bucket.
    pub rvol5: Option<f64>,
    /// Gap: the regular open (before 09:30 the day's first print) over the previous close, − 1.
    pub gap: Option<f64>,
    /// Change from close.
    pub chg: f64,
    pub chg5: Option<f64>,
    pub chg10: Option<f64>,
    pub cats: u16,
    /// The facts behind the tags, for the ledger.
    pub c1: f64,
    pub vol_base: Option<f64>,
    pub runmax: Option<f64>,
}

impl Row {
    pub fn cat_ids(&self) -> Vec<&'static str> {
        CATS.iter()
            .enumerate()
            .filter(|(i, _)| self.cats & (1 << i) != 0)
            .map(|(_, c)| *c)
            .collect()
    }
}

/// Which categories a new high of day at (`ns`, `px`) matches; `None` when the row is not
/// emitted (outside the band, no previous close, not up on the day, or no category).
#[allow(clippy::too_many_arguments)]
pub fn evaluate(
    p: &Policy,
    sym_name: &str,
    s: &Sym,
    t0: i64,
    ns: i64,
    px: f64,
    facts: &Facts,
    inp: &Inputs,
) -> Option<Row> {
    if !(px >= p.band.min && px <= p.band.max) {
        return None;
    }
    let c1 = inp.c1.or(facts.c1).filter(|c| *c > 0.0)?;
    let chg = px / c1 - 1.0;
    // NaN never passes.
    if chg.is_nan() || chg <= p.min_day_gain {
        return None;
    }
    let rvol = facts
        .vol_base
        .filter(|b| *b > 0.0)
        .map(|b| s.vol as f64 / b);
    let m = minute(t0, ns);
    let r = |x: Option<f64>, min: f64| x.is_some_and(|v| v >= min);
    let high = r(rvol, p.rvol.high_min);
    let med = r(rvol, p.rvol.medium_min) && !high;
    let base = r(rvol, p.rvol.base_min);
    let low_f = inp.float.is_some_and(|f| f < p.float.low_max);
    let med_f = inp
        .float
        .is_some_and(|f| f >= p.float.low_max && f < p.float.medium_max);
    let p20 = px >= p.price_high_min;
    let mut cats = 0u16;
    let mut set = |id: &str, on: bool| {
        if on {
            cats |= cat_bit(id);
        }
    };
    set(
        "lf_momo",
        low_f && base && facts.runmax.is_some_and(|x| x >= p.momo.run_min),
    );
    set("lf_hrv", low_f && high);
    set("lf_hrv_20", low_f && high && p20);
    set("lf_mrv", low_f && med);
    set("mf_hrv_20", med_f && high && p20);
    set("mf_hrv_u20", med_f && high && !p20);
    set("mf_mrv_20", med_f && med && p20);
    for c in &p.categories {
        if let (Some(pct), Some(minutes)) = (c.pct, c.minutes) {
            let up = s
                .low_within(m, minutes)
                .is_some_and(|l| px >= (1.0 + pct) * l);
            set(&c.id, base && up);
        }
    }
    set("sq_52wk", base && facts.hi52.is_some_and(|h| px > h));
    if cats == 0 {
        return None;
    }
    let chg_back = |k: i64| {
        s.close_at(m, m - k)
            .filter(|c| *c > 0.0)
            .map(|c| px / c - 1.0)
    };
    Some(Row {
        id: 0,
        ns,
        sym: sym_name.to_string(),
        price: px,
        vol: s.vol,
        float: inp.float,
        rvol,
        rvol5: inp
            .tod5_base
            .filter(|b| *b > 0.0)
            .map(|b| s.vol_within(m, 5) as f64 / b),
        gap: s.open.or(s.first).map(|o| o / c1 - 1.0),
        chg,
        chg5: chg_back(5),
        chg10: chg_back(10),
        cats,
        c1,
        vol_base: facts.vol_base,
        runmax: facts.runmax,
    })
}

/// A symbol's daily bar for the facts (`d` = days since CE, any monotone day number).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DailyBar {
    pub d: i32,
    pub h: Option<f64>,
    pub c: f64,
    pub v: Option<u64>,
}

/// The split factor (old/new) of every split effective in `(from, to]`: a price on `from`'s
/// basis × this = the price on `to`'s basis; a volume ÷ it.
pub fn split_factor(splits: &[(i32, f64)], from: i32, to: i32) -> f64 {
    splits
        .iter()
        .filter(|(d, _)| *d > from && *d <= to)
        .map(|(_, f)| *f)
        .product()
}

/// [`Facts`] from a symbol's daily bars strictly before `day`, oldest first.
pub fn facts_of(p: &Policy, bars: &[DailyBar], splits: &[(i32, f64)], day: i32) -> Facts {
    let Some(last) = bars.last() else {
        return Facts::default();
    };
    let c1 = Some(last.c * split_factor(splits, last.d, day)).filter(|c| *c > 0.0);
    let n = p.rvol.sessions;
    let vols: Vec<f64> = bars
        .iter()
        .rev()
        .filter_map(|b| b.v.map(|v| v as f64 / split_factor(splits, b.d, day)))
        .take(n)
        .collect();
    let vol_base = (n > 0 && vols.len() == n).then(|| vols.iter().sum::<f64>() / n as f64);
    let lo = bars.len().saturating_sub(p.momo.sessions);
    let mut runmax: Option<f64> = None;
    for i in lo.max(1)..bars.len() {
        let (a, b) = (&bars[i - 1], &bars[i]);
        let pc = a.c * split_factor(splits, a.d, b.d);
        if let Some(h) = b.h
            && pc > 0.0
        {
            let run = h / pc - 1.0;
            runmax = Some(runmax.map_or(run, |r| r.max(run)));
        }
    }
    let hi52 = (p.hi52_sessions > 0 && bars.len() >= p.hi52_sessions)
        .then(|| {
            bars[bars.len() - p.hi52_sessions..]
                .iter()
                .filter_map(|b| b.h.map(|h| h * split_factor(splits, b.d, day)))
                .fold(f64::NEG_INFINITY, f64::max)
        })
        .filter(|h| h.is_finite());
    Facts {
        c1,
        vol_base,
        runmax,
        hi52,
    }
}
