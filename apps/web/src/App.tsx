import {
  buildSlots,
  changeReference,
  currentReplayDay,
  formatChange,
  formatDayKey,
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
import { detectionOverlays } from "@stockwatcher/shared";
import { CandleChart, Icon, ReplayBar, Tape } from "@stockwatcher/chart-web";
import { ScrollArea, Tip, ToggleGroup, ToggleGroupItem, cn } from "@stockwatcher/chart-web/ui";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { ConnectionGate } from "./ConnectionGate.tsx";
import { DashboardPanels } from "./dashboard/DashboardPanels.tsx";
import { ZoneNote } from "./dashboard/ZoneNote.tsx";
import { DEFAULT_SYMBOL, followCandidates, followServer, initialSymbol, type SourceChoice } from "./feed/source.ts";
import { useCandidates } from "./feed/useCandidates.ts";
import type { ChartFeed } from "./feed/types.ts";
import { useChartFeed } from "./feed/useChartFeed.ts";
import { useDetection } from "./feed/useDetection.ts";
import { useHod } from "./feed/useHod.ts";
import { usePending } from "./feed/usePending.ts";
import { useZones } from "./feed/useZones.ts";
import { TickerLabel } from "./lib/TickerLabel.tsx";
import { NavToggle } from "./nav/NavToggle.tsx";
import { SettingsButton } from "./nav/settings.tsx";
import { usePaperTrading } from "./feed/usePaperTrading.ts";
import { useReplayDays } from "./feed/useReplayDays.ts";
import { useReplayAlerts } from "./feed/useReplayAlerts.ts";
import { useMediaQuery } from "./lib/useMediaQuery.ts";
import { offPanels } from "./practice/offPanels.ts";
import { NotInPractice, PaperSummary, PracticeBadge } from "./practice/PracticeMarker.tsx";
import { usePracticeToasts } from "./practice/practiceToasts.ts";
import { SearchPill } from "./search/SearchPill.tsx";
import { TickerSearch } from "./search/TickerSearch.tsx";
import { getAppStore, useApp } from "./state/store.ts";
import { StarButton } from "./favorites/StarButton.tsx";

const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);

const PHONE_CHART: Partial<ChartOptions> = { priceAxisWidth: 42, volumeHeight: 36, timeAxisHeight: 18, paneGap: 4, fontSize: 9, showProfile: true, profileWidth: 0.2, timeLabelSpacing: 60 };
/** The session volume profile sits inside the chart (the phone's way): the panels took its side column. */
const DESKTOP_CHART: Partial<ChartOptions> = { showProfile: true, profileWidth: 0.18 };

