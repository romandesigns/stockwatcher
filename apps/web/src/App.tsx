import {
  buildSlots,
  changeReference,
  currentReplayDay,
  formatChange,
  formatEtDateKey,
  formatPrice,
  lastCandle,
  replayDayOptions,
  sessionBands,
  tradingDayOf,
  volumeBuckets,
  type ChartOptions,
  type PriceScaleType,
  type Timeframe,
} from "@stockwatcher/chart-core";
import { CandleChart, ReplayControls, SessionTimeline, Tape, VolumeProfile } from "@stockwatcher/chart-web";
import { Tip, ToggleGroup, ToggleGroupItem, cn } from "@stockwatcher/chart-web/ui";
import { useEffect, useMemo, useState } from "react";
import { createServerFeed } from "./feed/serverFeed.ts";
import { clearToken, getToken, setToken } from "./feed/token.ts";
import { chooseSource, followCandidates, followServer, initialSymbol, probeServer, type SourceChoice } from "./feed/source.ts";
import { useCandidates } from "./feed/useCandidates.ts";
import type { ChartFeed } from "./feed/types.ts";
import { useChartFeed } from "./feed/useChartFeed.ts";
import { usePaperTrading } from "./feed/usePaperTrading.ts";
import { useReplayDays } from "./feed/useReplayDays.ts";
import { createFixtureFeed, FIXTURE_SYMBOL } from "./fixtures/fixtureFeed.ts";
import { useMediaQuery } from "./lib/useMediaQuery.ts";
import { TickerSearch } from "./search/TickerSearch.tsx";

/** The search icon for the quick-open pill and the phone header. */
const SearchIcon = (
  <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" aria-hidden="true">
    <circle cx="7" cy="7" r="4.5" />
    <path d="M10.5 10.5 14 14" />
  </svg>
);

const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);

// Data source: the Rust server by default, the in-browser fixture only when the
// server is unreachable or has no data (feed/source.ts), or when forced with
// VITE_CHART_SOURCE=fixture. The active source is always labelled.
const ENV_SOURCE = import.meta.env.VITE_CHART_SOURCE;
const BINS = [0.01, 0.02, 0.05] as const;

const PHONE_CHART: Partial<ChartOptions> = { priceAxisWidth: 42, volumeHeight: 36, timeAxisHeight: 18, paneGap: 4, fontSize: 9, showProfile: true, profileWidth: 0.2, timeLabelSpacing: 60 };
const DESKTOP_CHART: Partial<ChartOptions> = {};

/** One line saying where the chart's data comes from, and whether it is fixture data. */
function sourceText(choice: SourceChoice): string {
  if (choice.kind === "fixture") return `In-browser fixture — not market data (${choice.reason})`;
  return choice.fixtureData ? `${choice.label} · fixture archive — not market data` : `${choice.label} · ${choice.reason}`;
}

/** Replay: a clock turned back. */
const ReplayIcon = (
  <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <path d="M2.5 8a5.5 5.5 0 1 0 1.6-3.9" />
    <path d="M2.5 2.5v2.2h2.2" />
    <path d="M8 5.2V8l1.9 1.3" />
  </svg>
);

/** Live: a broadcast dot with its waves. */
const LiveIcon = (
  <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" aria-hidden="true">
    <circle cx="8" cy="8" r="1.6" fill="currentColor" stroke="none" />
    <path d="M5.2 5.2a4 4 0 0 0 0 5.6M10.8 5.2a4 4 0 0 1 0 5.6" />
    <path d="M3.1 3.1a7 7 0 0 0 0 9.8M12.9 3.1a7 7 0 0 1 0 9.8" strokeOpacity="0.55" />
  </svg>
);

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


