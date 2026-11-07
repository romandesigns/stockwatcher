import { dayChartContext, DEFAULT_CHART_SETTINGS, DEFAULT_LAYERS, formatEtTime12, indicatorDemand, tickerIndicatorValues, type ChartLayers, type ChartSettings, type EventMarker, type IndicatorSpec } from "@stockwatcher/chart-core";
import { CandleChart, Icon } from "@stockwatcher/chart-web";
import { cn, Collapsible, CollapsibleContent, CollapsibleTrigger, ScrollArea, Tip } from "@stockwatcher/chart-web/ui";
import { tickerPageHash, tickerPageView, type TickerAlertView, type TickerChannel, type TickerMissedView, type TickerPackageWire, type TickerPageView, type TickerRequest, type TickerState, type Tone } from "@stockwatcher/shared";
import { useEffect, useMemo, useState, type ReactElement } from "react";
import { useTickerPackage } from "./useTickerPage.ts";

/**
 * The approved design's chart: candles, the day's high and low, the markers. The key-level lines
 * (premarket high/low, HOD/LOD, previous close) are off: their labels sat on top of the numbered
 * markers and the day's own high/low labels, and the page states those levels in its tiles.
 */
const TICKER_LAYERS: ChartLayers = { ...DEFAULT_LAYERS, premarket: false, dayRange: false, prevClose: false };

const time = (ms: number, seconds?: boolean) => formatEtTime12(ms, seconds);

/**
 * The page chart's indicators, remembered in this browser (a viewing preference for this page,
 * apart from the main chart's synced list): kept across days and visits.
 */
const INDICATORS_KEY = "stockwatcher:insights-ticker:indicators";
function loadIndicators(): IndicatorSpec[] {
  try {
    const v: unknown = JSON.parse(localStorage.getItem(INDICATORS_KEY) ?? "[]");
    return Array.isArray(v) ? v.filter((s): s is IndicatorSpec => typeof s === "object" && s !== null && typeof (s as IndicatorSpec).id === "string" && typeof (s as IndicatorSpec).kind === "string") : [];
  } catch {
    return [];
  }
}
function saveIndicators(specs: IndicatorSpec[]) {
  try {
    localStorage.setItem(INDICATORS_KEY, JSON.stringify(specs));
  } catch {
    // Private window or blocked storage: the choice lasts for this visit.
  }
}

const TONE: Record<Tone, string> = { neutral: "text-foreground", up: "text-good", down: "text-critical", dim: "text-muted-foreground" };

/**
 * The Insights ticker page (owner, 2026-10-31, approved design): one symbol on one trading day,
 * entirely from the server's package (crates/server/src/insights): the top bar with the day
 * switcher, the day's 1-minute chart on top (the thing to watch; owner, 2026-11-01) with every sent
 * alert numbered and every missed setup ringed, the stat strip in one row, the notification history with each alert's outcome, the scorecard, halts
 * and LULD, and the missed setups with the rule that held each. Rendering only: every number is
 * the server's.
 */
