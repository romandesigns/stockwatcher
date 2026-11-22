import { Icon } from "@stockwatcher/chart-web";
import { cn, ScrollArea, Tip } from "@stockwatcher/chart-web/ui";
import { formatEtTime } from "@stockwatcher/chart-core";
import { rocketHeaderView, rocketServerNow, rocketRowMarks, rocketSectionsView, rocketShadowView, type RocketRowView, type RocketSectionsView, type RocketShadowRowView, type RocketShadowView, type RocketState, type RocketTone, type RowMarks, withZoneSections } from "@stockwatcher/shared";
import { memo } from "react";
import { useTickerPress } from "../lib/useTickerPress.ts";
import { StarButton } from "../favorites/StarButton.tsx";
import { Reveal } from "./Reveal.tsx";
import { Flame, MarksLine, RowLine } from "./RowMarks.tsx";
import { SpreadChip } from "./SpreadChip.tsx";
import { ConfidenceChip } from "./ConfidenceChip.tsx";
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
 * by their momentum state (breaking out first; fading rows are left out) and colour-coded in the Ross row
 * language: accent for a breakout (pulsing) or a run, amber while building and on a halt, a
 * pullback muted with its depth and the micro-pullback trigger meter. Green only
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
 * flags, and no panel at all (DashboardPanels hides it) while off, not yet known or empty. While a socket
 * reconnects the last rows stay, dimmed and labelled.
 *
 * It takes its state, not the channel: the dashboard layout subscribes (useRocket) to decide whether
 * the panel shows and passes the same state down, and tests render it without a socket.
 *
 * A row whose symbol came down into a volume shelf (the frame's `zones`, shared zones.ts) says so
 * on its own full-width line, with the research base rate muted beside it, then "Shelf held" /
 * "Shelf lost" once resolved (ZoneNote): plain text, never green, never the accent.
 *
 * The Shadow section (2026-11-17) sits last: research detectors measured BESIDE production, never
 * part of it (crates/server/src/n1_live, the N1 "early leg"). Each row is labelled by its section
 * and chip, quiet throughout (no accent, no edge, no outcome colour): a shadow fire is something
 * to measure, not a call. Present only on a day it fired.
 *
 * Each row ends its second line with the alert's fire-time spread (owner, 2026-11-19; SpreadChip,
 * packages/shared spread.ts): "Spread 0.42%" quiet up to 100 bp, amber to 300 bp, red past it. It
 * is the cost of trading the alert, which the 2026-11-19 diagnostics found decides whether one is
 * worth taking. No chip when the server had no market quote.
 */
export function RocketPanelView({ state, onPick }: { state: RocketState; onPick: (symbol: string) => void }) {
  const { enabled, panel } = state;
  // The server's market clock: the channel re-renders on its clock frames, so ages move with it.
  const now = rocketServerNow(state) ?? 0;
  const { bursts, runners: rows }: RocketSectionsView = panel ? withZoneSections(rocketSectionsView(panel, now), panel.zones) : { bursts: null, runners: [] };
  // The lighter rows' marks (rowMarks.ts), by row key (the wire id).
  const marks: MarksById = new Map((panel?.rows ?? []).map((w) => [w.id, { marks: rocketRowMarks(w, now, formatEtTime), trigger: w.levels?.trigger ?? null }]));
  const head = panel ? rocketHeaderView(panel, state.connected) : null;
  const shadow = panel ? rocketShadowView(panel) : null;
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
        <p className="m-0 shrink-0 truncate px-1 font-mono text-[10px] text-text-faint">
          {[head.ranLater, head.placeholder ? `${head.strategy} (placeholder)` : head.strategy, head.history].filter(Boolean).join(" · ")}
        </p>
      )}
      {panel && !state.connected && <p role="status" className="m-0 px-1 text-[11px] text-warning">Reconnecting · last known flags</p>}
      {/* Each section only while it has rows (owner, 2026-10-22), its heading included; the whole
          panel is hidden at 0 (DashboardPanels). Sections open and close by their real height. */}
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
                <RowList rows={bursts.rows} marks={marks} onPick={onPick} />
              </section>
            )}
          </Reveal>
          <Reveal show={rows.length > 0}>
            <section aria-label="Flags" className="flex flex-col">
              {/* The heading only beside a Bursts section: alone, the panel's title says it. */}
              <Reveal show={(bursts?.rows.length ?? 0) > 0}>
                <h3 className="m-0 px-1 pb-1 text-xs font-semibold">Flags &amp; running</h3>
              </Reveal>
              <RowList rows={rows} marks={marks} onPick={onPick} />
            </section>
          </Reveal>
          <Reveal show={(shadow?.rows.length ?? 0) > 0}>{shadow && <ShadowSection shadow={shadow} onPick={onPick} />}</Reveal>
        </ScrollArea>
      )}
    </aside>
  );
}

