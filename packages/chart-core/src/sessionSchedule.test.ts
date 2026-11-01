import { describe, expect, test } from "bun:test";
import { sessionSlots } from "./chartTypes.ts";
import { computeChartModel, layoutChart } from "./layout.ts";
import { buildSlots, defaultViewport } from "./scale.ts";
import { applyIncrement, applySnapshot } from "./store.ts";
import { DEFAULT_CHART_THEME } from "./theme.ts";
import { etToUtc, scheduledSession, sessionAt, sessionOf, sessionRefFromId } from "./time.ts";
import type { Candle, ChartSnapshot, ChartUpdate, SessionRef } from "./types.ts";

// The day after Thanksgiving 2026 (an NYSE early close): regular 09:30-13:00, after 13:00-17:00,
// as the server's schedule says (chart_engine::wire::ChartSnapshot::sessions, `sessions.v1`).
const at = (h: number, m: number) => etToUtc(2026, 11, 27, h, m);
const ref = (id: string, start: number, end: number): SessionRef => ({ id, start, end });
const SCHEDULE: SessionRef[] = [
  ref("2026-11-27/overnight", etToUtc(2026, 11, 26, 20, 0), at(4, 0)),
  ref("2026-11-27/pre", at(4, 0), at(9, 30)),
  ref("2026-11-27/regular", at(9, 30), at(13, 0)),
  ref("2026-11-27/after", at(13, 0), at(17, 0)),
  ref("2026-11-30/overnight", etToUtc(2026, 11, 29, 20, 0), etToUtc(2026, 11, 30, 4, 0)),
];
const candle = (t: number, c = 3): Candle => ({ t, o: c, h: c, l: c, c, v: 100, n: 1, final: true });

describe("the server's session schedule (sessions.v1)", () => {
  test("an early close's 13:00-16:00 is after hours by the schedule, regular by the fixed one", () => {
    expect(sessionOf(at(14, 0))).toBe("regular");
    expect(sessionAt(at(14, 0), SCHEDULE)).toBe("after");
    expect(sessionAt(at(12, 59), SCHEDULE)).toBe("regular");
    expect(sessionAt(at(9, 0), SCHEDULE)).toBe("pre");
    // Outside the schedule (17:00-20:00 on an early close, or no schedule): the fixed one.
    expect(scheduledSession(at(18, 0), SCHEDULE)).toBeNull();
    expect(sessionAt(at(18, 0), SCHEDULE)).toBe("after");
    expect(sessionAt(at(14, 0))).toBe("regular");
    expect(sessionAt(at(14, 0), [])).toBe("regular");
  });

  test("extended hours off keeps only the schedule's regular session, and the cache follows the schedule", () => {
    const candles = [at(9, 0), at(10, 0), at(12, 59), at(13, 0), at(15, 0)].map((t) => candle(t));
    const all = buildSlots(candles, []);
    const rth = sessionSlots(all, false, SCHEDULE);
    expect(rth.map((s) => s.t)).toEqual([at(10, 0), at(12, 59)]);
    expect(sessionSlots(all, false, SCHEDULE)).toBe(rth);
    // An older server (no schedule): the fixed 09:30-16:00, recomputed rather than the cached list.
    expect(sessionSlots(all, false).map((s) => s.t)).toEqual([at(10, 0), at(12, 59), at(13, 0), at(15, 0)]);
  });

  test("the shading starts at the early close", () => {
    const candles = [at(12, 0), at(12, 30), at(13, 0), at(13, 30)].map((t) => candle(t));
    const state = { timeframe: "1m" as const, candles, gaps: [], profile: null, sessions: SCHEDULE };
    const size = { width: 400, height: 300 };
    const vp = defaultViewport(40);
    const shaded = (s: typeof state | Omit<typeof state, "sessions">) =>
      layoutChart(computeChartModel(s, vp, size), s, DEFAULT_CHART_THEME).filter((c) => c.kind === "rect" && c.fill === DEFAULT_CHART_THEME.shadeAfter).length;
    expect(shaded(state)).toBe(1);
    const { sessions: _s, ...fixed } = state;
    expect(shaded(fixed)).toBe(0);
  });

  test("the store keeps the snapshot's schedule and rolls into the next session by it", () => {
    const snap: ChartSnapshot = {
      type: "chart.snapshot",
      symbol: "SMPL",
      timeframe: "1m",
      session: { id: "2026-11-27/regular", start: at(9, 30), end: at(13, 0) },
      candles: [candle(at(12, 59))],
      gaps: [],
      profile: { bin: 0.01, anchor: "2026-11-27/regular", rows: [{ p: 3, v: 100 }], total: 100, poc: 3, valueArea: null },
      quality: { duplicates: 0, late: 0, outOfOrder: 0, corrections: 0, cancels: 0, unmatchedCorrections: 0, unmatchedCancels: 0, outsideSession: 0, haltsUnmatched: 0 } as ChartSnapshot["quality"],
      sessions: SCHEDULE,
    };
    const s = applySnapshot(undefined, snap);
    expect(s.sessions).toEqual(SCHEDULE);
    const inc: ChartUpdate = { type: "chart.update", symbol: "SMPL", timeframe: "1m", session: "2026-11-27/after", candle: candle(at(13, 5)), profileBin: 0.01, profile: [{ p: 3, v: 50 }] };
    const next = applyIncrement(s, inc);
    expect(next.session).toEqual({ id: "2026-11-27/after", start: at(13, 0), end: at(17, 0) });
    // Without a schedule the id's bounds come from the fixed one (16:00-20:00), as before.
    expect(sessionRefFromId("2026-11-27/after")?.start).toBe(at(16, 0));
  });
});
