import { describe, expect, test } from "bun:test";
import type { RocketRowWire } from "./rocket.ts";
import { rossDemoPanel } from "./ross.ts";
import { burstOf, FLAME_MS, flameMark, favoriteRowMarks, marksReadout, newsMark, pathMark, pillarsFromCount, pillarsFromMissing, rocketRowMarks, rossRowMarks, rowLineOverlay, rvolMark, stageMark, tailMark, zoneMark } from "./rowMarks.ts";

const fmt = (ms: number) => `@${ms}`;
const T = 1_793_000_000_000;
const MIN = 60_000;

const rocketRow = (over: Partial<RocketRowWire> = {}): RocketRowWire => ({
  id: "1",
  kind: "burst",
  s: "SRTM",
  at: T - 2 * MIN,
  p: 0.61,
  last: 0.68,
  pc: 0.43,
  score: 1,
  reasons: [],
  outcome: { state: "pending", gain: null, drawdown: null, partial: false },
  ...over,
});

describe("pillar dots", () => {
  test("from the missing pillars: fixed order, fail vs unknown, label names what is missing", () => {
    const m = pillarsFromMissing([{ pillar: "float", state: "FAIL" }, { pillar: "news", state: "UNKNOWN" }]);
    expect(m.dots.map((d) => `${d.pillar}:${d.state}`)).toEqual(["price:pass", "gain:pass", "rvol:pass", "float:fail", "news:unknown"]);
    expect(m.passes).toBe(3);
    expect(m.label).toBe("Five pillars · 3 of 5 · missing: float, news (unknown)");
    expect(pillarsFromMissing([]).label).toBe("Five pillars · 5 of 5 · all pass");
  });

  test("from a count only: filled from the left, no pillar names claimed", () => {
    const m = pillarsFromCount(3, 5)!;
    expect(m.dots.map((d) => d.state)).toEqual(["pass", "pass", "pass", "fail", "fail"]);
    expect(m.dots.every((d) => d.pillar === null)).toBe(true);
    expect(m.label).toBe("Five pillars · 3 of 5 pass");
    expect(pillarsFromCount(2, 0)).toBeNull();
  });
});

describe("relative volume bars", () => {
  test("buckets: under 2×, 2–5×, above 5×; unknown draws nothing", () => {
    expect(rvolMark(1.9)!.bars).toBe(1);
    expect(rvolMark(2)!.bars).toBe(2);
    expect(rvolMark(4.99)!.bars).toBe(2);
    expect(rvolMark(5)!.bars).toBe(3);
    expect(rvolMark(7.4)!.label).toBe("Relative volume 7.4×");
    expect(rvolMark(null)).toBeNull();
    expect(rvolMark(Number.NaN)).toBeNull();
  });
});

describe("news", () => {
  test("only when the pillar passes; financing is the dilution class", () => {
    expect(newsMark(false, "financing")).toBeNull();
    expect(newsMark(true, "financing")).toEqual({ dilution: true, label: "News: financing / offering (dilution risk)" });
    expect(newsMark(true, "regulatory_decision")!.dilution).toBe(false);
    // Reverse splits share corporate_action with mergers: neutral, never a false alarm.
    expect(newsMark(true, "corporate_action")!.dilution).toBe(false);
    expect(newsMark(true, null)!.label).toBe("News today");
  });
});

describe("stage", () => {
  test("a forming cue with a trigger has armed; states map to their tones; watching has none", () => {
    expect(stageMark("forming", null, null, null)!.stage).toBe("forming");
    const armed = stageMark("forming", 6_100_000, 5_790_000, 700)!;
    expect(armed).toEqual({ stage: "armed", tone: "muted", meter: 0.7, trigger: "6.10", label: "Setup armed · trigger 6.10 · stop 5.79" });
    expect(stageMark("near", 6_100_000, null, 920)!.tone).toBe("warning");
    expect(stageMark("crossing", 550_000, 470_000, 1000)!.tone).toBe("accent");
    expect(stageMark("invalidated", 6_100_000, 5_790_000, 300)!.meter).toBeNull();
    expect(stageMark("watching", 6_100_000, null, 100)).toBeNull();
    expect(stageMark(null, null, null, null)).toBeNull();
  });
});

