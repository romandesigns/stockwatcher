import { expect, test } from "bun:test";
import { dayChartContext } from "./dayContext.ts";

// The Insights ticker package's `chart` (crates/server/src/chart_context.rs `day_context`): the
// frames read like live ones, ns twins winning, so the page draws the server's levels and zones.
test("a package's chart context reads into the chart's server props", () => {
  const ns = (ms: number) => ms * 1_000_000 + 37;
  const ctx = dayChartContext({
    levels: { type: "chart.levels", symbol: "SMPL", timeframe: "1m", day: "2026-10-15", pmHigh: 3, pmLow: 2.8, hod: 3.5, lod: 3.1, open: 3.2, prevClose: 2.5, openingRanges: [{ day: "2026-10-15", t: 1000, tNs: ns(1000), end: 5000, endNs: ns(5000), high: 3.3, low: 3.1, forming: false }] },
    zones: { type: "chart.zones", symbol: "SMPL", timeframe: "1m", rth: false, zones: [{ kind: "demand", lo: 3, hi: 3.1, t: 2000, tNs: ns(2000), tested: false }] },
    zonesRth: { type: "chart.zones", symbol: "SMPL", timeframe: "1m", rth: true, zones: [] },
    sessions: [
      { id: "2026-10-15/regular", start: 3000, end: 4000 },
      { id: "2026-10-15/pre", start: 1000, end: 3000 },
    ],
  });
  expect(ctx.levels?.hod).toBe(3.5);
  expect(ctx.levels?.openingRanges[0]!.t).toBe(1000);
  expect(ctx.zones?.[0]?.kind).toBe("demand");
  expect(ctx.zonesRth).toEqual([]);
  expect(ctx.sessions?.map((s) => s.id)).toEqual(["2026-10-15/pre", "2026-10-15/regular"]);
});

test("an older server's package (no chart) or garbage draws nothing", () => {
  expect(dayChartContext(null)).toEqual({ levels: null, zones: null, zonesRth: null, sessions: undefined });
  expect(dayChartContext({ levels: { type: "nope" }, zones: 3, zonesRth: null, sessions: [] })).toEqual({ levels: null, zones: null, zonesRth: null, sessions: undefined });
});
