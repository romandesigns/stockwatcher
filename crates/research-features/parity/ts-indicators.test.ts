// Guards the checked-in TypeScript outputs against chart-core drift: if anyone changes an
// indicator in packages/chart-core, this fails, and the Rust port (and its fixtures) must be
// regenerated and re-verified together (docs/research/foundation/indicators.md).
//
//   bun test crates/research-features/parity

import { describe, expect, test } from "bun:test";
import { readdirSync } from "node:fs";
import { join } from "node:path";
import { run, type CandleFile } from "./ts-indicators.ts";

const dir = join(import.meta.dir, "..", "tests", "fixtures");
const names = readdirSync(dir)
  .filter((f) => f.endsWith(".candles.json"))
  .map((f) => f.replace(/\.candles\.json$/, ""));

describe("checked-in TS indicator outputs are what chart-core computes today", () => {
  test("there are fixtures", () => {
    expect(names.length).toBeGreaterThanOrEqual(6);
  });
  for (const name of names) {
    test(name, async () => {
      const candles = (await Bun.file(join(dir, `${name}.candles.json`)).json()) as CandleFile;
      const stored = await Bun.file(join(dir, `${name}.ts.json`)).json();
      // Exact: same engine, same code, same input.
      expect(JSON.parse(JSON.stringify(run(candles)))).toEqual(stored);
    });
  }
});
