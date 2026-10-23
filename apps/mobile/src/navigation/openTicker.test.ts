// The phone's "open this ticker" (owner, 2026-10-23: the chart must always show the ticker just
// tapped). The actions run through React Navigation's real StackRouter (the blank stack's router),
// so these are the stack states the app reaches, not a model of them. Notification responses go
// through the app's own router (route.ts createResponseRouter) and ready queue.
import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { join } from "node:path";

import { createResponseRouter, SAME_TAP_MS, type NotificationTarget, type ResponseLike } from "../notifications/route";
import { alertsAction, chartParams, createReadyQueue, openChartAction } from "./openTicker";

// @react-navigation/routers is @react-navigation/native's own dependency (not this app's): resolve
// it the way native does, so the test runs the exact router the app ships.
const req = createRequire(import.meta.path);
const core = createRequire(req.resolve("@react-navigation/native/package.json")).resolve("@react-navigation/core/package.json");
type Router = { getInitialState(o: unknown): unknown; getStateForAction(s: unknown, a: unknown, o: unknown): unknown };
const { StackRouter } = (await import(createRequire(core).resolve("@react-navigation/routers"))) as { StackRouter: (o: { initialRouteName: string }) => Router };

type Route = { key: string; name: string; params?: Record<string, unknown> };
type State = { index: number; routes: Route[] } & Record<string, unknown>;

const read = (p: string) => readFileSync(join(import.meta.dir, p), "utf8");

/** The root stack (RootStack.tsx: Home, TickerChart) and the app's dispatch, on the real router. */
function phone() {
  const router = StackRouter({ initialRouteName: "Home" });
  const options = { routeNames: ["Home", "TickerChart"], routeParamList: {}, routeGetIdList: {} };
  let state = router.getInitialState(options) as unknown as State;
  let ready = false;
  const queue = createReadyQueue(() => ready);
  const dispatch = (action: unknown) => {
    const next = router.getStateForAction(state, action, options);
    if (next) state = next as unknown as State;
  };
  /** ref.ts openTickerChart, minus the store and the alert sheet. */
  const openTicker = (symbol: string, episode?: string, run?: string) => {
    const params = chartParams(symbol, episode, run);
    if (params) queue.run(() => dispatch(openChartAction(params)));
  };
  /** push.ts goToTarget. */
  const go = (t: NotificationTarget) => (t.to === "chart" ? openTicker(t.symbol) : queue.run(() => dispatch(alertsAction())));
  const top = () => state.routes[state.index]!;
  const charts = () => state.routes.filter((r) => r.name === "TickerChart");
  return {
    openTicker,
    go,
    dispatch,
    top,
    charts,
    state: () => state,
    /** The container's onReady. */
    becomeReady: () => {
      ready = true;
      queue.flush();
    },
    readyNow: () => {
      ready = true;
    },
  };
}

let seq = 0;
const response = (data: unknown, identifier = `n-${++seq}`): ResponseLike => ({ actionIdentifier: "expo.modules.notifications.actions.DEFAULT", notification: { request: { identifier, content: { data } } } });
const runners = (symbol: string) => ({ symbol, kind: "runners.new", alert_id: `2026-10-23-${symbol}-1` });

