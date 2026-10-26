// Parity fixtures for the server's port of autoZones() (crates/server/src/auto_zones.rs, the
// Runners rows' demand-zone mark): seeded synthetic days in several price scales, with quiet bases
// and decisive moves planted among ordinary candles, and the zones THIS function gives for each.
// The Rust test checks its port against them zone for zone; autoZones.parity.test.ts checks the
// committed file still matches this function, so changing the rule without regenerating fails.
//
//   bun packages/chart-core/src/autoZoneFixtures.ts --write

import { autoZones } from "./autoZones.ts";
import { buildSlots } from "./scale.ts";
import type { Candle } from "./types.ts";

export const FIXTURE_PATH = new URL("../../../crates/server/src/auto_zones_fixtures.json", import.meta.url);

/** mulberry32: a small seeded generator, so the fixtures are the same on every machine. */
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

/** Rounded to the tick a stock of that price trades in, as wire candles are. */
const tick = (p: number) => (p < 1 ? Math.round(p * 10_000) / 10_000 : Math.round(p * 100) / 100);

function day(seed: number, start: number, n: number): Candle[] {
  const r = rng(seed);
  const out: Candle[] = [];
  let p = start;
  const t0 = Date.UTC(2026, 9, 26, 8, 0);
  for (let i = 0; i < n; i++) {
    const vol = start * (0.004 + r() * 0.01);
    const phase = r();
    let o = p;
    let c: number;
    let h: number;
    let l: number;
    if (phase < 0.12) {
      // A quiet candle (a base candidate): small body inside a modest range.
      const w = vol * (0.4 + r() * 0.5);
      c = o + (r() - 0.5) * w * 0.4;
      h = Math.max(o, c) + w * 0.4;
      l = Math.min(o, c) - w * 0.4;
    } else if (phase < 0.2) {
      // A decisive bar: most of the range is body, either way.
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
    o = tick(o);
    c = tick(c);
    h = Math.max(tick(h), o, c);
    l = Math.min(tick(l), o, c);
    if (l <= 0) l = tick(Math.min(o, c) * 0.99);
    out.push({ t: t0 + i * 60_000, o, h, l, c, v: 1000, n: 10, final: true });
    p = c > 0 ? c : start;
  }
  return out;
}

export interface Fixture {
  name: string;
  candles: [number, number, number, number, number][];
  zones: { kind: "supply" | "demand"; lo: number; hi: number; t: number; tested: boolean }[];
}

export function buildFixtures(): Fixture[] {
  const out: Fixture[] = [];
  const scales = [0.42, 1.37, 3.1, 8.6, 18.9];
  for (let k = 0; k < 80; k++) {
    const start = scales[k % scales.length]!;
    const n = [17, 30, 120, 240, 480][k % 5]!;
    const candles = day(1000 + k, start, n);
    const zones = autoZones(buildSlots(candles, [])).map((z) => ({ kind: z.kind, lo: z.lo, hi: z.hi, t: z.t, tested: z.tested }));
    out.push({ name: `day-${k}-${start}-${n}`, candles: candles.map((c) => [c.t, c.o, c.h, c.l, c.c]), zones });
  }
  return out;
}

if (import.meta.main && process.argv.includes("--write")) {
  await Bun.write(FIXTURE_PATH, JSON.stringify(buildFixtures()) + "\n");
  console.log(`wrote ${FIXTURE_PATH.pathname}`);
}
