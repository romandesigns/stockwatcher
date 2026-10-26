// RESEARCH ONLY: runs the shipped autoZones() on fixture candles for the Python parity test.
// usage: bun parity.ts <fixtures.json> <out.json>   (fixtures: [[[t,o,h,l,c], ...], ...])
import { autoZones } from "../../../../packages/chart-core/src/autoZones.ts";
import type { Slot } from "../../../../packages/chart-core/src/scale.ts";

const [inPath, outPath] = process.argv.slice(2);
const cases = (await Bun.file(inPath!).json()) as [number, number, number, number, number][][];
const out = cases.map((cs) => {
  const slots: Slot[] = cs.map(([t, o, h, l, c]) => ({ kind: "candle", t, candle: { t, o, h, l, c, v: 0, n: 0, final: true } }));
  return autoZones(slots);
});
await Bun.write(outPath!, JSON.stringify(out));
