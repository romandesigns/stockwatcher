import { formatEtTime } from "@stockwatcher/chart-core";
import { cn, Collapsible, CollapsibleContent, CollapsibleTrigger, ScrollArea, Tip } from "@stockwatcher/chart-web/ui";
import { detectionFamilyLabel, detectionStateLabel, groupDetections, rocketServerNow, ROSS_NEAR_PREVIEW, rossAttention, rossFreshness, rossNearView, rossPanelMode, rossRowMarks, rossRowView, runnersBySymbol, type DetectionChannel, type RocketState, type RossNearRowView, type RossRowView, type RossTone, type RowMarks } from "@stockwatcher/shared";
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
 * known and empty all hide the panel (DashboardPanels), so it has no "Connecting…" or quiet line of its
 * own. While a socket reconnects it keeps the last rows, dimmed and labelled.
 */
export function RossPanelView({ state, onPick, detection, onEpisodePick, runners }: { state: RossDemoState; onPick: (symbol: string) => void; detection?: DetectionChannel; onEpisodePick?: (symbol: string, episodeId: string, runEpoch: string) => void; runners?: RocketState }) {
  const detected = useDetection(detection);
  const joined = new Map(groupDetections(detected.snapshot).map((g) => [g.symbol, g.selected]));
  const { enabled, panel, demo } = state;
  // Today's Runners row per member: its path and age, and the burst behind the flame (rowMarks.ts).
  const byRunner = runnersBySymbol(runners);
  // The Runners market clock (a replay's own day); without Runners there is no path or flame to age.
  const nowMs = rocketServerNow(runners) ?? 0;
  const rows = panel?.rows.map((w) => ({ ...rossRowView(w, panel.cutoff, hhmm), marks: rossRowMarks(w, byRunner.get(w.symbol), nowMs, formatEtTime), trigger: w.setup?.trigger?.price ?? null })) ?? [];
  const attention = panel ? rossAttention(panel) : 0;
  // A dropped socket keeps the last rows and says so: they are the last known state, not now.
  const fresh = rossFreshness(state, hhmm);
  const near = rossNearView(state.readiness);
  const mode = rossPanelMode(panel);
  return (
    <aside aria-labelledby="ross-title" className="flex min-h-0 min-w-0 flex-col gap-2 rounded-xl bg-card p-2.5">
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1 px-1">
        <h2 id="ross-title" className="panel-title whitespace-nowrap">
          Ross Panel
        </h2>
        <span className="font-mono text-[11px] whitespace-nowrap text-text-faint">$0.10–$25</span>
        {/* The membership rule in force when the owner switched "require a news catalyst" off:
            a neutral note, not a status colour or the accent (it is a setting, not an alarm). */}
        {mode.newsOptional && (
          <Tip label={mode.title}>
            <span className="rounded bg-bg-row px-1.5 py-px text-[10px] font-medium whitespace-nowrap text-muted-foreground" aria-label={mode.title}>
              {mode.label}
            </span>
          </Tip>
        )}
        {demo && <span className="rounded bg-warning px-1 py-px whitespace-nowrap text-[9px] font-semibold text-background">DEMO DATA</span>}
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
            {/* The filing or story behind the news pillar, beside the row like the Detection line
                (a link cannot sit inside the row's button). A new tab: it leaves the app. */}
            {r.news?.href && (
              <a href={r.news.href} target="_blank" rel="noreferrer" className="flex min-h-6 items-center gap-1 rounded-b px-2.5 text-[10px] text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring pointer-coarse:min-h-11" aria-label={`Open ${r.news.sec ? "the SEC filing" : "the story"} ${r.news.label} in a new tab`}>
                News: {r.news.label} <span aria-hidden>↗</span>
              </a>
            )}
            {r.flags.filter((f) => f.href).map((f) => (
              <a key={f.kind} href={f.href!} target="_blank" rel="noreferrer" className="flex min-h-6 items-center gap-1 rounded-b px-2.5 text-[10px] text-warning outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring pointer-coarse:min-h-11" aria-label={`Open ${f.title} in a new tab`}>
                {f.title} <span aria-hidden>↗</span>
              </a>
            ))}
            {joined.has(r.symbol) && (() => {
              const e = joined.get(r.symbol)!;
              return <Tip label={e.episode_id}><button type="button" className="w-full rounded-b px-2.5 py-1 text-left text-[10px] text-primary outline-none focus-visible:ring-2 focus-visible:ring-ring" onClick={() => onEpisodePick ? onEpisodePick(e.symbol, e.episode_id, detected.frame?.runEpoch ?? "") : onPick(e.symbol)}>Detection: {detectionFamilyLabel(e.family)} · {detectionStateLabel(e.state)}{!detected.connected ? " · last known" : ""}</button></Tip>;
            })()}
          </li>
        ))}
      </ul>
      {near.rows.length > 0 && <NearSection rows={near.rows} total={near.total} onPick={onPick} />}
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
        "flex w-full cursor-pointer touch-manipulation flex-col gap-1.5 bg-bg-row px-2.5 py-2 text-left font-mono outline-none select-none transition-colors duration-150 hover:bg-bg-raised focus-visible:ring-2 focus-visible:ring-ring motion-reduce:transition-none",
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
      {r.flags.length > 0 && (
        // Warnings that ask the owner to check, on their own line under the marks: warning tone on
        // a neutral surface like the near rows' badges, never the accent. They wrap, never widen.
        <span className="flex flex-wrap gap-1">
          {r.flags.map((f) => (
            <span key={f.kind} className="rounded bg-bg-raised px-1.5 py-px font-sans text-[10px] font-semibold text-warning">
              {f.label}
            </span>
          ))}
        </span>
      )}
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

