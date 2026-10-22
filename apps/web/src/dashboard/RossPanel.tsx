import { formatEtTime } from "@stockwatcher/chart-core";
import { cn, Tip } from "@stockwatcher/chart-web/ui";
import { detectionFamilyLabel, detectionStateLabel, groupDetections, rossAttention, rossFreshness, rossLinePaths, rossRowView, type DetectionChannel, type RossLineView, type RossRowView, type RossTone } from "@stockwatcher/shared";
import { memo } from "react";
import type { RossDemoState } from "../feed/useRossState.ts";
import { useDetection } from "../feed/useDetection.ts";
import { useTickerPress } from "../lib/useTickerPress.ts";

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
export function RossPanelView({ state, onPick, detection, onEpisodePick }: { state: RossDemoState; onPick: (symbol: string) => void; detection?: DetectionChannel; onEpisodePick?: (symbol: string, episodeId: string, runEpoch: string) => void }) {
  const detected = useDetection(detection);
  const joined = new Map(groupDetections(detected.snapshot).map((g) => [g.symbol, g.selected]));
  const { enabled, panel, demo } = state;
  const rows = panel?.rows.map((r) => rossRowView(r, panel.cutoff, hhmm)) ?? [];
  const attention = panel ? rossAttention(panel) : 0;
  // A dropped socket keeps the last rows and says so: they are the last known state, not now.
  const fresh = rossFreshness(state, hhmm);
  return (
    <aside aria-labelledby="ross-title" className="flex min-h-0 min-w-0 flex-col gap-2 rounded-xl bg-card p-2.5">
      <div className="flex items-center gap-2 px-1">
        <h2 id="ross-title" className="m-0 text-sm font-semibold">
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
      <ul className={cn("m-0 flex min-h-0 list-none flex-col gap-1 overflow-y-auto p-0 transition-opacity duration-200 motion-reduce:transition-none", fresh.stale && "opacity-60")}>
        {rows.map((r) => (
          <li key={r.symbol}>
            <RossRow row={r} onPick={onPick} />
            {joined.has(r.symbol) && (() => {
              const e = joined.get(r.symbol)!;
              return <Tip label={e.episode_id}><button type="button" className="w-full rounded-b px-2.5 py-1 text-left text-[10px] text-primary outline-none focus-visible:ring-2 focus-visible:ring-ring" onClick={() => onEpisodePick ? onEpisodePick(e.symbol, e.episode_id, detected.frame?.runEpoch ?? "") : onPick(e.symbol)}>Detection: {detectionFamilyLabel(e.family)} · {detectionStateLabel(e.state)}{!detected.connected ? " · last known" : ""}</button></Tip>;
            })()}
          </li>
        ))}
      </ul>
    </aside>
  );
}

const RossRow = memo(function RossRow({ row: r, onPick }: { row: RossRowView; onPick: (symbol: string) => void }) {
  const press = useTickerPress(r.symbol, () => onPick(r.symbol));
  return (
    <button
      type="button"
      {...press}
      aria-label={`${r.aria}. Load into the selected chart`}
      className={cn(
        "flex w-full cursor-pointer touch-manipulation flex-col gap-1 rounded-lg bg-bg-row px-2.5 py-2 text-left font-mono outline-none select-none transition-colors duration-150 hover:bg-bg-raised focus-visible:ring-2 focus-visible:ring-ring motion-reduce:transition-none",
        EDGE[r.setup.state],
        r.dimmed && "opacity-60",
      )}
    >
      <span className="flex items-baseline gap-1.5">
        <span className={cn("w-2 text-[10px]", TONE[r.moveTone])}>{r.move}</span>
        <span className="text-sm font-semibold">{r.symbol}</span>
        <span className="text-[13px] tabular-nums">{r.price}</span>
        <span className="text-xs tabular-nums text-good">{r.gain}</span>
        {r.badge && <span className={cn("rounded px-1 py-px font-sans text-[9px] font-semibold text-background", r.badge.label === "NEW" ? "bg-primary" : "bg-warning")}>{r.badge.label}</span>}
        <span className={cn("ml-auto text-[13px] font-semibold tabular-nums", TONE[r.m1Tone])}>{r.m1}</span>
      </span>
      <span className="grid grid-cols-[minmax(0,1fr)_72px] items-center gap-2 pl-3.5">
        <span className="flex min-w-0 flex-col gap-1">
          <span className="flex gap-2 text-[11px] whitespace-nowrap text-muted-foreground tabular-nums">
            <span className={TONE[r.fromHighTone]}>{r.fromHigh}</span>
            <span>{r.rvol}</span>
            {r.tag && <span className={cn("font-sans", TONE[r.tagTone])}>{r.tag}</span>}
          </span>
          <span className={cn("flex items-center gap-1.5 overflow-hidden font-sans text-xs whitespace-nowrap", TONE[r.setup.tone], r.setup.attention && "font-semibold")}>
            {r.setup.pulse && <span className="size-1.5 shrink-0 animate-pulse rounded-full bg-current motion-reduce:animate-none" aria-hidden />}
            {r.setup.label}
            {r.setup.note && <span className="truncate font-mono font-normal text-muted-foreground">{r.setup.note}</span>}
          </span>
        </span>
        <Sparkline line={r.line} />
      </span>
      {r.setup.meter !== null && <TriggerMeter value={r.setup.meter} tone={r.setup.tone} />}
    </button>
  );
});

/**
 * The row chart (owner, 2026-10-21): the day's 30 s closes from 04:00 ET with the live price as
 * the end, or an older server's last 15 s closes, as an area sparkline (the shadcn area-chart
 * look, inline SVG). Good when the line ends at or above the day's first price; the previous
 * close is a faint dotted baseline when it is near enough to draw (`rossLinePaths`).
 */
function Sparkline({ line }: { line: RossLineView | null }) {
  if (!line) return <span className="h-7 w-[72px]" aria-hidden />;
  const p = rossLinePaths(line, 72, 28);
  return (
    <svg viewBox="0 0 72 28" className={cn("h-7 w-[72px] animate-line-in", line.tone === "good" ? "text-good" : "text-critical")} aria-hidden>
      {p.baseY !== null && <line x1={0} x2={72} y1={p.baseY} y2={p.baseY} stroke="currentColor" strokeOpacity={0.35} strokeWidth={1} strokeDasharray="1.5 2.5" />}
      <path d={p.area} fill="currentColor" fillOpacity={0.12} />
      <path d={p.line} fill="none" stroke="currentColor" strokeWidth={1.5} strokeLinejoin="round" />
    </svg>
  );
}

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
