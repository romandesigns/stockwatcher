import { Icon } from "@stockwatcher/chart-web";
import {
  cn,
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuLabel,
  DropdownMenuTrigger,
  ScrollArea,
} from "@stockwatcher/chart-web/ui";
import {
  filterHodRows,
  fmtHodMultiple,
  fmtHodPct,
  fmtHodPrice,
  fmtHodShares,
  fmtHodTime,
  hodPolicyLine,
  hodShort,
  hodTint,
  parseHodFilter,
  toggleHodFilter,
  type HodRowWire,
  type HodState,
  type HodTint,
} from "@stockwatcher/shared";
import { memo, useMemo, type ReactNode } from "react";
import { useTickerPress } from "../lib/useTickerPress.ts";
import { useApp } from "../state/store.ts";

// The page's store needs `window`; static-markup tests have none (useFavoriteList's rule).
const HAS_WINDOW = typeof window !== "undefined";
const NO_FILTER: { raw: unknown; set: (next: { on: string[] }) => void } = { raw: null, set: () => {} };
const useHodFilter: () => { raw: unknown; set: (next: { on: string[] }) => void } = HAS_WINDOW
  ? () => {
      const raw = useApp((s) => s.hodFilter);
      const set = useApp((s) => s.setHodFilter);
      return { raw, set };
    }
  : () => NO_FILTER;

/**
 * Row tints by category family: a few percent of a palette token over the panel, so the rows read
 * as grouped without spending the accent (which marks only the active filter). The first shown
 * tag decides the row's tint.
 */
const TINT_ROW: Record<HodTint, string> = {
  lowFloat: "bg-[color-mix(in_srgb,var(--warning)_7%,var(--bg-row))]",
  medFloat: "bg-[color-mix(in_srgb,var(--text-dim)_6%,var(--bg-row))]",
  squeeze: "bg-[color-mix(in_srgb,var(--flame)_7%,var(--bg-row))]",
};
const TINT_DOT: Record<HodTint, string> = {
  lowFloat: "bg-warning/70",
  medFloat: "bg-text-faint",
  squeeze: "bg-flame/70",
};

const signed = (x: number | null | undefined) =>
  x === null || x === undefined || !Number.isFinite(x) ? "text-text-faint" : x > 0 ? "text-good" : x < 0 ? "text-critical" : "text-muted-foreground";

/** The full table's columns, wide panels only (`@4xl/hod`); narrower rows wrap into two lines. */
const COLS =
  "@4xl/hod:grid @4xl/hod:grid-cols-[4.25rem_minmax(3.5rem,1fr)_minmax(3.5rem,1fr)_repeat(8,minmax(3.25rem,1fr))_minmax(9rem,2.2fr)] @4xl/hod:items-center @4xl/hod:gap-x-2";

const HEADERS = ["Time", "Symbol", "Price", "Volume", "Float", "Rel vol", "Rel vol 5m", "Gap", "Chg", "Chg 5m", "Chg 10m", "Category"];

/**
 * The HOD Momentum scanner (owner, 2026-11-22): every new high of day of a stock in the band
 * that matches one of the policy's categories (crates/server/src/hod/), newest first, with the
 * columns of a high-of-day momentum table: time, symbol, price, the day's volume, float, rel vol
 * at the daily rate and over the last 5 minutes, gap, change from the close and over 5 and 10
 * minutes, and the matching categories as tags. Short interest is not shown: there is no source
 * for it yet.
 *
 * The category menu is a checklist (Base UI checkbox items), saved per user in the synced
 * settings (`hodFilter`), so every device shows the same categories; the defaults are the
 * policy's. Rows are tinted by category family from the palette tokens. Not an alert: nothing
 * here notifies. Laid out by its own width (`@container/hod`): a full table when wide, two-line
 * rows when narrow. Pressing a ticker loads it (tap, then hold copies it).
 */