describe("phone: every entry point opens THAT ticker", () => {
  test("a row tap with no chart open pushes the chart modal on the ticker", () => {
    const p = phone();
    p.readyNow();
    p.openTicker("cjmb");
    expect(p.top()).toMatchObject({ name: "TickerChart", params: { symbol: "CJMB" } });
  });

  test("a notification arriving while the modal shows another ticker replaces it (foreground, background, watch)", () => {
    const p = phone();
    p.readyNow();
    p.openTicker("AAL", "ep-1", "run-1");
    const modalKey = p.top().key;
    const route = createResponseRouter(p.go);
    expect(route(response(runners("CJMB")))).toBe(true);
    // One modal, the same route (no second modal stacked over the old chart), on CJMB, and the
    // previous pick's detection episode did not stick to the new ticker.
    expect(p.charts()).toHaveLength(1);
    expect(p.top().key).toBe(modalKey);
    expect(p.top().params).toEqual({ symbol: "CJMB" });
  });

  test("a price alert's notification opens its ticker too, not the Alerts tab under the open chart", () => {
    const p = phone();
    p.readyNow();
    p.openTicker("AAL");
    createResponseRouter(p.go)(response({ alertId: "a1", symbol: "XLAB", eventId: "h-1", url: "stockwatcher://alerts" }));
    expect(p.top()).toMatchObject({ name: "TickerChart", params: { symbol: "XLAB" } });
  });

  test("a modal below another screen: the screens above it are popped, never a second modal over the old one", () => {
    const p = phone();
    p.readyNow();
    p.openTicker("AAL");
    // What the old navigateToAlerts did with a modal open: a second Home pushed over it.
    p.dispatch({ type: "NAVIGATE", payload: { name: "Home", params: { screen: "Alerts" } } });
    expect(p.top().name).toBe("Home");
    p.openTicker("CJMB");
    expect(p.state().routes.map((r) => r.name)).toEqual(["Home", "TickerChart"]);
    expect(p.top().params).toEqual({ symbol: "CJMB" });
  });

  test("the Alerts tab (a payload without a ticker) closes the modal instead of stacking a Home over it", () => {
    const p = phone();
    p.readyNow();
    p.openTicker("AAL");
    createResponseRouter(p.go)(response({ kind: "something.else" }));
    expect(p.state().routes.map((r) => r.name)).toEqual(["Home"]);
    expect(p.top().params).toEqual({ screen: "Alerts" });
  });

  test("cold start: the last-notification response is read before the navigation exists and opens on ready", () => {
    const p = phone();
    let cleared = 0;
    const route = createResponseRouter(p.go, { clearLast: () => void cleared++ });
    // initPush: getLastNotificationResponse() at launch, then the listener delivers the same tap.
    const launch = response(runners("CJMB"), "launch-1");
    expect(route(launch)).toBe(true);
    expect(route(launch)).toBe(false);
    expect(p.charts()).toHaveLength(0);
    p.becomeReady();
    expect(p.top()).toMatchObject({ name: "TickerChart", params: { symbol: "CJMB" } });
    // The native "last response" was emptied, so a later read cannot replay it.
    expect(cleared).toBe(2);
  });

  test("cold start with two taps before ready: the newest wins", () => {
    const p = phone();
    const route = createResponseRouter(p.go);
    route(response(runners("AAL")));
    route(response(runners("CJMB")));
    p.becomeReady();
    expect(p.charts()).toHaveLength(1);
    expect(p.top().params).toEqual({ symbol: "CJMB" });
  });

  test("the same notification tapped twice is idempotent", () => {
    let t = 1_000;
    const p = phone();
    p.readyNow();
    const route = createResponseRouter(p.go, { now: () => t });
    const tap = response(runners("CJMB"), "same-1");
    expect(route(tap)).toBe(true);
    const after = JSON.stringify(p.state());
    expect(route(tap)).toBe(false);
    expect(JSON.stringify(p.state())).toBe(after);
    // Tapped again later (the watch, then the phone), after another ticker was opened: it is a new
    // request to see CJMB and opens it again, still as the one modal.
    p.openTicker("AAL");
    t += SAME_TAP_MS + 1;
    expect(route(tap)).toBe(true);
    expect(p.charts()).toHaveLength(1);
    expect(p.top().params).toEqual({ symbol: "CJMB" });
  });

  test("junk never navigates", () => {
    const p = phone();
    p.readyNow();
    p.openTicker("AAL");
    p.openTicker("not a ticker!");
    expect(p.top().params).toEqual({ symbol: "AAL" });
    expect(createResponseRouter(p.go)(null)).toBe(false);
  });
});

describe("phone: the wiring a bun test cannot render", () => {
  test("every entry point goes through openTickerChart", () => {
    const dash = read("../screens/DashboardScreen.tsx");
    // Halt watcher, Ross, Signals, Runners and the search all call `open`, which is openTickerChart.
    expect(dash).toContain("openTickerChart(symbol, detectionEpisodeId, detectionRunEpoch)");
    for (const panel of ["HaltWatcher", "RossPanel", "DetectionPanel", "RocketPanel"]) expect(dash).toMatch(new RegExp(`<${panel}[^>]*onPick={open}`));
    expect(dash).toMatch(/onSelect=\{\(symbol\) => \{\s*setSearchOpen\(false\);\s*open\(symbol\);/);
    const alerts = read("../screens/AlertsScreen.tsx");
    expect(alerts).toContain("openTickerChart(r.symbol)");
    expect(alerts).toContain("openTickerChart(e.symbol)");
    expect(read("../alerts/actions.ts")).toContain('action: { label: "Chart", run: () => openTickerChart(symbol) }');
    expect(read("../App.tsx")).toContain('TickerChart: { path: "ticker/:symbol"');
  });

  test("the modal reads its ticker from the params every render and restarts the chart per ticker", () => {
    const src = read("../screens/TickerChartScreen.tsx");
    expect(src).toContain("const { symbol, detectionEpisodeId, detectionRunEpoch } = route.params;");
    expect(src).toContain("<Animated.View key={symbol}");
    // The crossfade comes from the motion module, never inline numbers.
    expect(src).toContain("FadeIn.duration(chartSwapTiming.duration)");
    expect(src).not.toMatch(/useState\([^)]*route\.params/);
  });
});