/**
 * Near-qualifying (owner, 2026-11-22): stocks passing price, gain and RVOL whose only non-passes
 * are UNKNOWN float or news, under the members and quieter than them. Each says which pillar is
 * missing with a warning-toned badge on a neutral surface (never the accent: these are not on
 * the panel). The first few show; the rest open from a disclosure, closed by default.
 */
function NearSection({ rows, total, onPick }: { rows: RossNearRowView[]; total: number; onPick: (symbol: string) => void }) {
  const head = rows.slice(0, ROSS_NEAR_PREVIEW);
  const rest = rows.slice(ROSS_NEAR_PREVIEW);
  const more = total - head.length;
  return (
    <section aria-labelledby="ross-near-title" className="flex flex-col gap-1 pt-3">
      <h3 id="ross-near-title" className="m-0 flex items-baseline gap-1.5 px-1 text-[11px] font-medium text-muted-foreground">
        Near · evidence missing
        <span className="font-mono text-text-faint">{total}</span>
      </h3>
      <ul className="m-0 flex list-none flex-col gap-1 p-0">
        {head.map((r) => <NearRow key={r.symbol} row={r} onPick={onPick} />)}
      </ul>
      {rest.length > 0 && (
        <Collapsible>
          <CollapsibleTrigger className="min-h-11 px-1 text-[11px] text-muted-foreground hover:text-foreground">{more} more</CollapsibleTrigger>
          <CollapsibleContent>
            <ul className="m-0 flex list-none flex-col gap-1 p-0">
              {rest.map((r) => <NearRow key={r.symbol} row={r} onPick={onPick} />)}
            </ul>
          </CollapsibleContent>
        </Collapsible>
      )}
    </section>
  );
}

const NearRow = memo(function NearRow({ row: r, onPick }: { row: RossNearRowView; onPick: (symbol: string) => void }) {
  const press = useTickerPress(r.symbol, () => onPick(r.symbol));
  return (
    <li className="@container/near">
      <button
        type="button"
        {...press}
        aria-label={`${r.aria}. Load into the selected chart`}
        className="flex min-h-11 w-full cursor-pointer touch-manipulation flex-wrap items-center gap-x-2 gap-y-1 rounded bg-bg-row/60 px-2.5 py-1.5 text-left font-mono outline-none select-none transition-colors duration-150 hover:bg-bg-raised focus-visible:ring-2 focus-visible:ring-ring motion-reduce:transition-none"
      >
        <span className="text-[13px] font-semibold text-muted-foreground">{r.symbol}</span>
        <span className="text-xs tabular-nums text-muted-foreground">{r.price}</span>
        <span className="text-xs tabular-nums text-good">{r.gain}</span>
        <span className="hidden text-[11px] tabular-nums text-text-faint @[14rem]/near:inline">{r.rvol}</span>
        <span className="ml-auto flex gap-1">
          {/* The reason is in the row's label: a tooltip trigger cannot sit inside this button. */}
          {r.badges.map((b) => (
            <span key={b.label} className="rounded bg-bg-raised px-1.5 py-px font-sans text-[10px] font-semibold text-warning">{b.label}</span>
          ))}
        </span>
      </button>
    </li>
  );
});
