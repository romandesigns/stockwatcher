import { cn, Tip } from "@stockwatcher/chart-web/ui";
import { rocketHeaderView, rocketLinePaths, rocketNow, rocketRowsView, type RocketChannel, type RocketRowView, type RocketState, type RocketTone } from "@stockwatcher/shared";
import { memo, useEffect, useState } from "react";
import { useRocket } from "../feed/useRocket.ts";
import { useTickerPress } from "../lib/useTickerPress.ts";

const TONE: Record<RocketTone, string> = {
  good: "text-good",
  warning: "text-warning",
  accent: "text-primary",
  text: "text-foreground",
  muted: "text-muted-foreground",
  dim: "text-text-faint",
};

/**
 * The inset edge a momentum state earns (the Ross cue edge: a filled shadow, never a border):
 * amber while building, accent while breaking out (with a faint wash) or running. Quiet states
 * get none.
 */
const EDGE: Partial<Record<RocketTone, string>> = {
  warning: "shadow-[inset_3px_0_0_var(--warning)]",
  accent: "shadow-[inset_3px_0_0_var(--accent)]",
};

/** Re-renders every `ms` so ages ("now", "3m") keep moving between frames. */
function useTick(ms: number): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), ms);
    return () => clearInterval(id);
  }, [ms]);
  return now;
}

/**
 * Runners (owner, 2026-10-21/22; EXPERIMENTAL): symbols the server's strategy flags as likely to
 * start a +30 % leg soon, each then watched for bullish momentum until 20:00 ET. Rows are ordered
 * by their momentum state (breaking out first, fading last) and colour-coded in the Ross row
 * language: accent for a breakout (pulsing) or a run, amber while building and on a halt, a
 * pullback muted with its depth and the micro-pullback trigger meter, a fade dimmed. Green only
 * for the strict outcome tag. The header counts what is building or breaking out. Pressing a row
 * loads it into the selected chart; tap, then hold copies the ticker. Server feed only.
 */
export function RocketPanel({ channel, onPick }: { channel: RocketChannel; onPick: (symbol: string) => void }) {
  const state = useRocket(channel);
  const wall = useTick(15_000);
  return <RocketPanelView state={state} wallNow={wall} onPick={onPick} />;
}

/** The panel for a given state (separate so it renders in tests without a socket). */
export function RocketPanelView({ state, wallNow, onPick }: { state: RocketState; wallNow: number; onPick: (symbol: string) => void }) {
  const { enabled, panel } = state;
  const now = rocketNow(state, wallNow);
  const rows = panel ? rocketRowsView(panel, now) : [];
  const head = panel ? rocketHeaderView(panel) : null;
  return (
    <aside aria-labelledby="rocket-title" className="flex min-h-0 min-w-0 flex-col gap-2 rounded-xl bg-card p-2.5">
      <div className="flex items-center gap-2 px-1">
        <h2 id="rocket-title" className="m-0 text-sm font-semibold">
          Runners
        </h2>
        <span className="rounded bg-bg-raised px-1.5 py-px text-[10px] font-medium text-muted-foreground">Experimental</span>
        {head && head.attention > 0 && (
          <span className="inline-flex h-[22px] items-center gap-1.5 rounded-full bg-primary/15 px-2 text-[11px] text-primary" aria-label={`${head.attention} building or breaking out`}>
            <span className="size-1.5 animate-pulse rounded-full bg-primary motion-reduce:animate-none" aria-hidden />
            {head.attention}
          </span>
        )}
        {head && <span className="ml-auto truncate font-mono text-[11px] text-muted-foreground tabular-nums">{head.record}</span>}
      </div>
      {head && (head.strategy || head.history || head.ranLater) && (
        <p className="m-0 truncate px-1 font-mono text-[10px] text-text-faint">
          {[head.ranLater, head.placeholder ? `${head.strategy} (placeholder)` : head.strategy, head.history].filter(Boolean).join(" · ")}
        </p>
      )}
      {panel && !state.connected && <p className="m-0 px-1 text-[11px] text-warning">Reconnecting · last known flags</p>}
      {enabled === false ? (
        <p className="m-0 px-1 py-2 text-xs text-warning">Off on this server</p>
      ) : enabled === null ? (
        <p className="m-0 px-1 py-2 text-xs text-text-faint">Connecting…</p>
      ) : rows.length === 0 ? (
        <p className="m-0 px-1 py-2 text-xs text-text-faint">Nothing flagged right now</p>
      ) : (
        <ul className="m-0 flex min-h-0 list-none flex-col gap-1 overflow-y-auto p-0">
          {rows.map((r) => (
            <li key={r.key}>
              <RocketRow row={r} onPick={onPick} />
            </li>
          ))}
        </ul>
      )}
    </aside>
  );
}