describe("sparkline tail", () => {
  test("coloured by the last minute's direction", () => {
    expect(tailMark(3.04)).toEqual({ tone: "good", label: "Last minute +3.0%" });
    expect(tailMark(-2)).toEqual({ tone: "critical", label: "Last minute −2.0%" });
    expect(tailMark(0.01)!.tone).toBe("muted");
    expect(tailMark(null)).toBeNull();
  });
});

describe("flame (look now)", () => {
  const burst = { at: T, p: 0.61 };
  test("lit under 5 minutes while the price holds at or above the alert", () => {
    expect(flameMark(burst, 0.68, T + 2 * MIN)?.lit).toBe(true);
    expect(flameMark(burst, 0.61, T + 4 * MIN)?.lit).toBe(true);
    expect(flameMark(burst, 0.68, T + 30_000)!.label).toBe("Look now: Buying burst under a minute ago, price 0.6800 still at or above its alert 0.6100");
  });
  test("goes out by itself at 5 minutes, and as soon as the price falls under the alert", () => {
    expect(flameMark(burst, 0.68, T + FLAME_MS)).toBeNull();
    expect(flameMark(burst, 0.6, T + MIN)).toBeNull();
    expect(flameMark(null, 0.68, T)).toBeNull();
    expect(flameMark(burst, null, T)).toBeNull();
  });
  test("only a burst row lights it", () => {
    expect(burstOf(rocketRow())).toEqual({ at: T - 2 * MIN, p: 0.61 });
    expect(burstOf(rocketRow({ kind: "fast" }))).toBeNull();
    expect(burstOf(rocketRow({ kind: undefined }))).toBeNull();
  });
});

describe("demand zone (Runners rows)", () => {
  test("named with its edges and the measured hold rate; tested is kept; nonsense draws nothing", () => {
    const z = zoneMark({ lo: 3.1, hi: 3.21, tested: false });
    expect(z!.label).toBe("In demand zone $3.10–$3.21 · zones held about 6 in 10 retests (5 in 10 at random levels)");
    expect(z!.tested).toBe(false);
    expect(zoneMark({ lo: 0.4512, hi: 0.4733, tested: true })!.label).toContain("$0.4512–$0.4733");
    expect(zoneMark({ lo: 0.4512, hi: 0.4733, tested: true })!.tested).toBe(true);
    for (const bad of [null, undefined, { lo: 3, hi: 3, tested: false }, { lo: 3.2, hi: 3.1, tested: false }, { lo: 0, hi: 1, tested: false }, { lo: Number.NaN, hi: 1, tested: false }]) {
      expect(zoneMark(bad)).toBeNull();
    }
  });

  test("a Runners row carries it from its wire; Ross and Favorites rows never do", () => {
    const m = rocketRowMarks(rocketRow({ zone: { lo: 0.6, hi: 0.63, tested: false, formed_t: T - 20 * MIN } }), T, fmt);
    expect(m.zone!.label).toContain("$0.6000–$0.6300");
    expect(rocketRowMarks(rocketRow(), T, fmt).zone).toBeNull();
    // A Ross row joined to that same Runners row does not borrow it (computed for Runners rows only).
    const zoned = rocketRow({ zone: { lo: 0.6, hi: 0.63, tested: false, formed_t: T - 20 * MIN } });
    expect(rossRowMarks(rossDemoPanel(T * 1_000_000).rows[0]!, zoned, T, fmt).zone).toBeNull();
    // Read out between the path and the setup, as the row draws it.
    const out = marksReadout(m).split("\n");
    expect(out.indexOf(m.zone!.label)).toBe(out.indexOf(m.path!.label) + 1);
  });
});

describe("path", () => {
  test("names the path and its age; unknown kinds draw nothing", () => {
    expect(pathMark("burst", T - 3 * MIN, T, fmt)).toEqual({ kind: "burst", age: "3m", label: `Runners: Buying burst at @${T - 3 * MIN} (3m ago)` });
    expect(pathMark("fast", T - 10_000, T, fmt)!.age).toBe("now");
    expect(pathMark("mystery", T, T, fmt)).toBeNull();
  });
});

