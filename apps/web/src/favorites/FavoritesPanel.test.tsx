import { describe, expect, test } from "bun:test";
import type { FavoritesState } from "@stockwatcher/shared";
import { renderToStaticMarkup } from "react-dom/server";
import { FavoritesPanelView } from "./FavoritesPanel.tsx";

const T = Date.UTC(2026, 9, 26, 15, 0);

const state = (patch: Partial<FavoritesState> = {}): FavoritesState => ({
  connected: true,
  t: T,
  rows: [
    {
      symbol: "SMPL",
      price: 3_450_000,
      pc: 3_150_000,
      volume: 1_250_000,
      high: 3_540_000,
      pm_high: 3_180_000,
      day_line: [3_200_000, 3_300_000, 3_400_000],
      runners: { kind: "burst", at: Date.UTC(2026, 9, 26, 14, 42), latest_kind: "burst", latest_at: 0, entries: 1, state: "running" },
      levels: { trigger: 3_420_000, invalidation: 3_210_000 },
      pillars: { passes: 5, of: 5, news: "pass", rvol: 7.4, member: true },
    },
    { symbol: "OLD", price: 1_000_000, pc: 1_100_000, stale: true, last_at: T - 6 * 60_000, day_line: [1_000_000] },
  ],
  ...patch,
});

const render = (list: string[], s: FavoritesState) => renderToStaticMarkup(<FavoritesPanelView list={list} state={s} onPick={() => {}} />);

describe("Favorites panel", () => {
  test("rows in the list's order with price, change, line, marks and controls", () => {
    const html = render(["OLD", "SMPL", "NEW"], state());
    expect(html.indexOf(">OLD<")).toBeLessThan(html.indexOf(">SMPL<"));
    expect(html).toContain("3.45");
    expect(html).toContain("+9.52%");
    // Lighter rows (2026-10-26): the facts are marks, each named in its label (and tooltip).
    expect(html).toContain("1.25M volume");
    expect(html).toContain("PM high 3.18");
    expect(html).toContain('aria-label="Relative volume 7.4×"');
    expect(html).toContain('aria-label="Setup armed · trigger 3.42 · stop 3.21"');
    expect(html).toContain('aria-label="Five pillars · 5 of 5 pass"');
    expect(html).toContain('aria-label="News today"');
    expect(html).toContain('data-testid="mark-path" data-kind="burst"');
    expect(html).toContain('data-testid="row-line"');
    expect(html).toContain('aria-label="Move SMPL up"');
    expect(html).toContain('aria-label="Add SMPL to favorites"');
    // The down move is gone on the last row, the up move on the first.
    expect(html).toMatch(/aria-label="Move OLD up"[^>]*disabled/);
    // A favorite whose row has not arrived yet: listed at once, loading.
    expect(html).toContain("Loading…");
  });

  test("a stale row says how long ago it traded and dims; a dropped feed keeps the rows, labelled", () => {
    const html = render(["OLD"], state());
    expect(html).toContain("last trade 6m ago");
    expect(html).toContain("opacity-60");
    const down = render(["SMPL"], state({ connected: false }));
    expect(down).toContain("Reconnecting");
  });

  test("a ticker that has not traded today says so instead of inventing a price", () => {
    const html = render(["NOPE"], state({ rows: [{ symbol: "NOPE" }] }));
    expect(html).toContain("No trades today yet");
  });
});
