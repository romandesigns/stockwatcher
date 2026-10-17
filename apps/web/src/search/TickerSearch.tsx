import { formatPrice } from "@stockwatcher/chart-core";
import { cn, Dialog, DialogContent, DialogTitle } from "@stockwatcher/chart-web/ui";
import { useEffect, useMemo, useRef, useState } from "react";
import { rankTickers, type Ranked } from "./fuzzy.ts";
import type { TickerInfo } from "./mockTickers.ts";

const RECENTS_KEY = "sw-search:recent";
const MAX_RECENTS = 5;

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
    localStorage.setItem(RECENTS_KEY, JSON.stringify([symbol, ...loadRecents().filter((s) => s !== symbol)].slice(0, MAX_RECENTS)));
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
}

/**
 * Quick-open for tickers, modelled on VS Code's Ctrl+P: a panel near the
 * top of the screen with one input and a list under it. Letters match in
 * order (fuzzy), tickers rank above names, matched letters are
 * highlighted; ↑/↓ move, Enter opens, Esc closes. With an empty query the
 * recently opened tickers come first. Rows the simulator has no data for
 * (the mock universe) are shown, labelled, and cannot be opened.
 */
export function TickerSearch({ open, onOpenChange, tickers, current, onSelect }: TickerSearchProps) {
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const listRef = useRef<HTMLUListElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  // Every opening starts clean, like VS Code.
  useEffect(() => {
    if (!open) return;
    setQuery("");
    setActive(0);
  }, [open]);

  const rows: (Ranked<TickerInfo> & { recent?: boolean })[] = useMemo(() => {
    if (query.trim()) return rankTickers(tickers, query);
    const recents = loadRecents();
    const bySym = new Map(tickers.map((t) => [t.symbol, t]));
    const recent = recents.map((s) => bySym.get(s)).filter((t): t is TickerInfo => !!t);
    const rest = tickers.filter((t) => !recents.includes(t.symbol));
    return [...recent.map((item) => ({ item, score: 0, symbolIdx: [], nameIdx: [], recent: true })), ...rest.map((item) => ({ item, score: 0, symbolIdx: [], nameIdx: [] }))];
    // `open` re-reads recents each time the panel opens.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tickers, query, open]);

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
  const firstMockIndex = rows.findIndex((r) => r.item.mock);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent initialFocus={inputRef} className="top-16 left-1/2 w-[min(560px,calc(100vw-32px))] -translate-x-1/2 overflow-hidden p-0">
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
        <ul ref={listRef} id="ticker-search-list" role="listbox" aria-label="Tickers" className="m-0 max-h-[min(420px,60vh)] list-none overflow-y-auto p-1.5 pt-0">
          {rows.length === 0 && <li className="px-3 py-6 text-center text-xs text-text-faint">No ticker matches “{query}”.</li>}
          {rows.map((r, i) => {
            const t = r.item;
            const showHeader = (i === 0 && r.recent) || (i === firstMockIndex && firstMockIndex > 0);
            return (
              <li key={t.symbol} role="presentation">
                {showHeader && <div className="px-2.5 pt-2 pb-1 text-[10px] font-medium tracking-wide text-text-faint uppercase">{r.recent ? "Recently opened" : "Mock universe · not in the simulator"}</div>}
                {i > 0 && rows[i - 1]?.recent && !r.recent && !r.item.mock && <div className="px-2.5 pt-2 pb-1 text-[10px] font-medium tracking-wide text-text-faint uppercase">In the simulator</div>}
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
          <span className="ml-auto">Mock list until real symbols are imported</span>
        </div>
      </DialogContent>
    </Dialog>
  );
}
