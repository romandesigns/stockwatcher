import { describe, expect, test } from "bun:test";
import {
  ALERT_ON_MS,
  alertDots,
  alertJumpScope,
  alertNear,
  alertsForSymbol,
  formatElapsed,
  nextAlert,
  parseReplayAlerts,
  prevAlert,
  REPLAY_ALERT_COLOR,
  REPLAY_SPEEDS,
  replayBarLayout,
  replaySpeedLabel,
  SCRUBBER_TICK_HOURS,
  scrubberBars,
  scrubberFraction,
  scrubberKeySeek,
  scrubberSegments,
  scrubberSpan,
  scrubberTicks,
  scrubberTimeAt,
  scrubberTooltip,
  scrubberVolumeAt,
  sessionProgress,
  nextReplaySpeed,
  replayDetailsInline,
  replayScrubberInline,
  scrubberReleaseTarget,
  type ReplayAlert,
} from "./replayBar.ts";
import { etToUtc, sessionBands } from "./time.ts";

const day = { year: 2026, month: 10, day: 23 };
const bands = sessionBands(day);
const span = scrubberSpan(bands)!;
const at = (h: number, m = 0, s = 0) => etToUtc(2026, 10, 23, h, m, s);

describe("session proportions", () => {
  test("the scrubber runs 04:00–20:00 ET, the overnight band left out", () => {
    expect(span).toEqual({ start: at(4), end: at(20) });
  });

  test("pre-market, regular and after hours at their true widths (5.5 h : 6.5 h : 4 h)", () => {
    const segs = scrubberSegments(bands);
    expect(segs.map((s) => s.kind)).toEqual(["pre", "regular", "after"]);
    expect(segs.map((s) => s.label)).toEqual(["Pre-market", "Regular", "After hours"]);
    expect(segs.map((s) => s.short)).toEqual(["Pre", "Regular", "After"]);
    expect(segs.map((s) => +(s.to - s.from).toFixed(4))).toEqual([0.3438, 0.4063, 0.25]);
    expect(segs[0]!.from).toBe(0);
    expect(segs[2]!.to).toBe(1);
    expect(segs[1]!.from).toBe(segs[0]!.to);
  });

  test("no span without the bands", () => {
    expect(scrubberSpan([])).toBeNull();
    expect(scrubberSegments([])).toEqual([]);
  });
});

describe("the compact bar", () => {
  test("the speed chip cycles 1 → 2 → 5 → 10 → 30 → 1; an off-list speed goes to the next one up", () => {
    expect([1, 2, 5, 10, 30].map(nextReplaySpeed)).toEqual([2, 5, 10, 30, 1]);
    expect(nextReplaySpeed(3)).toBe(5);
    expect(nextReplaySpeed(60)).toBe(1);
  });

  test("the thin scrubber is inline from the unfolded width; day and ticker join the row from 900 px", () => {
    expect(replayScrubberInline("cover")).toBe(false);
    expect(replayScrubberInline("fold")).toBe(true);
    expect(replayDetailsInline(740)).toBe(false);
    expect(replayDetailsInline(900)).toBe(true);
  });

  test("a release near a drawn alert dot snaps onto that alert", () => {
    const a: ReplayAlert = { t: at(9, 41, 15), kind: "fast", symbol: "LABT" };
    const dots = alertDots([a], span);
    const x = dots[0]!.frac * 400;
    expect(scrubberReleaseTarget(dots, 400, x + 6, at(9, 50))).toBe(a.t);
    expect(scrubberReleaseTarget(dots, 400, x + 12, at(9, 50))).toBe(at(9, 50));
  });
});