export function TickerPage({ channel, want, onBack }: { channel: TickerChannel | undefined; want: TickerRequest; onBack: () => void }) {
  const state = useTickerPackage(channel, want);
  // Held here, not in the chart: the chart remounts with every day, the choice should not.
  const [indicators, setIndicatorsState] = useState<IndicatorSpec[]>(loadIndicators);
  const setIndicators = (next: IndicatorSpec[]) => {
    setIndicatorsState(next);
    saveIndicators(next);
  };
  const pkg = state.pkg && state.pkg.symbol === want.symbol && state.pkg.day === want.day ? state.pkg : null;
  const view = useMemo(() => (pkg ? tickerPageView(pkg, time) : null), [pkg]);
  const go = (day: string | null) => {
    // The day switcher replaces the entry, keeping its state (whether it was opened from the
    // log): Back still returns to the notification log. replaceState fires no hashchange, so
    // the hash's followers (nav/routes useHash) are told here.
    if (!day) return;
    window.history.replaceState(window.history.state, "", tickerPageHash(want.symbol, day));
    window.dispatchEvent(new Event("hashchange"));
  };
  return (
    <ScrollArea className="min-h-0 flex-1" viewportClassName="@container/ticker">
      <div className="mx-auto flex w-full max-w-[1360px] flex-col gap-4 pb-6 @3xl/ticker:gap-5">
        <TopBar symbol={want.symbol} name={view?.name ?? null} day={want.day} prevDay={pkg ? (view?.prevDay ?? null) : null} nextDay={pkg ? (view?.nextDay ?? null) : null} onBack={onBack} onDay={go} />
        {state.available === false ? (
          <p className="m-0 text-[13px] text-text-faint">This server does not build ticker pages yet. The notification log still lists every alert.</p>
        ) : !view || !pkg ? (
          <p className="m-0 text-[13px] text-text-faint" aria-live="polite">
            Building {want.symbol} for {want.day}: the server replays the whole day…
          </p>
        ) : view.unavailable ? (
          <p className="m-0 max-w-md text-[13px] text-text-faint">{view.unavailable}</p>
        ) : (
          <Body pkg={pkg} view={view} chart={{ channel, state, indicators, onIndicatorsChange: setIndicators }} />
        )}
      </div>
    </ScrollArea>
  );
}

function TopBar({ symbol, name, day, prevDay, nextDay, onBack, onDay }: { symbol: string; name: string | null; day: string; prevDay: string | null; nextDay: string | null; onBack: () => void; onDay: (d: string | null) => void }) {
  const label = useMemo(() => new Intl.DateTimeFormat("en-US", { weekday: "short", month: "short", day: "numeric", year: "numeric", timeZone: "UTC" }).format(new Date(`${day}T12:00:00Z`)), [day]);
  return (
    <header className="flex flex-wrap items-center gap-x-3 gap-y-2">
      <button
        type="button"
        onClick={onBack}
        aria-label="Back to the notification log"
        className="inline-flex h-9 items-center gap-1.5 rounded-lg bg-card px-2.5 text-[13px] font-medium text-muted-foreground outline-none transition-colors duration-150 hover:bg-bg-row hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring pointer-coarse:h-11 motion-reduce:transition-none [&_svg]:size-4"
      >
        <Icon name="chevronLeft" />
        <span className="hidden @md/ticker:inline">Notifications</span>
      </button>
      <div className="flex min-w-0 items-baseline gap-2">
        <h2 className="m-0 font-mono text-xl font-bold tracking-tight">{symbol}</h2>
        {name && <span className="truncate text-[13px] text-muted-foreground">{name}</span>}
      </div>
      <div className="ml-auto flex items-center gap-1">
        <DayStep dir="prev" day={prevDay} onDay={onDay} />
        <span className="inline-flex h-9 items-center gap-1.5 rounded-lg bg-bg-row px-3 text-[13px] font-medium pointer-coarse:h-11 [&_svg]:size-4">
          <Icon name="calendar" />
          <span className="@md/ticker:hidden">{label.replace(/^\w+, /, "").replace(/, \d{4}$/, "")}</span>
          <span className="hidden @md/ticker:inline">{label}</span>
        </span>
        <DayStep dir="next" day={nextDay} onDay={onDay} />
      </div>
    </header>
  );
}

function DayStep({ dir, day, onDay }: { dir: "prev" | "next"; day: string | null; onDay: (d: string | null) => void }) {
  const label = dir === "prev" ? "Previous trading day" : "Next trading day";
  return (
    <Tip label={day ? `${label} · ${day}` : label}>
      <button
        type="button"
        aria-label={day ? `${label}, ${day}` : `${label} (none)`}
        disabled={!day}
        onClick={() => onDay(day)}
        className="inline-grid size-9 place-items-center rounded-lg bg-card text-muted-foreground outline-none transition-colors duration-150 hover:bg-bg-row hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-40 disabled:hover:bg-card pointer-coarse:size-11 motion-reduce:transition-none [&_svg]:size-4"
      >
        <Icon name={dir === "prev" ? "chevronLeft" : "chevronRight"} />
      </button>
    </Tip>
  );
}