export default function App() {
  // The feed is created and closed by the same effect, so React StrictMode's
  // dev-only mount → unmount → mount gets a fresh feed, never a closed one.
  const [ready, setReady] = useState<{ choice: SourceChoice; feed: ChartFeed } | null>(null);
  // The server wants an access token (production): show the sign-in card instead of falling back to the fixture.
  const [signIn, setSignIn] = useState<{ rejected: boolean } | null>(null);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let alive = true;
    let feed: ChartFeed | null = null;
    const token = getToken();
    void (ENV_SOURCE === "fixture" ? Promise.resolve(null) : probeServer(fetch, 2500, token)).then((probe) => {
      if (!alive) return;
      if (probe && !probe.ok && probe.unauthorized) {
        if (token) clearToken();
        setSignIn({ rejected: !!token });
        return;
      }
      setSignIn(null);
      const choice = chooseSource(ENV_SOURCE, probe, FIXTURE_SYMBOL);
      feed = choice.kind === "server" ? createServerFeed({ token }) : createFixtureFeed();
      setReady({ choice, feed });
    });
    return () => {
      alive = false;
      feed?.close();
    };
  }, [attempt]);
  if (signIn) {
    return (
      <SignIn
        rejected={signIn.rejected}
        onSubmit={(t) => {
          setToken(t);
          setAttempt((n) => n + 1);
        }}
      />
    );
  }
  if (!ready) {
    return <div className="flex min-h-dvh items-center justify-center bg-background text-sm text-muted-foreground">Connecting to the server…</div>;
  }
  return <ChartApp choice={ready.choice} feed={ready.feed} />;
}

