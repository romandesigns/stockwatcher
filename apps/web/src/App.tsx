import {
  buildSlots,
  formatChange,
  formatEtDateKey,
  formatPrice,
  lastCandle,
  sessionBands,
  slotIndexOf,
  tradingDayOf,
  volumeBuckets,
  type ChartOptions,
  type Timeframe,
} from "@stockwatcher/chart-core";
import { CandleChart, CandleInspector, ReplayControls, SessionTimeline, Tape, TimeframeSelector, VolumeProfile } from "@stockwatcher/chart-web";
import { ToggleGroup, ToggleGroupItem, cn } from "@stockwatcher/chart-web/ui";
import { useEffect, useMemo, useState } from "react";
import { createServerFeed } from "./feed/serverFeed.ts";
import { chooseSource, initialSymbol, probeServer, type SourceChoice } from "./feed/source.ts";
import type { ChartFeed } from "./feed/types.ts";
import { useChartFeed } from "./feed/useChartFeed.ts";
import { createFixtureFeed, FIXTURE_SYMBOL } from "./fixtures/fixtureFeed.ts";
import { useMediaQuery } from "./lib/useMediaQuery.ts";

// Data source: the Rust server by default, the in-browser fixture only when the
// server is unreachable or has no data (feed/source.ts), or when forced with
// VITE_CHART_SOURCE=fixture. The active source is always labelled.
const ENV_SOURCE = import.meta.env.VITE_CHART_SOURCE;
const BINS = [0.01, 0.02, 0.05] as const;

const PHONE_CHART: Partial<ChartOptions> = { priceAxisWidth: 42, volumeHeight: 36, timeAxisHeight: 18, paneGap: 4, fontSize: 9, showProfile: true, profileWidth: 0.26, timeLabelSpacing: 60 };
const DESKTOP_CHART: Partial<ChartOptions> = {};

/** One line saying where the chart's data comes from, and whether it is fixture data. */
function sourceText(choice: SourceChoice): string {
  if (choice.kind === "fixture") return `In-browser fixture — not market data (${choice.reason})`;
  return choice.fixtureData ? `${choice.label} · fixture archive — not market data` : `${choice.label} · ${choice.reason}`;
}

function SourceBadge({ choice, compact = false }: { choice: SourceChoice; compact?: boolean }) {
  return (
    <span className="inline-flex min-w-0 items-center gap-1.5 text-[11px] text-muted-foreground" title={sourceText(choice)}>
      <span className="rounded-full bg-bg-row px-2 py-0.5 text-[10px] font-semibold tracking-wider text-foreground">{choice.kind === "server" ? "SERVER" : "BROWSER"}</span>
      {choice.fixtureData && <span className="rounded-full bg-warning-bg px-2 py-0.5 text-[10px] font-semibold tracking-wider text-warning">FIXTURE</span>}
      {!compact && <span className="truncate">{sourceText(choice)}</span>}
    </span>
  );
}

function SymbolSelector({ symbols, value, onChange }: { symbols: string[]; value: string; onChange: (s: string) => void }) {
  if (symbols.length < 2) return null;
  return (
    <ToggleGroup aria-label="Symbol" size="sm" spacing={0} variant="outline" value={[value]} onValueChange={(v) => v[0] && onChange(v[0])}>
      {symbols.map((s) => (
        <ToggleGroupItem key={s} value={s} className="h-7 px-2 font-mono text-[11px] text-muted-foreground aria-pressed:bg-bg-row aria-pressed:text-foreground">
          {s}
        </ToggleGroupItem>
      ))}
    </ToggleGroup>
  );
}

export default function App() {
  // The feed is created and closed by the same effect, so React StrictMode's
  // dev-only mount → unmount → mount gets a fresh feed, never a closed one.
  const [ready, setReady] = useState<{ choice: SourceChoice; feed: ChartFeed } | null>(null);
  useEffect(() => {
    let alive = true;
    let feed: ChartFeed | null = null;
    void (ENV_SOURCE === "fixture" ? Promise.resolve(null) : probeServer()).then((probe) => {
      if (!alive) return;
      const choice = chooseSource(ENV_SOURCE, probe, FIXTURE_SYMBOL);
      feed = choice.kind === "server" ? createServerFeed() : createFixtureFeed();
      setReady({ choice, feed });
    });
    return () => {
      alive = false;
      feed?.close();
    };
  }, []);
  if (!ready) {
    return <div className="flex min-h-dvh items-center justify-center bg-background text-sm text-muted-foreground">Connecting to the server…</div>;
  }
  return <ChartApp choice={ready.choice} feed={ready.feed} />;
}

