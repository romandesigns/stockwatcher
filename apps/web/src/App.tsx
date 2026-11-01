import {
  buildSlots,
  changeReference,
  currentReplayDay,
  formatChange,
  formatEtDateKey,
  formatPrice,
  indicatorDemand,
  lastCandle,
  replayDayOptions,
  sessionBands,
  tradingDayOf,
  type ChartOptions,
} from "@stockwatcher/chart-core";
import { PROFILE_BINS } from "@stockwatcher/app-state";
import { CandleChart, Icon, ReplayBar, Tape, VolumeProfile } from "@stockwatcher/chart-web";
import { Tip, ToggleGroup, ToggleGroupItem, cn } from "@stockwatcher/chart-web/ui";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { ConnectionGate } from "./ConnectionGate.tsx";
import { followCandidates, followServer, initialSymbol, type SourceChoice } from "./feed/source.ts";
import { useCandidates } from "./feed/useCandidates.ts";
import type { ChartFeed } from "./feed/types.ts";
import { INDICATOR_PARITY, useChartFeed } from "./feed/useChartFeed.ts";
import { TickerLabel } from "./lib/TickerLabel.tsx";
import { NavToggle } from "./nav/NavToggle.tsx";
import { SettingsButton } from "./nav/settings.tsx";
import { usePaperTrading } from "./feed/usePaperTrading.ts";
import { useReplayDays } from "./feed/useReplayDays.ts";
import { useReplayAlerts } from "./feed/useReplayAlerts.ts";
import { FIXTURE_SYMBOL } from "./fixtures/fixtureFeed.ts";
import { useMediaQuery } from "./lib/useMediaQuery.ts";
import { SearchPill } from "./search/SearchPill.tsx";
import { TickerSearch } from "./search/TickerSearch.tsx";
import { getAppStore, useApp } from "./state/store.ts";
import { StarButton } from "./favorites/StarButton.tsx";


const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);

const PHONE_CHART: Partial<ChartOptions> = { priceAxisWidth: 42, volumeHeight: 36, timeAxisHeight: 18, paneGap: 4, fontSize: 9, showProfile: true, profileWidth: 0.2, timeLabelSpacing: 60 };
const DESKTOP_CHART: Partial<ChartOptions> = {};

/** One line saying where the chart's data comes from, and whether it is fixture data. */
function sourceText(choice: SourceChoice): string {
  if (choice.kind === "fixture") return `In-browser fixture — not market data (${choice.reason})`;
  return choice.fixtureData ? `${choice.label} · fixture archive — not market data` : `${choice.label} · ${choice.reason}`;
}

/**
 * The chart's DEVICE-LOCAL saved state lives under this prefix (`sw-chart:toolbox`,
 * `sw-chart:panes`, `sw-chart:toolsVisible`, `sw-chart:tradeQty`: CandleChart keeps them). The
 * user's preferences and drawings are synced across devices instead (settings/sync.ts).
 */
const CHART_KEY = "sw-chart";
/** How long a picked replay day may read "Loading…" before the label gives up (a refused switch sends
 *  an error frame and never changes the day). A whole-market rebuild takes one to two minutes on the VPS. */
const PENDING_DAY_TIMEOUT_MS = 5 * 60_000;


/** Replay: a clock turned back. Live: a broadcast dot with its waves. */
const ReplayIcon = <Icon name="replay" />;
const LiveIcon = <Icon name="live" />;

/**
 * Where the data comes from, as an icon beside the logo (owner, 2026-10-17:
 * an icon instead of words): a turned-back clock for replay, a broadcast
 * mark for live. Fixture data keeps the warning tone so it can never pass
 * for market data; the full sentence is the tooltip and is read by screen
 * readers.
 */
