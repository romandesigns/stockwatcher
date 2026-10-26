import { Icon } from "@stockwatcher/chart-web";
import { cn, ScrollArea, Tip } from "@stockwatcher/chart-web/ui";
import { rocketHeaderView, rocketLinePaths, rocketNow, rocketSectionsView, type RocketRowView, type RocketSectionsView, type RocketState, type RocketTone, withZoneSections } from "@stockwatcher/shared";
import { memo } from "react";
import { useTickerPress } from "../lib/useTickerPress.ts";
import { StarButton } from "../favorites/StarButton.tsx";
import { Reveal } from "./Reveal.tsx";
import { ZoneNote } from "./ZoneNote.tsx";

const TONE: Record<RocketTone, string> = {
  good: "text-good",
  loss: "text-critical",
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

/**
 * Runners (owner, 2026-10-21/22; EXPERIMENTAL): symbols the server's strategy flags as likely to
 * start a +30 % leg soon, each then watched for bullish momentum until 20:00 ET. Rows are ordered
 * by their momentum state (breaking out first, fading last) and colour-coded in the Ross row
 * language: accent for a breakout (pulsing) or a run, amber while building and on a halt, a
 * pullback muted with its depth and the micro-pullback trigger meter, a fade dimmed. Green only
 * for the strict outcome tag. The header counts what is building or breaking out. Pressing a row
 * loads it into the selected chart; tap, then hold copies the ticker. Server feed only.
 *
 * Buying bursts (2026-10-22) sit in their own section at the top, newest first: heavy buying
 * confirmed on the tape, typically about halfway into a move, so each row says how far the burst
 * already went and what typically remains, never "buy". The accent marks only a live burst (its
 * pulsing dot and edge while fresh and pending); green and red only its strict outcome tag.
 *
 * Running entries (2026-10-22, `kind: "running"`) sit with the flags in the same strongest-first
 * order: a stock that IS running by a price-relative rule on the tape, not a forecast. Every row
 * carries a quiet source chip ("R2 flag", "Burst", "Running") so an early pick stays
 * distinguishable from a move already under way; the chip never spends accent. "Run +20%" rows
 * (`kind: "run"`, the research labeller's +20 % leg) are the fourth source and sit there too,
 * measured from their entry like a running row; "Fast move" rows (`kind: "fast"`, +8 % in 60 s
 * or +12 % in 15 s, held, with real trading) the fifth.
 *
 * Nothing empty is drawn (owner, 2026-10-22): no Bursts section without bursts, no Flags without
 * flags, and no panel at all (its tab is hidden) while off, not yet known or empty. While a socket
 * reconnects the last rows stay, dimmed and labelled.
 *
 * It takes its state, not the channel: the side column subscribes (useRocket) to decide whether
 * the tab is offered and passes the same state down, and tests render it without a socket.
 *
 * A row whose symbol came down into a volume shelf (the frame's `zones`, shared zones.ts) says so
 * on its own full-width line, with the research base rate muted beside it, then "Shelf held" /
 * "Shelf lost" once resolved (ZoneNote): plain text, never green, never the accent.
 */
export function RocketPanelView({ state, wallNow, onPick }: { state: RocketState; wallNow: number; onPick: (symbol: string) => void }) {
  const { enabled, panel } = state;
  const now = rocketNow(state, wallNow);
  const { bursts, runners: rows }: RocketSectionsView = panel ? withZoneSections(rocketSectionsView(panel, now), panel.zones) : { bursts: null, runners: [] };
  const head = panel ? rocketHeaderView(panel, state.connected) : null;
  return (
    <aside aria-labelledby="rocket-title" className="flex min-h-0 min-w-0 flex-col gap-2 rounded-xl bg-card p-2.5">
      <div className="flex items-center gap-2 px-1">
        <h2 id="rocket-title" className="panel-title">
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
      {panel && !state.connected && <p role="status" className="m-0 px-1 text-[11px] text-warning">Reconnecting · last known flags</p>}
      {/* Each section only while it has rows (owner, 2026-10-22), its heading included; the whole
          panel is hidden by its tab at 0 (SideTabs). Sections open and close by their real height. */}
      {enabled === true && (
        <ScrollArea className={cn("transition-opacity duration-200 motion-reduce:transition-none", panel && !state.connected && "opacity-60")} viewportClassName="flex flex-col">
          <Reveal show={(bursts?.rows.length ?? 0) > 0}>
            {bursts && (
              <section aria-labelledby="burst-title" className="flex flex-col gap-1 pb-3">
                <div className="flex items-center gap-1.5 px-1">
                  <Icon name="trend" className="size-3.5 text-muted-foreground" />
                  <h3 id="burst-title" className="m-0 text-xs font-semibold">
                    Bursts
                  </h3>
                  <span className="ml-auto font-mono text-[11px] text-muted-foreground tabular-nums">{bursts.head.count}</span>
                </div>
                <p className="m-0 px-1 text-[10px] leading-snug text-text-faint">{[bursts.head.what, bursts.head.typical].filter(Boolean).join(". ")}</p>
                <RowList rows={bursts.rows} onPick={onPick} />
              </section>
            )}
          </Reveal>
          <Reveal show={rows.length > 0}>
            <section aria-label="Flags" className="flex flex-col">
              {/* The heading only beside a Bursts section: alone, the panel's title says it. */}
              <Reveal show={(bursts?.rows.length ?? 0) > 0}>
                <h3 className="m-0 px-1 pb-1 text-xs font-semibold">Flags &amp; running</h3>
              </Reveal>
              <RowList rows={rows} onPick={onPick} />
            </section>
          </Reveal>
        </ScrollArea>
      )}
    </aside>
  );
}

function RowList({ rows, onPick }: { rows: RocketRowView[]; onPick: (symbol: string) => void }) {
  return (
    <ul className="m-0 flex list-none flex-col gap-1 p-0">
      {rows.map((r) => (
        <li key={r.key} className="relative">
          <RocketRow row={r} onPick={onPick} />
          {/* Beside the row, not in it (a button cannot hold a button). */}
          <StarButton symbol={r.symbol} className="absolute top-1 right-1" />
        </li>
      ))}
    </ul>
  );
}

const RocketRow = memo(function RocketRow({ row: r, onPick }: { row: RocketRowView; onPick: (symbol: string) => void }) {
  const press = useTickerPress(r.symbol, () => onPick(r.symbol));
  const m = r.momentum;
  const b = r.burst;
  // A live burst takes the accent edge; once it settles the row falls back to its momentum edge.
  const edge = b?.live ? "accent" : m.edge;
  return (
    <button
      type="button"
      {...press}
      aria-label={`${r.aria}. Load into the selected chart`}
      data-state={m.state}
      data-kind={r.source.kind}
      className={cn(
        // State changes crossfade (colours, edge, wash); reduced motion: instant.
        "flex w-full cursor-pointer touch-manipulation flex-col gap-1 rounded-lg bg-bg-row px-2.5 py-2 text-left font-mono outline-none select-none transition-[background-color,box-shadow,opacity,color] duration-300 hover:bg-bg-raised focus-visible:ring-2 focus-visible:ring-ring motion-reduce:transition-none",
        edge && EDGE[edge],
        m.state === "breaking_out" && "bg-primary/10",
        m.dimmed && "opacity-60",
      )}
    >
      <span className="flex items-baseline gap-1.5 pr-7">
        <span className="text-sm font-semibold">{r.symbol}</span>
        <span className="text-[13px] tabular-nums">{r.price}</span>
        {r.gain && <span className="text-xs text-muted-foreground tabular-nums">{r.gain}</span>}
        <span className="self-center rounded bg-bg-raised px-1 py-px font-sans text-[9px] font-medium whitespace-nowrap text-muted-foreground" data-testid="rocket-source">
          {r.source.label}
        </span>
        {/* Later paths on the same stock (follow-ups): secondary, quieter than the source chip. */}
        {r.also.map((a) => (
          <Tip key={a.kind} label={`${a.label} ${a.age === "now" ? "just now" : `${a.age} ago`}, after the row`}>
            <span className="self-center truncate font-sans text-[9px] whitespace-nowrap text-text-faint" data-testid="rocket-also">
              + {a.label}
            </span>
          </Tip>
        ))}
        {/* The armed MP5 setup ("Pullback · trigger $3.42"): a fact on the row, quiet like the source chip, never accent. */}
        {r.setup && (
          <span className="self-center truncate rounded bg-bg-raised px-1 py-px font-sans text-[9px] font-medium whitespace-nowrap text-foreground" data-testid="rocket-setup">
            {r.setup.label}
          </span>
        )}
        {r.halted && <span className="rounded bg-warning px-1 py-px font-sans text-[9px] font-semibold text-background">HALTED</span>}
        <span className={cn("ml-auto inline-flex items-center gap-1 text-[11px] tabular-nums", r.fresh ? "font-semibold text-primary" : "text-text-faint")}>
          {r.fresh && <span className="size-1.5 animate-pulse rounded-full bg-primary motion-reduce:animate-none" aria-hidden />}
          {r.age}
        </span>
      </span>
      <span className="grid grid-cols-[minmax(0,1fr)_72px] items-center gap-2">
        <span className="flex min-w-0 flex-col gap-1">
          {b && (
            <span className="flex items-center gap-1.5 overflow-hidden font-sans text-xs whitespace-nowrap">
              {b.live && <span className="size-1.5 shrink-0 animate-pulse rounded-full bg-primary motion-reduce:animate-none" aria-hidden />}
              <span className={cn("font-semibold transition-colors duration-300 motion-reduce:transition-none", b.live ? "text-primary" : "text-foreground")}>{b.headline}</span>
              <span className="truncate font-mono text-muted-foreground tabular-nums">{b.since}</span>
            </span>
          )}
          <span className={cn("flex items-center gap-1.5 overflow-hidden font-sans text-xs whitespace-nowrap transition-colors duration-300 motion-reduce:transition-none", TONE[m.tone], m.attention && "font-semibold")}>
            {m.pulse && <span className="size-1.5 shrink-0 animate-pulse rounded-full bg-current motion-reduce:animate-none" aria-hidden />}
            {m.label}
            {m.provisional && <span className="font-normal text-text-faint">provisional</span>}
            {m.since && <span className="font-mono font-normal text-text-faint">{m.since}</span>}
            {m.note && <span className="truncate font-mono font-normal text-muted-foreground">{m.note}</span>}
          </span>
          <span className="flex gap-2 text-[11px] whitespace-nowrap tabular-nums" aria-label={b ? "last 30 s before the alert, from the alert, from high, last minute" : `from ${r.fromWhat}, from high, last minute`}>
            {b?.move && <Tip label="The 30 s before the alert"><span className="text-foreground">{b.move}</span></Tip>}
            <Tip label={`From the ${r.fromWhat} price`}><span className={TONE[r.fromFlagTone]}>{r.fromFlag}</span></Tip>
            <Tip label={`Off the high since the ${r.fromWhat}`}><span className={TONE[r.fromHighTone]}>{r.fromHigh}</span></Tip>
            <Tip label="Last minute"><span className={TONE[r.move1mTone]}>{r.move1m}</span></Tip>
          </span>
        </span>
        <DayLine line={r.line} />
      </span>
      {r.meter !== null && <TriggerMeter value={r.meter} tone={m.tone} />}
      {/* The row's whole width (the shelf range and base rate need it on a narrow column); while
          closed it cancels the row's gap-1 so an absent note costs no space. */}
      <ZoneNote note={r.zone ?? null} closedClassName="-mt-1" />
      <span className="flex items-center gap-1.5 overflow-hidden whitespace-nowrap">
        {r.reasons.map((x) => (
          <span key={x} className="truncate rounded bg-bg-raised px-1.5 py-px font-sans text-[11px] text-muted-foreground">
            {x}
          </span>
        ))}
        <span className="ml-auto flex shrink-0 items-center gap-2">
          {r.outcome && <span className={cn("font-sans text-[11px] font-semibold", TONE[r.outcome.tone])}>{r.outcome.label}</span>}
          {r.score && <Tip label={b ? "Burst model score" : r.source.kind === "running" ? "Move over its requirement" : r.source.kind === "run" ? "Leg gain over the +20% it needed" : r.source.kind === "fast" ? "Held move over the % it needed" : "Strategy score"}><span className="text-[10px] text-text-faint tabular-nums">{r.score}</span></Tip>}
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
