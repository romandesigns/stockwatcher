import { describe, expect, test } from "bun:test";
import { applyIncrement, applySnapshot, ChartStore } from "./store.ts";
import { fixtureSnapshot, T0 } from "./testFixture.ts";
import type { Candle, ChartGap, ChartRemove, ChartUpdate, ProfileRow } from "./types.ts";

const candle = (t: number, c: number, final: boolean, v = 100): Candle => ({ t, o: 3.2, h: Math.max(3.2, c), l: Math.min(3.2, c), c, v, n: 1, final });
const update = (c: Candle, profile: ProfileRow[] = [], session = "2026-10-15/regular", timeframe: ChartUpdate["timeframe"] = "1m"): ChartUpdate => ({
  type: "chart.update",
  symbol: "SMPL",
  timeframe,
  session,
  candle: c,
  profileBin: 0.01,
  profile,
});
const remove = (t: number, profile: ProfileRow[] = []): ChartRemove => ({ type: "chart.remove", symbol: "SMPL", timeframe: "1m", session: "2026-10-15/regular", t, profileBin: 0.01, profile });
const gapMsg = (t: number, end: number): ChartGap => ({
  type: "chart.gap",
  symbol: "SMPL",
  timeframe: "1m",
  session: "2026-10-15/regular",
  gap: { t, end, reason: "no_trades", v: 0, intervals: (end - t) / 60_000 },
  profileBin: 0.01,
  profile: [],
});

describe("snapshot and increments", () => {
  test("forming candle is upserted in place, finalized, then a newer one appends", () => {
    let s = applySnapshot(undefined, fixtureSnapshot("1m", 3, []));
    const formingT = s.candles[2]!.t;
    expect(s.candles[2]!.final).toBe(false);
    s = applyIncrement(s, update(candle(formingT, 3.25, false, 500)));
    expect(s.candles).toHaveLength(3);
    expect(s.candles[2]).toMatchObject({ c: 3.25, v: 500, final: false });
    s = applyIncrement(s, update(candle(formingT, 3.26, true, 600)));
    expect(s.candles).toHaveLength(3);
    expect(s.candles[2]).toMatchObject({ c: 3.26, final: true });
    s = applyIncrement(s, update(candle(formingT + 60_000, 3.27, false)));
    expect(s.candles).toHaveLength(4);
  });

  test("a revised older (final) candle replaces its own interval, never the forming one", () => {
    let s = applySnapshot(undefined, fixtureSnapshot("1m", 5, []));
    const forming = s.candles[4]!;
    s = applyIncrement(s, update(candle(s.candles[1]!.t, 3.0, true, 42)));
    expect(s.candles[1]).toMatchObject({ c: 3.0, v: 42 });
    expect(s.candles[4]).toEqual(forming);
    const ts = s.candles.map((c) => c.t);
    expect(ts).toEqual(ts.toSorted((a, b) => a - b));
  });

  test("chart.remove deletes the candle at t (a cancel emptied the interval)", () => {
    let s = applySnapshot(undefined, fixtureSnapshot("1m", 5, []));
    const t = s.candles[2]!.t;
    s = applyIncrement(s, remove(t, [{ p: 3.2, v: 5000 }]));
    expect(s.candles.some((c) => c.t === t)).toBe(false);
    expect(s.profile!.rows.find((r) => r.p === 3.2)!.v).toBe(5000);
  });

  test("a candle supersedes the gap covering it; a multi-interval gap is split around it", () => {
    let s = applySnapshot(undefined, fixtureSnapshot("1m", 3, []));
    const g0 = T0 + 3 * 60_000;
    s = applyIncrement(s, gapMsg(g0, g0 + 4 * 60_000));
    expect(s.gaps).toEqual([{ t: g0, end: g0 + 240_000, reason: "no_trades", v: 0, intervals: 4 }]);
    s = applyIncrement(s, update(candle(g0 + 60_000, 3.3, true)));
    expect(s.gaps).toEqual([
      { t: g0, end: g0 + 60_000, reason: "no_trades", v: 0, intervals: 1 },
      { t: g0 + 120_000, end: g0 + 240_000, reason: "no_trades", v: 0, intervals: 2 },
    ]);
    s = applyIncrement(s, update(candle(g0, 3.3, true)));
    expect(s.gaps.map((g) => g.t)).toEqual([g0 + 120_000]);
  });

  test("profile rows are absolute totals: replace, idempotent on re-send; POC/VA recomputed with engine rules", () => {
    let s = applySnapshot(undefined, fixtureSnapshot());
    const last = s.candles.at(-1)!;
    const rows = [{ p: 3.21, v: 9000 }, { p: 3.23, v: 50 }];
    s = applyIncrement(s, update(last, rows));
    s = applyIncrement(s, update(last, rows));
    expect(s.profile!.rows.find((r) => r.p === 3.21)!.v).toBe(9000);
    expect(s.profile!.rows.map((r) => r.p)).toEqual([3.19, 3.2, 3.21, 3.22, 3.23, 3.24]);
    expect(s.profile!.poc).toBe(3.21);
    expect(s.profile!.total).toBe(1200 + 5400 + 9000 + 800 + 50 + 2000);
    // Float noise maps to the same bin.
    s = applyIncrement(s, update(last, [{ p: 3.2199999999, v: 900 }]));
    expect(s.profile!.rows.filter((r) => Math.abs(r.p - 3.22) < 1e-6)).toEqual([{ p: 3.22, v: 900 }]);
  });

  test("rows for an older session are ignored; a newer session starts a fresh profile", () => {
    let s = applySnapshot(undefined, fixtureSnapshot());
    const last = s.candles.at(-1)!;
    s = applyIncrement(s, update(last, [{ p: 3.5, v: 1 }], "2026-10-15/pre"));
    expect(s.profile!.anchor).toBe("2026-10-15/regular");
    expect(s.profile!.rows.some((r) => r.p === 3.5)).toBe(false);
    const after = Date.UTC(2026, 9, 15, 20, 0, 0);
    s = applyIncrement(s, update(candle(after, 3.3, false), [{ p: 3.3, v: 100 }], "2026-10-15/after"));
    expect(s.profile).toMatchObject({ anchor: "2026-10-15/after", rows: [{ p: 3.3, v: 100 }], total: 100, poc: 3.3 });
    expect(s.session!.id).toBe("2026-10-15/after");
  });

  test("a snapshot is authoritative and keeps the server's derived profile fields", () => {
    const snap = fixtureSnapshot();
    const s = applySnapshot(undefined, snap);
    expect(s.profile!.valueArea).toEqual(snap.profile!.valueArea);
    expect(s.quality).toEqual(snap.quality);
  });

  test("ring buffer drops the oldest candles and their gaps", () => {
    const s = applySnapshot(undefined, fixtureSnapshot("1m", 20, [2, 15]), { maxCandles: 10 });
    expect(s.candles).toHaveLength(10);
    expect(s.gaps).toHaveLength(1);
  });
});

