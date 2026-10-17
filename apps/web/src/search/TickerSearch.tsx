import { formatPrice } from "@stockwatcher/chart-core";
import { cn, Dialog, DialogContent, DialogTitle, ScrollArea } from "@stockwatcher/chart-web/ui";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  pushRecentTicker,
  searchTickers,
  TICKER_FILTER_GROUPS,
  TICKER_SECTION_TITLES,
  type TickerFilterId,
  type TickerInfo,
} from "@stockwatcher/shared";

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

export interface TickerSearchProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  tickers: readonly TickerInfo[];
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
 * recently opened tickers come first. Rows the simulator has no data for
 * (the mock universe) are shown, labelled, and cannot be opened.
 */
export function TickerSearch({ open, onOpenChange, tickers, current, onSelect, anchored = false }: TickerSearchProps) {
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  // Filters persist between openings (unlike the query): they are a way of working, not a search.
  const [filters, setFilters] = useState<ReadonlySet<TickerFilterId>>(new Set());
  const toggleFilter = (id: TickerFilterId) => {
    setFilters((f) => {
      const next = new Set(f);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
    setActive(0);
  };
  const listRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  // Every opening starts clean, like VS Code.
  useEffect(() => {
    if (!open) return;
    setQuery("");
    setActive(0);
  }, [open]);

  // `open` re-reads recents each time the panel opens.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const rows = useMemo(() => searchTickers(tickers, query, filters, loadRecents()), [tickers, query, open, filters]);

  useEffect(() => setActive((a) => Math.min(a, Math.max(0, rows.length - 1))), [rows.length]);
  useEffect(() => {
    listRef.current?.querySelector(`[data-index="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [active]);

  const choose = (row: (typeof rows)[number] | undefined) => {
    if (!row || !row.item.available) return;
    saveRecent(row.item.symbol);
    onSelect(row.item.symbol);
    onOpenChange(false);
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "ArrowDown" || (e.ctrlKey && e.key === "n")) {
      e.preventDefault();
      setActive((a) => Math.min(rows.length - 1, a + 1));
    } else if (e.key === "ArrowUp" || (e.ctrlKey && e.key === "p")) {
      e.preventDefault();
      setActive((a) => Math.max(0, a - 1));
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
          <svg viewBox="0 0 16 16" className="size-4 shrink-0 text-muted-foreground" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" aria-hidden="true">
            <circle cx="7" cy="7" r="4.5" />
            <path d="M10.5 10.5 14 14" />
          </svg>
          <input
            ref={inputRef}
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setActive(0);
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
          {TICKER_FILTER_GROUPS.map((g, gi) => (
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
          {rows.map((r, i) => {
            const t = r.item;
            return (
              <li key={t.symbol} role="presentation">
                {r.section && <div className="px-2.5 pt-2 pb-1 text-[10px] font-medium tracking-wide text-text-faint uppercase">{TICKER_SECTION_TITLES[r.section]}</div>}
                <div
                  id={`ticker-opt-${t.symbol}`}
                  role="option"
                  aria-selected={i === active}
                  aria-disabled={!t.available}
                  data-index={i}
                  onMouseMove={() => setActive(i)}
                  onClick={() => choose(r)}
                  className={cn(
                    "grid cursor-pointer grid-cols-[64px_minmax(0,1fr)_auto] items-center gap-3 rounded-lg px-2.5 py-1.5",
                    i === active && "bg-bg-raised",
                    !t.available && "cursor-default opacity-55",
                  )}
                >
                  <Highlight text={t.symbol} indices={r.symbolIdx} className="font-mono text-[13px] font-medium text-foreground" />
                  <span className="flex min-w-0 items-baseline gap-2">
                    <Highlight text={t.name} indices={r.nameIdx} className="truncate text-[13px] text-muted-foreground" />
                    <span className="shrink-0 text-[10px] text-text-faint">{t.exchange}</span>
                  </span>
                  <span className="flex items-center gap-2 font-mono text-[11px] tabular-nums">
                    {t.symbol === current && <span className="rounded bg-primary/15 px-1.5 py-0.5 font-sans text-[10px] font-medium text-primary">Open</span>}
                    {t.available && t.symbol !== current && <span className="font-sans text-[10px] text-good">In simulator</span>}
                    {t.mock && t.price !== undefined && (
                      <>
                        <span className="text-foreground/80">{formatPrice(t.price)}</span>
                        <span className={(t.changePct ?? 0) >= 0 ? "text-good" : "text-critical"}>
                          {(t.changePct ?? 0) >= 0 ? "+" : "−"}
                          {Math.abs(t.changePct ?? 0).toFixed(1)}%
                        </span>
                        <span className="rounded bg-warning-bg px-1 font-sans text-[9px] font-medium text-warning">mock</span>
                      </>
                    )}
                  </span>
                </div>
              </li>
            );
          })}
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
