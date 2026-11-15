// The chart modal reads its params live by route key (liveRouteParams.ts; owner, 2026-11-15: a
// tapped notification left the open chart on the old ticker). These run the app's own response
// router and ready queue through React Navigation's real StackRouter, and check the property the
// screen relies on: the chart route keeps its key while its params follow every tap, so
// paramsForKey(state, thatKey) always names the ticker just tapped.
import { describe, expect, test } from "bun:test";
import { createRequire } from "node:module";

import { createResponseRouter, type NotificationTarget, type ResponseLike } from "../notifications/route";
import { paramsForKey } from "./routeParams";
import { alertsAction, chartParams, createReadyQueue, openChartAction, type ChartParams } from "./openTicker";

const req = createRequire(import.meta.path);
const core = createRequire(req.resolve("@react-navigation/native/package.json")).resolve("@react-navigation/core/package.json");
type Router = { getInitialState(o: unknown): unknown; getStateForAction(s: unknown, a: unknown, o: unknown): unknown };
const { StackRouter } = (await import(createRequire(core).resolve("@react-navigation/routers"))) as { StackRouter: (o: { initialRouteName: string }) => Router };

type Route = { key: string; name: string; params?: Record<string, unknown> };
type State = { index: number; routes: Route[] } & Record<string, unknown>;

/** The root stack plus push.ts's wiring (router → goToTarget → openTickerChart), on the real router. */
function phone(startReady = true) {
  const router = StackRouter({ initialRouteName: "Home" });
  const options = { routeNames: ["Home", "TickerChart", "InsightsTicker"], routeParamList: {}, routeGetIdList: {} };
  let state = router.getInitialState(options) as unknown as State;
  let ready = startReady;
  const queue = createReadyQueue(() => ready);
  const dispatch = (action: unknown) => {
    const next = router.getStateForAction(state, action, options);
    if (next) state = next as unknown as State;
  };
  const openTicker = (symbol: string) => {
    const params = chartParams(symbol);
    if (params) queue.run(() => dispatch(openChartAction(params)));
  };
  const go = (t: NotificationTarget) => (t.to === "chart" ? openTicker(t.symbol) : queue.run(() => dispatch(alertsAction())));
  const route = createResponseRouter(go);
  let n = 0;
  /** A tapped Runners notification for `symbol`, as expo-notifications hands it over. */
  const tap = (symbol: string): ResponseLike => {
    n += 1;
    const response = { actionIdentifier: "expo.modules.notifications.actions.DEFAULT", notification: { request: { identifier: `n${n}`, content: { data: { symbol, kind: "runners.new", alert_id: `a${n}` } } } } };
    route(response);
    return response;
  };
  const chart = () => state.routes.find((r) => r.name === "TickerChart");
  /** What TickerChartScreen draws: the live params for the chart route's key. */
  const shown = () => {
    const c = chart();
    return c ? paramsForKey<ChartParams>(state, c.key)?.symbol : undefined;
  };
  return { tap, chart, shown, dispatch, state: () => state, becomeReady: () => ((ready = true), queue.flush()) };
}

describe("a tapped notification always shows its ticker on the open chart", () => {
  test("app in the background with a chart open: the open chart switches in place", () => {
    const p = phone();
    p.tap("AAPL");
    const key = p.chart()!.key;
    p.tap("TSLA");
    expect(p.chart()!.key).toBe(key); // same modal, so the screen relies on the live params
    expect(p.shown()).toBe("TSLA");
    expect(p.state().routes.filter((r) => r.name === "TickerChart")).toHaveLength(1);
  });

  test("app killed: the tap that launched it opens that ticker once the navigation is ready", () => {
    const p = phone(false);
    p.tap("PLTR");
    expect(p.chart()).toBeUndefined();
    p.becomeReady();
    expect(p.shown()).toBe("PLTR");
  });

  test("app in the foreground, tapping the banner over an open chart: switches in place", () => {
    const p = phone();
    p.tap("AAPL");
    p.tap("NVDA");
    expect(p.shown()).toBe("NVDA");
  });

  test("a non-chart screen open (another screen pushed over the tabs): the chart opens on the ticker", () => {
    const p = phone();
    p.dispatch({ type: "NAVIGATE", payload: { name: "InsightsTicker", params: { symbol: "XYZ", day: "2026-11-13" } } });
    p.tap("INTC");
    expect(p.state().routes[p.state().index]!.name).toBe("TickerChart");
    expect(p.shown()).toBe("INTC");
  });

  test("two notifications tapped in a row: each one switches the open chart", () => {
    const p = phone();
    p.tap("AAPL");
    const key = p.chart()!.key;
    p.tap("AMD");
    expect(p.shown()).toBe("AMD");
    p.tap("MSFT");
    expect(p.shown()).toBe("MSFT");
    expect(p.chart()!.key).toBe(key);
  });

  test("the switch replaces the params: an earlier pick's detection episode never sticks to the new ticker", () => {
    const p = phone();
    p.dispatch(openChartAction(chartParams("AAL", "ep-1", "run-1")!));
    p.tap("CJMB");
    const params = paramsForKey<ChartParams>(p.state(), p.chart()!.key)!;
    expect(params).toEqual({ symbol: "CJMB" });
  });
});

test("paramsForKey: undefined for a missing key or state", () => {
  expect(paramsForKey(undefined, "k")).toBeUndefined();
  expect(paramsForKey({ routes: [{ key: "a", params: { symbol: "X" } }] }, "b")).toBeUndefined();
  expect(paramsForKey<{ symbol: string }>({ routes: [{ key: "a", params: { symbol: "X" } }] }, "a")).toEqual({ symbol: "X" });
});