/** What the page chart's indicators need: the channel to ask the server, its answer, and the list. */
interface ChartIndicatorsProps {
  channel: TickerChannel | undefined;
  state: TickerState;
  indicators: IndicatorSpec[];
  onIndicatorsChange: (specs: IndicatorSpec[]) => void;
}

function Body({ pkg, view, chart }: { pkg: TickerPackageWire; view: TickerPageView; chart: ChartIndicatorsProps }) {
  // The history row under the pointer rings its marker on the chart; a clicked row turns its
  // marker orange until it is clicked again or another row is (owner, 2026-11-01).
  const [focus, setFocus] = useState<number | null>(null);
  const [selected, setSelected] = useState<number | null>(null);
  const onSelect = (n: number) => setSelected((cur) => (cur === n ? null : n));
  return (
    <>
      <DayChart pkg={pkg} view={view} focus={focus} selected={selected} chart={chart} />
      <Stats view={view} />
      {/* Two columns once there is room: the history with the missed setups under it, beside the
          scorecard and halts. With the missed setups in their own full-width row, a short history
          left an empty block beside the taller scorecard (owner, 2026-11-01). One column below
          that, in reading order: history, scorecard and halts, then the missed setups. */}
      <div className="grid grid-cols-1 items-start gap-5 [grid-template-areas:'hist'_'side'_'missed'] @5xl/ticker:grid-cols-[minmax(0,1fr)_340px] @5xl/ticker:grid-rows-[auto_1fr] @5xl/ticker:[grid-template-areas:'hist_side'_'missed_side']">
        <div className="flex min-w-0 [grid-area:hist]">
          <History view={view} onFocus={setFocus} selected={selected} onSelect={onSelect} />
        </div>
        <aside className="flex min-w-0 flex-col gap-5 [grid-area:side]">
          <Scorecard view={view} />
          <Halts view={view} />
        </aside>
        <div className="min-w-0 [grid-area:missed]">
          <Missed view={view} />
        </div>
      </div>
    </>
  );
}

function Stats({ view }: { view: TickerPageView }) {
  return (
    <section aria-label="The day in numbers" className="@container/stats">
      {/* One row of eight compact tiles when there is room (owner, 2026-11-01: "a single row instead
          of two"; the three price ranges get the extra width so each reads on one line); 4 or 2 equal
          columns below that, so eight tiles always fill their rows (no orphan).
          In the single row each note moves into the tile's tooltip; with fewer columns it shows. */}
      <ul className="m-0 grid list-none grid-cols-2 gap-2 p-0 @3xl/stats:grid-cols-4 @6xl/stats:grid-cols-[1fr_1fr_1.45fr_1.45fr_1.45fr_1fr_1fr_1fr]">
        {view.stats.map((s) => (
          <StatTile key={s.id} note={s.note}>
          <li className="flex min-w-0 flex-col gap-0.5 rounded-xl bg-card px-3 py-2.5 @6xl/stats:px-2.5 @6xl/stats:py-2">
            <span className="truncate text-xs text-muted-foreground">{s.label}</span>
            {/* Wraps rather than truncates: a price range must read whole on a 320 px phone. */}
            <span className={cn("font-mono text-[15px] font-medium [overflow-wrap:anywhere] tabular-nums @6xl/stats:text-[13px] @6xl/stats:whitespace-nowrap", TONE[s.tone])}>{s.value}</span>
            {/* The note wraps too: truncated, "no previous close on fi…" and "high 4:5…" lost their point at 320 px. */}
            <span className="text-[11px] text-text-faint @6xl/stats:hidden">{s.note || " "}</span>
          </li>
          </StatTile>
        ))}
      </ul>
    </section>
  );
}