export function HodPanelView({ state, onPick }: { state: HodState; onPick: (symbol: string) => void }) {
  const id = "hod-title";
  const filter = useHodFilter();
  const on = useMemo(() => parseHodFilter(filter.raw, state.policy), [filter.raw, state.policy]);
  const rows = useMemo(() => filterHodRows(state.rows, on), [state.rows, on]);
  const reconnecting = !state.connected && state.rows.length > 0;
  const cats = state.policy?.categories ?? [];
  const filtered = cats.some((c) => on.has(c.id) !== c.default_on);
  return (
    <aside aria-labelledby={id} data-panel="hod" className="@container/hod flex h-full min-h-0 min-w-0 flex-col gap-2 rounded-xl bg-card p-2.5">
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1.5 px-1">
        <div className="flex min-w-0 flex-auto items-baseline gap-2">
          <h2 id={id} className="panel-title shrink-0">
            HOD Momentum
          </h2>
          <span className="truncate font-mono text-[11px] text-text-faint">new highs of day · not alerts</span>
          <span className="ml-auto font-mono text-[11px] text-text-faint tabular-nums">{rows.length}</span>
        </div>
        <DropdownMenu>
          <DropdownMenuTrigger
            aria-label="Choose categories"
            className={cn(
              "inline-flex h-7 shrink-0 items-center gap-1.5 rounded-md bg-bg-row px-2 text-xs text-muted-foreground outline-none pointer-coarse:min-h-11 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring [&_svg]:size-3.5",
              filtered && "text-primary hover:text-primary",
            )}
          >
            <Icon name="menu" />
            <span className="tabular-nums">
              {on.size}/{cats.length}
            </span>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="min-w-64">
            <DropdownMenuGroup>
            <DropdownMenuLabel>Categories (saved for you)</DropdownMenuLabel>
            {cats.map((c) => {
              const unavailable = c.needs_history && !state.hi52Available;
              return (
                <DropdownMenuCheckboxItem
                  key={c.id}
                  checked={on.has(c.id)}
                  closeOnClick={false}
                  onCheckedChange={() => filter.set(toggleHodFilter(on, c.id))}
                >
                  <span className={cn("size-1.5 shrink-0 rounded-full", TINT_DOT[hodTint(c.id)])} aria-hidden />
                  <span className="flex min-w-0 flex-col">
                    <span className="truncate">{c.name}</span>
                    {unavailable && <span className="text-[11px] text-text-faint">Needs 52 weeks of daily bars (not kept yet)</span>}
                  </span>
                </DropdownMenuCheckboxItem>
              );
            })}
            </DropdownMenuGroup>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
      {(reconnecting || !state.complete || !state.on) && (
        <p className="m-0 flex items-center gap-1.5 px-1 text-[11px] text-warning" role="status">
          <span className="size-1.5 rounded-full bg-warning" aria-hidden />
          {!state.on
            ? "Switched off (Settings → Runners) · no new rows"
            : reconnecting
              ? "Reconnecting · last known rows"
              : `Watching since ${state.sinceMs ? fmtHodTime(state.sinceMs) : "a restart"} · highs since then`}
        </p>
      )}
      <div className={cn("flex min-h-0 flex-1 flex-col transition-opacity duration-200 motion-reduce:transition-none", reconnecting && "opacity-60")}>
        <div aria-hidden className={cn("hidden px-2 pb-1 font-mono text-[10px] text-text-faint uppercase", COLS)}>
          {HEADERS.map((h, i) => (
            <span key={h} className={i >= 2 && i <= 10 ? "text-right" : undefined}>
              {h}
            </span>
          ))}
        </div>
        <ScrollArea className="min-h-0 flex-1" viewportClassName="h-full">
          <ul aria-label="HOD Momentum rows" className="m-0 flex list-none flex-col gap-1 p-0">
            {rows.map(({ row, cats }) => (
              <HodRow key={row.id} row={row} cats={cats} onPick={onPick} />
            ))}
          </ul>
        </ScrollArea>
        {state.policy && <p className="m-0 px-1 pt-1.5 text-[10px] leading-snug text-text-faint">{hodPolicyLine(state.policy)} Short interest: no data source yet.</p>}
      </div>
    </aside>
  );
}

const Cell = ({ label, children, className }: { label: string; children: ReactNode; className?: string }) => (
  <span className={cn("flex items-baseline gap-1 font-mono text-[11px] tabular-nums @4xl/hod:justify-end", className)}>
    <span className="text-text-faint @4xl/hod:sr-only">{label}</span>
    {children}
  </span>
);

const HodRow = memo(function HodRow({ row, cats, onPick }: { row: HodRowWire; cats: string[]; onPick: (s: string) => void }) {
  const press = useTickerPress(row.sym, () => onPick(row.sym));
  const tint = hodTint(cats[0] ?? "");
  return (
    <li data-hod-row={row.sym} className={cn("rounded-lg px-2 py-1.5", TINT_ROW[tint], COLS)}>
      {/* Narrow: line one is time, ticker, price and change; line two the figures and tags. */}
      <div className="flex min-w-0 items-baseline gap-2 @4xl/hod:contents">
        <span className="shrink-0 font-mono text-[11px] text-text-faint tabular-nums">{fmtHodTime(row.t)}</span>
        <button
          type="button"
          {...press}
          className="min-w-0 truncate rounded text-left text-sm font-semibold text-foreground outline-none select-none [touch-action:manipulation] pointer-coarse:min-h-11 hover:underline focus-visible:ring-2 focus-visible:ring-ring"
        >
          {row.sym}
        </button>
        <span className="ml-auto font-mono text-[13px] text-foreground tabular-nums @4xl/hod:ml-0 @4xl/hod:text-right">{fmtHodPrice(row.price)}</span>
        <span className={cn("font-mono text-[13px] tabular-nums @4xl/hod:hidden", signed(row.chg))}>{fmtHodPct(row.chg)}</span>
      </div>
      <div className="mt-1 flex flex-wrap gap-x-3 gap-y-0.5 @4xl/hod:contents">
        <Cell label="Vol">{fmtHodShares(row.vol)}</Cell>
        <Cell label="Float">{fmtHodShares(row.float)}</Cell>
        <Cell label="RV">{fmtHodMultiple(row.rvol)}</Cell>
        <Cell label="RV 5m">{row.rvol5 === null ? "—" : `${Math.round(row.rvol5 * 100)}%`}</Cell>
        <Cell label="Gap" className={signed(row.gap)}>
          {fmtHodPct(row.gap)}
        </Cell>
        <Cell label="Chg" className={cn("hidden @4xl/hod:flex", signed(row.chg))}>
          {fmtHodPct(row.chg)}
        </Cell>
        <Cell label="5m" className={signed(row.chg5)}>
          {fmtHodPct(row.chg5)}
        </Cell>
        <Cell label="10m" className={signed(row.chg10)}>
          {fmtHodPct(row.chg10)}
        </Cell>
      </div>
      <div className="mt-1 flex flex-wrap gap-1 @4xl/hod:mt-0">
        {cats.map((c) => (
          <span key={c} className="inline-flex items-center gap-1 rounded bg-bg-raised/60 px-1.5 py-0.5 text-[10px] text-muted-foreground">
            <span className={cn("size-1.5 rounded-full", TINT_DOT[hodTint(c)])} aria-hidden />
            {hodShort(c)}
          </span>
        ))}
      </div>
    </li>
  );
});