function SourceChip({ choice, mode, className }: { choice: SourceChoice; mode: "replay" | "live"; className?: string }) {
  const fixture = choice.kind === "fixture" || choice.fixtureData;
  return (
    <Tip label={`${mode === "live" ? "Live market" : "Replay"}${fixture ? " · fixture data" : ""}: ${sourceText(choice)}`}>
      <span
        role="img"
        aria-label={`${mode === "live" ? "Live market" : "Replay"}: ${sourceText(choice)}`}
        className={cn(
          "inline-grid size-7 shrink-0 place-items-center rounded-full [&_svg]:size-3.5",
          fixture ? "bg-warning-bg text-warning" : mode === "live" ? "bg-good-bg text-good" : "bg-card text-muted-foreground",
          className,
        )}
      >
        {mode === "live" ? LiveIcon : ReplayIcon}
      </span>
    </Tip>
  );
}


/** The chart page: the shared connection gate (ConnectionGate.tsx), then the chart app. */
export default function App() {
  return <ConnectionGate source="replay">{(c) => <ChartApp choice={c.choice} feed={c.feed} />}</ConnectionGate>;
}

function ChartApp({ choice, feed }: { choice: SourceChoice; feed: ChartFeed }) {
  // App state lives in the one store (state/store.ts, @stockwatcher/app-state). The ticker,
  // timeframe, bin, trade size, search recents, indicators, levels, chart and drawing settings
  // and drawings are synced live with every other client (desktop, web, phone) and kept on the
  // server; dialogs and the inspected candle are this page's own.
  // The store's actions (the same functions in every state, so read once; values come through useApp).
  const [app] = useState(() => getAppStore().getState());
  const syncedSymbol = useApp((s) => s.symbol);
  const timeframe = useApp((s) => s.timeframe);
  const bin = useApp((s) => s.bin);
  const inspectT = useApp((s) => s.inspectT);
  // The trading day the replay clock is on; it changes on a day switch (and once a night while playing).
  const [clockDay, setClockDay] = useState<string | null>(null);
  const candidates = useCandidates(choice, clockDay, feed.symbols);
  // What is playing now (the day can be switched under a running page): drives the source chip,
  // the tape note, the chart's label and the search's mock rows. `choice` stays the startup
  // decision (server or fixture), which never changes.
  const current = useMemo(() => followServer(choice, candidates.server), [choice, candidates.server]);
  // What the chart shows: the ticker last opened on any device, or this source's first one. A
  // stock that traded out of the price band is no longer let through by the server, so the chart
  // moves to the first remaining candidate. That fallback is NOT written back: a device with a
  // momentarily stale candidate list must not move every other device's chart.
  // A ticker this device opened is never swapped for a candidate (app-state `picked`).
  const picked = useApp((s) => s.picked);
  const symbol = followCandidates(syncedSymbol ?? initialSymbol(choice, FIXTURE_SYMBOL), candidates.symbols, picked);
  // Drawings follow the shown symbol; before paint, so a ticker switch never flashes the old marks.
  useLayoutEffect(() => app.showSymbol(symbol), [app, symbol]);
  // The chart is controlled with the synced values (CandleChart keeps only this screen's layout).
  const indicators = useApp((s) => s.indicators);
  const chartSettings = useApp((s) => s.chartSettings);
  const layers = useApp((s) => s.layers);
  // What the chart shows of its indicators (and whether its zones layer is on), for the server to
  // compute (indicators.v1, zones.v1).
  const demand = useMemo(() => indicatorDemand(indicators, chartSettings, layers), [indicators, chartSettings, layers]);
  const { series, tape, clock, indicators: serverIndicators, levels: serverLevels, zones: serverZones } = useChartFeed(feed, symbol, timeframe, bin, undefined, demand);
  const trading = usePaperTrading(feed);
  const replayDays = useReplayDays(choice, clockDay);
  const dayOptions = useMemo(() => replayDayOptions(replayDays.state?.days ?? []), [replayDays.state]);

  const phone = useMediaQuery("(max-width: 600px)");
  // A finger (phones, tablets; owner 2026-10-19). The floating drawing tools start hidden there
  // (they covered a third of a phone's plot; the toolbar's pencil shows them, and that choice is
  // kept), and a tablet's toolbar moves out of the chart into its own row with the ticker: at
  // touch size it no longer fits beside the candle readout in the chart's top corners.
  const coarse = useMediaQuery("(pointer: coarse)");
  const chartSettingsFromRemote = useApp((s) => s.chartSettingsFromRemote);
  // The Settings dialog's Drawing tab: which buttons the chart's drawing toolbox shows.
  const drawingSettings = useApp((s) => s.drawingSettings);
  const drawings = useApp((s) => s.drawings);
  const tradeQty = useApp((s) => s.tradeQty);
  const synced = {
    indicators,
    onIndicatorsChange: app.setIndicators,
    serverIndicators,
    serverLevels,
    serverZones,
    indicatorParity: INDICATOR_PARITY,
    layers,
    onLayersChange: app.setLayers,
    settings: chartSettings,
    onSettingsChange: app.setChartSettings,
    settingsFromRemote: chartSettingsFromRemote,
    drawingSettings,
    drawings,
    onDrawingsChange: app.setDrawings,
    tradeQty,
    onTradeQtyChange: app.setTradeQty,
  };
  // Settings: one dialog for every page, rendered by the Shell (nav/settings.tsx). Wider screens
  // open it from the rail's footer; the phone layout (no rail) keeps a gear in this header.
  // Ticker quick-open, VS Code's Ctrl+P (Cmd+P on a Mac): the browser's print shortcut is taken
  // over on purpose while the app is focused.
  const searchOpen = useApp((s) => s.searchOpen);
  const recents = useApp((s) => s.searchRecents);
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
  // On the desktop the panel drops down right under the header's search pill, at its width.
  // The header pill the quick-open panel drops into (wider layouts).
  const pillRef = useRef<HTMLButtonElement>(null);
  const search = <TickerSearch open={searchOpen} onOpenChange={app.setSearchOpen} symbols={candidates.symbols} quotes={feed.quotes} search={feed.quoteSearch} mock={current.fixtureData} current={symbol} recents={recents} onSelect={app.openTicker} anchor={phone ? undefined : pillRef} />;
  const tickerButton = (size: string) => (
    <Tip label={`Search tickers (${isMac ? "⌘" : "Ctrl+"}P) · tap, then hold to copy`}>
      <TickerLabel symbol={symbol} onTap={() => app.setSearchOpen(true)} className={size} />
    </Tip>
  );

  const slots = useMemo(() => (series ? buildSlots(series.candles, series.gaps) : []), [series]);
  const last = useMemo(() => lastCandle(slots), [slots]);

  // Change vs the previous close when the server knows it, else the anchor session's first
  // candle (changeReference in chart-core; the search rows use the same rule).
  const reference = useMemo(() => changeReference(series), [series]);

  // Bands change only when the trading day does, not on every clock tick.
  const dayKey = clock ? formatEtDateKey(tradingDayOf(clock.now)) : "";
  // Derived during render (React's "adjusting state when a prop changes"), not in an effect: the hooks
  // above that read it run before the clock is known, and an effect would cost an extra commit.
  if ((dayKey || null) !== clockDay) setClockDay(dayKey || null);
  const bands = useMemo(() => {
    if (!dayKey) return [];
    const [year, month, day] = dayKey.split("-").map(Number) as [number, number, number];
    return sessionBands({ year, month, day });
  }, [dayKey]);

  // A day picked here that the server is still switching to (ReplayBar `pendingDay`). Cleared
  // during render once the replay names it (the clock's new trading day refetches the day state),
  // and after a timeout in case the server refused it (an error frame, no day change).
  const [pendingDay, setPendingDay] = useState<string | null>(null);
  const replayDay = currentReplayDay(replayDays.state, clock);
  if (pendingDay && replayDay === pendingDay) setPendingDay(null);
  useEffect(() => {
    if (!pendingDay) return;
    const id = setTimeout(() => setPendingDay(null), PENDING_DAY_TIMEOUT_MS);
    return () => clearTimeout(id);
  }, [pendingDay]);
  // The day's Runners entries: the scrubber's alert dots and the Prev/Next alert jumps.
  const replayAlerts = useReplayAlerts(current, replayDay ?? clockDay);
  const replay = {
    clock,
    onPlayPause: (play: boolean) => feed.control({ action: play ? "play" : "pause" }),
    onSpeedChange: (speed: number) => feed.control({ action: "speed", value: speed }),
    // Jump the replay; the server rebuilds from the session start and resends snapshots.
    onSeek: (t: number) => feed.control({ action: "seek", value: t }),
    bands,
    // The histogram re-sums the shown chart's candle volume (what fed the old session timeline).
    candles: series?.candles ?? [],
    symbol,
    price: last ? formatPrice(last.c) : "",
    // The ticker search; its pick (app.openTicker) replaces this page's chart.
    onTickerClick: () => app.setSearchOpen(true),
    alerts: replayAlerts,
    // An "any ticker" alert jump: that ticker replaces the chart, as a search pick would.
    onOpenTicker: (s: string) => app.openTicker(s),
    days: dayOptions,
    day: replayDay,
    pendingDay,
    // The server rebuilds on the new day and resends every snapshot; the clock's new trading day
    // then refetches the candidates and this list.
    onDayChange: (day: string) => {
      setPendingDay(day);
      feed.control({ action: "day", value: day });
    },
    onDayMenuOpenChange: (open: boolean) => open && replayDays.refresh(),
  };
  const price = last ? formatPrice(last.c) : "—";
  const change = last && reference ? formatChange(reference.price, last.c) : "";
  const changeColor = change.startsWith("-") ? "text-critical" : "text-good";
  const chartLabel = `${symbol} ${timeframe} candles, ${current.fixtureData ? "fixture data" : "market data"} from ${current.kind === "server" ? "the server" : "the in-browser fixture"}`;
  const tapeNote = current.fixtureData ? "Fixture data" : current.label;
  const mode = clock?.mode === "live" ? "live" : "replay";

  const binSelector = (
    <>
      <span>Bin</span>
      <ToggleGroup aria-label="Profile bin width" size="sm" spacing={0.5} variant="segmented" value={[String(bin)]} onValueChange={(v) => v[0] && app.setBin(Number(v[0]))}>
        {PROFILE_BINS.map((b) => (
          <ToggleGroupItem key={b} value={String(b)} className="h-6 px-1.5 font-mono text-[11px]">
            {b.toFixed(2)}
          </ToggleGroupItem>
        ))}
      </ToggleGroup>
    </>
  );

  if (phone) {
    return (
      // min-h-full: the Shell's page area already stops above the tab bar (a dvh here overshot it).
      <div className="flex min-h-full flex-col gap-2.5 bg-background p-3 text-foreground">
        <header className="flex items-center gap-2">
          <div className="flex min-w-0 items-center gap-1">
            <div className="flex shrink-0 items-baseline gap-2">
              {tickerButton("text-base")}
              <span className="font-mono text-base tabular-nums">{price}</span>
              <span className={cn("font-mono text-[11px]", changeColor)}>{change}</span>
            </div>
            {symbol && <StarButton symbol={symbol} />}
          </div>
          <SourceChip className="ml-auto" choice={current} mode={mode} />
          <SettingsButton />
          {search}
        </header>
        {/* The timeframe in the toolbar row as the wheel (as on the native phone app), not four
            segments floating over the chart's top-right, where they ran into the candle readout. */}
        <CandleChart className="min-h-[360px] flex-1" toolbar inspector headingInToolbar timeframeControl="wheel" defaultToolsVisible={false} symbol={symbol} persistKey={CHART_KEY} {...synced} trading={trading} now={clock?.now ?? null} onTimeframeChange={app.setTimeframe} series={series} timeframe={timeframe} inspectT={inspectT} onInspectChange={app.setInspectT} options={PHONE_CHART} barSpacing={10} label={chartLabel} />
        {/* The compact replay bar (one row; "more" opens the rest over the chart). It pads itself
            per layout, so the section only gives it its surface. */}
        <section aria-label="Replay" className="rounded-[18px] bg-card">
          <ReplayBar {...replay} />
        </section>
        <section aria-label="Time and sales" className="flex flex-col gap-1.5 rounded-xl bg-card p-3">
          <div className="flex items-baseline">
            <h2 className="panel-title flex-1">Time &amp; sales</h2>
            <span className="text-[10px] text-text-faint">{tapeNote}</span>
          </div>
          <Tape trades={tape} variant="compact" maxRows={8} />
        </section>
      </div>
    );
  }

  return (
    <div className="flex min-h-full flex-col bg-background text-foreground">
      {/* Three columns (logo | search | data source): equal side columns put the search pill at the
          page's exact centre, where the quick-open panel drops down, at the same width. */}
      <header className="grid grid-cols-[1fr_auto_1fr] items-center gap-4 px-4 py-2.5">
        {/* The rail's collapse toggle, far left (the brand is in the rail); the first column still
            balances the third, so the search pill stays centred. */}
        <div className="flex items-center">
          <NavToggle />
        </div>

        <SearchPill ref={pillRef} onOpen={() => app.setSearchOpen(true)} />
        <div className="flex items-center gap-2 justify-self-end">
          <SourceChip choice={current} mode={mode} />
        </div>
        {search}
      </header>

      <div className="flex min-h-0 flex-1 flex-col gap-2 px-4 pb-3 min-[901px]:flex-row">
        <main className="flex min-w-0 flex-1 flex-col gap-2">
          <div className="flex min-h-[360px] flex-1 flex-col gap-1">
            <CandleChart
              className="min-h-[360px] flex-1"
              toolbar
              inspector
              toolbarInChart={!coarse}
              headingInToolbar={coarse}
              defaultToolsVisible={!coarse}
              heading={
                <div className="flex items-center gap-1">
                  <div className="flex shrink-0 items-baseline gap-2">
                    {tickerButton("text-[15px]")}
                    <span className="font-mono text-[15px] tabular-nums">{price}</span>
                    <Tip label="Change since the session's first candle">
                      <span className={cn("pointer-events-auto font-mono text-[11px]", changeColor)}>{change}</span>
                    </Tip>
                  </div>
                  {symbol && <StarButton symbol={symbol} className="pointer-events-auto" />}
                </div>
              } symbol={symbol} persistKey={CHART_KEY} {...synced} trading={trading} now={clock?.now ?? null} onTimeframeChange={app.setTimeframe} series={series} timeframe={timeframe} inspectT={inspectT} onInspectChange={app.setInspectT} options={DESKTOP_CHART} label={chartLabel} />
          </div>
        </main>

        <aside className="grid gap-2 min-[601px]:max-[900px]:grid-cols-2 min-[901px]:flex min-[901px]:w-[300px] min-[901px]:shrink-0 min-[901px]:flex-col">
          <section aria-label="Session volume profile" className="rounded-xl bg-card p-3">
            <VolumeProfile className="h-[210px]" profile={series?.profile} actions={binSelector} />
          </section>
          <section aria-label="Time and sales" className="flex min-h-0 flex-1 flex-col gap-1.5 rounded-xl bg-card p-3">
            <h2 className="panel-title">Time &amp; sales</h2>
            <Tape trades={tape} maxRows={12} />
          </section>
        </aside>
      </div>
      {/* The replay bar spans the page under the chart and the side panels, as the approved
          two-tier design draws it: its one-row top tier needs the full width (ReplayBar picks
          the compact one-row bar itself when the window is narrower). It pads itself. */}
      <section aria-label="Replay" className="mx-4 mb-3 rounded-[14px] bg-card">
        <ReplayBar {...replay} />
      </section>
    </div>
  );
}
