import { dayChartContext, DEFAULT_CHART_SETTINGS, DEFAULT_LAYERS, formatEtTime12, type ChartLayers, type ChartSettings, type EventMarker } from "@stockwatcher/chart-core";
import { CandleChart, Icon } from "@stockwatcher/chart-web";
import { cn, Collapsible, CollapsibleContent, CollapsibleTrigger, ScrollArea, Tip } from "@stockwatcher/chart-web/ui";
import { tickerPageHash, tickerPageView, type TickerAlertView, type TickerChannel, type TickerMissedView, type TickerPackageWire, type TickerPageView, type TickerRequest, type Tone } from "@stockwatcher/shared";
import { useMemo, useState } from "react";
import { useTickerPackage } from "./useTickerPage.ts";

/**
 * The approved design's chart: candles, the day's high and low, the markers. The key-level lines
 * (premarket high/low, HOD/LOD, previous close) are off: their labels sat on top of the numbered
 * markers and the day's own high/low labels, and the page states those levels in its tiles.
 */
const TICKER_LAYERS: ChartLayers = { ...DEFAULT_LAYERS, premarket: false, dayRange: false, prevClose: false };

const time = (ms: number, seconds?: boolean) => formatEtTime12(ms, seconds);

const TONE: Record<Tone, string> = { neutral: "text-foreground", up: "text-good", down: "text-critical", dim: "text-muted-foreground" };

/**
 * The Insights ticker page (owner, 2026-10-31, approved design): one symbol on one trading day,
 * entirely from the server's package (crates/server/src/insights): the top bar with the day
 * switcher, the stat strip, the day's 1-minute chart with every sent alert numbered and every
 * missed setup ringed, the notification history with each alert's outcome, the scorecard, halts
 * and LULD, and the missed setups with the rule that held each. Rendering only: every number is
 * the server's.
 */
export function TickerPage({ channel, want, onBack }: { channel: TickerChannel | undefined; want: TickerRequest; onBack: () => void }) {
  const state = useTickerPackage(channel, want);
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
          <Body pkg={pkg} view={view} />
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

function Body({ pkg, view }: { pkg: TickerPackageWire; view: TickerPageView }) {
  // The history row under the pointer rings its marker on the chart.
  const [focus, setFocus] = useState<number | null>(null);
  return (
    <>
      <Stats view={view} />
      <DayChart pkg={pkg} view={view} focus={focus} />
      <div className="flex flex-wrap items-start gap-5">
        <History view={view} onFocus={setFocus} />
        <aside className="flex min-w-0 flex-[1_1_320px] flex-col gap-5">
          <Scorecard view={view} />
          <Halts view={view} />
        </aside>
      </div>
      <Missed view={view} />
    </>
  );
}

function Stats({ view }: { view: TickerPageView }) {
  return (
    <section aria-label="The day in numbers" className="@container/stats">
      {/* 2 or 4 equal columns: eight tiles always fill their rows (no orphan). Eight across left a
          price range too little room even at 1920 px, so four is the widest. */}
      <ul className="m-0 grid list-none grid-cols-2 gap-2 p-0 @3xl/stats:grid-cols-4">
        {view.stats.map((s) => (
          <li key={s.id} className="flex min-w-0 flex-col gap-0.5 rounded-xl bg-card px-3 py-2.5">
            <span className="text-xs text-muted-foreground">{s.label}</span>
            {/* Wraps rather than truncates: a price range must read whole on a 320 px phone. */}
            <span className={cn("font-mono text-[15px] font-medium [overflow-wrap:anywhere] tabular-nums", TONE[s.tone])}>{s.value}</span>
            {/* The note wraps too: truncated, "no previous close on fi…" and "high 4:5…" lost their point at 320 px. */}
            <span className="text-[11px] text-text-faint">{s.note || " "}</span>
          </li>
        ))}
      </ul>
    </section>
  );
}

function DayChart({ pkg, view, focus }: { pkg: TickerPackageWire; view: TickerPageView; focus: number | null }) {
  const [settings, setSettings] = useState<ChartSettings>(() => ({ ...DEFAULT_CHART_SETTINGS, highLow: true }));
  const [layers, setLayers] = useState<ChartLayers>(TICKER_LAYERS);
  // Levels, ranges, zones and the session schedule are the server's, computed over these bars
  // (the package's `chart`); the chart reads nothing off its candles.
  const context = useMemo(() => dayChartContext(pkg.chart), [pkg]);
  const series = useMemo(() => ({ candles: pkg.bars, gaps: [], profile: null, halts: pkg.halts?.halts ?? [], ...(context.sessions ? { sessions: context.sessions } : {}) }), [pkg, context]);
  const markers = useMemo<EventMarker[]>(() => view.markers.map((m) => ({ kind: m.kind, at: m.at, price: m.price, n: m.n, focused: m.kind === "alert" && m.n === focus })), [view.markers, focus]);
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
          inspector
          priceAxisMenu={false}
          label={`${pkg.symbol} on ${view.dayLabel}, 1-minute candles, ${sent} sent alert${sent === 1 ? "" : "s"} and ${missed} missed setup${missed === 1 ? "" : "s"} marked`}
        />
      </div>
    </section>
  );
}

