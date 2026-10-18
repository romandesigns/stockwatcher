import { Icon } from "@stockwatcher/chart-web";
import { cn } from "@stockwatcher/chart-web/ui";
import { useCallback, useEffect, useMemo, useState } from "react";
import type { Connection } from "../ConnectionGate.tsx";
import { followCandidates, followServer } from "../feed/source.ts";
import { useCandidates } from "../feed/useCandidates.ts";
import { usePaperTrading } from "../feed/usePaperTrading.ts";
import { FIXTURE_SYMBOL } from "../fixtures/fixtureFeed.ts";
import { TickerSearch } from "../search/TickerSearch.tsx";
import { getAppStore, useApp } from "../state/store.ts";
import { ChartCell } from "./ChartCell.tsx";
import { MultiViewMenu } from "./MultiViewMenu.tsx";
import { TickerList } from "./TickerList.tsx";

const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);

/**
 * The panel's grid by chart count, laid out by the PANEL's own width (container queries, so the
 * nav rail, the ticker list and the window all count): below 48rem every count stacks, one chart
 * per row; from 48rem two columns (three charts: two over one wide); from 72rem three charts sit
 * side by side. Rows share the height on a wide screen and never shrink below 18rem; past that
 * the panel scrolls.
 */
const GRID: Readonly<Record<number, string>> = {
  1: "",
  2: "@3xl/panel:grid-cols-2",
  3: "@3xl/panel:grid-cols-2 @6xl/panel:grid-cols-3",
  4: "@3xl/panel:grid-cols-2",
};
/** The third of three charts spans the row in two columns, and is one of three in three. */
const SPAN_THIRD = "@3xl/panel:col-span-2 @6xl/panel:col-span-1";

/**
 * The main page (owner, 2026-10-18): a panel of 1–4 charts in their own containers, a multi-view
 * control for how many, and a live ticker list beside them (below when narrow). Select a chart by
 * pressing it, then click a ticker (in the list, or from the search) to load it there. What each
 * chart shows syncs across devices (app-state `dashboard`); the selection is this device's.
 * One socket carries every chart (up to 4 slots per connection).
 */
export function Dashboard({ choice, feed }: Connection) {
  const [app] = useState(() => getAppStore().getState());
  const dashboard = useApp((s) => s.dashboard);
  const selected = useApp((s) => s.dashboardSelected);
  const searchOpen = useApp((s) => s.searchOpen);
  const recents = useApp((s) => s.searchRecents);
  // The trading day the charts' clock is on: the candidates follow it (a day switch, a new session).
  const [clockDay, setClockDay] = useState<string | null>(null);
  const candidates = useCandidates(choice, clockDay);
  const current = useMemo(() => followServer(choice, candidates.server), [choice, candidates.server]);
  const trading = usePaperTrading(feed);

  // What each chart shows: its ticker, or (empty slot) a different candidate per chart. A ticker
  // that left the price band falls back too. Neither fallback is written back: a device with a
  // momentarily stale list must not change every other device's panel.
  const shown = dashboard.slots.slice(0, dashboard.count).map((slot, i) => {
    const fallback = candidates.symbols[i] ?? candidates.symbols[0] ?? FIXTURE_SYMBOL;
    return followCandidates(slot.symbol ?? fallback, candidates.symbols);
  });

  // Ticker quick-open (Ctrl+P / ⌘P), as on the chart page; here it loads into the selected chart.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey && (e.key === "p" || e.key === "P")) {
        e.preventDefault();
        app.setSearchOpen(true);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [app]);
  const pickFor = useCallback(
    (index: number) => {
      app.selectDashboardChart(index);
      app.setSearchOpen(true);
    },
    [app],
  );
  const onDay = useCallback((day: string | null) => {
    if (day) setClockDay(day);
  }, []);

  return (
    <div className="flex h-full min-h-0 flex-col gap-2 p-3 text-foreground">
      <header className="flex flex-wrap items-center gap-2">
        <h1 className="m-0 text-base font-semibold tracking-tight">Dashboard</h1>
        {dashboard.count > 1 && <span className="text-xs text-text-faint">Chart {selected + 1} selected</span>}
        <div className="ml-auto flex items-center gap-2">
          <button
            type="button"
            onClick={() => app.setSearchOpen(true)}
            className="flex h-8 items-center gap-2 rounded-lg bg-card px-3 text-xs text-text-faint outline-none hover:bg-bg-row hover:text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring [&_svg]:size-3.5"
          >
            <Icon name="search" />
            <span>Search tickers</span>
            <kbd className="hidden rounded bg-bg-row px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground sm:inline">{isMac ? "⌘P" : "Ctrl P"}</kbd>
          </button>
          <MultiViewMenu count={dashboard.count} onChange={app.setDashboardCount} />
        </div>
      </header>

      <div className="@container/dash min-h-0 flex-1">
        <div className="flex h-full min-h-0 flex-col gap-2 @5xl/dash:flex-row">
          <div className="@container/panel min-h-0 min-w-0 flex-1 overflow-y-auto">
            <div role="group" aria-label="Charts" className={cn("grid h-full auto-rows-[minmax(18rem,1fr)] gap-2", GRID[dashboard.count])}>
              {shown.map((symbol, i) => (
                <div key={i} className={cn("flex min-h-0 min-w-0 flex-col", dashboard.count === 3 && i === 2 && SPAN_THIRD)}>
                  <ChartCell
                    index={i}
                    slot={dashboard.slots[i]!}
                    symbol={symbol}
                    feed={feed}
                    trading={trading}
                    selected={selected === i}
                    showSelection={dashboard.count > 1}
                    fixture={current.fixtureData}
                    onSelect={app.selectDashboardChart}
                    onPickTicker={pickFor}
                    onDay={onDay}
                  />
                </div>
              ))}
            </div>
          </div>
          <div className="flex h-72 min-h-0 shrink-0 flex-col @5xl/dash:h-auto @5xl/dash:w-72">
            <TickerList quotes={feed.quotes} symbols={candidates.symbols} selectedSymbol={shown[selected] ?? ""} selectedIndex={selected} onPick={app.loadIntoSelected} />
          </div>
        </div>
      </div>

      <TickerSearch open={searchOpen} onOpenChange={app.setSearchOpen} symbols={candidates.symbols} quotes={feed.quotes} mock={current.fixtureData} current={shown[selected] ?? ""} recents={recents} onSelect={app.loadIntoSelected} />
    </div>
  );
}