const RocketRow = memo(function RocketRow({ row: r, onPick }: { row: RocketRowView; onPick: (symbol: string) => void }) {
  const press = useTickerPress(r.symbol, () => onPick(r.symbol));
  const m = r.momentum;
  return (
    <button
      type="button"
      {...press}
      aria-label={`${r.aria}. Load into the selected chart`}
      data-state={m.state}
      className={cn(
        // State changes crossfade (colours, edge, wash); reduced motion: instant.
        "flex w-full cursor-pointer touch-manipulation flex-col gap-1 rounded-lg bg-bg-row px-2.5 py-2 text-left font-mono outline-none select-none transition-[background-color,box-shadow,opacity,color] duration-300 hover:bg-bg-raised focus-visible:ring-2 focus-visible:ring-ring motion-reduce:transition-none",
        m.edge && EDGE[m.edge],
        m.state === "breaking_out" && "bg-primary/10",
        m.dimmed && "opacity-60",
      )}
    >
      <span className="flex items-baseline gap-1.5">
        <span className="text-sm font-semibold">{r.symbol}</span>
        <span className="text-[13px] tabular-nums">{r.price}</span>
        {r.gain && <span className="text-xs text-muted-foreground tabular-nums">{r.gain}</span>}
        {r.halted && <span className="rounded bg-warning px-1 py-px font-sans text-[9px] font-semibold text-background">HALTED</span>}
        <span className={cn("ml-auto inline-flex items-center gap-1 text-[11px] tabular-nums", r.fresh ? "font-semibold text-primary" : "text-text-faint")}>
          {r.fresh && <span className="size-1.5 animate-pulse rounded-full bg-primary motion-reduce:animate-none" aria-hidden />}
          {r.age}
        </span>
      </span>
      <span className="grid grid-cols-[minmax(0,1fr)_72px] items-center gap-2">
        <span className="flex min-w-0 flex-col gap-1">
          <span className={cn("flex items-center gap-1.5 overflow-hidden font-sans text-xs whitespace-nowrap transition-colors duration-300 motion-reduce:transition-none", TONE[m.tone], m.attention && "font-semibold")}>
            {m.pulse && <span className="size-1.5 shrink-0 animate-pulse rounded-full bg-current motion-reduce:animate-none" aria-hidden />}
            {m.label}
            {m.provisional && <span className="font-normal text-text-faint">provisional</span>}
            {m.since && <span className="font-mono font-normal text-text-faint">{m.since}</span>}
            {m.note && <span className="truncate font-mono font-normal text-muted-foreground">{m.note}</span>}
          </span>
          <span className="flex gap-2 text-[11px] whitespace-nowrap tabular-nums" aria-label="from flag, from high, last minute">
            <Tip label="From the flag price"><span className={TONE[r.fromFlagTone]}>{r.fromFlag}</span></Tip>
            <Tip label="Off the high since the flag"><span className={TONE[r.fromHighTone]}>{r.fromHigh}</span></Tip>
            <Tip label="Last minute"><span className={TONE[r.move1mTone]}>{r.move1m}</span></Tip>
          </span>
        </span>
        <DayLine line={r.line} />
      </span>
      {r.meter !== null && <TriggerMeter value={r.meter} tone={m.tone} />}
      <span className="flex items-center gap-1.5 overflow-hidden whitespace-nowrap">
        {r.reasons.map((x) => (
          <span key={x} className="truncate rounded bg-bg-raised px-1.5 py-px font-sans text-[11px] text-muted-foreground">
            {x}
          </span>
        ))}
        <span className="ml-auto flex shrink-0 items-center gap-2">
          {r.outcome && <span className={cn("font-sans text-[11px] font-semibold", TONE[r.outcome.tone])}>{r.outcome.label}</span>}
          {r.score && <Tip label="Strategy score"><span className="text-[10px] text-text-faint tabular-nums">{r.score}</span></Tip>}
        </span>
      </span>
    </button>
  );
});

/**
 * The row chart (the Ross row's size): the day's 30 s closes from 04:00 ET with the live price as
 * its end, drawn in the neutral text tone (green belongs to the strict outcome here), with the
 * previous close as a faint dotted baseline when it is near enough to draw.
 */
function DayLine({ line }: { line: RocketRowView["line"] }) {
  if (!line) return <span className="h-7 w-[72px]" aria-hidden />;
  const p = rocketLinePaths(line, 72, 28);
  return (
    <svg viewBox="0 0 72 28" className="h-7 w-[72px] animate-line-in text-muted-foreground" aria-hidden data-testid="rocket-day-line">
      {p.baseY !== null && <line x1={0} x2={72} y1={p.baseY} y2={p.baseY} stroke="currentColor" strokeOpacity={0.35} strokeWidth={1} strokeDasharray="1.5 2.5" />}
      <path d={p.area} fill="currentColor" fillOpacity={0.12} />
      <path d={p.line} fill="none" stroke="currentColor" strokeWidth={1.5} strokeLinejoin="round" />
    </svg>
  );
}

/** Pullback low (left, red: the failure level) to trigger (right, neutral), the price as a dot: the Ross row's meter. */
function TriggerMeter({ value, tone }: { value: number; tone: RocketTone }) {
  const v = Math.max(0, Math.min(1, value));
  return (
    <svg viewBox="0 0 160 8" preserveAspectRatio="none" className={cn("block h-2 w-full", TONE[tone])} role="img" aria-label={`price ${Math.round(v * 100)}% of the way from the pullback low to the trigger`}>
      <rect x="0" y="3" width="160" height="2" rx="1" className="fill-bg-raised" />
      <rect x="0" y="0" width="2" height="8" className="fill-critical" />
      <rect x="158" y="0" width="2" height="8" className="fill-muted-foreground" />
      <rect x="0" y="3" width={v * 156} height="2" rx="1" fill="currentColor" />
      <circle cx={2 + v * 154} cy="4" r="3" fill="currentColor" />
    </svg>
  );
}