function History({ view, onFocus }: { view: TickerPageView; onFocus: (n: number | null) => void }) {
  return (
    <section aria-labelledby="ticker-history" className="@container/hist flex min-w-0 flex-[999_1_640px] flex-col gap-2">
      <div className="flex items-baseline gap-2.5">
        <h3 id="ticker-history" className="m-0 text-[15px] font-semibold">
          Notification history
        </h3>
        <span className="text-xs text-muted-foreground">{view.alertSummary}</span>
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
              <AlertRow key={a.key} a={a} onFocus={onFocus} />
            ))}
          </ol>
        </>
      )}
    </section>
  );
}

const COLS = "grid-cols-[1.75rem_5.5rem_minmax(9rem,1.4fr)_5.5rem_minmax(8rem,1fr)_5.5rem_5.5rem]";

function MarkerDot({ a }: { a: TickerAlertView }) {
  return a.marker ? (
    <span className="inline-grid size-[22px] shrink-0 place-items-center rounded-full bg-primary text-[11px] font-bold text-primary-foreground" aria-label={`Alert ${a.marker}`}>
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
function AlertRow({ a, onFocus }: { a: TickerAlertView; onFocus: (n: number | null) => void }) {
  const text = <span className={cn("truncate text-xs", a.held ? "text-warning" : "text-muted-foreground")}>{a.held ?? a.detail}</span>;
  return (
    <li onPointerEnter={() => onFocus(a.marker)} onPointerLeave={() => onFocus(null)} className="rounded-xl bg-card transition-colors duration-150 hover:bg-bg-row motion-reduce:transition-none">
      <div className={cn("hidden items-center gap-3 px-3.5 py-2.5 @3xl/hist:grid", COLS)}>
        <MarkerDot a={a} />
        <span className="font-mono text-[13px] text-muted-foreground tabular-nums">{a.time}</span>
        <span className="flex min-w-0 flex-col gap-0.5">
          <span className="truncate text-[13px] font-medium">{a.kind}</span>
          {text}
        </span>
        <span className="font-mono text-[13px] tabular-nums">{a.price}</span>
        <span className={cn("truncate font-mono text-[13px] tabular-nums", TONE[a.bestTone])}>
          {a.best} · {a.bestPct}
        </span>
        <span className={cn("font-mono text-[13px] tabular-nums", TONE[a.plus5Tone])}>{a.plus5}</span>
        <span className={cn("text-[13px]", TONE[a.stopTone])}>{a.stop}</span>
      </div>
      <div className="flex flex-col gap-1 px-3 py-2.5 @3xl/hist:hidden">
        <div className="flex min-w-0 items-center gap-2">
          <MarkerDot a={a} />
          <span className="shrink-0 font-mono text-xs whitespace-nowrap text-muted-foreground tabular-nums">{a.time}</span>
          <span className="min-w-0 truncate text-[13px] font-medium">{a.kind}</span>
          <span className="ml-auto font-mono text-[13px] tabular-nums">{a.price}</span>
        </div>
        {text}
        <div className="flex flex-wrap gap-x-3 gap-y-0.5 text-xs">
          <span className={cn("font-mono tabular-nums", TONE[a.bestTone])}>best {a.bestPct}</span>
          <span className={TONE[a.plus5Tone]}>+5% {a.plus5}</span>
          <span className={TONE[a.stopTone]}>stop {a.stop}</span>
        </div>
      </div>
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

