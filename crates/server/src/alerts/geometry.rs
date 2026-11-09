//! Drawing alerts' geometry: the price a user drawing stands at, at a given candle, computed
//! exactly as the chart draws it (owner request 2026-11-09, "If the price line merges with the
//! drawing line…"; docs/architecture/chart-packages.md "Drawing alerts").
//!
//! The TypeScript twin is `packages/chart-core/src/drawingAlerts.ts` (`drawingLevelAt`), which
//! the chart's cue uses. Both are pinned to the same golden cases in
//! `drawing_alert_fixtures.json` (this module's tests and `drawingAlerts.test.ts` read the same
//! file), so a sloped line can never trigger at a price other than the one drawn.
//!
//! # The x axis is slots, not time
//!
//! The chart's x axis is a slot index (chart-core `scale.ts`): every candle and every gap run
//! takes ONE slot, however long the gap, and the time between sessions takes none. A trend line
//! is straight in slot space, so its price at a candle is
//! `a.p + (b.p - a.p) × (i - ia) / (ib - ia)` with `i`, `ia`, `ib` the fractional slot indexes
//! of the candle and the two anchors ([`t_to_index`], a port of chart-core `tToIndex`, including
//! its off-the-ends rule: one slot per timeframe interval before the first slot and after the
//! last). In time, the same line bends at every gap, which is why the server cannot just
//! interpolate on the clock.
//!
//! The slot list is the chart's for the alert's timeframe with extended hours shown: the
//! engine's candles and gap runs ([`build_slots`], a port of chart-core `buildSlots`), the
//! forming candle included. The candle a trade lands in is the slot at the start of its
//! interval, so the line's price "at that trade" is the price the chart draws at that candle's
//! centre, where the person sees the price meet the line.
//!
//! **Approximation, documented:** the server holds the engine's retained sessions (four per
//! timeframe, `EngineConfig::sessions_retained`), the chart may hold older history fetched from
//! the archive. An anchor older than the server's first slot is placed by the off-the-ends rule
//! (one slot per interval) instead of by the real slots, so a trend line anchored more than about
//! four sessions back can stand a little off what a chart with that history loaded draws. A chart
//! without that history loaded places it the same way the server does. Horizontal lines and the
//! position tool's levels are flat and exact whatever the slots.

use serde_json::Value;

/// Which level of a drawing an alert watches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    /// The line itself (hline, trend, ray).
    Line,
    /// A position's entry (`a.p`), target (`b.p`) and stop (`c.p`, else the entry).
    Entry,
    Target,
    Stop,
}

impl Part {
    pub fn parse(v: Option<&Value>) -> Part {
        match v.and_then(Value::as_str) {
            Some("entry") => Part::Entry,
            Some("target") => Part::Target,
            Some("stop") => Part::Stop,
            _ => Part::Line,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Part::Line => "line",
            Part::Entry => "entry",
            Part::Target => "target",
            Part::Stop => "stop",
        }
    }
}

/// One anchor: epoch ms and price (chart-core `Anchor`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    pub t: f64,
    pub p: f64,
}

/// The drawings an alert can watch in v1. Rectangles, supply/demand zones and vertical lines
/// are the documented second pass (chart-packages.md "Drawing alerts").
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    HLine {
        p: f64,
    },
    /// Active only between its two anchors.
    Trend {
        a: Anchor,
        b: Anchor,
    },
    /// From `a` through `b` and beyond.
    Ray {
        a: Anchor,
        b: Anchor,
    },
    Position {
        short: bool,
        entry: f64,
        target: f64,
        stop: f64,
    },
}

impl Shape {
    /// The drawing's own name for a level, for the alert's text ("Long stop", "Trend line").
    pub fn label(&self, part: Part) -> &'static str {
        match (self, part) {
            (Shape::HLine { .. }, _) => "line",
            (Shape::Trend { .. }, _) => "trend line",
            (Shape::Ray { .. }, _) => "ray",
            (Shape::Position { short: false, .. }, Part::Target) => "long target",
            (Shape::Position { short: false, .. }, Part::Stop) => "long stop",
            (Shape::Position { short: false, .. }, _) => "long entry",
            (Shape::Position { short: true, .. }, Part::Target) => "short target",
            (Shape::Position { short: true, .. }, Part::Stop) => "short stop",
            (Shape::Position { short: true, .. }, _) => "short entry",
        }
    }

    /// Whether the level depends on the slots (a sloped line). Flat ones never ask for them.
    pub fn sloped(&self) -> bool {
        matches!(self, Shape::Trend { .. } | Shape::Ray { .. })
    }
}

