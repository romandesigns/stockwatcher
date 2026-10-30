import { formatEtTime, formatPrice } from "@stockwatcher/chart-core";
import { Icon } from "@stockwatcher/chart-web";
import {
  cn,
  ScrollArea,
  Tip,
  ToggleGroup,
  ToggleGroupItem,
} from "@stockwatcher/chart-web/ui";
import {
  DEFAULT_MOVER_SORT,
  formatChangePct,
  formatDollarVolume,
  MOVER_SORT_KEYS,
  MOVER_SORT_LABEL,
  moverLine,
  moverRowMarks,
  moversRows,
  nextMoverSort,
  parseMoverSort,
  rocketNow,
  runnersBySymbol,
  sortMovers,
  type MoverRowWire,
  type MoversList,
  type MoversState,
  type MoverSort,
  type MoverSortKey,
  type RocketRowWire,
  type RocketState,
} from "@stockwatcher/shared";
import { memo, useCallback, useMemo, useState } from "react";
import { StarButton } from "../favorites/StarButton.tsx";
import { useTickerPress } from "../lib/useTickerPress.ts";
import { Reveal } from "./Reveal.tsx";
import { Flame, MarksLine, RowLine } from "./RowMarks.tsx";

const hhmm = (ms: number) => formatEtTime(ms);

export type MoversKind = "gainers" | "hot";

const TITLE: Record<MoversKind, string> = {
  gainers: "Top Gainers",
  hot: "Hot stocks",
};
/** What each list is, under its title (the definitions live in crates/server/src/movers.rs). */
const CAPTION: Record<MoversKind, string> = {
  gainers: "% today · $250K+",
  hot: "$ volume today",
};

const SORT_KEY = (kind: MoversKind) => `stockwatcher.moversSort.${kind}`;

/** The panel's sort, kept on this device (a screen preference, never synced). */
function readSort(kind: MoversKind): MoverSort {
  try {
    const raw = window.localStorage.getItem(SORT_KEY(kind));
    return raw ? parseMoverSort(JSON.parse(raw)) : DEFAULT_MOVER_SORT;
  } catch {
    return DEFAULT_MOVER_SORT;
  }
}

function writeSort(kind: MoversKind, s: MoverSort): void {
  try {
    window.localStorage.setItem(SORT_KEY(kind), JSON.stringify(s));
  } catch {
    // A private window: the choice lasts this session.
  }
}

function useMoverSort(
  kind: MoversKind,
): [MoverSort, (key: MoverSortKey) => void] {
  const [sort, setSort] = useState<MoverSort>(() =>
    typeof window === "undefined" ? DEFAULT_MOVER_SORT : readSort(kind),
  );
  const press = useCallback(
    (key: MoverSortKey) =>
      setSort((cur) => {
        const next = nextMoverSort(cur, key);
        writeSort(kind, next);
        return next;
      }),
    [kind],
  );
  return [sort, press];
}

const DIR_WORDS: Record<MoverSortKey, [string, string]> = {
  dv: ["most traded first", "least traded first"],
  chg: ["biggest move first", "smallest move first"],
  price: ["highest price first", "lowest price first"],
  symbol: ["A to Z", "Z to A"],
};

/**
 * The sort as a compact segmented control: one segment per key, the active one carrying a chevron
 * for its direction (down: the key's natural order, strongest first; up: reversed). Pressing the
 * active segment again flips it, so the group never ends up with nothing pressed.
 */