/** A tile's note as its tooltip: in the single row the note line is hidden, so hovering shows it. */
function StatTile({ note, children }: { note: string; children: ReactElement }) {
  return note ? <Tip label={note}>{children}</Tip> : children;
}

function DayChart({ pkg, view, focus, selected, chart }: { pkg: TickerPackageWire; view: TickerPageView; focus: number | null; selected: number | null; chart: ChartIndicatorsProps }) {
  const [settings, setSettings] = useState<ChartSettings>(() => ({ ...DEFAULT_CHART_SETTINGS, highLow: true }));
  const [layers, setLayers] = useState<ChartLayers>(TICKER_LAYERS);
  // Indicators are computed on the server over this package's bars (`insights.ticker.indicators.v1`);
  // the chart draws them as sent. The demand is the main chart's (none while the layer is hidden,
  // the regular session only with extended hours off).
  const { channel, state, indicators, onIndicatorsChange } = chart;
  const demand = useMemo(() => indicatorDemand(indicators, settings), [indicators, settings]);
  useEffect(() => channel?.setIndicators({ indicators: demand.indicators, rth: demand.rth }), [channel, demand]);
  // Absent (an older server) = no lines and the chart's "needs a server update" note; null = asked.
  const serverIndicators = useMemo(() => (state.indicatorsAvailable ? tickerIndicatorValues(state.indicators) : undefined), [state.indicatorsAvailable, state.indicators]);
  // The chart's tools (indicators, levels, drawing) sit in the heading row, not a row of their own.
  const [toolbarSlot, setToolbarSlot] = useState<HTMLElement | null>(null);
  // Levels, ranges, zones and the session schedule are the server's, computed over these bars
  // (the package's `chart`); the chart reads nothing off its candles.
  const context = useMemo(() => dayChartContext(pkg.chart), [pkg]);
  const series = useMemo(() => ({ candles: pkg.bars, gaps: [], profile: null, halts: pkg.halts?.halts ?? [], ...(context.sessions ? { sessions: context.sessions } : {}) }), [pkg, context]);
  const markers = useMemo<EventMarker[]>(() => view.markers.map((m) => ({ kind: m.kind, at: m.at, price: m.price, n: m.n, focused: m.kind === "alert" && m.n === focus, selected: m.kind === "alert" && m.n === selected })), [view.markers, focus, selected]);
  const sent = view.markers.filter((m) => m.kind === "alert").length;
  const missed = view.markers.length - sent;
  return (
    <section aria-label="Price chart with alerts" className="flex flex-col gap-2 rounded-2xl bg-card p-3 @3xl/ticker:p-4">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <span className="text-sm font-semibold">{view.dayShort} · 1-minute</span>
        <span className="hidden text-xs text-muted-foreground @2xl/ticker:inline">Alerts are numbered as in the history below</span>
        <span className="ml-auto inline-flex items-center gap-1.5 text-xs text-muted-foreground">
          <span className="size-2.5 rounded-full bg-primary" aria-hidden />
          Sent alert
        </span>
        <span className="inline-flex items-center gap-1.5 text-xs text-muted-foreground">
          <span className="size-2.5 rounded-full shadow-[inset_0_0_0_2px_var(--text-dim)]" aria-hidden />
          Missed setup
        </span>
        <div ref={setToolbarSlot} className="flex items-center" />
      </div>
      <div className="h-[clamp(240px,42vh,440px)] min-h-0">
        {/* Keyed by the day: another day reopens on its own focus window. */}
        <CandleChart
          key={pkg.day}
          className="h-full"
          series={series}
          timeframe="1m"
          symbol={pkg.symbol}
          settings={settings}
          onSettingsChange={setSettings}
          eventMarkers={markers}
          serverLevels={context.levels}
          serverZones={settings.extendedHours ? context.zones : context.zonesRth}
          initialFocus={pkg.focus}
          layers={layers}
          onLayersChange={setLayers}
          toolbar
          toolbarContainer={toolbarSlot}
          // The drawing tools stay folded until the pencil asks for them: this chart is for reading the day.
          defaultToolsVisible={false}
          indicators={indicators}
          onIndicatorsChange={onIndicatorsChange}
          serverIndicators={serverIndicators}
          macdStyles={state.macdStyles ?? false}
          inspector
          priceAxisMenu={false}
          label={`${pkg.symbol} on ${view.dayLabel}, 1-minute candles, ${sent} sent alert${sent === 1 ? "" : "s"} and ${missed} missed setup${missed === 1 ? "" : "s"} marked`}
        />
      </div>
    </section>
  );
}