/** The Shadow section: today's shadow fires, newest first (module docs). */
function ShadowSection({ shadow, onPick }: { shadow: RocketShadowView; onPick: (symbol: string) => void }) {
  return (
    <section aria-labelledby="rocket-shadow-title" className="flex flex-col gap-1 pt-3" data-testid="rocket-shadow">
      <div className="flex items-center gap-1.5 px-1">
        <Icon name="insights" className="size-3.5 text-muted-foreground" />
        <h3 id="rocket-shadow-title" className="m-0 text-xs font-semibold">
          Shadow
        </h3>
        <Tip label="Research detector measured beside production. It does not change any Runners alert.">
          <span className="rounded bg-bg-raised px-1.5 py-px text-[10px] font-medium text-muted-foreground">{shadow.label}</span>
        </Tip>
        <span className="ml-auto font-mono text-[11px] text-muted-foreground tabular-nums">{shadow.rows.length} today</span>
      </div>
      <ul className="m-0 flex list-none flex-col gap-1 p-0">
        {shadow.rows.map((r) => (
          <li key={`${r.symbol}-${r.at}`} className="@container/row relative">
            <ShadowRow row={r} onPick={onPick} />
            <StarButton symbol={r.symbol} className="absolute top-1 right-1" />
          </li>
        ))}
      </ul>
    </section>
  );
}

const ShadowRow = memo(function ShadowRow({ row: r, onPick }: { row: RocketShadowRowView; onPick: (symbol: string) => void }) {
  const press = useTickerPress(r.symbol, () => onPick(r.symbol));
  return (
    <button
      type="button"
      {...press}
      aria-label={`${r.aria}. Load into the selected chart`}
      data-kind="shadow"
      className="flex w-full cursor-pointer touch-manipulation flex-col gap-1 bg-bg-row px-2.5 py-2 text-left font-mono outline-none select-none transition-colors duration-300 hover:bg-bg-raised focus-visible:ring-2 focus-visible:ring-ring motion-reduce:transition-none pointer-coarse:min-h-11"
    >
      <span className="flex min-w-0 flex-wrap items-center gap-x-1.5 gap-y-0.5 pr-7">
        <span className="text-sm font-semibold">{r.symbol}</span>
        <span className="text-sm tabular-nums">{r.price}</span>
        <span className="text-xs text-muted-foreground tabular-nums">{r.gain}</span>
        <span className="ml-auto text-[11px] text-muted-foreground tabular-nums">{formatEtTime(r.at, true)}</span>
      </span>
      <span className="flex min-w-0 items-center gap-1.5 font-sans text-xs text-muted-foreground">
        <span className="truncate">{r.evidence}</span>
        {!r.notified && <span className="shrink-0 text-text-faint">· not sent</span>}
      </span>
    </button>
  );
});

/** A row's marks and its setup's trigger (micros), by the row's key (its wire id). */
type MarksById = ReadonlyMap<string, { marks: RowMarks; trigger: number | null }>;

const NO_MARKS: RowMarks = { flame: null, pillars: null, rvol: null, news: null, path: null, zone: null, stage: null, tail: null };

function RowList({ rows, marks, onPick }: { rows: RocketRowView[]; marks: MarksById; onPick: (symbol: string) => void }) {
  return (
    <ul className="m-0 flex list-none flex-col gap-1 p-0">
      {rows.map((r) => (
        <li key={r.key} className="@container/row relative">
          <RocketRow row={r} marks={marks.get(r.key)?.marks ?? NO_MARKS} trigger={marks.get(r.key)?.trigger ?? null} onPick={onPick} />
          {/* Beside the row, not in it (a button cannot hold a button). */}
          <StarButton symbol={r.symbol} className="absolute top-1 right-1" />
        </li>
      ))}
    </ul>
  );
}