function SortControl({
  kind,
  sort,
  onPress,
}: {
  kind: MoversKind;
  sort: MoverSort;
  onPress: (key: MoverSortKey) => void;
}) {
  return (
    <ToggleGroup
      aria-label={`Sort ${TITLE[kind]}`}
      size="sm"
      spacing={0.5}
      variant="segmented"
      value={[sort.key]}
      onValueChange={(v) =>
        onPress((v[0] as MoverSortKey | undefined) ?? sort.key)
      }
      className="shrink-0"
    >
      {MOVER_SORT_KEYS.map((k) => {
        const on = sort.key === k;
        const words = DIR_WORDS[k][sort.dir === 1 ? 0 : 1];
        return (
          <ToggleGroupItem
            key={k}
            value={k}
            aria-label={
              on
                ? `Sorted by ${MOVER_SORT_LABEL[k]}, ${words}. Press to reverse`
                : `Sort by ${MOVER_SORT_LABEL[k]}`
            }
            className="h-6 gap-0.5 px-1.5 font-mono text-[11px]"
          >
            {MOVER_SORT_LABEL[k]}
            {on && (
              <Icon
                name={sort.dir === 1 ? "chevronDown" : "chevronUp"}
                className="size-2.5!"
              />
            )}
          </ToggleGroupItem>
        );
      })}
    </ToggleGroup>
  );
}

/**
 * Top Gainers, and Hot stocks as two sections, Bull and Bear (owner, 2026-10-30: "make the hot
 * stocks panel into two sections, bull and bear and I should be able to sort them. Also add a line
 * chart that constantly updates live based on each stock price action"). The lists are ranked on
 * the server once a tick for every client and pushed over the movers channel
 * (crates/server/src/movers.rs): the day's biggest % gainers with $250K+ behind them, and the
 * most traded names up and down on the day, all $0.10–$25 with at least 100 prints, 25 each.
 *
 * Each panel has its own sort (dollar volume by default, % change, price, ticker; press the active
 * key again to reverse it), kept on this device. Bull and Bear each show their count and scroll on
 * their own; a section with no rows closes (animated), and the panel hides when it has none.
 *
 * One compact row per name, in the other panels' row language: ticker, price and day change, the
 * live day line (the Favorites row chart: 30 s closes from 04:00 ET with the live price as its
 * end, the previous close dotted), the dollar volume; the flame, the Runners path and the last
 * minute's tail as marks. Rows are memoized on their wire object, which the channel keeps for a
 * row an update did not touch, so a frame re-renders only the rows it changed. Pressing a row
 * loads it into the selected chart (tap, then hold copies it). It takes its state, so tests need
 * no socket.
 */
export function MoversPanelView({
  kind,
  state,
  runners,
  wallNow,
  onPick,
}: {
  kind: MoversKind;
  state: MoversState;
  runners?: RocketState;
  wallNow: number;
  onPick: (symbol: string) => void;
}) {
  const id = `movers-${kind}-title`;
  const [sort, pressSort] = useMoverSort(kind);
  const nowMs = runners ? rocketNow(runners, wallNow) : wallNow;
  const byRunner = useMemo(() => runnersBySymbol(runners), [runners]);
  const reconnecting = !state.connected && state.rows !== null;
  const section = (list: MoversList) =>
    sortMovers(moversRows(state, list), sort, list);
  return (
    <aside
      aria-labelledby={id}
      data-panel={kind}
      className="@container/movers flex h-full min-h-0 min-w-0 flex-col gap-2 rounded-xl bg-card p-2.5"
    >
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1.5 px-1">
        <div className="flex min-w-0 flex-auto items-baseline gap-2">
          <h2 id={id} className="panel-title shrink-0">
            {TITLE[kind]}
          </h2>
          <span className="truncate font-mono text-[11px] text-text-faint">
            {CAPTION[kind]}
          </span>
          {kind === "gainers" && (
            <span className="ml-auto font-mono text-[11px] text-text-faint tabular-nums">
              {state.gainers.length}
            </span>
          )}
        </div>
        <SortControl kind={kind} sort={sort} onPress={pressSort} />
      </div>
      {reconnecting && (
        <p
          className="m-0 flex items-center gap-1.5 px-1 text-[11px] text-warning"
          role="status"
        >
          <span className="size-1.5 rounded-full bg-warning" aria-hidden />
          Reconnecting · last known rows
        </p>
      )}
      <div
        className={cn(
          "flex min-h-0 flex-1 flex-col gap-2 transition-opacity duration-200 motion-reduce:transition-none @2xl/movers:flex-row",
          reconnecting && "opacity-60",
        )}
      >
        {kind === "gainers" ? (
          <MoverList
            label="Top Gainers"
            rows={section("gainers")}
            list="gainers"
            byRunner={byRunner}
            nowMs={nowMs}
            onPick={onPick}
          />
        ) : (
          <>
            <MoverSection
              list="bull"
              rows={section("bull")}
              byRunner={byRunner}
              nowMs={nowMs}
              onPick={onPick}
            />
            <MoverSection
              list="bear"
              rows={section("bear")}
              byRunner={byRunner}
              nowMs={nowMs}
              onPick={onPick}
            />
          </>
        )}
      </div>
    </aside>
  );
}