/** The one-field sign-in for a server that requires its access token. */
function SignIn({ rejected, onSubmit }: { rejected: boolean; onSubmit: (token: string) => void }) {
  const [value, setValue] = useState("");
  return (
    <div className="flex min-h-dvh items-center justify-center bg-background p-4 text-foreground">
      <form
        className="flex w-full max-w-sm flex-col gap-3 rounded-2xl bg-card p-5"
        onSubmit={(e) => {
          e.preventDefault();
          if (value.trim()) onSubmit(value.trim());
        }}
      >
        <div className="flex flex-col gap-1">
          <h1 className="m-0 text-base font-semibold">StockWatcher</h1>
          <p className="m-0 text-xs text-muted-foreground">Enter your access token to connect to the server.</p>
        </div>
        <label htmlFor="access-token" className="sr-only">
          Access token
        </label>
        <input
          id="access-token"
          type="password"
          autoComplete="current-password"
          autoFocus
          value={value}
          onChange={(e) => setValue(e.target.value)}
          placeholder="Access token"
          aria-invalid={rejected}
          className="h-10 rounded-lg bg-bg-row px-3 font-mono text-sm outline-none placeholder:text-text-faint focus-visible:ring-2 focus-visible:ring-ring"
        />
        {rejected && <p className="m-0 text-xs text-critical">That token wasn't accepted. Check it and try again.</p>}
        <button type="submit" disabled={!value.trim()} className="h-10 rounded-lg bg-primary text-sm font-semibold text-primary-foreground disabled:opacity-50">
          Sign in
        </button>
      </form>
    </div>
  );
}

function ChartApp({ choice, feed }: { choice: SourceChoice; feed: ChartFeed }) {

  const [symbol, setSymbol] = useState(() => initialSymbol(choice, FIXTURE_SYMBOL));
  const [timeframe, setTimeframe] = useState<Timeframe>("1m");
  const [bin, setBin] = useState<number>(0.01);
  // The trading day the replay clock is on; it changes on a day switch (and once a night while playing).
  const [clockDay, setClockDay] = useState<string | null>(null);
  const candidates = useCandidates(choice, clockDay);
  // What is playing now (the day can be switched under a running page): drives the source chip,
  // the tape note, the chart's label and the search's mock rows. `choice` stays the startup
  // decision (server or fixture), which never changes.
  const current = useMemo(() => followServer(choice, candidates.server), [choice, candidates.server]);
  const [inspectT, setInspectT] = useState<number | null>(null);
  // Held here, not in the chart, so it survives switching phone/desktop layouts.
  const [priceScale, setPriceScale] = useState<PriceScaleType>("linear");
  // A stock that traded out of the price band is no longer let through by the server.
  useEffect(() => {
    const next = followCandidates(symbol, candidates.symbols);
    if (next !== symbol) {
      setSymbol(next);
      setInspectT(null);
    }
  }, [candidates.symbols, symbol]);
  const { series, tape, clock } = useChartFeed(feed, symbol, timeframe, bin);
  const trading = usePaperTrading(feed);
  const replayDays = useReplayDays(choice, clockDay);
  const dayOptions = useMemo(() => replayDayOptions(replayDays.state?.days ?? []), [replayDays.state]);

  const phone = useMediaQuery("(max-width: 600px)");
  // Ticker quick-open, VS Code's Ctrl+P (Cmd+P on a Mac): the browser's print shortcut is taken
  // over on purpose while the app is focused.
  const [searchOpen, setSearchOpen] = useState(false);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey && (e.key === "p" || e.key === "P")) {
        e.preventDefault();
        setSearchOpen(true);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
  const openTicker = (s: string) => {
    setSymbol(s);
    setInspectT(null);
  };
  // On the desktop the panel drops down right under the header's search pill, at its width.
  const search = <TickerSearch open={searchOpen} onOpenChange={setSearchOpen} symbols={candidates.symbols} quotes={feed.quotes} mock={current.fixtureData} current={symbol} onSelect={openTicker} anchored={!phone} />;
  const tickerButton = (size: string) => (
    <Tip label={`Search tickers (${isMac ? "⌘" : "Ctrl+"}P)`}>
      <button type="button" onClick={() => setSearchOpen(true)} className={cn("pointer-events-auto cursor-pointer rounded-md font-semibold outline-none hover:text-primary focus-visible:ring-2 focus-visible:ring-ring", size)}>
        {symbol}
      </button>
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
    days: dayOptions,
    day: currentReplayDay(replayDays.state, clock),
    // The server rebuilds on the new day and resends every snapshot; the clock's new trading day
    // then refetches the candidates and this list.
    onDayChange: (day: string) => feed.control({ action: "day", value: day }),
    onDayMenuOpenChange: (open: boolean) => open && replayDays.refresh(),
  };
  // Jump the replay; the server rebuilds from the session start and resends snapshots.
  const seek = (t: number) => feed.control({ action: "seek", value: t });
  const price = last ? formatPrice(last.c) : "—";
  const change = last && reference ? formatChange(reference.price, last.c) : "";
  const changeColor = change.startsWith("-") ? "text-critical" : "text-good";
  const chartLabel = `${symbol} ${timeframe} candles, ${current.fixtureData ? "fixture data" : "market data"} from ${current.kind === "server" ? "the server" : "the in-browser fixture"}`;
  const tapeNote = current.fixtureData ? "Fixture data" : current.label;
  const mode = clock?.mode === "live" ? "live" : "replay";

  const binSelector = (
    <>
      <span>Bin</span>
      <ToggleGroup aria-label="Profile bin width" size="sm" spacing={0.5} variant="segmented" value={[String(bin)]} onValueChange={(v) => v[0] && setBin(Number(v[0]))}>
        {BINS.map((b) => (
          <ToggleGroupItem key={b} value={String(b)} className="h-6 px-1.5 font-mono text-[11px]">
            {b.toFixed(2)}
          </ToggleGroupItem>
        ))}
      </ToggleGroup>
    </>
  );

  if (phone) {
    return (
      <div className="flex min-h-dvh flex-col gap-2.5 bg-background p-3 text-foreground">
        <header className="flex items-center gap-2">
          <div className="flex min-w-0 items-baseline gap-2">
            {tickerButton("text-base")}
            <span className="font-mono text-base tabular-nums">{price}</span>
            <span className={cn("font-mono text-[11px]", changeColor)}>{change}</span>
          </div>
          <SourceChip className="ml-auto" choice={current} mode={mode} />
          {search}
        </header>
        <CandleChart className="h-[360px]" toolbar inspector symbol={symbol} persistKey="sw-chart" trading={trading} now={clock?.now ?? null} onTimeframeChange={changeTf} series={series} timeframe={timeframe} inspectT={inspectT} onInspectChange={setInspectT} priceScaleType={priceScale} onPriceScaleTypeChange={setPriceScale} options={PHONE_CHART} barSpacing={10} label={chartLabel} />
        <section aria-label="Session timeline and replay" className="flex flex-col gap-1.5 rounded-xl bg-card px-3 py-2">
          <ReplayControls {...replay} variant="compact" />
          <SessionTimeline bands={bands} buckets={buckets} bucketMs={bucketMs} now={clock?.now ?? null} variant="minimal" onSeek={seek} />
        </section>
        <section aria-label="Time and sales" className="flex flex-col gap-1.5 rounded-xl bg-card p-3">
          <div className="flex items-baseline">
            <h2 className="m-0 flex-1 text-xs font-medium text-muted-foreground">Time &amp; sales</h2>
            <span className="text-[10px] text-text-faint">{tapeNote}</span>
          </div>
          <Tape trades={tape} variant="compact" maxRows={8} />
        </section>
      </div>
    );
  }

  return (
    <div className="flex min-h-dvh flex-col bg-background text-foreground">
      {/* Three columns (logo | search | data source): equal side columns put the search pill at the
          page's exact centre, where the quick-open panel drops down, at the same width. */}
      <header className="grid grid-cols-[1fr_auto_1fr] items-center gap-4 px-4 py-2.5">
        <div className="text-sm font-semibold tracking-tight">StockWatcher</div>
        <button
          type="button"
          onClick={() => setSearchOpen(true)}
          className="flex h-8 w-(--search-w) items-center gap-2 rounded-lg bg-card px-3 text-xs text-text-faint outline-none hover:bg-bg-row hover:text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring [&_svg]:size-3.5"
        >
          {SearchIcon}
          <span>Search tickers</span>
          <kbd className="ml-auto rounded bg-bg-row px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground">{isMac ? "⌘P" : "Ctrl P"}</kbd>
        </button>
        <SourceChip className="justify-self-end" choice={current} mode={mode} />
        {search}
      </header>

      <div className="flex min-h-0 flex-1 flex-col gap-2 px-4 pb-3 min-[901px]:flex-row">
        <main className="flex min-w-0 flex-1 flex-col gap-2">
          <div className="flex min-h-[360px] flex-1 flex-col gap-1">
            <CandleChart
              className="min-h-[360px] flex-1"
              toolbar
              inspector
              toolbarInChart
              heading={
                <div className="flex items-baseline gap-2">
                  {tickerButton("text-[15px]")}
                  <span className="font-mono text-[15px] tabular-nums">{price}</span>
                  <Tip label="Change since the session's first candle">
                    <span className={cn("pointer-events-auto font-mono text-[11px]", changeColor)}>{change}</span>
                  </Tip>
                </div>
              } symbol={symbol} persistKey="sw-chart" trading={trading} now={clock?.now ?? null} onTimeframeChange={changeTf} series={series} timeframe={timeframe} inspectT={inspectT} onInspectChange={setInspectT} priceScaleType={priceScale} onPriceScaleTypeChange={setPriceScale} options={DESKTOP_CHART} label={chartLabel} />
          </div>
          <section aria-label="Session timeline and replay" className="flex flex-col gap-2 rounded-xl bg-card px-3 py-2.5">
            <ReplayControls {...replay} />
            <SessionTimeline bands={bands} buckets={buckets} bucketMs={bucketMs} now={clock?.now ?? null} variant="minimal" onSeek={seek} />
          </section>
        </main>

        <aside className="grid gap-2 min-[601px]:max-[900px]:grid-cols-2 min-[901px]:flex min-[901px]:w-[300px] min-[901px]:shrink-0 min-[901px]:flex-col">
          <section aria-label="Session volume profile" className="rounded-xl bg-card p-3">
            <VolumeProfile className="h-[210px]" profile={series?.profile} actions={binSelector} />
          </section>
          <section aria-label="Time and sales" className="flex min-h-0 flex-1 flex-col gap-1.5 rounded-xl bg-card p-3">
            <h2 className="m-0 text-xs font-medium text-muted-foreground">Time &amp; sales</h2>
            <Tape trades={tape} maxRows={16} />
          </section>
        </aside>
      </div>
    </div>
  );
}
