import { cn } from "@stockwatcher/chart-web/ui";
import { rocketHeaderView, rocketNow, rocketRowView, type RocketChannel, type RocketRowView, type RocketState, type RocketTone } from "@stockwatcher/shared";
import { memo, useEffect, useState } from "react";
import { useRocket } from "../feed/useRocket.ts";
import { useTickerPress } from "../lib/useTickerPress.ts";

const TONE: Record<RocketTone, string> = {
  good: "text-good",
  accent: "text-primary",
  text: "text-foreground",
  muted: "text-muted-foreground",
  dim: "text-text-faint",
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
 * Rocket watch (owner, 2026-10-21; EXPERIMENTAL): symbols the server's strategy flags as likely
 * to start a +30 % leg soon, newest first, with today's track record in the header. Accent only
 * for a flag under a minute old; green only for an outcome that rocketed. Pressing a row loads it
 * into the selected chart; tap, then hold copies the ticker. Server feed only.
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
  const rows = panel?.rows.map((r) => rocketRowView(r, now)) ?? [];
  const head = panel ? rocketHeaderView(panel) : null;
  return (
    <aside aria-labelledby="rocket-title" className="flex min-h-0 min-w-0 flex-col gap-2 rounded-xl bg-card p-2.5">
      <div className="flex items-center gap-2 px-1">
        <h2 id="rocket-title" className="m-0 text-sm font-semibold">
          Runners
        </h2>
        <span className="rounded bg-bg-raised px-1.5 py-px text-[10px] font-medium text-muted-foreground">Experimental</span>
        {head && <span className="ml-auto truncate font-mono text-[11px] text-muted-foreground tabular-nums">{head.record}</span>}
      </div>
      {head && (head.strategy || head.history) && (
        <p className="m-0 truncate px-1 font-mono text-[10px] text-text-faint">
          {[head.placeholder ? `${head.strategy} (placeholder)` : head.strategy, head.history].filter(Boolean).join(" · ")}
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
  return (
    <button
      type="button"
      {...press}
      aria-label={`${r.aria}. Load into the selected chart`}
      className="flex w-full cursor-pointer touch-manipulation flex-col gap-1 rounded-lg bg-bg-row px-2.5 py-2 text-left font-mono outline-none select-none transition-colors duration-150 hover:bg-bg-raised focus-visible:ring-2 focus-visible:ring-ring motion-reduce:transition-none"
    >
      <span className="flex items-baseline gap-1.5">
        <span className="text-sm font-semibold">{r.symbol}</span>
        <span className="text-[13px] tabular-nums">{r.price}</span>
        {r.gain && <span className="text-xs text-muted-foreground tabular-nums">{r.gain}</span>}
        <span className={cn("ml-auto inline-flex items-center gap-1 text-[11px] tabular-nums", r.fresh ? "font-semibold text-primary" : "text-text-faint")}>
          {r.fresh && <span className="size-1.5 animate-pulse rounded-full bg-primary motion-reduce:animate-none" aria-hidden />}
          {r.age}
        </span>
      </span>
      <span className="flex items-center gap-1.5 overflow-hidden whitespace-nowrap">
        {r.reasons.map((x) => (
          <span key={x} className="truncate rounded bg-bg-raised px-1.5 py-px font-sans text-[11px] text-muted-foreground">
            {x}
          </span>
        ))}
        <span className="ml-auto flex shrink-0 items-center gap-2">
          {r.outcome && <span className={cn("font-sans text-[11px] font-semibold", TONE[r.outcome.tone])}>{r.outcome.label}</span>}
          {r.score && <span className="text-[10px] text-text-faint tabular-nums" title="Strategy score">{r.score}</span>}
        </span>
      </span>
    </button>
  );
});
