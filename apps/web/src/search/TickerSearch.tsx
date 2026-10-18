import { formatPrice, formatVolume } from "@stockwatcher/chart-core";
import { Icon } from "@stockwatcher/chart-web";
import { cn, Dialog, DialogContent, DialogTitle, ScrollArea } from "@stockwatcher/chart-web/ui";
import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  activeTickerFilters,
  pushRecentTicker,
  refreshTickerRows,
  searchTickers,
  TICKER_SECTION_TITLES,
  tickerUniverse,
  visibleTickerFilterGroups,
  type QuoteChannel,
  type TickerFilterId,
  type TickerRow,
} from "@stockwatcher/shared";
import { useQuoteBoard } from "../feed/useQuoteBoard.ts";

const RECENTS_KEY = "sw-search:recent";

function loadRecents(): string[] {
  try {
    const v = JSON.parse(localStorage.getItem(RECENTS_KEY) ?? "[]");
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
  } catch {
    return [];
  }
}

function saveRecent(symbol: string) {
  try {
    localStorage.setItem(RECENTS_KEY, JSON.stringify(pushRecentTicker(loadRecents(), symbol)));
  } catch {
    // Private mode: recents just are not remembered.
  }
}

/** Text with the matched characters emphasised (accent), as VS Code's quick open does. */
function Highlight({ text, indices, className }: { text: string; indices: readonly number[]; className?: string }) {
  if (!indices.length) return <span className={className}>{text}</span>;
  const hit = new Set(indices);
  return (
    <span className={className}>
      {[...text].map((ch, i) =>
        hit.has(i) ? (
          <span key={i} className="font-semibold text-primary">
            {ch}
          </span>
        ) : (
          <span key={i}>{ch}</span>
        ),
      )}
    </span>
  );
}

// Rows rendered at a time: the list holds every candidate (thousands on a real day), but a DOM
// of 6,800 rows re-rendered once a second is what makes a list slow. More render as the end
// scrolls into view (or ↓ walks past it); typing narrows long before that matters.
const PAGE = 200;

/** Price, change % (vs the previous close, else the open; good/critical: the status colours) and day volume. */
function Quote({ price, changePct, volume }: { price?: number; changePct?: number; volume?: number }) {
  const up = (changePct ?? 0) >= 0;
  return (
    <>
      <span className="w-14 text-right text-foreground/80">{price === undefined ? "" : formatPrice(price)}</span>
      <span className={cn("w-14 text-right", up ? "text-good" : "text-critical")}>
        {changePct === undefined ? "" : `${up ? "+" : "−"}${Math.abs(changePct).toFixed(1)}%`}
      </span>
      <span className="w-11 text-right text-text-faint">{volume === undefined ? "" : formatVolume(volume)}</span>
    </>
  );
}

interface RowProps {
  row: TickerRow;
  index: number;
  active: boolean;
  current: boolean;
  onHover: (symbol: string) => void;
  onChoose: (row: TickerRow) => void;
}

// Memoized: refreshTickerRows keeps an unchanged row's identity, so a quote update re-renders
// only the rows whose values moved.
const Row = memo(function Row({ row: r, index: i, active, current, onHover, onChoose }: RowProps) {
  const t = r.item;
  return (
    <li role="presentation">
      {r.section && <div className="px-2.5 pt-2 pb-1 text-[10px] font-medium tracking-wide text-text-faint uppercase">{TICKER_SECTION_TITLES[r.section]}</div>}
      <div
        id={`ticker-opt-${t.symbol}`}
        role="option"
        aria-selected={active}
        aria-disabled={!t.available}
        data-index={i}
        onMouseMove={() => onHover(t.symbol)}
        onClick={() => onChoose(r)}
        className={cn(
          "grid cursor-pointer grid-cols-[64px_minmax(0,1fr)_auto] items-center gap-3 rounded-lg px-2.5 py-1.5",
          active && "bg-bg-raised",
          !t.available && "cursor-default opacity-55",
        )}
      >
        <Highlight text={t.symbol} indices={r.symbolIdx} className="font-mono text-[13px] font-medium text-foreground" />
        <span className="flex min-w-0 items-baseline gap-2">
          <Highlight text={t.name} indices={r.nameIdx} className="truncate text-[13px] text-muted-foreground" />
          {t.exchange && <span className="shrink-0 text-[10px] text-text-faint">{t.exchange}</span>}
        </span>
        <span className="flex items-center gap-2 font-mono text-[11px] tabular-nums">
          {current && <span className="rounded bg-primary/15 px-1.5 py-0.5 font-sans text-[10px] font-medium text-primary">Open</span>}
          {t.mock && <span className="rounded bg-warning-bg px-1 font-sans text-[9px] font-medium text-warning">mock</span>}
          <Quote price={t.price} changePct={t.changePct} volume={t.volume} />
        </span>
      </div>
    </li>
  );
});

