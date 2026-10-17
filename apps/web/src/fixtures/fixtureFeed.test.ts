import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { ChartStore, TIMEFRAME_MS, etToUtc, type ChartGap, type ChartMessage, type ChartUpdate, type Timeframe } from "@stockwatcher/chart-core";
import { createFixtureFeed, FIXTURE_SYMBOL, generateTrades } from "./fixtureFeed.ts";

/** A feed whose timer we drive by hand. */
function manualFeed(opts: Parameters<typeof createFixtureFeed>[0] = {}) {
  let tick: () => void = () => {};
  const feed = createFixtureFeed({ ...opts, setInterval: (fn) => ((tick = fn), 1), clearInterval: () => {} });
  return { feed, tick: (n = 1) => Array.from({ length: n }, () => tick()) };
}

function collect(feed: ReturnType<typeof createFixtureFeed>, timeframe: Timeframe, profileBin = 0.01) {
  const msgs: ChartMessage[] = [];
  const store = new ChartStore();
  const off = feed.subscribe({ symbol: FIXTURE_SYMBOL, timeframe, profileBin }, (m) => {
    msgs.push(m);
    if (m.type === "chart.snapshot") store.applySnapshot(m);
    else if (m.type === "chart.update" || m.type === "chart.remove" || m.type === "chart.gap") store.applyIncrement(m);
    else if (m.type === "replay.clock") store.applyClock(m.clock);
  });
  return { msgs, store, off };
}

describe("fixture trades", () => {
  test("deterministic for a seed, ordered, and silent during the 10:05 halt-like gap", () => {
    const a = generateTrades(7);
    const b = generateTrades(7);
    expect(a).toEqual(b);
    expect(generateTrades(8)).not.toEqual(a);
    for (let i = 1; i < a.length; i++) expect(a[i]!.t).toBeGreaterThanOrEqual(a[i - 1]!.t);
    const silenceStart = etToUtc(2026, 10, 15, 10, 5);
    expect(a.some((t) => t.t >= silenceStart && t.t < silenceStart + 90_000)).toBe(false);
    const kinds = new Set(a.map((t) => t.counted));
    for (const k of ["ohlc_and_volume", "volume_only", "excluded:late", "excluded:duplicate", "excluded:condition_Q", "excluded:condition_M"]) expect(kinds.has(k as never)).toBe(true);
    expect(new Set(a.map((t) => t.reason))).toEqual(new Set([null, "odd_lot", "condition_W", "condition_Q", "condition_M"]));
    // A duplicate repeats the identity of the print before it.
    const dup = a.findIndex((t) => t.counted === "excluded:duplicate");
    expect(a[dup]!.seq).toBe(a[dup - 1]!.seq);
  });
});

describe("fixture feed", () => {
  test("snapshot: aligned candles, a marked gap for the silence, no zero-volume candles", () => {
    const { feed } = manualFeed();
    const { msgs, store } = collect(feed, "1m");
    expect(msgs[0]!.type).toBe("chart.snapshot");
    const s = store.getSeries(FIXTURE_SYMBOL, "1m")!;
    expect(s.candles.length).toBeGreaterThan(300);
    for (const c of s.candles) {
      expect(c.t % 60_000).toBe(0);
      expect(c.v).toBeGreaterThan(0);
      expect(c.h).toBeGreaterThanOrEqual(Math.max(c.o, c.c));
      expect(c.l).toBeLessThanOrEqual(Math.min(c.o, c.c));
    }
    const silence = etToUtc(2026, 10, 15, 10, 5);
    expect(s.gaps.some((g) => g.t <= silence && (g.end ?? 0) > silence)).toBe(true);
    expect(s.candles.filter((c) => !c.final).length).toBeLessThanOrEqual(1);
    expect(s.profile!.anchor).toBe("2026-10-15/regular");
    feed.close();
  });

  test("all timeframes come from the same trades: 15s candles add up to each final 1m candle", () => {
    const { feed } = manualFeed();
    const one = collect(feed, "1m").store.getSeries(FIXTURE_SYMBOL, "1m")!;
    const fifteen = collect(feed, "15s").store.getSeries(FIXTURE_SYMBOL, "15s")!;
    let checked = 0;
    for (const m of one.candles.filter((c) => c.final)) {
      const parts = fifteen.candles.filter((c) => c.t >= m.t && c.t < m.t + 60_000);
      expect(parts.reduce((a, c) => a + c.v, 0)).toBe(m.v);
      expect(Math.max(...parts.map((c) => c.h))).toBe(m.h);
      expect(Math.min(...parts.map((c) => c.l))).toBe(m.l);
      expect(parts[parts.length - 1]!.c).toBe(m.c);
      checked++;
    }
    expect(checked).toBeGreaterThan(300);
    feed.close();
  });

  test("forming candle updates in place, finalizes at its interval end, then a new one opens", () => {
    const { feed, tick } = manualFeed({ speed: 10 });
    const { msgs, store } = collect(feed, "15s");
    const snapLast = store.getSeries(FIXTURE_SYMBOL, "15s")!.candles.at(-1)!.t;
    tick(120); // 120 ticks × 50 ms × 10 = 60 s of virtual time
    const updates = msgs.filter((m): m is ChartUpdate => m.type === "chart.update" && !!m.candle).map((u) => u.candle!);
    const first = updates.find((c) => c.t > snapLast)!;
    expect(first.final).toBe(false);
    // Several in-place updates while forming, then exactly one final version.
    const sameT = updates.filter((c) => c.t === first.t);
    expect(sameT.filter((c) => c.final)).toHaveLength(1);
    const finIdx = updates.findIndex((c) => c.t === first.t && c.final);
    expect(updates.slice(0, finIdx).filter((c) => c.t > first.t)).toHaveLength(0);
    expect(updates.slice(finIdx + 1).some((c) => c.t > first.t)).toBe(true);
    const s = store.getSeries(FIXTURE_SYMBOL, "15s")!;
    expect(s.candles.filter((c) => !c.final).length).toBeLessThanOrEqual(1);
    for (const c of s.candles) expect(c.t % TIMEFRAME_MS["15s"]).toBe(0);
    feed.close();
  });

  test("step advances exactly one event and pauses", () => {
    const { feed } = manualFeed();
    const { msgs, store } = collect(feed, "1m");
    const before = msgs.length;
    const nowBefore = feed.now();
    feed.control({ action: "step" });
    const tape = msgs.slice(before).filter((m) => m.type === "tape.trade");
    expect(tape).toHaveLength(1);
    expect(feed.now()).toBeGreaterThan(nowBefore);
    expect(store.getClock()!.playing).toBe(false);
    feed.close();
  });

  test("a new profile bin is recomputed by the feed (as the server would), same total volume", () => {
    const { feed } = manualFeed();
    const fine = collect(feed, "1m", 0.01).store.getSeries(FIXTURE_SYMBOL, "1m")!.profile!;
    const coarse = collect(feed, "5m", 0.05).store.getSeries(FIXTURE_SYMBOL, "5m")!.profile!;
    const total = (rows: { v: number }[]) => rows.reduce((a, r) => a + r.v, 0);
    expect(total(coarse.rows)).toBe(total(fine.rows));
    for (const r of coarse.rows) expect(Math.abs(r.p / 0.05 - Math.round(r.p / 0.05))).toBeLessThan(1e-9);
    expect(coarse.rows.length).toBeLessThan(fine.rows.length);
    feed.close();
  });
});