describe("seek maths", () => {
  test("x → time, linear over the span, clamped and whole seconds", () => {
    expect(scrubberTimeAt(span, 1600, 0)).toBe(at(4));
    expect(scrubberTimeAt(span, 1600, 1600)).toBe(at(20));
    expect(scrubberTimeAt(span, 1600, 550)).toBe(at(9, 30)); // 5.5 h of 16 h = 34.375 %
    expect(scrubberTimeAt(span, 1600, -40)).toBe(at(4));
    expect(scrubberTimeAt(span, 1600, 9999)).toBe(at(20));
    expect(scrubberTimeAt(span, 777, 333) % 1000).toBe(0);
    expect(scrubberTimeAt(span, 0, 10)).toBe(at(4));
  });

  test("time → fraction is the inverse, clamped", () => {
    expect(scrubberFraction(span, at(12))).toBe(0.5);
    expect(scrubberFraction(span, at(3))).toBe(0);
    expect(scrubberFraction(span, at(21))).toBe(1);
    const t = scrubberTimeAt(span, 1000, 437);
    expect(scrubberFraction(span, t) * 1000).toBeCloseTo(437, 0);
  });

  test("keys: arrows a minute (Shift 15), Page keys 30, Home/End the ends, others ignored", () => {
    const now = at(11);
    expect(scrubberKeySeek("ArrowRight", false, now, span)).toBe(at(11, 1));
    expect(scrubberKeySeek("ArrowLeft", false, now, span)).toBe(at(10, 59));
    expect(scrubberKeySeek("ArrowRight", true, now, span)).toBe(at(11, 15));
    expect(scrubberKeySeek("PageDown", false, now, span)).toBe(at(10, 30));
    expect(scrubberKeySeek("Home", false, now, span)).toBe(at(4));
    expect(scrubberKeySeek("End", false, now, span)).toBe(at(20));
    expect(scrubberKeySeek("ArrowLeft", false, at(4), span)).toBe(at(4));
    expect(scrubberKeySeek("a", false, now, span)).toBeNull();
  });

  test("ticks: hourly, 2-hourly, 4-hourly", () => {
    expect(scrubberTicks(span, SCRUBBER_TICK_HOURS.desktop)).toHaveLength(17);
    expect(scrubberTicks(span, SCRUBBER_TICK_HOURS.fold).map((t) => t.label)).toEqual(["04:00", "06:00", "08:00", "10:00", "12:00", "14:00", "16:00", "18:00", "20:00"]);
    expect(scrubberTicks(span, SCRUBBER_TICK_HOURS.cover).map((t) => [t.label, t.frac])).toEqual([
      ["04:00", 0],
      ["08:00", 0.25],
      ["12:00", 0.5],
      ["16:00", 0.75],
      ["20:00", 1],
    ]);
  });
});

describe("volume histogram", () => {
  test("candles summed into n bars, sqrt-scaled, outside the span ignored", () => {
    const candles = [
      { t: at(3, 59), v: 1e9 },
      { t: at(4), v: 100 },
      { t: at(4, 10), v: 300 },
      { t: at(12), v: 1600 },
      { t: at(20), v: 1e9 },
    ];
    const bars = scrubberBars(candles, span, 64);
    expect(bars).toHaveLength(64);
    expect(bars[0]!.v).toBe(400);
    expect(bars[32]!.v).toBe(1600);
    expect(bars[32]!.h).toBe(1);
    expect(bars[0]!.h).toBe(0.5);
    expect(scrubberVolumeAt(bars, span, at(12, 5))).toBe(1600);
    expect(scrubberVolumeAt(bars, span, at(21))).toBeNull();
  });
});

describe("session progress", () => {
  test("names the session and the time into it", () => {
    expect(sessionProgress(at(11, 11, 35), bands).text).toBe("Regular · 1h 41m in");
    expect(sessionProgress(at(4, 12), bands).text).toBe("Pre-market · 12m in");
    expect(sessionProgress(at(16), bands).text).toBe("After hours · 0m in");
    expect(formatElapsed(-5)).toBe("0m");
  });
});

const alerts: ReplayAlert[] = [
  { t: at(9, 41, 15), kind: "fast", symbol: "LABT", price: 0.8877 },
  { t: at(9, 50), kind: "run", symbol: "LABT" },
  { t: at(11, 0), kind: "r2", symbol: "XXII", price: 0.58 },
  { t: at(13, 0), kind: "shelf", symbol: "LABT" },
  { t: at(21, 0), kind: "burst", symbol: "LABT" },
];

