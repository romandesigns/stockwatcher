import { describe, expect, test } from "bun:test";
import { computeChartModel, type ChartState } from "./layout.ts";
import { defaultViewport, priceY } from "./scale.ts";
import { DEFAULT_CHART_THEME } from "./theme.ts";
import { fixtureSnapshot } from "./testFixture.ts";
import { hitTrading, layoutTrading, orderLinePrice, orderTypeAt, parsePaperMessage, snapPrice, type PaperOrder } from "./trading.ts";

const snap = fixtureSnapshot("1m", 20);
const state: ChartState = { timeframe: "1m", candles: snap.candles, gaps: snap.gaps, profile: snap.profile, inspectT: null, pointer: null };
const model = computeChartModel(state, defaultViewport(16), { width: 400, height: 300 });

const order = (o: Partial<PaperOrder>): PaperOrder => ({
  id: "o-1",
  clientOrderId: "c-1",
  symbol: "SMPL",
  side: "buy",
  orderType: "limit",
  qty: 100,
  filledQty: 0,
  avgFillPrice: null,
  limitPrice: null,
  stopPrice: null,
  status: "open",
  rejectReason: null,
  createdAt: 0,
  updatedAt: 0,
  ...o,
});

describe("prices", () => {
  test("snap to cents at or above $1 and to hundredths of a cent below", () => {
    expect(snapPrice(3.4049)).toBe(3.4);
    expect(snapPrice(3.405)).toBe(3.41);
    expect(snapPrice(0.123456)).toBe(0.1235);
    expect(snapPrice(0.99996)).toBe(1);
  });

  test("a click below the market is a buy limit or a sell stop; above, a buy stop or a sell limit", () => {
    expect(orderTypeAt("buy", 3, 3.4)).toBe("limit");
    expect(orderTypeAt("sell", 3, 3.4)).toBe("stop");
    expect(orderTypeAt("buy", 3.6, 3.4)).toBe("stop");
    expect(orderTypeAt("sell", 3.6, 3.4)).toBe("limit");
  });

  test("a stop-limit shows its stop until triggered, then its limit", () => {
    const o = order({ orderType: "stop_limit", stopPrice: 3.5, limitPrice: 3.55 });
    expect(orderLinePrice(o)).toEqual({ price: 3.5, field: "stopPrice" });
    expect(orderLinePrice({ ...o, status: "triggered" })).toEqual({ price: 3.55, field: "limitPrice" });
  });
});

describe("order lines", () => {
  const price = model.price!;
  const mid = (price.lo + price.hi) / 2;
  const o = order({ limitPrice: mid });
  const layer = { orders: [o, order({ id: "o-2", status: "filled", limitPrice: mid })], position: null, last: null };

  test("only working orders draw, as a dashed line with a pill", () => {
    const list = layoutTrading(model, layer, DEFAULT_CHART_THEME);
    expect(list.filter((p) => p.kind === "line" && p.dash).length).toBe(1);
    expect(list.some((p) => p.kind === "text" && p.text.includes("Buy LMT 100"))).toBe(true);
  });

  test("hits the cancel cell, the pill, and the line itself", () => {
    const y = priceY(price, mid);
    const plot = model.geometry.plot;
    // Pill sits 22 px left of the plot's right edge; its cancel cell is its last 18 px.
    expect(hitTrading(model, layer, plot.x + plot.w - 22 - 9, y, 6)).toEqual({ kind: "order", id: "o-1", part: "cancel" });
    expect(hitTrading(model, layer, plot.x + plot.w - 22 - 40, y, 6)).toEqual({ kind: "order", id: "o-1", part: "drag" });
    expect(hitTrading(model, layer, 20, y + 3, 6)).toEqual({ kind: "order", id: "o-1", part: "drag" });
    expect(hitTrading(model, layer, 20, y + 30, 6)).toBeNull();
  });

  test("the place button is hit where it is drawn", () => {
    const plot = model.geometry.plot;
    expect(hitTrading(model, { ...layer, placeY: 50 }, plot.x + plot.w - 12, 50, 6)).toEqual({ kind: "place", y: 50 });
  });
});

test("parsePaperMessage accepts only paper frames", () => {
  expect(parsePaperMessage('{"type":"paper.fill","simulated":true}')?.type).toBe("paper.fill");
  expect(parsePaperMessage('{"type":"chart.update"}')).toBeNull();
  expect(parsePaperMessage("not json")).toBeNull();
});