/** One line saying where the chart's data comes from, and whether it is fixture data. */
function sourceText(choice: SourceChoice): string {
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
  const fixture = choice.fixtureData;
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


/**
 * The practice page (the Chart nav item; replay parity plan Phase 1, owner 2026-11-23): the live
 * dashboard's panels around one chart, all read from the REPLAY market, with the replay bar, the
 * order ticket on the chart and the practice account. The only page that shows replay data; the
 * dashboard reads live only.
 */
export default function App() {
  return <ConnectionGate source="replay">{(c) => <PracticeApp choice={c.choice} feed={c.feed} />}</ConnectionGate>;
}

function PracticeApp({ choice, feed }: { choice: SourceChoice; feed: ChartFeed }) {
  // App state lives in the one store (state/store.ts, @stockwatcher/app-state). The ticker,
  // timeframe, bin, trade size, search recents, indicators, levels, chart and drawing settings
  // and drawings are synced live with every other client (desktop, web, phone) and kept on the
  // server; dialogs and the inspected candle are this page's own. The ticker and timeframe are
  // the chart page's own (`view`), never the dashboard's slots: picking a ticker here does not
  // move the live dashboard.
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
  const symbol = followCandidates(syncedSymbol ?? initialSymbol(choice, DEFAULT_SYMBOL), candidates.symbols, picked);
  // Drawings follow the shown symbol; before paint, so a ticker switch never flashes the old marks.
  useLayoutEffect(() => app.showSymbol(symbol), [app, symbol]);
  // The chart is controlled with the synced values (CandleChart keeps only this screen's layout).
  const indicators = useApp((s) => s.indicators);
  const chartSettings = useApp((s) => s.chartSettings);
  const layers = useApp((s) => s.layers);
  // What the chart shows of its indicators (and whether its zones layer is on), for the server to
  // compute (indicators.v1, zones.v1).
  const demand = useMemo(() => indicatorDemand(indicators, chartSettings, layers), [indicators, chartSettings, layers]);
  const { series, tape, clock, indicators: serverIndicators, levels: serverLevels, zones: serverZones, macdStyles } = useChartFeed(feed, symbol, timeframe, bin, undefined, demand);
  const trading = usePaperTrading(feed);
  const replayDays = useReplayDays(clockDay);
  const dayOptions = useMemo(() => replayDayOptions(replayDays.state?.days ?? []), [replayDays.state]);
  // Volume shelves for this symbol from the replay's Runners channel, as a dashboard chart has them.
  const zones = useZones(feed.rocket, symbol);

  // A Signals (or Ross) row picked in the panels: its ticker opens here and its episode is drawn
  // (the dashboard's ChartCell rule: only the picked row's episode, and only from the current run).
  const discovery = useDetection(feed.detection);
  const [episode, setEpisode] = useState<{ symbol: string; episodeId: string; runEpoch: string } | null>(null);
  const pickEpisode = useCallback(
    (s: string, episodeId: string, runEpoch: string) => {
      setEpisode(runEpoch === feed.detection?.get().frame?.runEpoch ? { symbol: s, episodeId, runEpoch } : null);
      app.openTicker(s);
    },
    [app, feed],
  );
  const clearEpisode = useCallback(() => setEpisode(null), []);
  const annotations = useMemo(() => {
    if (!episode || episode.symbol !== symbol || episode.runEpoch !== discovery.frame?.runEpoch) return [];
    return detectionOverlays(discovery.snapshot, symbol, episode.episodeId).map((x) => (discovery.connected && !discovery.resyncing ? x : { ...x, stale: true, label: `Last known · ${x.label}` }));
  }, [discovery, symbol, episode]);
  // A panel row loads its ticker into the practice chart (never the live dashboard's slots).
  const pick = useCallback((s: string) => app.openTicker(s), [app]);
  // The replay's own alerts as "Practice" toasts, real live ones meanwhile as "Live" toasts.
  usePracticeToasts(feed, pick);
  // The dashboard panels that do not run on the replay clock yet, said once.
  const hodState = useHod(feed.hod);
  const pendingState = usePending(feed.pending);
  const off = offPanels(hodState, pendingState);

  const phone = useMediaQuery("(max-width: 600px)");
  // Where the panels stack under the chart (about the dashboard layout's @4xl/panel), the tape is
  // under the chart too and keeps to a few rows.
  const stacked = useMediaQuery("(max-width: 1099px)");
  // A finger (phones, tablets; owner 2026-10-19). The floating drawing tools start hidden there
  // (they covered a third of a phone's plot; the toolbar's pencil shows them, and that choice is
  // kept).
  const coarse = useMediaQuery("(pointer: coarse)");
  const chartSettingsFromRemote = useApp((s) => s.chartSettingsFromRemote);
  // The Settings dialog's Drawing tab: which buttons the chart's drawing toolbox shows.
  const drawingSettings = useApp((s) => s.drawingSettings);
  const drawings = useApp((s) => s.drawings);
  const tradeQty = useApp((s) => s.tradeQty);
  const synced = {
    indicators,
    onIndicatorsChange: app.setIndicators,
    indicatorColorActions: app.indicatorColorActions,
    serverIndicators,
    macdStyles,
    serverLevels,
    serverZones,
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
    detection: annotations,
    volumeShelves: zones.bands,
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
  // The header pill the quick-open panel drops into (wider layouts).
  const pillRef = useRef<HTMLButtonElement>(null);
  const search = <TickerSearch open={searchOpen} onOpenChange={app.setSearchOpen} symbols={candidates.symbols} search={feed.quoteSearch} mock={current.fixtureData} current={symbol} recents={recents} onSelect={app.openTicker} />;
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
    // The server's schedule for the day when the chart has one (an early close ends at 13:00).
    return sessionBands({ year, month, day }, series?.sessions);
  }, [dayKey, series?.sessions]);

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
  const replayAlerts = useReplayAlerts(replayDay ?? clockDay);
  // The speed segments are 1× to 30× (chart-core REPLAY_SPEEDS): the server's Max is never offered
  // here (owner, 2026-11-23), since above about 30× the Runners board skips seconds.
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
  const shownDay = replayDay ?? clockDay;
  const dayLabel = shownDay ? (dayOptions.find((d) => d.value === shownDay)?.label ?? formatDayKey(shownDay)) : null;
  const dayShort = shownDay ? formatDayKey(shownDay) : null;
  const price = last ? formatPrice(last.c) : "—";
  const change = last && reference ? formatChange(reference.price, last.c) : "";
  const changeColor = change.startsWith("-") ? "text-critical" : "text-good";
  const chartLabel = `Practice chart: ${symbol} ${timeframe} candles, ${current.fixtureData ? "fixture data" : "recorded market data"} on the replay clock`;
  const tapeNote = current.fixtureData ? "Fixture data" : current.label;
  const mode = clock?.mode === "live" ? "live" : "replay";

  const binSelector = (
    <div className="flex items-center gap-1.5 text-[11px] text-text-faint">
      <span>Bin</span>
      <ToggleGroup aria-label="Profile bin width" size="sm" spacing={0.5} variant="segmented" value={[String(bin)]} onValueChange={(v) => v[0] && app.setBin(Number(v[0]))}>
        {PROFILE_BINS.map((b) => (
          <ToggleGroupItem key={b} value={String(b)} className="h-6 px-1.5 font-mono text-[11px]">
            {b.toFixed(2)}
          </ToggleGroupItem>
        ))}
      </ToggleGroup>
    </div>
  );

  const heading = (size: string) => (
    <div className="flex items-center gap-1">
      <div className="flex shrink-0 items-baseline gap-2">
        {tickerButton(size)}
        <span className={cn("font-mono tabular-nums", size)}>{price}</span>
        <Tip label="Change since the previous close">
          <span className={cn("pointer-events-auto font-mono text-[11px]", changeColor)}>{change}</span>
        </Tip>
      </div>
      {symbol && <StarButton symbol={symbol} className="pointer-events-auto" />}
    </div>
  );

  // The chart and its tape: the dashboard layout's main area (DashboardPanels' children). The tape
  // sits beside the chart where the panel is wide, under it otherwise (container queries: the
  // panel's own width, so the rail and the right column both count).
  const main = (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col gap-2 @5xl/panel:flex-row">
      <section aria-label={`Practice chart: ${symbol}`} className="flex min-h-[18rem] min-w-0 flex-1 flex-col overflow-hidden rounded-xl bg-card p-1.5">
        <ZoneNote note={layers.shelves ? zones.note : null} className="px-2 py-1" />
        <CandleChart
          className="min-h-0 flex-1"
          toolbar
          inspector
          toolbarInChart={false}
          headingInToolbar
          timeframeControl={phone ? "wheel" : "segments"}
          defaultToolsVisible={!coarse && !phone}
          heading={heading(phone ? "text-base" : "text-[15px]")}
          symbol={symbol}
          persistKey={CHART_KEY}
          {...synced}
          trading={trading}
          now={clock?.now ?? null}
          onTimeframeChange={app.setTimeframe}
          series={series}
          timeframe={timeframe}
          inspectT={inspectT}
          onInspectChange={app.setInspectT}
          options={phone ? PHONE_CHART : DESKTOP_CHART}
          barSpacing={phone ? 10 : undefined}
          label={chartLabel}
        />
      </section>
      <section aria-label="Time and sales" className="flex min-h-0 shrink-0 flex-col gap-1.5 rounded-xl bg-card p-3 @5xl/panel:w-60">
        <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
          <h2 className="panel-title flex-1">Time &amp; sales</h2>
          <span className="text-[10px] text-text-faint">{tapeNote}</span>
        </div>
        <Tape trades={tape} variant={phone ? "compact" : undefined} maxRows={stacked ? 6 : 12} />
        {binSelector}
      </section>
    </div>
  );

  return (
    <div className="flex h-full min-h-0 flex-col gap-2 p-3 text-foreground">
      {phone ? (
        // Phone layout (no rail): the practice marker and the Settings gear; the ticker is in the chart's toolbar.
        <header className="flex min-w-0 flex-col gap-1">
          <div className="flex items-center gap-2">
            <PracticeBadge day={dayLabel} dayShort={dayShort} off={off} />
            <PaperSummary state={trading?.state ?? null} className="ml-auto" />
            <SourceChip choice={current} mode={mode} />
            <SettingsButton />
          </div>
          <NotInPractice panels={off} />
          {search}
        </header>
      ) : (
        // Three parts, as on the dashboard: equal side parts put the search pill at the content's
        // exact centre, where the quick-open panel drops down. Here the sides never give way (the
        // Practice marker must always read in full); the pill gives up width first instead.
        <header className="flex items-center gap-4">
          <div className="flex min-w-max flex-1 basis-0 items-center gap-2">
            {/* The rail's collapse toggle at the far left. */}
            <NavToggle />
            <PracticeBadge day={dayLabel} dayShort={dayShort} off={off} />
            <NotInPractice panels={off} className="hidden min-[1800px]:inline" />
          </div>
          <SearchPill ref={pillRef} className="min-w-0 overflow-hidden" onOpen={() => app.setSearchOpen(true)} />
          <div className="flex min-w-max flex-1 basis-0 items-center justify-end gap-3">
            <PaperSummary state={trading?.state ?? null} />
            <SourceChip choice={current} mode={mode} />
          </div>
          {search}
        </header>
      )}

      {/* The live dashboard's panels around the chart (DashboardPanels: the right column and the
          bottom row, each panel shown while it has items), every one on the replay market. */}
      <ScrollArea className="flex-1" viewportClassName="@container/panel flex flex-col">
        <DashboardPanels ross={feed.ross} rocket={feed.rocket} detection={feed.detection} favorites={feed.favorites} pressure={feed.pressure} movers={feed.movers} hod={feed.hod} pending={feed.pending} onPick={pick} onEpisodePick={pickEpisode} onEpisodeClear={clearEpisode}>
          {main}
        </DashboardPanels>
      </ScrollArea>

      {/* The replay bar spans the page under the chart and the panels, as the approved two-tier
          design draws it (ReplayBar picks the compact one-row bar itself when the window is
          narrower). It pads itself. Outside the scroll, so it stays in reach. */}
      <section aria-label="Replay" className="shrink-0 rounded-[14px] bg-card">
        <ReplayBar {...replay} />
      </section>
    </div>
  );
}
