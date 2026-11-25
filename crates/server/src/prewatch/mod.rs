//! **Pre-open Watch card** (owner, 2026-11-25, after AKAN 11-24): a quiet card, 04:00–09:30 ET,
//! for a stock that looks like AKAN did before its open, with the reasons it matched. **No sound
//! and no push**: it is a list to look at before the bell, never a notification (nothing here
//! reaches the alerts task).
//!
//! # The profile (fixed before testing: `outputs/stockwatcher-research-v2/70-akan/PROFILE-A-PRIORI.md`)
//!
//! Profile **B**, every condition on what is known at the moment it is checked:
//!
//! | | Rule | Source here |
//! |---|---|---|
//! | P1 | last pre-market price $1–$20 | the stock's last counted print since 04:00 |
//! | P2 | float at most 10 M shares | the stock-context engine's float (`ContextBoard::float_shares`, FMP) |
//! | P3 | at least +15 % over the prior regular close | the quote board's split-guarded previous close, else the daily-bars close |
//! | P4 | pre-market volume (04:00 on) at least 50 % of the float | the volume-counted prints since 04:00 |
//! | P5 | a day in the previous 10 sessions whose high was +50 % over its prior close | `daily-bars/raw/`, split-restated (`crate::gap_ctx::logic::recent_big_day`) |
//!
//! # What it is worth (Train 2021-10-29..2024-06-28, `70-akan/RESULTS.md`)
//!
//! 453 matches, 0.68 a day: 21.6 % ran +30 % from the 09:30 open by 11:00, against 10.4 % for
//! every $1–$20 gapper of +15 % (the baseline); but 64 % were below their open at 11:00. So the
//! card informs and never says "buy"; a confirmed-runner alert built on it failed its own Train
//! gate (`70-akan/CONFIRMED-A-PRIORI.md`) and is not shipped. Validation was not run on the
//! profile itself: it is a hypothesis, which is one more reason it rings nothing.
//!
//! # Known differences from the research
//!
//! - **Float.** The research used the SEC-derived `float_ex_insiders` (not point-in-time either);
//!   live has only FMP's latest float, whose median differs by ×1.14
//!   (`crate::xfloat_live`). Without `STOCK_CONTEXT` there is no float and no card: it says so
//!   rather than guessing (the snapshot's `float_source`).
//! - **Time.** The research checked once at 09:29; the card checks every second from 04:00, so it
//!   can appear early. A card stays for the morning once it appeared and says when a condition
//!   stops holding (`fails`), so a stock that faded before the bell reads as faded.
//! - **A restart after 04:00** loses the earlier prints: the volume is short, so the card can
//!   only be late, never wrong in the stock's favour; it is marked `partial`.
//!
//! # Traffic
//!
//! `prewatch.subscribe` → `prewatch.snapshot` (every card), again only when a card changed,
//! decided on the Runners board's tick, never per print. A handful of cards a day.
//!
//! # Switch
//!
//! `PREWATCH=0` (env, a restart) removes it; on by default for the live market with
//! `ROCKET_WATCH` and the x-gap facts table (`GAP_CTX_LIVE`), which carries P5.

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap};

use chrono::NaiveDate;
use market_events::session::{SessionKind, et};
use market_events::{Trade, UnixNanos};
use serde_json::{Value, json};

use crate::quotes::Counted;

/// The welcome feature a client checks before sending `prewatch.subscribe`.
pub const FEATURE: &str = "prewatch.v1";

/// P1: the price band, dollars.
pub const PRICE_MIN: f64 = 1.0;
pub const PRICE_MAX: f64 = 20.0;
/// P2: the float ceiling, shares.
pub const FLOAT_MAX: f64 = 10_000_000.0;
/// A float below this is a data error, not a float.
pub const FLOAT_MIN: f64 = 10_000.0;
/// P3: the gap over the prior close.
pub const GAP_MIN: f64 = 0.15;
/// P4: pre-market volume as a share of the float.
pub const PM_FLOAT_MIN: f64 = 0.5;
/// How long after 09:30 the morning's cards stay (the run AKAN made started at the open).
pub const KEEP_AFTER_OPEN_MS: i64 = 15 * 60_000;
/// Cards kept at most (about one a day on Train; this only bounds a broken feed).
pub const MAX_CARDS: usize = 32;

/// What a symbol is checked against, read when it is checked.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Inputs {
    pub float: Option<f64>,
    pub c1: Option<f64>,
    /// P5: the most recent +50 % day of the last 10 sessions, (date, run).
    pub big_day: Option<(NaiveDate, f64)>,
}

