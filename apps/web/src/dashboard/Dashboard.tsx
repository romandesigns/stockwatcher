import { Icon } from "@stockwatcher/chart-web";
import { cn, TOUCH_HIT } from "@stockwatcher/chart-web/ui";
import { lastSessionLabel, parseLiveSession } from "@stockwatcher/shared";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Connection } from "../ConnectionGate.tsx";
import { followCandidates, followServer } from "../feed/source.ts";
import { useCandidates } from "../feed/useCandidates.ts";
import { usePaperTrading } from "../feed/usePaperTrading.ts";
import { FIXTURE_SYMBOL } from "../fixtures/fixtureFeed.ts";
import { SearchPill } from "../search/SearchPill.tsx";
import { TickerSearch } from "../search/TickerSearch.tsx";
import { NavToggle } from "../nav/NavToggle.tsx";
import { useHasRail } from "../nav/navRail.ts";
import { SettingsButton } from "../nav/settings.tsx";
import { getAppStore, useApp } from "../state/store.ts";
import { ChartCell } from "./ChartCell.tsx";
import { HaltWatcher } from "./HaltWatcher.tsx";
import { RossPanel } from "./RossPanel.tsx";
import { SideTabs } from "./SideTabs.tsx";
import { useRunnersToasts } from "./runnersToasts.ts";
import { MultiViewMenu } from "./MultiViewMenu.tsx";

/**
 * The panel's grid by chart count, laid out by the PANEL's own width (container queries, so the
 * nav rail and the window both count): below 48rem every count stacks, one chart
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
 * control for how many, and the ticker quick-open centred in the header. Select a chart by
 * pressing it, then pick a ticker from the search to load it there. It reads the server's LIVE
 * market (ConnectionGate source="live"); replay is the chart page's practice feature. What each
 * chart shows syncs across devices (app-state `dashboard`); the selection is this device's.
 * One socket carries every chart (up to 4 slots per connection).
 */