fn finite(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64).filter(|x| x.is_finite())
}

fn anchor(v: Option<&Value>) -> Option<Anchor> {
    let o = v?.as_object()?;
    Some(Anchor {
        t: finite(o.get("t"))?,
        p: finite(o.get("p"))?,
    })
}

/// One stored drawing (`drawings:<SYMBOL>` entry's `d`, chart-core `parseDrawing`) as a shape an
/// alert can watch. None for a malformed drawing or a kind v1 does not watch.
pub fn parse_shape(d: &Value) -> Option<Shape> {
    let o = d.as_object()?;
    let a = anchor(o.get("a"))?;
    let b = anchor(o.get("b"))?;
    match o.get("kind")?.as_str()? {
        "hline" => Some(Shape::HLine { p: a.p }),
        "trend" => Some(Shape::Trend { a, b }),
        "ray" => Some(Shape::Ray { a, b }),
        k @ ("long" | "short") => Some(Shape::Position {
            short: k == "short",
            entry: a.p,
            target: b.p,
            stop: anchor(o.get("c")).map_or(a.p, |c| c.p),
        }),
        _ => None,
    }
}

/// Every watchable drawing of one `drawings:<SYMBOL>` document (a map `id → {z, d}`), by id.
/// The key is the identity: an entry whose drawing claims another id is skipped, as the clients do.
pub fn parse_drawings(doc: Option<&Value>) -> std::collections::HashMap<String, Shape> {
    let mut out = std::collections::HashMap::new();
    let Some(map) = doc.and_then(Value::as_object) else {
        return out;
    };
    for (id, e) in map {
        let Some(d) = e.get("d") else { continue };
        if d.get("id").and_then(Value::as_str) != Some(id.as_str()) {
            continue;
        }
        if let Some(s) = parse_shape(d) {
            out.insert(id.clone(), s);
        }
    }
    out
}

/// chart-core `buildSlots`, times only: candles and gap runs merged in time order, a candle
/// superseding a gap at the same t.
pub fn build_slots(candles: &[i64], gaps: &[i64]) -> Vec<i64> {
    let mut out = Vec::with_capacity(candles.len() + gaps.len());
    let (mut i, mut j) = (0, 0);
    while i < candles.len() || j < gaps.len() {
        let c = candles.get(i);
        let g = gaps.get(j);
        match (c, g) {
            (Some(&c), g) if g.is_none_or(|&g| c <= g) => {
                out.push(c);
                i += 1;
                if g == Some(&c) {
                    j += 1;
                }
            }
            (_, Some(&g)) => {
                out.push(g);
                j += 1;
            }
            _ => break,
        }
    }
    out
}

/// chart-core `tToIndex`: the fractional slot index of time `t` (between two slots it
/// interpolates; past either end, one slot per timeframe interval).
pub fn t_to_index(slots: &[i64], t: f64, tf_ms: i64) -> f64 {
    let n = slots.len();
    if n == 0 {
        return 0.0;
    }
    let tf = tf_ms as f64;
    let first = slots[0] as f64;
    let last = slots[n - 1] as f64;
    if t <= first {
        return (t - first) / tf;
    }
    if t >= last {
        return (n - 1) as f64 + (t - last) / tf;
    }
    // lowerBound: the first slot at or after t (1..n-1 here, since first < t < last).
    let i = slots.partition_point(|&s| (s as f64) < t);
    let hi = slots[i] as f64;
    if hi == t {
        return i as f64;
    }
    let lo = slots[i - 1] as f64;
    (i - 1) as f64 + (t - lo) / (hi - lo)
}

/// Index slack for "between the anchors": anchors sit on slot centres, so an exact comparison
/// would let float noise switch a trend line off at its own end candle.
const INDEX_EPS: f64 = 1e-6;

