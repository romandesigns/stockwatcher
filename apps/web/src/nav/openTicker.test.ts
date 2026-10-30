import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { APP_NS, createAppStore, shownTicker } from "@stockwatcher/app-state";
import { createSettingsSync } from "@stockwatcher/shared";
import { followCandidates } from "../feed/source.ts";
import { openTickerIn } from "./openTicker.ts";
import { breakoutTestFixture } from "../../../../packages/shared/src/breakoutTestFixture.ts";

/** The page's store on a real (offline) sync engine, a hash, and a way to play other devices' writes. */
function rig(hash = "#/") {
  let n = 0;
  const sync = createSettingsSync({ initial: null, newOpId: () => `op-${++n}`, setTimer: () => 0, clearTimer: () => {} });
  const store = createAppStore({ sync });
  let rev = 100;
  const remote = (ns: string, value: unknown) => sync.handle({ type: "settings.changed", ns, rev: ++rev, value, opId: "other-device", updatedAt: 1 });
  const env = { store, hash: { get: () => hash, set: (h: string) => void (hash = h) } };
  return { sync, store, remote, env, hash: () => hash, s: () => store.getState() };
}

/** What the dashboard paints in chart `i` (Dashboard.tsx: the slot's ticker through followCandidates). */
const visible = (r: ReturnType<typeof rig>, i: number, candidates: string[]) => followCandidates(r.s().dashboard.slots[i]!.symbol ?? candidates[0]!, candidates, r.s().picked);

describe("openTickerIn: every web/desktop entry point's one action", () => {
  test("an event click retains frozen context and the selected chart timeframe", () => {
    const r = rig();
    r.s().setDashboardCount(2);
    r.s().selectDashboardChart(1);
    r.s().setDashboardTimeframe(1, "5m");
    const context = { event_id: "runners-breakout-fixture", symbol: "CJMB", breakout: breakoutTestFixture };
    Reflect.apply(openTickerIn, null, [r.env, "CJMB", context]);
    expect((r.s() as unknown as Record<string, unknown>).noticeSelection).toEqual({ index: 1, source: "live", context });
    expect(r.s().dashboard.slots[1]!.timeframe).toBe("5m");
    expect(r.s().dashboard.slots[0]!.symbol).toBeNull();
  });
  test("context is ephemeral, survives timeframe changes, and clears on another ticker or source", () => {
    const r = rig();
    const context = { event_id: "runners-breakout-fixture", symbol: "CJMB", breakout: breakoutTestFixture };
    openTickerIn(r.env, "CJMB", context);
    for (const tf of ["15s", "1m", "5m"] as const) {
      r.s().setDashboardTimeframe(0, tf);
      expect(r.s().noticeSelection?.context).toEqual(context);
    }
    expect(r.sync.get(APP_NS.dashboard)).not.toHaveProperty("noticeSelection");
    r.s().setDashboardSource("replay");
    expect(r.s().noticeSelection).toBeNull();
    r.s().setDashboardSource("live");
    expect(r.s().noticeSelection).toBeNull();
    openTickerIn(r.env, "CJMB", context);
    openTickerIn(r.env, "AAPL");
    expect(r.s().noticeSelection).toBeNull();
    openTickerIn(r.env, "AAPL", context);
    expect(r.s().noticeSelection).toBeNull();
  });
  test("a notification click while the selected slot holds another ticker replaces it", () => {
    const r = rig();
    r.s().setDashboardSymbol(0, "AAL");
    expect(visible(r, 0, ["AAL", "SMPL"])).toBe("AAL");
    expect(openTickerIn(r.env, "cjmb")).toBe("CJMB");
    expect(r.s().dashboard.slots[0]!.symbol).toBe("CJMB");
    // CJMB is not in the (polled) candidate list yet: it is still what the chart shows.
    expect(visible(r, 0, ["AAL", "SMPL"])).toBe("CJMB");
  });

  test("loads into the SELECTED chart of several, the one the person is looking at", () => {
    const r = rig();
    r.s().setDashboardCount(4);
    for (const [i, s] of ["AAL", "SMPL", "TSLA", "NVDA"].entries()) r.s().setDashboardSymbol(i, s);
    r.s().selectDashboardChart(2);
    openTickerIn(r.env, "CJMB");
    expect(r.s().dashboard.slots.slice(0, 4).map((s) => s.symbol)).toEqual(["AAL", "SMPL", "CJMB", "NVDA"]);
  });

  test("another page open: the dashboard is shown, with the new ticker already in its slot", () => {
    const r = rig("#/alerts");
    r.s().setDashboardSymbol(0, "AAL");
    let slotWhenPageChanged: string | null | undefined;
    r.env.hash.set = (h) => {
      slotWhenPageChanged = r.s().dashboard.slots[0]!.symbol;
      void h;
    };
    openTickerIn(r.env, "CJMB");
    // The page switches only after the slot holds CJMB: the dashboard never mounts on AAL.
    expect(slotWhenPageChanged).toBe("CJMB");
  });

  test("closes an open search panel; ignores junk", () => {
    const r = rig("#/chart");
    r.s().setSearchOpen(true);
    openTickerIn(r.env, "CJMB");
    expect(r.s().searchOpen).toBe(false);
    expect(r.hash()).toBe("#/");
    expect(openTickerIn(r.env, "x\");alert(1)//")).toBeNull();
    expect(r.s().dashboard.slots[0]!.symbol).toBe("CJMB");
  });

  test("the same click twice is idempotent", () => {
    const r = rig();
    openTickerIn(r.env, "CJMB");
    const after = r.s().dashboard;
    openTickerIn(r.env, "CJMB");
    expect(r.s().dashboard.slots[0]!.symbol).toBe("CJMB");
    expect(r.s().dashboard.slots).toEqual(after.slots);
    expect(r.s().picked.filter((s) => s === "CJMB")).toHaveLength(1);
  });

  test("a persisted/remote slot state arriving after the click cannot put the old ticker back", () => {
    const r = rig();
    r.s().setDashboardSymbol(0, "AAL");
    openTickerIn(r.env, "CJMB");
    // A stale document (the server's, or another device's older write) lands while our write is
    // still in flight: the pending edit stays on top.
    r.remote(APP_NS.dashboard, { count: 1, slots: [{ symbol: "AAL", timeframe: "1m" }] });
    expect(r.s().dashboard.slots[0]!.symbol).toBe("CJMB");
    // And a connection's snapshot (a reconnect, a desktop window coming back) too.
    const sent: unknown[] = [];
    r.sync.attach((m) => (sent.push(m), true));
    r.sync.handle({ type: "settings.snapshot", docs: [{ ns: APP_NS.dashboard, value: { count: 1, slots: [{ symbol: "AAL", timeframe: "1m" }] }, rev: 500, updatedAt: 1 }] });
    expect(r.s().dashboard.slots[0]!.symbol).toBe("CJMB");
  });
});

