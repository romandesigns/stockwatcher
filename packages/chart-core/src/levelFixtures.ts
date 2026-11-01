// Parity fixtures for the server's key levels, opening ranges and automatic zones
// (crates/chart-engine/src/levels.rs, crates/server/src/chart_context.rs, `levels.v1` / `zones.v1`):
// seeded synthetic charts of two FULL trading days (2026-10-14 and the morning of 2026-10-15, so no
// early close: the one intended difference, documented in levels.rs, is tested on the Rust side
// alone), at 1m and 5m, every session from the 04:00 premarket through after hours and the
// overnight between them, with planted holes (a thin stock's 09:30 interval with no trade) and a
// forming last candle; and what keyLevels, openingRanges and autoZones (extended hours on and off)
// give for each. The Rust tests (crates/chart-engine/tests/levels_parity.rs and the server's
// chart_context tests) require the same values bit for bit; levels.parity.test.ts checks the
// committed file still matches this package, so changing a rule without regenerating fails here.
//
//   bun packages/chart-core/src/levelFixtures.ts --write

import { autoZones } from "./autoZones.ts";
import { sessionSlots } from "./chartTypes.ts";
import { keyLevels, openingRanges } from "./levels.ts";
import { buildSlots } from "./scale.ts";
import type { Candle } from "./types.ts";

export const LEVEL_FIXTURE_PATH = new URL("../../../crates/chart-engine/tests/fixtures/level_fixtures.json", import.meta.url);

/** mulberry32, as in the other fixture generators, so the cases are the same on every machine. */
function rng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const tick = (p: number) => (p < 1 ? Math.round(p * 10_000) / 10_000 : Math.round(p * 100) / 100);

// 2026-10-14 04:00 EDT and 2026-10-15 11:00 EDT: the overnight between them is 20:00–04:00.
const START = Date.UTC(2026, 9, 14, 8, 0);
const END = Date.UTC(2026, 9, 15, 15, 0);

/** A seeded chart of `stepMs` candles from START to END with some intervals left empty. */
function chart(seed: number, stepMs: number, start: number, holes: { rate: number; skipOpen: boolean }): Candle[] {
  const r = rng(seed);
  const out: Candle[] = [];
  let p = start;
  for (let t = START; t < END; t += stepMs) {
    const d = new Date(t);
    const minuteUtc = d.getUTCHours() * 60 + d.getUTCMinutes();
    // The overnight trades thinly; 09:30 ET is 13:30 UTC on both days (EDT).
    const overnight = minuteUtc >= 24 * 60 || minuteUtc < 8 * 60;
    if (holes.skipOpen && minuteUtc === 13 * 60 + 30) continue;
    if (r() < (overnight ? 0.85 : holes.rate)) continue;
    const vol = start * (0.003 + r() * 0.012);
    const phase = r();
    const o = p;
    let c: number;
    let h: number;
    let l: number;
    if (phase < 0.12) {
      const w = vol * (0.4 + r() * 0.5);
      c = o + (r() - 0.5) * w * 0.4;
      h = Math.max(o, c) + w * 0.4;
      l = Math.min(o, c) - w * 0.4;
    } else if (phase < 0.2) {
      const dir = r() < 0.5 ? 1 : -1;
      const w = vol * (2.5 + r() * 4);
      c = o + dir * w * 0.85;
      h = Math.max(o, c) + w * 0.05;
      l = Math.min(o, c) - w * 0.05;
    } else {
      const w = vol * (0.6 + r() * 1.6);
      c = o + (r() - 0.48) * w;
      h = Math.max(o, c) + r() * w * 0.4;
      l = Math.min(o, c) - r() * w * 0.4;
    }
    const to = tick(o);
    const tc = tick(Math.max(start * 0.2, c));
    const th = Math.max(tick(h), to, tc);
    let tl = Math.min(tick(l), to, tc);
    if (tl <= 0) tl = tick(Math.min(to, tc) * 0.99);
    out.push({ t, o: to, h: th, l: tl, c: tc, v: 1000, n: 10, final: true });
    p = tc;
  }
  // The newest candle is still forming: an opening range on it would say so (not here: 11:00).
  if (out.length) out[out.length - 1] = { ...out[out.length - 1]!, final: false };
  return out;
}

/** `[t, o, h, l, c, final]`, arrays to keep the committed file small. */
export type LevelFixtureCandle = [number, number, number, number, number, boolean];

export interface LevelFixture {
  name: string;
  candles: LevelFixtureCandle[];
  /** keyLevels(slots) with no official close: the fallback previous close is part of the rule. */
  levels: { id: string; price: number }[];
  /** openingRanges(slots) with no session ref: every day's end is the standard 16:00. */
  ranges: { day: string; t: number; end: number; high: number; low: number; forming: boolean }[];
  /** autoZones over every candle (extended hours on) and over the regular session only. */
  zonesExt: { kind: string; lo: number; hi: number; t: number; tested: boolean }[];
  zonesRth: { kind: string; lo: number; hi: number; t: number; tested: boolean }[];
}

const zoneWire = (zs: ReturnType<typeof autoZones>) => zs.map((z) => ({ kind: z.kind, lo: z.lo, hi: z.hi, t: z.t, tested: z.tested }));

export function buildLevelFixtures(): LevelFixture[] {
  const out: LevelFixture[] = [];
  const scales = [0.37, 1.42, 4.2, 12.6];
  for (let k = 0; k < 16; k++) {
    const stepMs = k % 2 === 0 ? 60_000 : 300_000;
    const start = scales[k % scales.length]!;
    const candles = chart(5000 + k, stepMs, start, { rate: [0.02, 0.15, 0.4][k % 3]!, skipOpen: k % 4 === 3 });
    const slots = buildSlots(candles, []);
    out.push({
      name: `chart-${k}-${stepMs / 60_000}m-${start}`,
      candles: candles.map((c) => [c.t, c.o, c.h, c.l, c.c, c.final]),
      levels: keyLevels(slots, null).map((l) => ({ id: l.id, price: l.price })),
      ranges: openingRanges(slots, null).map((r) => ({ day: r.day, t: r.t, end: r.end, high: r.high, low: r.low, forming: r.forming })),
      zonesExt: zoneWire(autoZones(slots)),
      zonesRth: zoneWire(autoZones(sessionSlots(slots, false))),
    });
  }
  return out;
}

if (import.meta.main && process.argv.includes("--write")) {
  await Bun.write(LEVEL_FIXTURE_PATH, JSON.stringify(buildLevelFixtures()) + "\n");
  console.log(`wrote ${LEVEL_FIXTURE_PATH.pathname}`);
}