describe("alert dots", () => {
  test("parse: known kinds only, uppercased, ascending", () => {
    const parsed = parseReplayAlerts({
      day: "2026-10-23",
      alerts: [{ t: 5, kind: "run", symbol: "abc" }, { t: 2, kind: "fast", symbol: "X", price: 1.5, reason: "+23%" }, { t: 3, kind: "state", symbol: "X" }, { kind: "fast" }, null],
    });
    expect(parsed).toEqual({ day: "2026-10-23", alerts: [{ t: 2, kind: "fast", symbol: "X", price: 1.5, reason: "+23%" }, { t: 5, kind: "run", symbol: "ABC" }] });
    expect(parseReplayAlerts({ alerts: [] })).toBeNull();
    expect(parseReplayAlerts(null)).toBeNull();
  });

  test("placed by time along the span, the shown ticker only, outside the span dropped, each labelled", () => {
    const dots = alertDots(alertsForSymbol(alerts, "labt"), span);
    expect(dots.map((d) => d.alert.kind)).toEqual(["fast", "run", "shelf"]);
    expect(dots[0]!.frac).toBeCloseTo((5 * 60 + 41.25) / 960, 6);
    expect(dots[2]!.frac).toBe(9 / 16);
    expect(dots[0]!.label).toBe("Fast move at 09:41:15, $0.8877");
    expect(alertNear(dots, 1600, (9 / 16) * 1600 + 4)?.alert.kind).toBe("shelf");
    expect(alertNear(dots, 1600, 1500)).toBeNull();
  });

  test("every kind has its own hue", () => {
    expect(new Set(Object.values(REPLAY_ALERT_COLOR)).size).toBe(Object.keys(REPLAY_ALERT_COLOR).length);
  });

  test("prev/next relative to the playhead, skipping the alert just jumped to", () => {
    const own = alertsForSymbol(alerts, "LABT");
    expect(nextAlert(own, at(9))?.kind).toBe("fast");
    expect(prevAlert(own, at(9))).toBeNull();
    // On the fast move (the clock a second past it): next is the run, prev is nothing before it.
    expect(nextAlert(own, at(9, 41, 16))?.kind).toBe("run");
    expect(prevAlert(own, at(9, 41, 16))).toBeNull();
    expect(prevAlert(own, at(9, 41, 15) + ALERT_ON_MS + 1)?.kind).toBe("fast");
    expect(prevAlert(own, at(12))?.kind).toBe("run");
    expect(nextAlert(own, at(21, 0, 30))).toBeNull();
  });

  test("a ticker with no alerts that day jumps across every ticker instead", () => {
    expect(alertJumpScope(alerts, "LABT")).toEqual({ alerts: alertsForSymbol(alerts, "LABT"), any: false });
    const any = alertJumpScope(alerts, "NONE");
    expect(any.any).toBe(true);
    expect(any.alerts).toHaveLength(alerts.length);
    expect(alertJumpScope([], "LABT")).toEqual({ alerts: [], any: false });
  });

  test("tooltip: time · volume · alert", () => {
    expect(scrubberTooltip(at(12, 58, 20), 412_000, alerts[0]!)).toEqual({ time: "12:58:20", volume: "Vol 412.0K", alert: "Fast move" });
    expect(scrubberTooltip(at(12), null, null)).toEqual({ time: "12:00:00", volume: null, alert: null });
  });
});

describe("speed and layout", () => {
  test("speeds 1× 2× 5× 10× 30×", () => {
    expect(REPLAY_SPEEDS.map(replaySpeedLabel)).toEqual(["1×", "2×", "5×", "10×", "30×"]);
    expect(replaySpeedLabel(0)).toBe("Max");
  });

  test("cover below 600, fold to 1099, desktop from 1100", () => {
    expect(replayBarLayout(360)).toBe("cover");
    expect(replayBarLayout(599)).toBe("cover");
    expect(replayBarLayout(600)).toBe("fold");
    expect(replayBarLayout(740)).toBe("fold");
    expect(replayBarLayout(1099)).toBe("fold");
    expect(replayBarLayout(1100)).toBe("desktop");
  });
});
