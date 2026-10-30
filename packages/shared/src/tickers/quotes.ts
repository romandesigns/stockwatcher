// The client side of the server's quote board (protocol.ts `quotes.*`):
// one row per price-band candidate with last price, change % and day volume,
// kept current by the frames the server pushes. Pure apart from the channel,
// so web and phone fold frames identically.
//
// Change % is against the previous trading day's official close when the
// server knows it (`pc`, what a broker calls "today's change"), else against
// the open (`o`). The chart header uses the same rule (changeReference in
// chart-core), and the Gainers/Losers chips and their sort read `changePct`,
// so the list, the filters and the header never disagree.

import type { QuoteRowWire, QuotesMessage } from "../protocol.ts";

export interface TickerQuote {
  price: number;
  /** The previous trading day's official close; absent when the server has none. */
  prevClose?: number;
  /** The fallback reference: the regular open, or the day's first price while `preOpen`. */
  open: number;
  /** Percent, e.g. 4.2 for +4.2 %, against `prevClose` if known, else `open`; 0 when the reference is 0. */
  changePct: number;
  volume: number;
  trades: number;
  /** How `prevClose` was arrived at: `split` (split-adjusted) or `guard` (withheld: the day trades on another share basis), absent when plain. */
  closeBasis?: "split" | "guard";
  /** No print at/after 09:30 yet. Describes `open`, so it only matters while there is no `prevClose`. */
  preOpen: boolean;
  /** From the server's asset metadata; absent when it has none (show the symbol). */
  name?: string;
  exchange?: string;
}

export type QuoteBoard = ReadonlyMap<string, TickerQuote>;

export const EMPTY_QUOTE_BOARD: QuoteBoard = new Map();

/** Percent change of `price` against the previous close when known, else against the open; 0 for a 0 reference. */
export function quoteChangePct(price: number, prevClose: number | undefined, open: number): number {
  const ref = prevClose !== undefined && prevClose > 0 ? prevClose : open;
  return ref ? ((price - ref) / ref) * 100 : 0;
}

function fromWire(r: QuoteRowWire, prev: TickerQuote | undefined): TickerQuote {
  // Updates leave `pc` out unless it changed: keep the one the row already has, unless the
  // server now withholds it (`pcb: "guard"`, a split it could not confirm).
  const prevClose = r.pcb === "guard" ? undefined : (r.pc ?? prev?.prevClose);
  return {
    price: r.p,
    prevClose,
    open: r.o,
    changePct: quoteChangePct(r.p, prevClose, r.o),
    volume: r.v,
    trades: r.n,
    preOpen: r.pre === true,
    closeBasis: r.pcb,
    // Updates leave names out (they never change); keep the snapshot's.
    name: r.name ?? prev?.name,
    exchange: r.x ?? prev?.exchange,
  };
}

/**
 * The board after one frame. A snapshot replaces the board (it follows a
 * subscribe, a seek or a lag, and is the whole truth); an update upserts its
 * rows and drops `gone`. Returns a new Map so React sees the change.
 */
export function applyQuotesMessage(board: QuoteBoard, m: QuotesMessage): Map<string, TickerQuote> {
  // A snapshot row carries everything it has (an absent `pc` means unknown), so nothing is kept from the old board.
  if (m.type === "quotes.snapshot") return new Map(m.quotes.map((r) => [r.s, fromWire(r, undefined)]));
  const next = new Map(board);
  for (const r of m.quotes) next.set(r.s, fromWire(r, board.get(r.s)));
  for (const s of m.gone ?? []) next.delete(s);
  return next;
}

/**
 * A feed's quote board, subscribed on demand: the first listener opts the
 * connection in (`subscribe`), the last one leaving opts it out, so the
 * server sends nothing while no search is open. The board is emptied when
 * the last listener leaves: values from minutes ago would be wrong, and a
 * search ranks its list once the board turns non-empty, so it must rank on
 * the fresh snapshot (a few ms after subscribing), not on stale volumes.
 */
export interface QuoteChannel {
  get(): QuoteBoard;
  /** `fn` runs after every frame. Returns the unsubscribe. */
  subscribe(fn: () => void): () => void;
}

export function createQuoteChannel(io: { subscribe(): void; unsubscribe(): void }): QuoteChannel & { apply(m: QuotesMessage): void } {
  let board: QuoteBoard = EMPTY_QUOTE_BOARD;
  const listeners = new Set<() => void>();
  return {
    get: () => board,
    subscribe(fn) {
      listeners.add(fn);
      if (listeners.size === 1) io.subscribe();
      return () => {
        if (!listeners.delete(fn)) return;
        if (listeners.size > 0) return;
        io.unsubscribe();
        board = EMPTY_QUOTE_BOARD;
      };
    },
    apply(m) {
      board = applyQuotesMessage(board, m);
      for (const l of listeners) l();
    },
  };
}