/**
 * One Runners row as a lighter row (owner, 2026-10-26, design canvas "StockWatcher lighter rows"):
 * ticker, price and day gain as text with the day line; the second line is the Runners path (its
 * age, later paths, the move since the flag and the reasons in its tooltip), the momentum word,
 * the outcome tag and the setup's stage and meter. The flame lights on a fresh burst that holds.
 */
const RocketRow = memo(function RocketRow({ row: r, marks, trigger, onPick }: { row: RocketRowView; marks: RowMarks; trigger: number | null; onPick: (symbol: string) => void }) {
  const press = useTickerPress(r.symbol, () => onPick(r.symbol));
  const m = r.momentum;
  const b = r.burst;
  // A live burst takes the accent edge; once it settles the row falls back to its momentum edge.
  const edge = b?.live ? "accent" : m.edge;
  // Everything the old state line and burst line spelled out, for the momentum word's tooltip.
  const stateLabel = [b ? [b.headline, b.since, b.move].filter(Boolean).join(" · ") : "", m.label, m.provisional ? "provisional" : "", m.since, m.note].filter(Boolean).join(" · ");
  return (
    <button
      type="button"
      {...press}
      aria-label={`${r.aria}. Load into the selected chart`}
      data-state={m.state}
      data-kind={r.source.kind}
      className={cn(
        // State changes crossfade (colours, edge, wash); reduced motion: instant.
        "flex w-full cursor-pointer touch-manipulation flex-col gap-1 bg-bg-row px-2.5 py-2 text-left font-mono outline-none select-none transition-[background-color,box-shadow,opacity,color] duration-300 hover:bg-bg-raised focus-visible:ring-2 focus-visible:ring-ring motion-reduce:transition-none",
        edge && EDGE[edge],
        m.state === "breaking_out" && "bg-primary/10",
        m.dimmed && "opacity-60",
      )}
    >
      <span className="flex items-center gap-1.5 pr-7">
        <Flame flame={marks.flame} />
        <span className="text-sm font-semibold">{r.symbol}</span>
        <span className="text-sm tabular-nums">{r.price}</span>
        {r.gain && <span className="text-xs text-muted-foreground tabular-nums">{r.gain}</span>}
        {r.fresh && <span className="size-1.5 shrink-0 animate-pulse rounded-full bg-primary motion-reduce:animate-none" aria-label="Flagged under a minute ago" role="img" />}
        {r.halted && <span className="rounded bg-warning px-1 py-px font-sans text-[9px] font-semibold text-background">HALTED</span>}
        {/* Neutral (green belongs to the strict outcome here); the tail still says the last minute. */}
        <RowLine line={r.line} tail={marks.tail} trigger={marks.stage ? trigger : null} tone="text-muted-foreground" stretch className="ml-auto w-18 @xs/row:w-26" />
      </span>
      <MarksLine
        marks={marks}
        extra={
          <>
            {/* The momentum state stays a word (it is the row's verdict, not a fact a mark can
                hold); a live burst says its own headline. The rest of the old sentence is in the
                path's tooltip. */}
            <Tip label={stateLabel}>
              <span className={cn("inline-flex min-w-0 items-center gap-1.5 truncate font-sans text-xs transition-colors duration-300 motion-reduce:transition-none", TONE[b?.live ? "accent" : m.tone], (m.attention || b?.live) && "font-semibold")} role="img" aria-label={stateLabel} data-testid="rocket-state">
                {(m.pulse || b?.live) && <span className="size-1.5 shrink-0 animate-pulse rounded-full bg-current motion-reduce:animate-none" aria-hidden />}
                <span className="truncate">{b?.live ? b.headline : m.label}</span>
              </span>
            </Tip>
            {r.outcome && <span className={cn("shrink-0 font-sans text-[11px] font-semibold", TONE[r.outcome.tone])}>{r.outcome.label}</span>}
            {/* What the alert cost to trade when it fired (owner, 2026-11-19): last on the line,
                so a chip arriving a moment after the row moves nothing before it. */}
            <SpreadChip spread={r.spread} />
            {/* b-good-1's tag, 30 s after the alert (owner, 2026-11-22): after the spread, so its
                late arrival moves nothing before it. */}
            <ConfidenceChip confidence={r.confidence} />
          </>
        }
      />
      {/* The row's whole width (the shelf range and base rate need it on a narrow column); while
          closed it cancels the row's gap so an absent note costs no space. */}
      <ZoneNote note={r.zone ?? null} closedClassName="-mt-1.5" />
    </button>
  );
});