describe("wiring: every web/desktop entry point goes through openTickerChart", () => {
  const read = (p: string) => readFileSync(join(import.meta.dir, p), "utf8");
  test("panel rows, Halt watcher, search, toasts, alerts and the notification click", () => {
    const dash = read("../dashboard/Dashboard.tsx");
    expect(dash).not.toContain("app.loadIntoSelected");
    expect(dash).toContain("useRunnersToasts(feed.rocket, openTickerChart)");
        expect(dash).toContain("onSelect={openTickerChart}");
    // Every panel (Halt watcher, Top Gainers and Hot stocks included) gets its onPick here.
    expect(dash.match(/<DashboardPanels [^>]*pressure=\{feed.pressure\}[^>]*onPick=\{openTickerChart\}/g)).toHaveLength(1);
    expect(read("../alerts/AlertsPage.tsx")).toContain('{ label: "Open chart", onSelect: () => openTickerChart(r.symbol) }');
    expect(read("../Shell.tsx")).toContain('action: { label: "Chart", run: () => openTickerChart(e.symbol) }');
    expect(read("../notify/runnersNotify.ts")).toContain("__stockwatcherOpenChart = openTickerChart");
    // The desktop shell's click hook exists from the first paint, not only once a notifier exists.
    expect(read("../main.tsx")).toContain("installOpenChartHook();");
  });
});

describe("shownTicker: the band fallback never replaces a pick", () => {
  test("an unpicked ticker that left the band falls back; a picked one stays", () => {
    expect(shownTicker("FXHI", ["FXLO", "SMPL"])).toBe("FXLO");
    expect(shownTicker("FXHI", ["FXLO", "SMPL"], ["FXHI"])).toBe("FXHI");
    expect(shownTicker("SMPL", ["FXLO", "SMPL"], [])).toBe("SMPL");
    expect(shownTicker("FXHI", [], [])).toBe("FXHI");
  });
});
