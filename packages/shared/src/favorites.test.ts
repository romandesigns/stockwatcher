import { describe, expect, test } from "bun:test";
import { createFavoritesChannel, favoriteLine, favoriteRows, favoriteRowView, mergeFavoriteDayLines, type FavoriteRowWire, type FavoritesMessage } from "./favorites.ts";
import { parseServerMessage } from "./protocol.ts";

const hhmm = (ms: number) => new Date(ms).toISOString().slice(11, 16);

describe("favorites channel", () => {
  test("subscribes with the first listener, merges day-line slices, unsubscribes with the last", () => {
    const calls: string[] = [];
    const ch = createFavoritesChannel({ subscribe: () => calls.push("sub"), unsubscribe: () => calls.push("unsub") });
    const off = ch.subscribe(() => {});
    expect(calls).toEqual(["sub"]);
    const snap: FavoritesMessage = { type: "favorites.snapshot", t: 1000, symbols: ["SMPL"], rows: [{ symbol: "SMPL", price: 3_450_000, day_line_start: 5, day_line_from: 0, day_line: [1, 2, 3] }] };
    ch.apply(snap);
    expect(ch.get().connected).toBe(true);
    ch.apply({ type: "favorites.update", t: 2000, symbols: ["SMPL"], rows: [{ symbol: "SMPL", price: 3_460_000, day_line_start: 5, day_line_from: 3, day_line: [4] }] });
    expect(ch.get().rows?.[0]?.day_line).toEqual([1, 2, 3, 4]);
    expect(ch.get().t).toBe(2000);
    // A slice it cannot place: the line is dropped and a fresh snapshot is asked for.
    ch.apply({ type: "favorites.update", t: 3000, symbols: ["SMPL"], rows: [{ symbol: "SMPL", day_line_start: 9, day_line_from: 7, day_line: [8] }] });
    expect(ch.get().rows?.[0]?.day_line).toBeUndefined();
    expect(calls).toEqual(["sub", "sub"]);
    ch.setConnected(false);
    expect(ch.get().connected).toBe(false);
    off();
    expect(calls.at(-1)).toBe("unsub");
    expect(ch.get().rows).toBeNull();
  });

  test("a snapshot replaces lines whole, and the parser accepts both frame types", () => {
    const { rows, resync } = mergeFavoriteDayLines(null, [{ symbol: "A", day_line_from: 0, day_line: [1, 2] }]);
    expect(rows[0]?.day_line).toEqual([1, 2]);
    expect(resync).toBe(false);
    expect(parseServerMessage(JSON.stringify({ type: "favorites.update", t: 1, symbols: [], rows: [] }))?.type).toBe("favorites.update");
  });
});

describe("favoriteRowView", () => {
  const row: FavoriteRowWire = {
    symbol: "SMPL",
    price: 3_450_000,
    pc: 3_150_000,
    volume: 1_250_000,
    high: 3_540_000,
    pm_high: 3_180_000,
    stale: false,
    day_line: [3_200_000, 3_300_000],
    runners: { kind: "burst", at: Date.UTC(2026, 9, 26, 14, 42), latest_kind: "fast", latest_at: 0, entries: 2, state: "running" },
    levels: { trigger: 3_420_000, invalidation: 3_210_000, near_trigger: true },
    pillars: { passes: 4, of: 5, news: "pass", rvol: 7.41, member: false },
  };

  test("price, change against the previous close, volume, distance from the high and context chips", () => {
    const v = favoriteRowView(row, null, hhmm);
    expect(v.price).toBe("3.45");
    expect(v.change).toBe("+9.52%");
    expect(v.changeTone).toBe("good");
    expect(v.volume).toBe("1.25M");
    expect(v.rvol).toBe("7.4× RVOL");
    expect(v.fromHigh).toBe("−2.54% from high");
    expect(v.pmHigh).toBe("PM high 3.18");
    expect(v.chips.map((c) => c.label)).toEqual(["Burst 14:42 · 2 entries", "Pullback · trigger 3.42 · stop 3.21", "4/5 pillars", "News"]);
    expect(v.chips[1]?.tone).toBe("warning");
    expect(v.aria).toContain("SMPL");
    expect(v.empty).toBe(false);
  });

  test("a stale row says how long ago it traded; a halted one leads with it; an unknown ticker is empty", () => {
    const t = Date.UTC(2026, 9, 26, 15, 0);
    const stale = favoriteRowView({ ...row, stale: true, last_at: t - 6 * 60_000, halted: true }, t, hhmm);
    expect(stale.stale).toBe("last trade 6m ago");
    expect(stale.chips[0]).toEqual({ label: "Halted", tone: "warning" });
    // No previous close: the change is measured from the quote board's reference instead.
    expect(favoriteRowView({ symbol: "R", price: 1_100_000, ref: 1_000_000 }, t, hhmm).change).toBe("+10.00%");
    const none = favoriteRowView({ symbol: "NOPE" }, t, hhmm);
    expect(none.empty).toBe(true);
    expect(none.price).toBe("—");
    expect(none.change).toBe("");
    expect(none.chips).toEqual([]);
  });

  test("rows follow the synced list; one the server has not built yet is null", () => {
    const rows = favoriteRows(["NEW", "SMPL"], { rows: [row], t: null, connected: true }, hhmm);
    expect(rows.map((r) => [r.symbol, r.view?.price ?? null])).toEqual([
      ["NEW", null],
      ["SMPL", "3.45"],
    ]);
  });

  test("the line ends at the live price and is red below the previous close", () => {
    const l = favoriteLine({ symbol: "X", price: 2_900_000, pc: 3_000_000, day_line: [3_100_000, 3_000_000] });
    expect(l?.values).toEqual([3_100_000, 3_000_000, 2_900_000]);
    expect(l?.tone).toBe("critical");
    expect(favoriteLine({ symbol: "X" })).toBeNull();
  });
});