const SECTION: Record<"bull" | "bear", { title: string; dot: string }> = {
  bull: { title: "Bull", dot: "bg-good" },
  bear: { title: "Bear", dot: "bg-critical" },
};

/**
 * One Hot stocks section: its title and count over its own scroll. It closes by its real size when
 * it has no rows. Stacked, the two share the panel's height by their content (the shorter keeps
 * its rows, the longer scrolls); side by side on a wide panel, half each.
 */
function MoverSection({
  list,
  rows,
  byRunner,
  nowMs,
  onPick,
}: {
  list: "bull" | "bear";
  rows: MoverRowWire[];
  byRunner: ReadonlyMap<string, RocketRowWire>;
  nowMs: number;
  onPick: (symbol: string) => void;
}) {
  const s = SECTION[list];
  const titleId = `movers-${list}-title`;
  return (
    <Reveal
      show={rows.length > 0}
      className={cn(
        // Wide (a fixed-height panel): half each, each scrolling its own rows; closing, it gives its
        // half to the other by flex-grow as it shrinks away. Stacked under the charts the panel is
        // as tall as its rows (to a cap), so the sections size by content and share the cap.
        "min-h-0 min-w-0 basis-auto @4xl/panel:basis-0 @2xl/movers:basis-0 transition-[grid-template-rows,flex-grow,opacity] duration-300 ease-out motion-reduce:transition-none",
        rows.length > 0 ? "grow" : "grow-0",
      )}
      innerClassName="flex flex-col gap-1.5"
    >
      <section
        aria-labelledby={titleId}
        data-section={list}
        className="flex min-h-0 flex-1 flex-col gap-1.5"
      >
        <div className="flex items-center gap-1.5 px-1">
          <span className={cn("size-1.5 rounded-full", s.dot)} aria-hidden />
          <h3
            id={titleId}
            className="m-0 text-xs font-semibold text-muted-foreground"
          >
            {s.title}
          </h3>
          <span className="ml-auto font-mono text-[11px] text-text-faint tabular-nums">
            {rows.length}
          </span>
        </div>
        <MoverList
          label={s.title}
          rows={rows}
          list={list}
          byRunner={byRunner}
          nowMs={nowMs}
          onPick={onPick}
        />
      </section>
    </Reveal>
  );
}

function MoverList({
  label,
  rows,
  list,
  byRunner,
  nowMs,
  onPick,
}: {
  label: string;
  rows: MoverRowWire[];
  list: MoversList;
  byRunner: ReadonlyMap<string, RocketRowWire>;
  nowMs: number;
  onPick: (symbol: string) => void;
}) {
  return (
    <ScrollArea className="min-h-0">
      <ol aria-label={label} className="m-0 flex list-none flex-col gap-1 p-0">
        {rows.map((r) => (
          <li key={r.symbol} className="relative">
            <MoverRowButton
              row={r}
              list={list}
              runner={byRunner.get(r.symbol)}
              nowMs={nowMs}
              onPick={onPick}
            />
            {/* Beside the row, not in it (a button cannot hold a button). */}
            <StarButton
              symbol={r.symbol}
              className="absolute top-1/2 right-1 -translate-y-1/2"
            />
          </li>
        ))}
      </ol>
    </ScrollArea>
  );
}