describe("line overlay", () => {
  const line = { kind: "day" as const, values: [1_000_000, 1_100_000, 1_200_000, 1_150_000], prevClose: null, tone: "good" as const };
  test("the tail is the last three points, on the line's own scale; a trigger in range is a level", () => {
    const o = rowLineOverlay(line, 90, 30, 1_150_000);
    // Range 1.00–1.20 over 26 drawn units (pad 2): 1.10 → 15, 1.20 → 2, 1.15 → 8.5.
    expect(o.tail).toBe("M30 15 L60 2 L90 8.5");
    expect(o.triggerY).toBe(8.5);
  });
  test("a trigger far outside the drawn range is left to the stage mark", () => {
    expect(rowLineOverlay(line, 90, 30, 5_000_000).triggerY).toBeNull();
    expect(rowLineOverlay(line, 90, 30, null).triggerY).toBeNull();
  });
});

describe("per panel", () => {
  test("a Ross member: named pillars, its rvol and news class, the joined Runners path and burst flame", () => {
    const r = { ...rossDemoPanel(T * 1_000_000).rows[0]!, price: 680_000, news_first_seen: T, news_kind: "financing", rvol_milli: 7400, missing: [{ pillar: "float", state: "FAIL" as const, reason: "x" }] };
    const m = rossRowMarks(r, rocketRow(), T, fmt);
    expect(m.pillars!.dots.map((d) => d.state)).toEqual(["pass", "pass", "pass", "fail", "pass"]);
    expect(m.rvol!.bars).toBe(3);
    expect(m.news).toEqual({ dilution: true, label: "News: financing / offering (dilution risk)" });
    // News missing: no news mark, whatever its class.
    expect(rossRowMarks({ ...r, missing: [{ pillar: "news", state: "FAIL", reason: "x" }] }, null, T, fmt).news).toBeNull();
    expect(m.path!.kind).toBe("burst");
    expect(m.flame?.lit).toBe(true);
    expect(rossRowMarks(r, null, T, fmt).path).toBeNull();
    expect(rossRowMarks(r, null, T, fmt).flame).toBeNull();
  });

  test("a Runners row: no pillars or news, its own source as the path with the extras in the label", () => {
    const m = rocketRowMarks(rocketRow({ kind: "run", from_flag_pct: 12.3, move_1m_pct: -0.4, also: [{ kind: "fast", at: T - MIN }], levels: { state: "near", trigger: 700_000, invalidation: 620_000, meter_milli: 900 } }), T, fmt);
    expect(m.pillars).toBeNull();
    expect(m.news).toBeNull();
    expect(m.path!.label).toContain("Run +20%");
    expect(m.path!.label).toContain("+12.3% since");
    expect(m.path!.label).toContain("then Fast move");
    expect(m.stage!.stage).toBe("near");
    expect(m.tail!.tone).toBe("critical");
    expect(m.flame).toBeNull();
  });

  test("a favorite: counted pillars, news class, tail from its own day line, flame from the joined burst", () => {
    const fav = { symbol: "SRTM", price: 680_000, day_line: [600_000, 640_000, 650_000], pillars: { passes: 4, of: 5, news: "pass" as const, news_kind: "regulatory_decision", rvol: 3.1, member: false } };
    const m = favoriteRowMarks(fav, rocketRow(), T, fmt);
    expect(m.pillars!.passes).toBe(4);
    expect(m.rvol!.bars).toBe(2);
    expect(m.news).toEqual({ dilution: false, label: "News: FDA / regulatory decision" });
    expect(m.tail!.label).toBe("Last minute +6.3%");
    expect(m.flame?.lit).toBe(true);
    const bare = favoriteRowMarks({ symbol: "XLAB" }, null, T, fmt);
    expect(Object.values(bare).every((v) => v === null)).toBe(true);
    expect(marksReadout(bare)).toBe("");
    expect(marksReadout(m).split("\n")[0]).toStartWith("Look now");
  });
});