function ChartApp({ choice, feed }: { choice: SourceChoice; feed: ChartFeed }) {

  const [symbol, setSymbol] = useState(() => initialSymbol(choice, FIXTURE_SYMBOL));
  const [timeframe, setTimeframe] = useState<Timeframe>("1m");
  const [bin, setBin] = useState<number>(0.01);
  const [inspectT, setInspectT] = useState<number | null>(null);
  const { series, tape, clock } = useChartFeed(feed, symbol, timeframe, bin);
  const phone = useMediaQuery("(max-width: 600px)");

  const slots = useMemo(() => (series ? buildSlots(series.candles, series.gaps) : []), [series]);
  const inspected = inspectT === null ? (slots.at(-1) ?? null) : (slots[slotIndexOf(slots, inspectT) ?? -1] ?? null);
  const last = useMemo(() => lastCandle(slots), [slots]);

  // Change vs the first candle of the anchor session (e.g. the regular open).
  const sessionOpen = useMemo(() => {
    const start = series?.session?.start;
    return start === undefined ? undefined : series?.candles.find((c) => c.t >= start)?.o;
  }, [series]);

  // Bands change only when the trading day does, not on every clock tick.
  const dayKey = clock ? formatEtDateKey(tradingDayOf(clock.now)) : "";
  const bands = useMemo(() => {
    if (!dayKey) return [];
    const [year, month, day] = dayKey.split("-").map(Number) as [number, number, number];
    return sessionBands({ year, month, day });
  }, [dayKey]);
  const bucketMs = phone ? 20 * 60_000 : 15 * 60_000;
  const buckets = useMemo(() => (series && bands.length ? volumeBuckets(series.candles, bands[0]!.start, bands[3]!.end, bucketMs) : []), [series, bands, bucketMs]);

  const changeTf = (tf: Timeframe) => {
    setTimeframe(tf);
    setInspectT(null);
  };
  const replay = {
    clock,
    onPlayPause: (play: boolean) => feed.control({ action: play ? "play" : "pause" }),
    onStep: () => feed.control({ action: "step" }),
    onSpeedChange: (speed: number) => feed.control({ action: "speed", value: speed }),
  };
  const changeSymbol = (s: string) => {
    setSymbol(s);
    setInspectT(null);
  };
  const price = last ? formatPrice(last.c) : "—";
  const change = last && sessionOpen ? formatChange(sessionOpen, last.c) : "";
  const changeColor = change.startsWith("-") ? "text-critical" : "text-good";
  const chartLabel = `${symbol} ${timeframe} candles, ${choice.fixtureData ? "fixture data" : "market data"} from ${choice.kind === "server" ? "the server" : "the in-browser fixture"}`;
  const tapeNote = choice.fixtureData ? "Fixture data" : choice.label;

  const binSelector = (
    <>
      <span>Bin</span>
      <ToggleGroup aria-label="Profile bin width" size="sm" spacing={0} variant="outline" value={[String(bin)]} onValueChange={(v) => v[0] && setBin(Number(v[0]))}>
        {BINS.map((b) => (
          <ToggleGroupItem key={b} value={String(b)} className="h-6 px-2 font-mono text-[11px] text-muted-foreground aria-pressed:bg-bg-row aria-pressed:text-foreground">
            {b.toFixed(2)}
          </ToggleGroupItem>
        ))}
      </ToggleGroup>
    </>
  );

  if (phone) {
    return (
      <div className="flex min-h-dvh flex-col gap-3 bg-background p-4 text-foreground">
        <header className="flex items-baseline gap-2">
          <span className="text-lg font-semibold">{symbol}</span>
          <span className="font-mono text-lg tabular-nums">{price}</span>
          <span className={cn("font-mono text-xs", changeColor)}>{change}</span>
          <span className="ml-auto flex items-center gap-1.5">
            <SourceBadge choice={choice} compact />
            <span className="rounded-full bg-primary/15 px-2 py-0.5 text-[10px] font-semibold tracking-wider text-primary">{clock?.mode === "live" ? "LIVE" : "REPLAY"}</span>
          </span>
        </header>
        <p className="-mt-2 truncate text-[11px] text-muted-foreground">{sourceText(choice)}</p>
        <SymbolSelector symbols={choice.symbols} value={symbol} onChange={changeSymbol} />
        <TimeframeSelector value={timeframe} onValueChange={changeTf} variant="grid" />
        <CandleChart className="h-[300px]" series={series} timeframe={timeframe} inspectT={inspectT} onInspectChange={setInspectT} options={PHONE_CHART} barSpacing={10} label={chartLabel} />
        <CandleInspector className="-mt-1" slot={inspected} timeframe={timeframe} variant="compact" />
        <section aria-label="Session timeline and replay" className="flex flex-col gap-2 rounded-xl bg-card px-3 py-2.5">
          <ReplayControls {...replay} variant="compact" />
          <SessionTimeline bands={bands} buckets={buckets} bucketMs={bucketMs} now={clock?.now ?? null} variant="compact" />
        </section>
        <section aria-label="Time and sales" className="flex min-h-0 flex-1 flex-col gap-1.5">
          <div className="flex items-baseline">
            <h2 className="m-0 flex-1 text-[11px] font-semibold tracking-wider text-text-faint">TAPE</h2>
            <span className="text-[10px] text-text-faint">{tapeNote}</span>
          </div>
          <Tape trades={tape} variant="compact" maxRows={8} />
        </section>
      </div>
    );
  }

  return (
    <div className="flex min-h-dvh flex-col bg-background text-foreground">
      <header className="flex flex-wrap items-center gap-3.5 border-b border-border px-4.5 py-3">
        <div className="text-[15px] font-semibold">StockWatcher</div>
        <div className="flex items-baseline gap-2 border-l border-border pl-3.5">
          <span className="text-[17px] font-semibold">{symbol}</span>
          <span className="font-mono text-[17px] tabular-nums">{price}</span>
          <span className={cn("font-mono text-xs", changeColor)} title="Change since the session's first candle">
            {change}
          </span>
        </div>
        <SourceBadge choice={choice} />
        <SymbolSelector symbols={choice.symbols} value={symbol} onChange={changeSymbol} />
        <div className="ml-auto flex flex-wrap items-center gap-2.5">
          <span className="text-xs text-muted-foreground">Mode</span>
          <ToggleGroup aria-label="Data mode" spacing={0} variant="outline" value={[clock?.mode === "live" ? "live" : "replay"]}>
            <ToggleGroupItem value="replay" className="text-xs text-muted-foreground aria-pressed:bg-bg-row aria-pressed:font-semibold aria-pressed:text-foreground">
              Replay
            </ToggleGroupItem>
            <ToggleGroupItem value="live" disabled title="Mode is server configuration (MARKET_DATA_MODE); the live provider is not connected yet" className="text-xs text-text-faint">
              Live
            </ToggleGroupItem>
          </ToggleGroup>
          <TimeframeSelector value={timeframe} onValueChange={changeTf} />
        </div>
      </header>

      <div className="flex min-h-0 flex-1 flex-col min-[901px]:flex-row">
        <main className="flex min-w-0 flex-1 flex-col gap-3 px-4.5 py-3.5">
          <div className="flex min-h-[380px] flex-1 flex-col gap-1.5">
            <CandleInspector slot={inspected} timeframe={timeframe} />
            <CandleChart className="min-h-[340px] flex-1" series={series} timeframe={timeframe} inspectT={inspectT} onInspectChange={setInspectT} options={DESKTOP_CHART} label={chartLabel} />
          </div>
          <section aria-label="Session timeline and replay" className="flex flex-col gap-2.5 rounded-xl bg-card px-3.5 py-3">
            <ReplayControls {...replay} />
            <SessionTimeline bands={bands} buckets={buckets} bucketMs={bucketMs} now={clock?.now ?? null} />
          </section>
        </main>

        <aside className="grid border-border min-[601px]:max-[900px]:grid-cols-2 min-[601px]:max-[900px]:border-t min-[901px]:flex min-[901px]:w-[340px] min-[901px]:shrink-0 min-[901px]:flex-col min-[901px]:border-l">
          <section aria-label="Session volume profile" className="border-border px-4 py-3.5 max-[900px]:border-r min-[901px]:border-b">
            <VolumeProfile className="h-[248px]" profile={series?.profile} actions={binSelector} />
          </section>
          <section aria-label="Time and sales" className="flex min-h-0 flex-1 flex-col gap-1.5 px-4 py-3.5">
            <h2 className="m-0 mb-1 text-xs font-semibold tracking-wider text-text-faint">TAPE · CONDITIONS</h2>
            <Tape trades={tape} maxRows={14} />
          </section>
        </aside>
      </div>
    </div>
  );
}