/** Field names of a Rust wire struct (serde camelCase + explicit renames). */
function rustFields(src: string, name: string): string[] {
  const m = new RegExp(`pub struct ${name} \\{([\\s\\S]*?)\\n\\}`).exec(src);
  if (!m) throw new Error(`struct ${name} not found`);
  const out: string[] = [];
  let rename: string | null = null;
  for (const line of m[1]!.split("\n")) {
    const r = /#\[serde\(rename = "([^"]+)"\)\]/.exec(line);
    if (r) rename = r[1]!;
    const f = /^\s*pub (\w+):/.exec(line);
    if (f) {
      out.push(rename ?? f[1]!.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase()));
      rename = null;
    }
  }
  return out.sort();
}

describe("fixture output matches the chart-engine wire structs exactly", () => {
  const crate = join(import.meta.dir, "../../../../crates/chart-engine/src");
  const wire = readFileSync(join(crate, "wire.rs"), "utf8");
  const quality = readFileSync(join(crate, "quality.rs"), "utf8");
  const keys = (o: object) => Object.keys(o).sort();
  const withType = (name: string) => [...rustFields(wire, name), "type"].sort();

  test("snapshot, increments and tape", () => {
    // Start just before the 10:05 silence so a chart.gap is emitted live.
    const { feed, tick } = manualFeed({ speed: 60, start: etToUtc(2026, 10, 15, 10, 4, 30) });
    const { msgs } = collect(feed, "15s");
    tick(200);
    const byType = (t: string) => msgs.filter((m) => m.type === t);
    const snap = byType("chart.snapshot")[0] as import("@stockwatcher/chart-core").ChartSnapshot;
    expect(keys(snap)).toEqual(rustFields(wire, "ChartSnapshot"));
    expect(keys(snap.profile!)).toEqual(rustFields(wire, "ProfileWire"));
    expect(keys(snap.profile!.valueArea!)).toEqual(rustFields(wire, "ValueArea"));
    expect(keys(snap.session!)).toEqual(rustFields(wire, "SessionInfo"));
    expect(keys(snap.clock!)).toEqual(rustFields(wire, "ClockInfo"));
    expect(keys(snap.quality)).toEqual(rustFields(quality, "DataQuality"));
    expect(keys(snap.candles[0]!)).toEqual(rustFields(wire, "CandleWire"));
    expect(keys(snap.gaps[0]!)).toEqual(rustFields(wire, "GapWire"));
    for (const [type, struct] of [["chart.update", "CandleUpdate"], ["chart.gap", "GapUpdate"], ["tape.trade", "TapeTrade"]] as const) {
      const list = byType(type);
      expect(list.length).toBeGreaterThan(0);
      for (const m of list) expect(keys(m)).toEqual(withType(struct));
    }
    for (const m of byType("chart.gap")) expect(keys((m as ChartGap).gap)).toEqual(rustFields(wire, "GapWire"));
    feed.close();
  });
});