export interface TickerSearchProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** The server's candidates (openable symbols). */
  symbols: readonly string[];
  /** The feed's live quote board; subscribed only while the panel is open. */
  quotes?: QuoteChannel;
  /** Append the mock universe (fixture data only: noise next to a real day). */
  mock: boolean;
  current: string;
  onSelect: (symbol: string) => void;
  /** Drop down right under the header's search pill, at its width (desktop); else a centred panel. */
  anchored?: boolean;
}

/**
 * Quick-open for tickers, modelled on VS Code's Ctrl+P: a panel near the
 * top of the screen with one input and a list under it. Letters match in
 * order (fuzzy), tickers rank above names, matched letters are
 * highlighted; ↑/↓ move, Enter opens, Esc closes. With an empty query the
 * recently opened tickers come first, then the most active. Every row shows
 * the live last price, change % (vs the previous close when known, else the open) and day volume, pushed by the
 * server while the panel is open. Rows the simulator has no data for (the
 * mock universe, fixture sources only) are shown, labelled, and cannot be
 * opened.
 */
export function TickerSearch({ open, onOpenChange, symbols, quotes, mock, current, onSelect, anchored = false }: TickerSearchProps) {
  const [query, setQuery] = useState("");
  // The active row is a symbol, not an index: rows refresh every second and an index would
  // silently point at another stock once the list changes under it.
  const [activeSym, setActiveSym] = useState<string | null>(null);
  const [limit, setLimit] = useState(PAGE);
  const board = useQuoteBoard(quotes, open);
  const tickers = useMemo(() => tickerUniverse(symbols, board, { mock }), [symbols, board, mock]);
  const groups = useMemo(() => visibleTickerFilterGroups(tickers), [tickers]);
  // Filters persist between openings (unlike the query): they are a way of working, not a search.
  const [filters, setFilters] = useState<ReadonlySet<TickerFilterId>>(new Set());
  const toggleFilter = (id: TickerFilterId) => {
    setFilters((f) => {
      const next = new Set(f);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
    setActiveSym(null);
    setLimit(PAGE);
  };
  const listRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  // Every opening starts clean, like VS Code (reset while rendering, on the opening itself).
  const [wasOpen, setWasOpen] = useState(open);
  if (open !== wasOpen) {
    setWasOpen(open);
    if (open) {
      setQuery("");
      setActiveSym(null);
      setLimit(PAGE);
    }
  }

  // Ranked (ordered) only when the query, filters or the opening change, and once more when the
  // first quote snapshot lands (the most-active order needs volumes); every other quote update
  // refreshes the values in place. Re-ranking each second would reshuffle the list under the
  // pointer while the user reads it.
  // React's "adjust state while rendering" pattern: re-rank when the key changes, else keep.
  // `open` is in the key so each opening re-reads the recents.
  const rankKey = [open, query, [...filters].sort().join(","), board.size > 0].join("|");
  const [rank, setRank] = useState<{ key: string; rows: TickerRow[] }>({ key: "", rows: [] });
  let ranked = rank.rows;
  if (rank.key !== rankKey) {
    ranked = searchTickers(tickers, query, filters, loadRecents());
    setRank({ key: rankKey, rows: ranked });
  }
  const grouped = !query.trim() && activeTickerFilters(tickers, filters).size === 0;
  const rows = useMemo(() => refreshTickerRows(ranked, tickers, grouped), [ranked, tickers, grouped]);
  const shown = rows.length > limit ? rows.slice(0, limit) : rows;
  const found = activeSym ? rows.findIndex((r) => r.item.symbol === activeSym) : 0;
  const active = Math.max(0, found);

  useEffect(() => {
    listRef.current?.querySelector(`[data-index="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [active]);

  // More rows when the end of the rendered ones scrolls into view.
  const observer = useRef<IntersectionObserver | null>(null);
  const sentinel = useCallback((el: HTMLLIElement | null) => {
    observer.current?.disconnect();
    if (!el) return;
    observer.current = new IntersectionObserver((entries) => entries.some((e) => e.isIntersecting) && setLimit((l) => l + PAGE), { root: listRef.current, rootMargin: "240px" });
    observer.current.observe(el);
  }, []);

  const choose = useCallback(
    (row: TickerRow | undefined) => {
      if (!row || !row.item.available) return;
      saveRecent(row.item.symbol);
      onSelect(row.item.symbol);
      onOpenChange(false);
    },
    [onSelect, onOpenChange],
  );

  const move = (to: number) => {
    const i = Math.min(rows.length - 1, Math.max(0, to));
    if (i < 0) return;
    if (i >= limit) setLimit((l) => Math.max(l, i + 1 + PAGE / 4));
    setActiveSym(rows[i]!.item.symbol);
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "ArrowDown" || (e.ctrlKey && e.key === "n")) {
      e.preventDefault();
      move(active + 1);
    } else if (e.key === "ArrowUp" || (e.ctrlKey && e.key === "p")) {
      e.preventDefault();
      move(active - 1);
    } else if (e.key === "Enter") {
      e.preventDefault();
      choose(rows[active]);
    }
  };

  const activeId = rows[active] ? `ticker-opt-${rows[active]!.item.symbol}` : undefined;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent initialFocus={inputRef} className={cn("left-1/2 -translate-x-1/2 overflow-hidden p-0", anchored ? "top-2.5 w-(--search-w)" : "top-16 w-[min(560px,calc(100vw-32px))]")}>
        <DialogTitle>Go to ticker</DialogTitle>
        <div className="flex items-center gap-2 px-3 pt-3 pb-2">
          <Icon name="search" className="size-4 text-muted-foreground" />
          <input
            ref={inputRef}
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setActiveSym(null);
              setLimit(PAGE);
            }}
            onKeyDown={onKeyDown}
            placeholder="Search tickers by symbol or name"
            role="combobox"
            aria-expanded="true"
            aria-controls="ticker-search-list"
            aria-activedescendant={activeId}
            aria-autocomplete="list"
            spellCheck={false}
            autoComplete="off"
            className="h-9 w-full bg-transparent text-sm text-foreground outline-none placeholder:text-text-faint"
          />
          <kbd className="shrink-0 rounded bg-bg-row px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground">Esc</kbd>
        </div>
        <div role="group" aria-label="Filters" className="flex flex-wrap items-center gap-y-1 px-3 pb-2">
          {groups.map((g, gi) => (
            <div key={g.id} className="flex items-center gap-0.5">
              {gi > 0 && <span aria-hidden="true" className="mx-1 h-4 w-px rounded-full bg-bg-raised" />}
              {g.chips.map((c) => {
                const on = filters.has(c.id);
                return (
                  <button
                    key={c.id}
                    type="button"
                    aria-pressed={on}
                    // Clicking a chip keeps the caret in the field, so typing and ↑/↓ carry on.
                    onMouseDown={(e) => e.preventDefault()}
                    onClick={() => toggleFilter(c.id)}
                    className={cn(
                      "h-6 rounded-md px-1.5 text-[11px] outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring",
                      on ? "bg-bg-raised text-foreground" : "bg-bg-row/60 text-muted-foreground hover:text-foreground",
                    )}
                  >
                    {c.label}
                  </button>
                );
              })}
            </div>
          ))}
        </div>
        <ScrollArea viewportRef={listRef} viewportClassName="max-h-[min(420px,55vh)]">
        <ul id="ticker-search-list" role="listbox" aria-label="Tickers" className="m-0 list-none p-1.5 pt-0 pr-2.5">
          {rows.length === 0 && <li className="px-3 py-6 text-center text-xs text-text-faint">{query ? `No ticker matches “${query}”` : "No ticker matches these filters"}.</li>}
          {shown.map((r, i) => (
            <Row key={r.item.symbol} row={r} index={i} active={i === active} current={r.item.symbol === current} onHover={setActiveSym} onChoose={choose} />
          ))}
          {shown.length < rows.length && (
            <li ref={sentinel} role="presentation" className="px-3 py-2 text-center text-[10px] text-text-faint">
              Scroll for more, or keep typing to narrow
            </li>
          )}
        </ul>
        </ScrollArea>
        <div className="flex items-center gap-3 bg-bg-row/50 px-3 py-1.5 text-[10px] text-text-faint">
          <span>
            <kbd className="font-mono">↑↓</kbd> move
          </span>
          <span>
            <kbd className="font-mono">Enter</kbd> open
          </span>
          <span>
            <kbd className="font-mono">Esc</kbd> close
          </span>
          <span className="ml-auto tabular-nums">
            {rows.length} {rows.length === 1 ? "ticker" : "tickers"}
            {filters.size > 0 && (
              <button
                type="button"
                // Clear unmounts itself: focus the field rather than lose focus to the page.
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => {
                  setFilters(new Set());
                  setLimit(PAGE);
                  inputRef.current?.focus();
                }} className="ml-2 rounded text-muted-foreground underline-offset-2 outline-none hover:text-foreground hover:underline focus-visible:ring-2 focus-visible:ring-ring">
                Clear
              </button>
            )}
          </span>
        </div>
      </DialogContent>
    </Dialog>
  );
}