describe("ChartStore", () => {
  test("timeframes are separate series; switching never mixes them", () => {
    const store = new ChartStore();
    store.applySnapshot(fixtureSnapshot("1m", 10));
    store.applySnapshot(fixtureSnapshot("15s", 40));
    const oneMin = store.getSeries("SMPL", "1m")!;
    expect(store.applyIncrement(update(candle(T0 + 40 * 15_000, 3.5, false), [], "2026-10-15/regular", "15s"))).toBe(true);
    expect(store.getSeries("SMPL", "1m")).toBe(oneMin);
    expect(store.getSeries("SMPL", "15s")!.candles.at(-1)!.c).toBe(3.5);
  });

  test("increments without a snapshot are dropped; the clock is store-wide", () => {
    const store = new ChartStore();
    expect(store.applyIncrement(update(candle(T0, 3, false), [], "2026-10-15/regular", "5m"))).toBe(false);
    expect(store.getSeries("SMPL", "5m")).toBeUndefined();
    store.applyClock({ mode: "replay", now: 1, speed: 2, playing: false });
    expect(store.getClock()).toEqual({ mode: "replay", now: 1, speed: 2, playing: false });
  });

  test("listeners fire on change and can unsubscribe", () => {
    const store = new ChartStore();
    let calls = 0;
    const off = store.subscribe(() => calls++);
    store.applySnapshot(fixtureSnapshot());
    off();
    store.applySnapshot(fixtureSnapshot());
    expect(calls).toBe(1);
  });
});