/// The price `shape`'s `part` stands at, at the candle that starts at `t` (epoch ms), on the
/// chart with these `slots`. None when the level is not there: a trend line outside its two
/// anchors, a ray before its start (or pointing back in time), a vertical one, or a part the
/// shape does not have (a line's "stop").
pub fn level_at(shape: &Shape, part: Part, slots: &[i64], t: f64, tf_ms: i64) -> Option<f64> {
    match (shape, part) {
        (Shape::HLine { p }, Part::Line) => Some(*p),
        (Shape::Position { entry, .. }, Part::Entry) => Some(*entry),
        (Shape::Position { target, .. }, Part::Target) => Some(*target),
        (Shape::Position { stop, .. }, Part::Stop) => Some(*stop),
        (Shape::Trend { a, b }, Part::Line) | (Shape::Ray { a, b }, Part::Line) => {
            let ia = t_to_index(slots, a.t, tf_ms);
            let ib = t_to_index(slots, b.t, tf_ms);
            let i = t_to_index(slots, t, tf_ms);
            let span = ib - ia;
            if span.abs() < INDEX_EPS {
                return None;
            }
            let f = (i - ia) / span;
            let inside = match shape {
                Shape::Trend { .. } => (-INDEX_EPS..=1.0 + INDEX_EPS).contains(&f),
                _ => f >= -INDEX_EPS,
            };
            inside.then_some(a.p + (b.p - a.p) * f)
        }
        _ => None,
    }
}

/// The tick a level sits on: $0.0001 under $1, $0.01 at and above (the broker's grid).
pub fn tick_at(price: f64) -> f64 {
    if price < 1.0 { 0.0001 } else { 0.01 }
}

/// How close counts as touching.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tolerance {
    /// Whole ticks of the level's own grid (default 1).
    Ticks(u32),
    /// A percentage of the level.
    Pct(f64),
}

pub const DEFAULT_TOLERANCE: Tolerance = Tolerance::Ticks(1);
pub const MAX_TICKS: u32 = 100;
pub const MAX_PCT: f64 = 10.0;

impl Tolerance {
    /// `{unit: "ticks"|"pct", value}`; junk falls back to the default, out-of-range values clamp.
    pub fn parse(v: Option<&Value>) -> Tolerance {
        let Some(o) = v.and_then(Value::as_object) else {
            return DEFAULT_TOLERANCE;
        };
        let value = finite(o.get("value"));
        match o.get("unit").and_then(Value::as_str) {
            Some("pct") => match value.filter(|x| *x > 0.0) {
                Some(x) => Tolerance::Pct(x.min(MAX_PCT)),
                None => DEFAULT_TOLERANCE,
            },
            _ => match value.filter(|x| *x >= 0.0) {
                Some(x) => Tolerance::Ticks((x.round() as u32).clamp(0, MAX_TICKS)),
                None => DEFAULT_TOLERANCE,
            },
        }
    }

    /// The band half-width, in dollars, around a level at `level`.
    pub fn dollars(self, level: f64) -> f64 {
        match self {
            Tolerance::Ticks(n) => n as f64 * tick_at(level),
            Tolerance::Pct(p) => level.abs() * p / 100.0,
        }
    }
}

/// Float slack for the band edge: 1.86 is one tick from 1.85 even though 1.86 - 1.85 is
/// 0.010000000000000009 in binary.
pub const PRICE_EPS: f64 = 1e-9;

