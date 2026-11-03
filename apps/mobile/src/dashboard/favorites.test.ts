// The Favorites card's phone-side wiring (owner, 2026-10-26). The rows' content is the shared
// model's (packages/shared/src/favorites.test.ts); a bun test cannot render RN, so these pin the
// wiring: the card collapses with an empty list, follows the synced list, and every place that
// shows a ticker carries a star.
import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { favoriteRows, type FavoritesState } from "@stockwatcher/shared";

const read = (p: string) => readFileSync(join(import.meta.dir, p), "utf8");

test("the card collapses with no favorites and lists the synced list in order", () => {
  const src = read("FavoritesPanel.tsx");
  expect(src).toContain("<Collapse open={list.length > 0} reducedMotion={reducedMotion} gap={gap}>");
  expect(src).toContain("favoriteRows(list, state, hhmm)");
  const state: FavoritesState = { rows: [{ symbol: "B", price: 2_000_000 }], t: null, connected: true };
  expect(favoriteRows(["A", "B"], state, String).map((r) => r.symbol)).toEqual(["A", "B"]);
});

test("a star on the chart header, the search, Runners and Ross rows; the card follows Ross and Runners on the dashboard", () => {
  expect(read("../screens/ChartScreen.tsx")).toContain("<StarToggle symbol={active} />");
  expect(read("../search/TickerSearchSheet.tsx")).toContain("<StarToggle symbol={t.symbol} />");
  expect(read("RocketPanel.tsx")).toContain("<StarToggle symbol={r.symbol} size={14} />");
  expect(read("RossPanel.tsx")).toContain("<StarToggle symbol={r.symbol} size={14} />");
  const dash = read("../screens/DashboardScreen.tsx");
  // Ross and Runners first after the gauges (owner, 2026-11-03), then Favorites.
  expect(dash.indexOf("<FavoritesPanel")).toBeGreaterThan(dash.indexOf("<RocketPanel"));
});

test("the row chart's width follows the row, within the Fold's cover screen and unfolded", () => {
  const src = read("FavoritesPanel.tsx");
  expect(src).toContain("Math.max(72, Math.min(160, Math.round(width * 0.26)))");
});