/// One condition that does not hold, as the wire spells it.
pub type Fail = &'static str;

/// The profile on one symbol's pre-market so far: every condition that fails (empty = a match).
/// A missing input fails its own condition (`float`, `c1`, `big_day`), so the card never claims a
/// condition nobody could check.
pub fn check(price: f64, pm_vol: u64, inp: &Inputs) -> Vec<Fail> {
    let mut out = Vec::new();
    if !(PRICE_MIN..=PRICE_MAX).contains(&price) {
        out.push("price");
    }
    match inp.float {
        Some(f) if f.is_finite() && f >= FLOAT_MIN => {
            if f > FLOAT_MAX {
                out.push("float");
            }
            if (pm_vol as f64) < PM_FLOAT_MIN * f {
                out.push("volume");
            }
        }
        _ => {
            out.push("float");
            out.push("volume");
        }
    }
    match inp.c1 {
        Some(c) if c > 0.0 && price / c - 1.0 >= GAP_MIN => {}
        _ => out.push("gap"),
    }
    if inp.big_day.is_none() {
        out.push("big_day");
    }
    out
}

/// One stock's pre-market since 04:00 (or since the server started, `partial`).
#[derive(Clone, Copy, Debug, Default)]
struct Acc {
    vol: u64,
    /// The last counted print's price (dollars) and execution time (epoch ns).
    px: f64,
    px_ns: i64,
    dirty: bool,
}

/// One card, as the wire carries it.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub symbol: String,
    /// When it first matched, epoch ms.
    pub since: i64,
    /// The values now (frozen at 09:30).
    pub price: f64,
    pub c1: f64,
    pub gap: f64,
    pub pm_vol: u64,
    pub float: f64,
    pub big_day: (NaiveDate, f64),
    /// Conditions that no longer hold (empty: still matches).
    pub fails: Vec<Fail>,
    pub partial: bool,
}

impl Card {
    fn wire(&self) -> Value {
        json!({
            "symbol": self.symbol,
            "since": self.since,
            "price": round(self.price, 4),
            "c1": round(self.c1, 4),
            "gap": round(self.gap, 4),
            "pmVol": self.pm_vol,
            "float": self.float.round() as u64,
            "pmVolPct": round(self.pm_vol as f64 / self.float, 3),
            "bigDay": { "date": self.big_day.0.to_string(), "run": round(self.big_day.1, 3) },
            "fails": self.fails,
            "partial": self.partial,
        })
    }

    /// What the clients see, rounded as they show it: a change below that is no frame.
    fn shown(&self) -> (i64, i64, i64, &[Fail]) {
        (
            (self.price * 100.0).round() as i64,
            (self.gap * 1000.0).round() as i64,
            (self.pm_vol as f64 / self.float * 100.0).round() as i64,
            &self.fails,
        )
    }
}

fn round(x: f64, dp: i32) -> f64 {
    let m = 10f64.powi(dp);
    (x * m).round() / m
}

/// The trading date's pre-market window, epoch ns: 04:00 and 09:30 ET.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Day {
    date: NaiveDate,
    t0: i64,
    open: i64,
}

impl Day {
    fn of(date: NaiveDate) -> Option<Self> {
        Some(Day {
            date,
            t0: et(date, 4, 0, 0, 0)?.0,
            open: et(date, 9, 30, 0, 0)?.0,
        })
    }
}

/// The board: fed every print the Runners board counts, checked on its tick.
#[derive(Debug, Default)]
pub struct WatchBoard {
    enabled: bool,
    day: Option<Day>,
    /// The first print this process saw for the day: after 04:00 the volume is short.
    started_ns: Option<i64>,
    syms: HashMap<String, Acc>,
    cards: BTreeMap<String, Card>,
    changed: bool,
    float_source: bool,
}

