import { expect, test } from "bun:test";
import { buildFixtures, FIXTURE_PATH } from "./autoZoneFixtures.ts";

// The server's port (crates/server/src/auto_zones.rs) is checked against the committed fixtures;
// this keeps those fixtures equal to what autoZones() gives today, so a rule change that is not
// regenerated (bun packages/chart-core/src/autoZoneFixtures.ts --write) fails here.
test("the committed parity fixtures are what autoZones() gives today", async () => {
  const committed = await Bun.file(FIXTURE_PATH).json();
  expect(committed).toEqual(JSON.parse(JSON.stringify(buildFixtures())));
});