function History({ view, onFocus, selected, onSelect }: { view: TickerPageView; onFocus: (n: number | null) => void; selected: number | null; onSelect: (n: number) => void }) {
  return (
    <section aria-labelledby="ticker-history" className="@container/hist flex min-w-0 flex-[999_1_640px] flex-col gap-2">
      <div className="flex items-baseline gap-2.5">
        <h3 id="ticker-history" className="m-0 text-[15px] font-semibold">
          Notification history
        </h3>
        <span className="text-xs text-muted-foreground">{view.alertSummary}</span>
        <span className="ml-auto hidden text-xs text-text-faint @2xl/hist:inline">Click an alert to find it on the chart</span>
      </div>
      {view.alerts.length === 0 ? (
        <p className="m-0 text-[13px] text-text-faint">No notification for {view.symbol} that day.</p>
      ) : (
        <>
          <div aria-hidden className={cn("hidden gap-3 px-3.5 text-xs text-text-faint @3xl/hist:grid", COLS)}>
            <span>#</span>
            <span>Time</span>
            <span>Alert</span>
            <span>Price</span>
            <span>Best after</span>
            <span>+5%</span>
            <span>Stop</span>
          </div>
          <ol className="m-0 flex list-none flex-col gap-1 p-0">
            {view.alerts.map((a) => (
              <AlertRow key={a.key} a={a} onFocus={onFocus} selected={a.marker !== null && a.marker === selected} onSelect={onSelect} />
            ))}
          </ol>
        </>
      )}
    </section>
  );
}

const COLS = "grid-cols-[1.75rem_5.5rem_minmax(9rem,1.4fr)_5.5rem_minmax(8rem,1fr)_5.5rem_5.5rem]";

function MarkerDot({ a, selected }: { a: TickerAlertView; selected?: boolean }) {
  return a.marker ? (
    <span className={cn("inline-grid size-[22px] shrink-0 place-items-center rounded-full text-[11px] font-bold text-primary-foreground transition-colors duration-150 motion-reduce:transition-none", selected ? "bg-flame" : "bg-primary")} aria-label={`Alert ${a.marker}`}>
      {a.marker}
    </span>
  ) : (
    <span className="inline-grid size-[22px] shrink-0 place-items-center rounded-full bg-warning-bg text-warning [&_svg]:size-3" aria-label="Held">
      <Icon name="pause" />
    </span>
  );
}

/**
 * One alert with its outcome: a table row when the history has the room (the approved desktop
 * layout), a card with its outcomes on one line when it does not (the approved phone layout).
 * The container decides, not the viewport, so a narrow column beside the scorecard gets cards too.
 */