impl WatchBoard {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enable(&mut self) {
        self.enabled = true;
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Starts a new trading date (and clears the last one's cards).
    fn roll(&mut self, date: NaiveDate, ts: i64) -> bool {
        if self.day.is_some_and(|d| d.date == date) {
            return true;
        }
        let Some(d) = Day::of(date) else { return false };
        self.day = Some(d);
        self.started_ns = Some(ts);
        self.syms.clear();
        if !self.cards.is_empty() {
            self.cards.clear();
            self.changed = true;
        }
        true
    }

    /// One print the Runners board counted. O(1).
    pub fn apply_trade(&mut self, t: &Trade, counted: Counted) {
        if !self.enabled || !(counted.ohlc || counted.volume) {
            return;
        }
        let Some(s) = t.session else { return };
        if s.kind != SessionKind::Pre {
            return;
        }
        let ts = t.event_ts.0;
        if !self.roll(s.date, ts) {
            return;
        }
        let d = self.day.expect("rolled");
        if ts < d.t0 || ts >= d.open {
            return;
        }
        let a = self.syms.entry(t.symbol.to_string()).or_default();
        if counted.volume {
            a.vol = a.vol.saturating_add(t.size);
        }
        if counted.ohlc && ts >= a.px_ns {
            a.px = t.price.to_f64();
            a.px_ns = ts;
        }
        a.dirty = true;
    }

    /// Checks every stock that printed since the last tick (`now` epoch ns); clears the morning
    /// [`KEEP_AFTER_OPEN_MS`] after 09:30. `inputs` is read only for a stock in the price band.
    pub fn tick(&mut self, now: i64, float_source: bool, mut inputs: impl FnMut(&str) -> Inputs) {
        if !self.enabled {
            return;
        }
        if self.float_source != float_source {
            self.float_source = float_source;
            self.changed = true;
        }
        let Some(d) = self.day else { return };
        if now >= d.open + KEEP_AFTER_OPEN_MS * 1_000_000 {
            if !self.cards.is_empty() {
                self.cards.clear();
                self.changed = true;
            }
            self.syms.clear();
            return;
        }
        let partial = self.started_ns.is_some_and(|s| s > d.t0 + 60_000_000_000);
        for (sym, a) in self.syms.iter_mut() {
            if !a.dirty {
                continue;
            }
            a.dirty = false;
            // A stock with no card outside the band needs nothing read.
            if !self.cards.contains_key(sym) && !(PRICE_MIN..=PRICE_MAX).contains(&a.px) {
                continue;
            }
            let inp = inputs(sym);
            let fails = check(a.px, a.vol, &inp);
            let gap = inp.c1.map_or(0.0, |c| a.px / c - 1.0);
            let room = self.cards.len() < MAX_CARDS;
            match self.cards.get_mut(sym) {
                Some(c) => {
                    let before = (c.shown().0, c.shown().1, c.shown().2, c.fails.clone());
                    c.price = a.px;
                    c.pm_vol = a.vol;
                    if let Some(c1) = inp.c1 {
                        c.c1 = c1;
                        c.gap = gap;
                    }
                    if let Some(f) = inp.float.filter(|f| *f >= FLOAT_MIN) {
                        c.float = f;
                    }
                    c.fails = fails;
                    let after = c.shown();
                    if (before.0, before.1, before.2, before.3.as_slice()) != after {
                        self.changed = true;
                    }
                }
                None if fails.is_empty() && room => {
                    let (Some(float), Some(c1), Some(big_day)) = (inp.float, inp.c1, inp.big_day)
                    else {
                        continue;
                    };
                    self.cards.insert(
                        sym.clone(),
                        Card {
                            symbol: sym.clone(),
                            since: now / 1_000_000,
                            price: a.px,
                            c1,
                            gap,
                            pm_vol: a.vol,
                            float,
                            big_day,
                            fails: Vec::new(),
                            partial,
                        },
                    );
                    self.changed = true;
                }
                None => {}
            }
        }
    }

    /// The cards, newest match first.
    pub fn cards(&self) -> Vec<&Card> {
        let mut v: Vec<&Card> = self.cards.values().collect();
        v.sort_by(|a, b| b.since.cmp(&a.since).then(a.symbol.cmp(&b.symbol)));
        v
    }

    /// The `prewatch.snapshot` frame: every card.
    pub fn snapshot_frame(&self) -> String {
        json!({
            "type": "prewatch.snapshot",
            "enabled": self.enabled,
            "date": self.day.map(|d| d.date.to_string()),
            "floatSource": if self.float_source { "fmp" } else { "none" },
            "cards": self.cards().into_iter().map(Card::wire).collect::<Vec<_>>(),
        })
        .to_string()
    }

    /// A snapshot when a card changed since the last one.
    pub fn take_update(&mut self) -> Option<String> {
        std::mem::take(&mut self.changed).then(|| self.snapshot_frame())
    }
}

/// The ET day number (`chrono` days from CE) as a date.
pub fn date_of_day_num(n: i32) -> Option<NaiveDate> {
    NaiveDate::from_num_days_from_ce_opt(n)
}

/// Epoch ns of a session instant (tests).
#[allow(dead_code)]
pub(crate) fn at(date: NaiveDate, h: u32, m: u32) -> UnixNanos {
    et(date, h, m, 0, 0).expect("valid ET time")
}
