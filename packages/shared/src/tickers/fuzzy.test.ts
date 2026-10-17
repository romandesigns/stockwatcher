import { expect, test } from "bun:test";
import { fuzzyMatch, rankTickers } from "./fuzzy.ts";

test("letters must appear in order; case does not matter", () => {
  expect(fuzzyMatch("smp", "SMPL")?.indices).toEqual([0, 1, 2]);
  expect(fuzzyMatch("spl", "SMPL")?.indices).toEqual([0, 2, 3]);
  expect(fuzzyMatch("lps", "SMPL")).toBeNull();
});

test("a prefix and consecutive letters beat a scattered match", () => {
  expect(fuzzyMatch("aqr", "AQRX")!.score).toBeGreaterThan(fuzzyMatch("aqr", "ALQR")!.score);
});

test("tickers rank above names, and an exact ticker comes first", () => {
  const items = [
    { symbol: "FXMD", name: "Fixture Medium" },
    { symbol: "MDXT", name: "Meridex Tech" },
    { symbol: "SMPL", name: "Sample Labs" },
  ];
  const r = rankTickers(items, "md");
  expect(r[0]!.item.symbol).toBe("MDXT");
  expect(r.map((x) => x.item.symbol)).toContain("FXMD");
  expect(rankTickers(items, "sample")[0]!.item.symbol).toBe("SMPL");
  expect(rankTickers(items, "zzz")).toEqual([]);
});
