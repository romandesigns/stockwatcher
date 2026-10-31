import { expect, test } from "bun:test";
import { buildFixtures, FIXTURE_PATH, SPEC_KEYS_PATH } from "./indicatorFixtures.ts";
import { specKey } from "./indicators.ts";
import { parseIndicators } from "./syncedSettings.ts";

// The server's port (crates/chart-engine/src/indicators.rs) is checked bit for bit against the
// committed fixtures; this keeps those fixtures equal to what this package's indicators give
// today, so a formula change that is not regenerated
// (bun packages/chart-core/src/indicatorFixtures.ts --write) fails here.
test("the committed parity fixtures are what the indicators give today", async () => {
  const committed = await Bun.file(FIXTURE_PATH).json();
  expect(committed.cases).toEqual(JSON.parse(JSON.stringify(await buildFixtures())));
});

// The same vectors the Rust IndicatorSpec::parse + key are tested against: a client and the
// server must name every study alike, or the client cannot find its values in a frame.
test("spec keys match the shared vectors", async () => {
  const { vectors } = (await Bun.file(SPEC_KEYS_PATH).json()) as { vectors: { raw: unknown; key: string | null }[] };
  expect(vectors.length).toBeGreaterThan(30);
  for (const v of vectors) {
    const [spec] = parseIndicators([v.raw]);
    expect({ raw: v.raw, key: spec ? specKey(spec) : null }).toEqual({ raw: v.raw, key: v.key });
  }
});
