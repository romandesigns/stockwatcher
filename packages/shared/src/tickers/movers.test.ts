import { describe, expect, test } from "bun:test";
import { formatChangePct, formatDollarVolume, GAINERS_MIN_DOLLAR_VOLUME, hotStocks, MOVERS_LIMIT, topGainers } from "./movers.ts";
import type { QuoteBoard, TickerQuote } from "./quotes.ts";

const q = (price: number, changePct: number, volume: number, trades = 500): TickerQuote => ({ price, open: price, changePct, volume, trades, preOpen: false });
const board = (rows: Record<string, TickerQuote>): QuoteBoard => new Map(Object.entries(rows));

describe("topGainers", () => {
  test("ranks by % change among band names with enough prints and dollar volume", () => {
    const b = board({
      UP10: q(2, 10, 200_000), // $400K
      UP50: q(1, 50, 300_000), // $300K
      UP30: q(5, 30, 100_000), // $500K
    });
    expect(topGainers(b).map((r) => r.symbol)).toEqual(["UP50", "UP30", "UP10"]);
    expect(topGainers(b)[0]).toEqual({ symbol: "UP50", price: 1, changePct: 50, dollarVolume: 300_000, trades: 500 });
  });

  test("drops names outside $0.10–$25, thin prints, and under $250K traded", () => {
    const b = board({
      CHEAP: q(0.09, 90, 10_000_000),
      DEAR: q(25.01, 90, 1_000_000),
      STRAY: q(5, 400, 1_000_000, 99),
      THIN: q(1, 80, 249_999), // $249,999
      EDGE: q(0.1, 5, 2_500_000, 100), // $250,000 exactly, 100 prints, at the band floor
      TOP: q(25, 1, 20_000), // $500K at the band ceiling
    });
    expect(topGainers(b).map((r) => r.symbol)).toEqual(["EDGE", "TOP"]);
    expect(GAINERS_MIN_DOLLAR_VOLUME).toBe(250_000);
  });

  test("ties go to the bigger dollar volume, then the symbol; capped at 20", () => {
    const b = board({ BBB: q(1, 10, 400_000), AAA: q(1, 10, 400_000), CCC: q(1, 10, 900_000) });
    expect(topGainers(b).map((r) => r.symbol)).toEqual(["CCC", "AAA", "BBB"]);
    const many = board(Object.fromEntries(Array.from({ length: 30 }, (_, i) => [`S${String(i).padStart(2, "0")}`, q(1, i, 1_000_000)])));
    const top = topGainers(many);
    expect(top).toHaveLength(MOVERS_LIMIT);
    expect(top[0]!.symbol).toBe("S29");
  });

  test("an empty or unknown board ranks nothing", () => {
    expect(topGainers(new Map())).toEqual([]);
    expect(topGainers(board({ NAN: q(Number.NaN, 10, 1_000_000) }))).toEqual([]);
  });
});

describe("hotStocks", () => {
  test("ranks by dollar volume, with no dollar floor, and keeps losers", () => {
    const b = board({
      BIG: q(2, -8, 5_000_000), // $10M
      MID: q(10, 3, 300_000), // $3M
      SMALL: q(0.5, 40, 100_000), // $50K
      STRAY: q(20, 1, 9_000_000, 10),
      OUT: q(30, 1, 9_000_000),
    });
    expect(hotStocks(b).map((r) => r.symbol)).toEqual(["BIG", "MID", "SMALL"]);
  });
});

describe("formatting", () => {
  test("compact dollars", () => {
    expect(formatDollarVolume(950)).toBe("$950");
    expect(formatDollarVolume(41_200)).toBe("$41.2K");
    expect(formatDollarVolume(250_000)).toBe("$250K");
    expect(formatDollarVolume(3_240_000)).toBe("$3.2M");
    expect(formatDollarVolume(12_000_000)).toBe("$12M");
    expect(formatDollarVolume(1_500_000_000)).toBe("$1.5B");
    expect(formatDollarVolume(0)).toBe("$0");
  });

  test("signed change", () => {
    expect(formatChangePct(12.44)).toBe("+12.4%");
    expect(formatChangePct(-3.06)).toBe("−3.1%");
    expect(formatChangePct(0.01)).toBe("0.0%");
  });
});