/// Whether `price` is within `tol` of `level`.
pub fn touching(price: f64, level: f64, tol: Tolerance) -> bool {
    (price - level).abs() <= tol.dollars(level) + PRICE_EPS
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn t_to_index_matches_the_chart_including_the_ends() {
        let s = [1000, 2000, 5000];
        assert_eq!(t_to_index(&s, 1000.0, 1000), 0.0);
        assert_eq!(t_to_index(&s, 2000.0, 1000), 1.0);
        assert_eq!(
            t_to_index(&s, 3500.0, 1000),
            1.5,
            "between slots it interpolates"
        );
        assert_eq!(
            t_to_index(&s, 0.0, 1000),
            -1.0,
            "before: one slot per interval"
        );
        assert_eq!(
            t_to_index(&s, 7000.0, 1000),
            4.0,
            "after: one slot per interval"
        );
        assert_eq!(t_to_index(&[], 7000.0, 1000), 0.0);
    }

    #[test]
    fn slots_merge_with_candles_superseding_gaps() {
        assert_eq!(
            build_slots(&[1, 3, 5], &[2, 3, 6]),
            vec![1, 2, 3, 5, 6],
            "the gap at 3 was filled by a late candle"
        );
    }

    #[test]
    fn shapes_parse_and_skip_what_v1_does_not_watch() {
        let doc = json!({
            "h": {"z": 0, "d": {"id": "h", "kind": "hline", "a": {"t": 1, "p": 1.85}, "b": {"t": 1, "p": 1.85}}},
            "p": {"z": 1, "d": {"id": "p", "kind": "long", "a": {"t": 1, "p": 2.0}, "b": {"t": 9, "p": 2.4}, "c": {"t": 9, "p": 1.8}}},
            "q": {"z": 2, "d": {"id": "q", "kind": "short", "a": {"t": 1, "p": 2.0}, "b": {"t": 9, "p": 1.6}}},
            "r": {"z": 3, "d": {"id": "r", "kind": "rect", "a": {"t": 1, "p": 1.0}, "b": {"t": 2, "p": 2.0}}},
            "x": {"z": 4, "d": {"id": "other", "kind": "hline", "a": {"t": 1, "p": 1.0}, "b": {"t": 1, "p": 1.0}}},
            "j": "junk",
        });
        let m = parse_drawings(Some(&doc));
        assert_eq!(m.len(), 3);
        assert_eq!(m["h"], Shape::HLine { p: 1.85 });
        assert_eq!(
            level_at(&m["p"], Part::Stop, &[], 0.0, 60_000),
            Some(1.8),
            "a long's stop is c"
        );
        assert_eq!(
            level_at(&m["q"], Part::Stop, &[], 0.0, 60_000),
            Some(2.0),
            "no c: the stop sits on the entry, as positionPlan reads it"
        );
        assert_eq!(level_at(&m["h"], Part::Stop, &[], 0.0, 60_000), None);
        assert_eq!(m["p"].label(Part::Stop), "long stop");
    }

    #[test]
    fn tolerance_parses_clamps_and_measures_on_the_levels_grid() {
        assert_eq!(Tolerance::parse(None), Tolerance::Ticks(1));
        assert_eq!(
            Tolerance::parse(Some(&json!({"unit": "ticks", "value": 500}))),
            Tolerance::Ticks(100)
        );
        assert_eq!(
            Tolerance::parse(Some(&json!({"unit": "pct", "value": 0.5}))),
            Tolerance::Pct(0.5)
        );
        assert_eq!(
            Tolerance::parse(Some(&json!({"unit": "pct", "value": -1}))),
            Tolerance::Ticks(1)
        );
        assert!(touching(1.86, 1.85, Tolerance::Ticks(1)));
        assert!(!touching(1.87, 1.85, Tolerance::Ticks(1)));
        assert!(touching(0.4501, 0.45, Tolerance::Ticks(1)));
        assert!(!touching(0.4502, 0.45, Tolerance::Ticks(1)));
        assert!(
            touching(1.85, 1.85, Tolerance::Ticks(0)),
            "0 ticks = exactly"
        );
        assert!(touching(2.01, 2.0, Tolerance::Pct(0.5)));
        assert!(!touching(2.02, 2.0, Tolerance::Pct(0.5)));
    }

    /// The cross-language golden cases (drawingAlerts.test.ts reads the same file).
    #[test]
    fn golden_fixtures_match() {
        let v: Value = serde_json::from_str(include_str!("drawing_alert_fixtures.json")).unwrap();
        let mut checked = 0;
        for case in v["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let tf = case["tfMs"].as_i64().unwrap();
            let slots: Vec<i64> = case["slots"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_i64().unwrap())
                .collect();
            let shape = parse_shape(&case["drawing"]).expect(name);
            for q in case["queries"].as_array().unwrap() {
                let part = Part::parse(q.get("part"));
                let got = level_at(&shape, part, &slots, q["t"].as_f64().unwrap(), tf);
                match q["expect"].as_f64() {
                    Some(want) => {
                        let got = got.unwrap_or_else(|| panic!("{name}: {q} inactive"));
                        assert!((got - want).abs() < 1e-9, "{name}: {q} got {got}");
                    }
                    None => assert!(got.is_none(), "{name}: {q} should be inactive, got {got:?}"),
                }
                checked += 1;
            }
        }
        assert!(checked >= 20, "{checked} fixture queries");
    }
}