const MoverRowButton = memo(function MoverRowButton({
  row: r,
  list,
  runner,
  nowMs,
  onPick,
}: {
  row: MoverRowWire;
  list: MoversList;
  runner: RocketRowWire | undefined;
  nowMs: number;
  onPick: (symbol: string) => void;
}) {
  const press = useTickerPress(r.symbol, () => onPick(r.symbol));
  const price = r.price / 1_000_000;
  const m = useMemo(
    () => moverRowMarks(price, runner, nowMs, hhmm, r.day_line),
    [price, runner, nowMs, r.day_line],
  );
  const line = useMemo(() => moverLine(r), [r]);
  const change = formatChangePct(r.chg);
  const dollars = formatDollarVolume(r.dv);
  const shown = formatPrice(price);
  const up = r.chg > 0;
  const down = r.chg < 0;
  // The split note (crates/server/src/quotes.rs PcBasis): why the change may not be against the close.
  const basis =
    r.pcb === "split"
      ? "vs previous close, split-adjusted"
      : r.pcb === "guard"
        ? "vs today's first price: the previous close is on another share basis (split)"
        : "";
  return (
    <button
      type="button"
      {...press}
      aria-label={`${r.symbol} ${shown}, ${change} today${basis ? ` ${basis}` : ""}, ${dollars} traded${r.halted ? ", halted" : ""}. Load into the selected chart`}
      data-flame={m.flame?.lit || undefined}
      className={cn(
        "flex w-full cursor-pointer touch-manipulation flex-col justify-center gap-1 rounded-lg bg-bg-row py-1.5 pr-9 pl-2.5 text-left font-mono outline-none select-none transition-colors duration-150 hover:bg-bg-raised focus-visible:ring-2 focus-visible:ring-ring pointer-coarse:min-h-11 motion-reduce:transition-none",
        m.flame?.lit && "bg-bg-raised/60",
      )}
    >
      <span className="flex min-w-0 items-center gap-1.5">
        <Flame flame={m.flame} />
        <span className="text-sm font-semibold">{r.symbol}</span>
        <span className="text-sm tabular-nums">{shown}</span>
        <ChangeText
          change={change}
          basis={basis}
          className={cn(
            up ? "text-good" : down ? "text-critical" : "text-muted-foreground",
            list === "gainers" && "font-semibold",
          )}
        />
        {r.halted && (
          <span className="rounded bg-warning px-1 py-px font-sans text-[9px] font-semibold text-background">
            HALTED
          </span>
        )}
        <RowLine
          line={line}
          tail={m.tail}
          label={`${dollars} traded today`}
          tone={line?.tone === "critical" ? "text-critical" : "text-good"}
          stretch
          className="ml-auto hidden w-16 shrink-0 @xs/movers:block @sm/movers:w-24"
        />
        <span
          className={cn(
            "ml-auto shrink-0 text-xs tabular-nums @xs/movers:ml-0",
            list === "gainers" ? "text-muted-foreground" : "text-foreground",
          )}
        >
          {dollars}
        </span>
      </span>
      {/* A narrow panel (a quarter of the bottom row) has no room beside the numbers: the line
          gets the row's full width under them instead, so every row still draws it. */}
      <RowLine
        line={line}
        tail={m.tail}
        h={18}
        tone={line?.tone === "critical" ? "text-critical" : "text-good"}
        stretch
        className="w-full @xs/movers:hidden"
      />
      <MarksLine marks={m} />
    </button>
  );
});

/** The day change, with a small mark and tooltip when its reference is not the plain previous close. */
function ChangeText({
  change,
  basis,
  className,
}: {
  change: string;
  basis: string;
  className: string;
}) {
  const text = (
    <span className={cn("text-xs tabular-nums", className)}>
      {change}
      {basis && <sup className="ml-px text-[8px] text-text-faint">s</sup>}
    </span>
  );
  return basis ? <Tip label={`Change ${basis}`}>{text}</Tip> : text;
}
