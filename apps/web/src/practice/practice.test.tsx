import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { EMPTY_HOD, EMPTY_PENDING } from "@stockwatcher/shared";
import type { PaperState } from "@stockwatcher/chart-core";
import { renderToStaticMarkup } from "react-dom/server";
import { DASHBOARD_SOURCE } from "../pages/DashboardPage.tsx";
import { offPanels } from "./offPanels.ts";
import { PaperSummary, PracticeBadge } from "./PracticeMarker.tsx";
import { showPracticeToast } from "./practiceToasts.ts";

// The practice page (replay parity plan Phase 1, owner 2026-11-23) and the rule beside it: the
// dashboard reads ONLY live data, the practice page is the only place replay data appears.

const src = (rel: string) => readFileSync(join(import.meta.dir, "..", rel), "utf8");

describe("the dashboard is live only", () => {
  test("its connection is pinned to the live market and nothing on it can pick replay", () => {
    expect(DASHBOARD_SOURCE).toBe("live");
    const page = src("pages/DashboardPage.tsx");
    expect(page).toContain("<ConnectionGate source={DASHBOARD_SOURCE}>");
    expect(page).not.toMatch(/useApp\(|dashboard\.source/);
    // The Settings → Data tab that switched it is gone from the one Settings dialog.
    expect(src("nav/settings.tsx")).not.toMatch(/dataSource|setDashboardSource/);
    // Every dashboard panel reads the feed the dashboard's gate opened: none opens its own.
    for (const f of ["dashboard/Dashboard.tsx", "dashboard/DashboardPanels.tsx", "dashboard/ChartCell.tsx"]) expect(src(f)).not.toMatch(/<ConnectionGate|createServerFeed|source="replay"|source: "replay"|Settings → Data/);
  });

  test("the practice page is the replay one", () => {
    expect(src("App.tsx")).toContain('<ConnectionGate source="replay">');
  });
});

describe("the practice marker", () => {
  test("says Practice and the day, and what is not in practice yet", () => {
    const html = renderToStaticMarkup(<PracticeBadge day="Thu, Oct 15" dayShort="Oct 15" off={[{ id: "hod", label: "HOD Momentum" }]} />);
    expect(html).toContain("Practice");
    expect(html).toContain("Thu, Oct 15");
    expect(html).toContain("replay, not live");
  });

  test("HOD Momentum and News pending are listed only once their replay snapshot says they are off", () => {
    expect(offPanels(EMPTY_HOD, EMPTY_PENDING)).toEqual([]);
    const hodOff = { ...EMPTY_HOD, connected: true, enabled: false };
    const pendingOff = { ...EMPTY_PENDING, snapshot: true, enabled: false };
    expect(offPanels(hodOff, pendingOff).map((p) => p.label)).toEqual(["HOD Momentum", "News pending"]);
    // A later phase turns one on for replay: it leaves the list by itself.
    expect(offPanels({ ...hodOff, enabled: true }, pendingOff).map((p) => p.id)).toEqual(["pending"]);
    // An older server without them: nothing to say.
    expect(offPanels({ ...hodOff, available: false }, { ...pendingOff, available: false })).toEqual([]);
  });

  test("the practice account's realized P&L and open positions", () => {
    const state = { type: "paper.state", simulated: true, seq: 1, account: { startingCash: 100_000, cash: 100_012.5, realizedPnl: 12.5 }, orders: [], positions: [{ symbol: "SMPL", qty: 100, avgPrice: 3.4, realizedPnl: 0 }] } as PaperState;
    const html = renderToStaticMarkup(<PaperSummary state={state} />);
    expect(html).toContain("+$12.50");
    expect(html).toContain("1 open");
    expect(renderToStaticMarkup(<PaperSummary state={null} />)).toBe("");
  });
});

describe("practice toasts", () => {
  test("a practice toast is labelled Practice and opens the practice chart; a live one is labelled Live and opens the live dashboard", () => {
    const shown: { text: string; tag?: string; run: () => void }[] = [];
    const show = (text: string, o: { tag?: "practice" | "live"; action?: { run: () => void } } = {}) => void shown.push({ text, tag: o.tag, run: o.action!.run });
    const opened: string[] = [];
    showPracticeToast({ label: "practice", symbol: "SMPL", text: "SMPL · Runners" }, (s) => opened.push(`practice:${s}`), (s) => opened.push(`live:${s}`), show);
    showPracticeToast({ label: "live", symbol: "CJMB", text: "CJMB · Runners" }, (s) => opened.push(`practice:${s}`), (s) => opened.push(`live:${s}`), show);
    expect(shown.map((t) => t.tag)).toEqual(["practice", "live"]);
    for (const t of shown) t.run();
    expect(opened).toEqual(["practice:SMPL", "live:CJMB"]);
  });

  test("nothing on the practice page can raise a system notification for a practice notice", () => {
    // The only reader of practice notices is the toast wiring, and it only calls the in-app toast.
    const toasts = src("practice/practiceToasts.ts");
    expect(toasts).not.toMatch(/new Notification|runners_notify|\.handle\(|playHalt|playAlertTone/);
    // The live notifier only ever takes `runners.notice`.
    expect(src("notify/runnersNotify.ts")).not.toContain("practice.notice");
  });
});
