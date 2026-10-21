import { formatEtTime } from "@stockwatcher/chart-core";
import { cn } from "@stockwatcher/chart-web/ui";
import { rossAttention, rossDemoPanel, rossFreshness, rossReadinessView, rossRowView, type RossChannel, type RossRowView, type RossTone } from "@stockwatcher/shared";
import { memo } from "react";
import { useRoss } from "../feed/useRoss.ts";
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

/** The left edge and background a setup cue earns; quiet rows get none (design canvas "Ross Panel"). */
const EDGE: Partial<Record<RossRowView["setup"]["state"], string>> = {
  crossing: "shadow-[inset_3px_0_0_var(--accent)] bg-primary/10",
  near: "shadow-[inset_3px_0_0_var(--warning)]",
  triggered: "shadow-[inset_3px_0_0_var(--good)]",
};

const hhmm = (ns: number) => formatEtTime(Math.round(ns / 1_000_000));

/**
 * The Ross Panel (owner, 2026-10-19/20; design canvas "Ross Panel", web): the five-pillar
 * watchlist beside the charts, ordered by the last minute's move, each member monitored for an
 * early bull-flag setup. Rows earn a cue only when they need you (near the trigger, crossing it,
 * triggered); the header counts them. Pressing a row loads its ticker into the selected chart;
 * tap, then hold copies it (the Halt watcher's gesture). Server feed only.
 */
export function RossPanel({ channel, onPick }: { channel: RossChannel; onPick: (symbol: string) => void }) {
  const live = useRoss(channel);
  // The labelled demo mode (dev builds only, `?rossDemo`): synthetic members to see the cues
  // before the evidence sources admit real ones. Never in a production build.
  const demo = import.meta.env.DEV && new URLSearchParams(window.location.search).has("rossDemo");
  const state = demo ? { ...live, enabled: true, panel: rossDemoPanel(Date.now() * 1_000_000), connected: true } : live;
  const { enabled, panel, readiness } = state;
  const rows = panel?.rows.map((r) => rossRowView(r, panel.cutoff, hhmm)) ?? [];
  const attention = panel ? rossAttention(panel) : 0;
  // A dropped socket keeps the last rows and says so: they are the last known state, not now.
  const fresh = rossFreshness(state, hhmm);
  const why = readiness ? rossReadinessView(readiness, hhmm) : null;
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
      {fresh.stale && enabled !== false && (
        <p className={cn("m-0 flex items-center gap-1.5 px-1 text-[11px]", TONE[fresh.tone])} role="status">
          <span className="size-1.5 rounded-full bg-warning" aria-hidden />
          {fresh.label}
        </p>
      )}
      {enabled === false ? (
        <p className="m-0 px-1 py-2 text-xs text-warning">Off on this server</p>
      ) : enabled === null ? (
        <p className="m-0 px-1 py-2 text-xs text-text-faint">Connecting…</p>
      ) : rows.length === 0 ? (
        // Not "nothing qualifies": the server's own counts say whether a pillar could even be
        // decided (crates/server/src/context.rs `Readiness`).
        <div className="m-0 flex flex-col gap-1 px-1 py-2 text-xs text-text-faint">
          <p className="m-0">{why ? why.headline : "No stock at 5/5 yet"}</p>
          {why && why.reasons.length > 0 && (
            <ul className="m-0 flex list-none flex-col gap-0.5 p-0 font-mono text-[11px]">
              {why.reasons.map((r) => (
                <li key={r}>{r}</li>
              ))}
            </ul>
          )}
          {why && <p className="m-0 text-[10px] opacity-70">{why.takenAt}</p>}
        </div>
      ) : (
        <ul className={cn("m-0 flex min-h-0 list-none flex-col gap-1 overflow-y-auto p-0", fresh.stale && "opacity-60")}>
          {rows.map((r) => (
            <li key={r.symbol}>
              <RossRow row={r} onPick={onPick} />
            </li>
          ))}
        </ul>
      )}
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
        <Sparkline points={r.spark} />
      </span>
      {r.setup.meter !== null && <TriggerMeter value={r.setup.meter} tone={r.setup.tone} />}
    </button>
  );
});

/** The last completed 15 s closes as an area sparkline (the shadcn area-chart look, inline SVG). */
function Sparkline({ points }: { points: number[] }) {
  if (points.length < 2) return <span className="h-7 w-[72px]" aria-hidden />;
  const lo = Math.min(...points);
  const hi = Math.max(...points);
  const xy = points.map((v, i) => [(i / (points.length - 1)) * 72, 25 - ((v - lo) / (hi - lo || 1)) * 21] as const);
  const line = `M${xy.map(([x, y]) => `${x.toFixed(1)} ${y.toFixed(1)}`).join(" L")}`;
  const up = points[points.length - 1]! >= points[0]!;
  return (
    <svg viewBox="0 0 72 28" className={cn("h-7 w-[72px]", up ? "text-good" : "text-critical")} aria-hidden>
      <path d={`${line} L72 28 L0 28 Z`} fill="currentColor" fillOpacity={0.12} />
      <path d={line} fill="none" stroke="currentColor" strokeWidth={1.5} />
    </svg>
  );
}

/** Invalidation (left, red) to trigger (right, green), the price as a dot. */
function TriggerMeter({ value, tone }: { value: number; tone: RossTone }) {
  const v = Math.max(0, Math.min(1, value));
  return (
    <svg viewBox="0 0 160 8" preserveAspectRatio="none" className={cn("ml-3.5 block h-2 w-[calc(100%-0.875rem)]", TONE[tone])} role="img" aria-label={`price ${Math.round(v * 100)}% of the way from invalidation to trigger`}>
      <rect x="0" y="3" width="160" height="2" rx="1" className="fill-bg-raised" />
      <rect x="0" y="0" width="2" height="8" className="fill-critical" />
      <rect x="158" y="0" width="2" height="8" className="fill-good" />
      <rect x="0" y="3" width={v * 156} height="2" rx="1" fill="currentColor" />
      <circle cx={2 + v * 154} cy="4" r="3" fill="currentColor" />
    </svg>
  );
}
