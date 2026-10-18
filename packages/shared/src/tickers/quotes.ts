// The client side of the server's quote board (protocol.ts `quotes.*`):
// one row per price-band candidate with last price, change since the open
// and day volume, kept current by the frames the server pushes. Pure apart
// from the channel, so web and phone fold frames identically.

import type { QuoteRowWire, QuotesMessage } from "../protocol.ts";

export interface TickerQuote {
  price: number;
  /** The change reference: the regular open, or the day's first price while `preOpen`. */
  open: number;
  /** Percent, e.g. 4.2 for +4.2 %; 0 when the reference is 0. */
  changePct: number;
  volume: number;
  trades: number;
  /** No print at/after 09:30 yet: `changePct` is against the premarket's first price. */
  preOpen: boolean;
  /** From the server's asset metadata; absent when it has none (show the symbol). */
  name?: string;
  exchange?: string;
}

export type QuoteBoard = ReadonlyMap<string, TickerQuote>;

export const EMPTY_QUOTE_BOARD: QuoteBoard = new Map();

function fromWire(r: QuoteRowWire, prev: TickerQuote | undefined): TickerQuote {
  return {
    price: r.p,
    open: r.o,
    changePct: r.o ? ((r.p - r.o) / r.o) * 100 : 0,
    volume: r.v,
    trades: r.n,
    preOpen: r.pre === true,
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
  if (m.type === "quotes.snapshot") return new Map(m.quotes.map((r) => [r.s, fromWire(r, board.get(r.s))]));
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