function AlertRow({ a, onFocus, selected, onSelect }: { a: TickerAlertView; onFocus: (n: number | null) => void; selected: boolean; onSelect: (n: number) => void }) {
  const text = <span className={cn("truncate text-xs", a.held ? "text-warning" : "text-muted-foreground")}>{a.held ?? a.detail}</span>;
  // A held alert has no marker on the chart, so there is nothing to pick out: not clickable.
  const marker = a.marker || null;
  return (
    <li
      onPointerEnter={() => onFocus(marker)}
      onPointerLeave={() => onFocus(null)}
      className={cn("rounded-xl transition-colors duration-150 motion-reduce:transition-none", selected ? "bg-bg-raised" : "bg-card", marker !== null && !selected && "hover:bg-bg-row")}
    >
      <button
        type="button"
        disabled={marker === null}
        aria-pressed={marker === null ? undefined : selected}
        aria-label={marker === null ? undefined : `${selected ? "Unmark" : "Mark"} alert ${marker} on the chart`}
        onClick={() => marker !== null && onSelect(marker)}
        className="block w-full cursor-pointer rounded-xl text-left outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-default"
      >
      <div className={cn("hidden items-center gap-3 px-3.5 py-2.5 @3xl/hist:grid", COLS)}>
        <MarkerDot a={a} selected={selected} />
        <span className="font-mono text-[13px] text-muted-foreground tabular-nums">{a.time}</span>
        <span className="flex min-w-0 flex-col gap-0.5">
          <span className="truncate text-[13px] font-medium">{a.kind}</span>
          {text}
        </span>
        <span className="font-mono text-[13px] tabular-nums">{a.price}</span>
        <span className="flex min-w-0 flex-col gap-0.5">
          <span className={cn("truncate font-mono text-[13px] tabular-nums", TONE[a.bestTone])}>
            {a.best} · {a.bestPct}
          </span>
          {a.dip ? <span className={cn("truncate text-xs", TONE[a.dipTone])}>{a.dip}</span> : null}
        </span>
        <span className={cn("font-mono text-[13px] tabular-nums", TONE[a.plus5Tone])}>{a.plus5}</span>
        <span className={cn("text-[13px]", TONE[a.stopTone])}>{a.stop}</span>
      </div>
      <div className="flex flex-col gap-1 px-3 py-2.5 @3xl/hist:hidden">
        <div className="flex min-w-0 items-center gap-2">
          <MarkerDot a={a} selected={selected} />
          <span className="shrink-0 font-mono text-xs whitespace-nowrap text-muted-foreground tabular-nums">{a.time}</span>
          <span className="min-w-0 truncate text-[13px] font-medium">{a.kind}</span>
          <span className="ml-auto font-mono text-[13px] tabular-nums">{a.price}</span>
        </div>
        {text}
        <div className="flex flex-wrap gap-x-3 gap-y-0.5 text-xs">
          <span className={cn("font-mono tabular-nums", TONE[a.bestTone])}>best {a.bestPct}</span>
          {a.dip ? <span className={TONE[a.dipTone]}>{a.dip}</span> : null}
          <span className={TONE[a.plus5Tone]}>+5% {a.plus5}</span>
          <span className={TONE[a.stopTone]}>stop {a.stop}</span>
        </div>
      </div>
      </button>
    </li>
  );
}

