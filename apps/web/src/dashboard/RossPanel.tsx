import { formatEtTime } from "@stockwatcher/chart-core";
import { cn, ScrollArea, Tip } from "@stockwatcher/chart-web/ui";
import { detectionFamilyLabel, detectionStateLabel, groupDetections, rocketNow, rossAttention, rossFreshness, rossRowMarks, rossRowView, runnersBySymbol, type DetectionChannel, type RocketState, type RossRowView, type RossTone, type RowMarks } from "@stockwatcher/shared";
import { memo } from "react";
import type { RossDemoState } from "../feed/useRossState.ts";
import { useDetection } from "../feed/useDetection.ts";
import { useTickerPress } from "../lib/useTickerPress.ts";
import { StarButton } from "../favorites/StarButton.tsx";
import { Flame, MarksLine, RowLine } from "./RowMarks.tsx";

const TONE: Record<RossTone, string> = {
  good: "text-good",
  critical: "text-critical",
  warning: "text-warning",
  accent: "text-primary",
  text: "text-foreground",
  muted: "text-muted-foreground",
  dim: "text-text-faint",
};

/**
 * The left edge and background a cue earns; quiet rows get none (design canvas "Ross Panel"). Only
 * a crossing in progress (accent) and a price at the trigger (amber) earn one: a broke-trigger is a
 * past stage that failed about three times in four on the holdout, never a green "go".
 */
const EDGE: Partial<Record<RossRowView["setup"]["state"], string>> = {
  crossing: "shadow-[inset_3px_0_0_var(--accent)] bg-primary/10",
  near: "shadow-[inset_3px_0_0_var(--warning)]",
};

const hhmm = (ns: number) => formatEtTime(Math.round(ns / 1_000_000));

/**
 * The Ross Panel (owner, 2026-10-19/20; design canvas "Ross Panel", web): the five-pillar
 * watchlist beside the charts, ordered by the last minute's move, each member's pullback stage
 * read from the same micro-pullback episodes the Detection Panel shows (one detector, 2026-10-21).
 * The cues describe stages, not entry signals. Rows earn an edge only at the trigger or crossing
 * it; the header counts them. Pressing a row loads its ticker into the selected chart;
 * tap, then hold copies it (the Halt watcher's gesture). Server feed only.
 *
 * Shown only while it has rows (owner, 2026-10-22: a panel with 0 items is hidden). Off, not yet
 * known and empty all hide its tab (SideTabs), so it has no "Connecting…" or quiet line of its
 * own. While a socket reconnects it keeps the last rows, dimmed and labelled.
 */
export function RossPanelView({ state, onPick, detection, onEpisodePick, runners, wallNow }: { state: RossDemoState; onPick: (symbol: string) => void; detection?: DetectionChannel; onEpisodePick?: (symbol: string, episodeId: string, runEpoch: string) => void; runners?: RocketState; wallNow?: number }) {
  const detected = useDetection(detection);
  const joined = new Map(groupDetections(detected.snapshot).map((g) => [g.symbol, g.selected]));
  const { enabled, panel, demo } = state;
  // Today's Runners row per member: its path and age, and the burst behind the flame (rowMarks.ts).
  const byRunner = runnersBySymbol(runners);
  // The Runners market clock (a replay's own day); without Runners there is no path or flame to age.
  const nowMs = runners ? rocketNow(runners, wallNow) : (wallNow ?? 0);
  const rows = panel?.rows.map((w) => ({ ...rossRowView(w, panel.cutoff, hhmm), marks: rossRowMarks(w, byRunner.get(w.symbol), nowMs, formatEtTime), trigger: w.setup?.trigger?.price ?? null })) ?? [];
  const attention = panel ? rossAttention(panel) : 0;
  // A dropped socket keeps the last rows and says so: they are the last known state, not now.
  const fresh = rossFreshness(state, hhmm);
  return (
    <aside aria-labelledby="ross-title" className="flex min-h-0 min-w-0 flex-col gap-2 rounded-xl bg-card p-2.5">
      <div className="flex items-center gap-2 px-1">
        <h2 id="ross-title" className="panel-title">
          Ross Panel
        </h2>
        <span className="font-mono text-[11px] text-text-faint">$0.10–$25</span>
        {demo && <span className="rounded bg-warning px-1 py-px text-[9px] font-semibold text-background">DEMO DATA</span>}
        {attention > 0 && (
          <span className="ml-auto inline-flex h-[22px] items-center gap-1.5 rounded-full bg-primary/15 px-2 text-[11px] text-primary" aria-label={`${attention} need attention`}>
            <span className="size-1.5 animate-pulse rounded-full bg-primary motion-reduce:animate-none" aria-hidden />
            {attention}
          </span>
        )}
      </div>
      {fresh.stale && enabled === true && (
        <p className={cn("m-0 flex items-center gap-1.5 px-1 text-[11px]", TONE[fresh.tone])} role="status">
          <span className="size-1.5 rounded-full bg-warning" aria-hidden />
          {fresh.label}
        </p>
      )}
      <ScrollArea className={cn("transition-opacity duration-200 motion-reduce:transition-none", fresh.stale && "opacity-60")}>
      <ul className="m-0 flex list-none flex-col gap-1 p-0">
        {rows.map((r) => (
          <li key={r.symbol} className="@container/row relative">
            <RossRow row={r} onPick={onPick} />
            {/* Beside the row, not in it (a button cannot hold a button). */}
            <StarButton symbol={r.symbol} className="absolute top-1 right-1" />
            {joined.has(r.symbol) && (() => {
              const e = joined.get(r.symbol)!;
              return <Tip label={e.episode_id}><button type="button" className="w-full rounded-b px-2.5 py-1 text-left text-[10px] text-primary outline-none focus-visible:ring-2 focus-visible:ring-ring" onClick={() => onEpisodePick ? onEpisodePick(e.symbol, e.episode_id, detected.frame?.runEpoch ?? "") : onPick(e.symbol)}>Detection: {detectionFamilyLabel(e.family)} · {detectionStateLabel(e.state)}{!detected.connected ? " · last known" : ""}</button></Tip>;
            })()}
          </li>
        ))}
      </ul>
      </ScrollArea>
    </aside>
  );
}