export function Dashboard({ choice, feed }: Connection) {
  const [app] = useState(() => getAppStore().getState());
  const hasRail = useHasRail();
  // The header pill the quick-open panel drops into.
  const pillRef = useRef<HTMLButtonElement>(null);
  const dashboard = useApp((s) => s.dashboard);
  const selected = useApp((s) => s.dashboardSelected);
  const searchOpen = useApp((s) => s.searchOpen);
  const recents = useApp((s) => s.searchRecents);
  // Whether a chart has focus (its ring shows): see the click-away effect below.
  const [focused, setFocused] = useState(false);
  const [detectionSelection, setDetectionSelection] = useState<{ index: number; symbol: string; episodeId: string; runEpoch: string } | null>(null);
  // The trading day the charts' clock is on: the candidates follow it (a day switch, a new session).
  const [clockDay, setClockDay] = useState<string | null>(null);
  const candidates = useCandidates(choice, clockDay);
  const current = useMemo(() => followServer(choice, candidates.server), [choice, candidates.server]);
  const trading = usePaperTrading(feed);
  const pickDetection = useCallback((symbol: string, episodeId: string, runEpoch: string) => {
    if (runEpoch !== feed.detection?.get().frame?.runEpoch) return;
    setDetectionSelection({ index: selected, symbol, episodeId, runEpoch });
    app.loadIntoSelected(symbol);
  }, [app, feed, selected]);
  // Runners' new flags and breakouts as toasts with "Chart", whatever side tab is open.
  useRunnersToasts(feed.rocket, app.loadIntoSelected);

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
      setFocused(true);
      app.setSearchOpen(true);
    },
    [app],
  );
  const onDay = useCallback((day: string | null) => {
    if (day) setClockDay(day);
  }, []);

  // The ring shows only while a chart has focus (owner, 2026-10-18: clicking away drops the
  // border). The selection itself stays, so a ticker picked later still loads where it did. A
  // press in a popup (the search panel, a menu) is not "away": it is how a ticker gets picked.
  const select = useCallback(
    (index: number) => {
      app.selectDashboardChart(index);
      setFocused(true);
    },
    [app],
  );
  useEffect(() => {
    const onDown = (e: PointerEvent) => {
      const t = e.target as Element | null;
      if (t?.closest?.('[data-chart-cell], [role="dialog"], [role="menu"], [role="listbox"]')) return;
      setFocused(false);
    };
    document.addEventListener("pointerdown", onDown, true);
    return () => document.removeEventListener("pointerdown", onDown, true);
  }, []);

  // The live market lists nothing before a session's first trade, and all weekend: say so instead
  // of four charts waiting on symbols that cannot load.
  const liveEmpty = choice.kind === "server" && choice.market === "live" && candidates.symbols.length === 0;
  const liveStatus = candidates.server?.status;
  // A closed market shows its last session from the archive (owner, 2026-10-18): say which day.
  const sessionLabel = choice.market === "live" ? lastSessionLabel(parseLiveSession(candidates.server?.session)) : null;

  return (
    <div className="flex h-full min-h-0 flex-col gap-2 p-3 text-foreground">
      {hasRail ? (
        // Three columns, as on the chart page: equal side columns put the VS Code-style search pill
        // at the content's exact centre, and the quick-open panel drops down into it (anchor).
        <header className="grid grid-cols-[1fr_auto_1fr] items-center gap-4">
          <div className="flex min-w-0 items-center gap-2">
            {/* The rail's collapse toggle at the far left. */}
            <NavToggle />
            <h1 className="m-0 text-base font-semibold tracking-tight">Dashboard</h1>
            {sessionLabel && <span className="truncate text-xs text-text-faint">{sessionLabel}</span>}
            {dashboard.count > 1 && <span className="truncate text-xs text-text-faint">Chart {selected + 1} selected</span>}
          </div>
          <SearchPill ref={pillRef} onOpen={() => app.setSearchOpen(true)} />
          <div className="flex items-center gap-2 justify-self-end">
            <MultiViewMenu count={dashboard.count} onChange={app.setDashboardCount} />
          </div>
        </header>
      ) : (
        // Phone layout (no rail): no room for the pill; a search button and the Settings gear instead.
        <header className="flex items-center gap-2">
          <h1 className="m-0 text-base font-semibold tracking-tight">Dashboard</h1>
          {sessionLabel && <span className="truncate text-xs text-text-faint">{sessionLabel}</span>}
          {dashboard.count > 1 && <span className="truncate text-xs text-text-faint">Chart {selected + 1} selected</span>}
          <div className="ml-auto flex items-center gap-2">
            <button
              type="button"
              aria-label="Search tickers"
              onClick={() => app.setSearchOpen(true)}
              className={cn("relative inline-grid size-8 place-items-center rounded-lg bg-card text-muted-foreground outline-none hover:bg-bg-row hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring pointer-coarse:size-10 [&_svg]:size-3.5", TOUCH_HIT)}
            >
              <Icon name="search" />
            </button>
            <MultiViewMenu count={dashboard.count} onChange={app.setDashboardCount} />
            <SettingsButton />
          </div>
        </header>
      )}

      {/* The panel takes the full width (the "Most active" side list was removed, owner 2026-10-18):
          tickers come from the search above. */}
      {liveEmpty ? (
        <div className="@container/panel flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto @4xl/panel:flex-row">
        <div className="flex flex-1 flex-col items-center justify-center gap-1 text-center">
          <p className="m-0 text-sm font-medium">{liveStatus === "not_configured" ? "Live market data is not configured" : "No live trades yet"}</p>
          <p className="m-0 max-w-sm text-xs text-text-faint">
            {liveStatus === "not_configured"
              ? "The server has no Alpaca credentials, so it cannot stream the live market."
              : "The dashboard shows the live market (Settings → Data). Charts appear with the session's first trades: overnight from 20:00 ET Sunday to Thursday, then 4:00 to 20:00 ET."}
          </p>
        </div>
        <div className="flex min-w-0 shrink-0 flex-col gap-2 @4xl/panel:w-[400px]">
          <SideTabs rocket={feed.rocket} detection={feed.detection} onPick={app.loadIntoSelected} onEpisodePick={pickDetection} ross={feed.ross && <RossPanel channel={feed.ross} detection={feed.detection} onPick={app.loadIntoSelected} onEpisodePick={pickDetection} />} />
        </div>
        </div>
      ) : (
      <div className="@container/panel flex min-h-0 min-w-0 flex-1 flex-col gap-3 overflow-y-auto">
        {/* The Halt watcher strip above the charts (owner, 2026-10-19); server feed only (the
            fixture has no pressure board). It scrolls with the panel, so its cards never squeeze
            the charts below their minimum height; on a phone it is one sideways-scrolling row,
            so a single chart fills the rest of the screen above the tab bar. */}
        {feed.pressure && <HaltWatcher channel={feed.pressure} onPick={app.loadIntoSelected} />}
        {/* The Ross Panel beside the charts (owner, 2026-10-20: "a side panel adjacent to the chart
            wrapper"); below them when the panel is narrow. Server feed only. */}
        <div className="flex min-h-0 flex-1 flex-col gap-2 @4xl/panel:flex-row">
        <div role="group" aria-label="Charts" className={cn("grid min-w-0 flex-1 auto-rows-[minmax(18rem,1fr)] gap-2", GRID[dashboard.count])}>
          {shown.map((symbol, i) => (
            <div key={i} className={cn("flex min-h-0 min-w-0 flex-col", dashboard.count === 3 && i === 2 && SPAN_THIRD)}>
              <ChartCell
                index={i}
                slot={dashboard.slots[i]!}
                symbol={symbol}
                feed={feed}
                trading={trading}
                selected={selected === i}
                showSelection={dashboard.count > 1 && focused}
                multi={dashboard.count > 1}
                fixture={current.fixtureData}
                onSelect={select}
                onPickTicker={pickFor}
                onDay={onDay}
                detectionSelection={detectionSelection?.index === i && detectionSelection.symbol === symbol ? detectionSelection : undefined}
              />
            </div>
          ))}
        </div>
        <div className="flex min-h-0 min-w-0 shrink-0 flex-col gap-2 @4xl/panel:w-[400px]">
          <SideTabs rocket={feed.rocket} detection={feed.detection} onPick={app.loadIntoSelected} onEpisodePick={pickDetection} ross={feed.ross && <RossPanel channel={feed.ross} detection={feed.detection} onPick={app.loadIntoSelected} onEpisodePick={pickDetection} />} />
        </div>
        </div>
      </div>
      )}

      <TickerSearch open={searchOpen} onOpenChange={app.setSearchOpen} symbols={candidates.symbols} quotes={feed.quotes} mock={current.fixtureData} current={shown[selected] ?? ""} recents={recents} onSelect={app.loadIntoSelected} anchor={hasRail ? pillRef : undefined} />
    </div>
  );
}
