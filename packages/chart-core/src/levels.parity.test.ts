import { expect, test } from "bun:test";
import { buildLevelFixtures, LEVEL_FIXTURE_PATH } from "./levelFixtures.ts";

// The server's key levels, opening ranges and zones (crates/chart-engine/src/levels.rs,
// crates/server/src/chart_context.rs) are checked against the committed fixtures; this keeps those
// fixtures equal to what keyLevels / openingRanges / autoZones give today, so a rule change that is
// not regenerated (bun packages/chart-core/src/levelFixtures.ts --write) fails here.
test("the committed level parity fixtures are what levels.ts and autoZones.ts give today", async () => {
  const committed = await Bun.file(LEVEL_FIXTURE_PATH).json();
  expect(committed).toEqual(JSON.parse(JSON.stringify(buildLevelFixtures())));
});

test("the fixtures exercise every level, ranges on both days, and zones both ways", () => {
  const fx = buildLevelFixtures();
  const ids = new Set(fx.flatMap((f) => f.levels.map((l) => l.id)));
  expect([...ids].sort()).toEqual(["hod", "lod", "open", "pmHigh", "pmLow", "prevClose"]);
  expect(fx.every((f) => f.ranges.length === 2)).toBe(true);
  expect(fx.reduce((n, f) => n + f.zonesExt.length, 0)).toBeGreaterThan(20);
  expect(fx.reduce((n, f) => n + f.zonesRth.length, 0)).toBeGreaterThan(20);
});