/**
 * One member as a lighter row (owner, 2026-10-26, design canvas "StockWatcher lighter rows"): the
 * ticker, price and day's gain as text, then the day line; the second line is marks (pillars,
 * relative volume, news, Runners path, the setup's stage and meter), each named on hover. The
 * last minute is the line's tail, the trigger a dotted level on it. A fresh Buying burst that holds
 * its alert price lights the flame. The cue edges (crossing, at the trigger) stay.
 */
const RossRow = memo(function RossRow({ row: r, onPick }: { row: RossRowView & { marks: RowMarks; trigger: number | null }; onPick: (symbol: string) => void }) {
  const press = useTickerPress(r.symbol, () => onPick(r.symbol));
  const m = r.marks;
  return (
    <button
      type="button"
      {...press}
      aria-label={`${r.aria}. Load into the selected chart`}
      data-flame={m.flame?.lit || undefined}
      className={cn(
        "flex w-full cursor-pointer touch-manipulation flex-col gap-1.5 rounded-lg bg-bg-row px-2.5 py-2 text-left font-mono outline-none select-none transition-colors duration-150 hover:bg-bg-raised focus-visible:ring-2 focus-visible:ring-ring motion-reduce:transition-none",
        m.flame?.lit && "bg-bg-raised/60",
        EDGE[r.setup.state],
        r.dimmed && "opacity-60",
      )}
    >
      <span className="flex items-center gap-1.5 pr-7">
        <Flame flame={m.flame} />
        <span className="text-sm font-semibold">{r.symbol}</span>
        <span className="text-sm tabular-nums">{r.price}</span>
        <span className="text-xs tabular-nums text-good">{r.gain}</span>
        {r.badge && <span className={cn("rounded px-1 py-px font-sans text-[9px] font-semibold text-background", r.badge.label === "NEW" ? "bg-primary" : "bg-warning")}>{r.badge.label}</span>}
        <RowLine line={r.line} tail={m.tail} trigger={m.stage ? r.trigger : null} tone={r.line?.tone === "critical" ? "text-critical" : "text-good"} stretch className="ml-auto w-18 @xs/row:w-26" />
      </span>
      <MarksLine marks={m} />
    </button>
  );
});

/** Pullback low (left, red: the failure level) to trigger (right, neutral: a level, not a "go"), the price as a dot. */
export function TriggerMeter({ value, tone, inset = true }: { value: number; tone: RossTone; inset?: boolean }) {
  const v = Math.max(0, Math.min(1, value));
  return (
    <svg viewBox="0 0 160 8" preserveAspectRatio="none" className={cn("block h-2", inset ? "ml-3.5 w-[calc(100%-0.875rem)]" : "w-full", TONE[tone])} role="img" aria-label={`price ${Math.round(v * 100)}% of the way from the pullback low to the trigger`}>
      <rect x="0" y="3" width="160" height="2" rx="1" className="fill-bg-raised" />
      <rect x="0" y="0" width="2" height="8" className="fill-critical" />
      <rect x="158" y="0" width="2" height="8" className="fill-muted-foreground" />
      <rect x="0" y="3" width={v * 156} height="2" rx="1" fill="currentColor" />
      <circle cx={2 + v * 154} cy="4" r="3" fill="currentColor" />
    </svg>
  );
}
