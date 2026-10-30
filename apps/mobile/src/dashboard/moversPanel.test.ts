// Top Gainers and Hot stocks on the phone (owner, 2026-10-30). The repo has no React Native
// renderer in its tests, so these pin the wiring a render would break silently: the server's
// movers channel (never the whole quote board), the shared sort and row logic, the same row chart
// as the other cards, the web's panel order, and no layout transition on the pressable rows.
import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { applyMoversMessage, EMPTY_MOVERS, moverLine, moversRows, sortMovers, type MoverRowWire } from "@stockwatcher/shared";

const read = (p: string) => readFileSync(join(import.meta.dir, p), "utf8");
const panel = read("MoversPanel.tsx");

test("the feed carries the movers channel: opted in on demand, told about the socket, frames fast-pathed", () => {
  const feed = read("../feed/serverFeed.ts");
  expect(feed).toContain("createMoversChannel({ subscribe: () => client.subscribeMovers(), unsubscribe: () => client.unsubscribeMovers() })");
  expect(feed).toContain("onMovers: (m) => movers.apply(m)");
  expect(feed).toContain('movers.setConnected(s === "open")');
  expect(feed).toContain(`ev.data.startsWith('{"type":"movers.')`);
  expect(read("../feed/hooks.ts")).toContain("export function useMovers(channel: MoversChannel): MoversState");
});

test("the panels read the movers channel, never the quote board, and reuse shared sorting and lines", () => {
  expect(panel).toContain("useMovers(channel)");
  expect(panel).not.toMatch(/useQuoteBoard|QuoteChannel|.quotesb/);
  for (const fn of ["sortMovers(", "moversRows(", "nextMoverSort(", "parseMoverSort(", "moverLine(", "moverRowMarks(", "moversCount("]) expect(panel).toContain(fn);
  // The app's segmented control and storage helper, the other cards' row chart.
  expect(panel).toContain("<Segmented");
  expect(panel).toMatch(/getPreference<unknown>\(SORT_PREF\(kind\)\)/);
  expect(panel).toContain("<RowLine ");
  expect(panel).toContain("<StarToggle ");
});

test("rows: no layout transitions, no borders, palette tokens only, 44 dp targets", () => {
  expect(panel).not.toMatch(/LinearTransition|layout=\{/);
  expect(panel).not.toMatch(/border(Width|Color|Top|Bottom|Left|Right)/);
  expect(panel).not.toMatch(/#[0-9a-fA-F]{3,8}\b|rgba?\(/);
  expect(panel).toContain("minHeight: 44");
});

test("the dashboard follows the web's order: Favorites, Signals, Top Gainers, then Ross, Runners, Hot stocks", () => {
  const screen = read("../screens/DashboardScreen.tsx");
  const at = (s: string) => screen.indexOf(s);
  const order = ["<FavoritesPanel ", "<DetectionPanel ", '<MoversPanel kind="gainers" ', "<RossPanel ", "<RocketPanel ", '<MoversPanel kind="hot" '].map(at);
  for (const i of order) expect(i).toBeGreaterThan(0);
  expect([...order].sort((a, b) => a - b)).toEqual(order);
});

const row = (symbol: string, chg: number, dv: number, price = 2_000_000): MoverRowWire => ({ symbol, price, ref: 1_000_000, chg, dv, volume: 1, trades: 200, pc: 1_000_000, day_line_start: 0, day_line_step_s: 30, day_line: [1_000_000, 1_500_000] });

test("a snapshot's Bear list sorts by % with the most negative first, and every row draws a line", () => {
  const { state } = applyMoversMessage(EMPTY_MOVERS, { type: "movers.snapshot", t: 1, gainers: [], bull: [], bear: ["A", "B"], rows: [row("A", -5, 10), row("B", -20, 5)] });
  const bear = sortMovers(moversRows(state, "bear"), { key: "chg", dir: 1 }, "bear");
  expect(bear.map((r) => r.symbol)).toEqual(["B", "A"]);
  for (const r of bear) expect(moverLine(r)).not.toBeNull();
});