function Scorecard({ view }: { view: TickerPageView }) {
  return (
    <section aria-labelledby="ticker-score" className="flex flex-col gap-2">
      <h3 id="ticker-score" className="m-0 text-[15px] font-semibold">
        Scorecard
      </h3>
      <ul className="m-0 grid list-none grid-cols-2 gap-2 p-0">
        {view.score.map((k) => (
          <li key={k.label} className="flex min-w-0 flex-col gap-0.5 rounded-xl bg-card px-3 py-2.5">
            <span className="text-xs text-muted-foreground">{k.label}</span>
            <span className="truncate font-mono text-[17px] font-medium tabular-nums">{k.value}</span>
            <span className="truncate text-[11px] text-text-faint">{k.note}</span>
          </li>
        ))}
      </ul>
      {view.byKind.length > 0 && (
        <ul className="m-0 flex list-none flex-col gap-1 p-0">
          {view.byKind.map((b) => (
            <li key={b.key} className="flex flex-wrap items-center gap-x-2.5 gap-y-0.5 rounded-lg bg-bg-panel/60 px-3 py-2 text-[13px]">
              {/* Never squeezed: on a 320 px phone the value drops to its own line, right-aligned. */}
              <span className="flex-[1_0_auto] whitespace-nowrap">{b.label}</span>
              <span className="ml-auto font-mono text-xs text-muted-foreground tabular-nums">{b.value}</span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function Halts({ view }: { view: TickerPageView }) {
  const h = view.halts;
  return (
    <section aria-labelledby="ticker-halts" className="flex flex-col gap-2">
      <h3 id="ticker-halts" className="m-0 text-[15px] font-semibold">
        Halts &amp; LULD
      </h3>
      <div className="flex flex-col gap-2.5 rounded-xl bg-card p-3.5 text-[13px] leading-normal">
        <ul className="m-0 flex list-none flex-col gap-1 p-0">
          {h.lines.map((l) => (
            <li key={l} className="flex items-start gap-2.5 [&_svg]:mt-0.5 [&_svg]:size-4 [&_svg]:shrink-0">
              <Icon name="pause" />
              <span>{l}</span>
            </li>
          ))}
        </ul>
        {h.luld && <p className="m-0 text-muted-foreground">{h.luld}</p>}
        <div className="flex flex-col gap-1">
          <span className="text-xs text-text-faint">{h.closestTitle}</span>
          {h.closest.map((c) => (
            <span key={c} className="font-mono text-xs text-muted-foreground">
              {c}
            </span>
          ))}
        </div>
      </div>
    </section>
  );
}

function Missed({ view }: { view: TickerPageView }) {
  if (view.missed.length === 0) {
    return (
      <section aria-labelledby="ticker-missed" className="flex flex-col gap-2">
        <h3 id="ticker-missed" className="m-0 text-[15px] font-semibold">
          Missed setups
        </h3>
        <p className="m-0 text-[13px] text-text-faint">Every setup the detector armed that day was notified.</p>
      </section>
    );
  }
  return (
    <section aria-labelledby="ticker-missed" className="@container/missed flex flex-col gap-2">
      <div className="flex flex-wrap items-baseline gap-x-2.5">
        <h3 id="ticker-missed" className="m-0 text-[15px] font-semibold">
          Missed setups
        </h3>
        <span className="text-xs text-muted-foreground">Found by the detector, not sent as alerts</span>
      </div>
      <MissedGrid items={view.missed} />
      {view.missedLater.length > 0 && (
        <Collapsible>
          <CollapsibleTrigger className="h-8 w-auto px-1 text-xs text-muted-foreground hover:text-foreground pointer-coarse:h-11">
            {view.missedLater.length} later setup{view.missedLater.length === 1 ? "" : "s"}
          </CollapsibleTrigger>
          <CollapsibleContent>
            <div className="pt-2">
              <MissedGrid items={view.missedLater} />
            </div>
          </CollapsibleContent>
        </Collapsible>
      )}
    </section>
  );
}

function MissedGrid({ items }: { items: TickerMissedView[] }) {
  return (
    // 1, 2 or 4 equal columns, so the four shown always fill their rows; 2 or 4 only when a card still
    // has the room for its outcome on the header line (a wrapped outcome misaligns the row).
    <ul className="m-0 grid list-none grid-cols-1 gap-2 p-0 @3xl/missed:grid-cols-2 @7xl/missed:grid-cols-4">
      {items.map((m) => (
        <li key={m.key} className="flex min-w-0 flex-col gap-1.5 rounded-xl bg-card p-3.5">
          <div className="flex flex-wrap items-center gap-x-2.5 gap-y-1">
            <span className="size-3.5 shrink-0 rounded-full shadow-[inset_0_0_0_2px_var(--text-dim)]" aria-hidden />
            <span className="font-mono text-xs whitespace-nowrap text-muted-foreground tabular-nums">{m.time}</span>
            <span className="text-[13px] font-medium">{m.kind}</span>
            <span className={cn("ml-auto font-mono text-xs whitespace-nowrap tabular-nums", TONE[m.outcomeTone])}>{m.outcome}</span>
          </div>
          <span className="font-mono text-[11px] text-muted-foreground">{m.levels}</span>
          <span className="text-[13px] text-foreground/85">{m.why}</span>
        </li>
      ))}
    </ul>
  );
}

