// The TypeScript half of the indicator parity check (docs/research/foundation/indicators.md).
//
// Reads a candle file written by `rf candles` (chart-engine candles + gaps for one symbol, day
// and timeframe), runs the chart's OWN code on it (packages/chart-core: buildSlots,
// sessionSlots, computeIndicators over every INDICATOR_PRESETS entry), and writes every line's
// values, NaN as null. `rf parity` (Rust) then compares them value by value with the port.
//
//   bun crates/research-features/parity/ts-indicators.ts <candles.json> <out.json>
//
// Imports are relative to the source files on purpose: the script needs no install and always
// tests the chart-core in this checkout. Two variants: "ext" (extended hours on, the default
// chart setting) and "rth" (extended hours off: the chart filters to regular-session slots
// BEFORE computing indicators).

import { sessionSlots } from "../../../packages/chart-core/src/chartTypes.ts";
import { computeIndicators, INDICATOR_PRESETS } from "../../../packages/chart-core/src/indicators.ts";
import { buildSlots } from "../../../packages/chart-core/src/scale.ts";
import type { Candle, Gap } from "../../../packages/chart-core/src/types.ts";

export interface CandleFile {
  symbol: string;
  day: string;
  timeframe: string;
  candles: Candle[];
  gaps: Gap[];
}

export type Lines = Record<string, (number | null)[]>;

/** Every preset's lines, keyed "<preset id>/<line name>", each preset computed on its own (so
 *  the overlay cap of computeIndicators never drops one). */
export function presetLines(file: CandleFile, extendedHours: boolean): Lines {
  const slots = sessionSlots(buildSlots(file.candles, file.gaps), extendedHours);
  const out: Lines = {};
  for (const spec of INDICATOR_PRESETS) {
    for (const r of computeIndicators(slots, [spec])) {
      for (const line of r.lines) {
        out[`${spec.id}/${line.name}`] = Array.from(line.values, (v) => (Number.isNaN(v) ? null : v));
      }
    }
  }
  return out;
}

export function run(file: CandleFile): { symbol: string; day: string; timeframe: string; variants: Record<string, Lines> } {
  return {
    symbol: file.symbol,
    day: file.day,
    timeframe: file.timeframe,
    variants: { ext: presetLines(file, true), rth: presetLines(file, false) },
  };
}

if (import.meta.main) {
  const [input, output] = process.argv.slice(2);
  if (!input || !output) {
    console.error("usage: bun ts-indicators.ts <candles.json> <out.json>");
    process.exit(2);
  }
  const file = (await Bun.file(input).json()) as CandleFile;
  await Bun.write(output, JSON.stringify(run(file)));
  console.error(`${output}: ${file.symbol} ${file.day} ${file.timeframe}, ${file.candles.length} candles, ${file.gaps.length} gaps`);
}
