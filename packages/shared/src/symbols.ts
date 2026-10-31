// The price-band candidate list pushed by the server (`symbols.v1`; crates/server/src/symbols.rs),
// replacing the clients' 10 s `GET /api/symbols` polling. The server checks once a second and
// sends only what entered and left, so a stock that enters the band reaches the symbol picker
// within about a second instead of up to ten, and an idle client no longer downloads the whole
// list six times a minute. `/api/symbols` stays the first probe, and the fallback (the hooks keep
// polling) whenever the server does not advertise the feature.

/** `GET /api/symbols` and the state a `symbols.*` frame stream rebuilds: the same object. */
export interface SymbolsListWire {
  mode: string;
  /** Provider status: starting, running, reconnecting, no_data, not_configured, error, stopped, idle. */
  status: string;
  source: string | null;
  day: string | null;
  fixture: boolean;
  /** Candidates only, sorted: symbols whose latest price is inside the server's price band. */
  symbols: string[];
  priceBand?: { min: string; max: string };
  screened?: { total: number; candidates: number };
  /** Live: the trading day shown (read with `parseLiveSession`). */
  session?: unknown;
}

/** The server pushes the candidate list (`symbols.subscribe`); see the module comment. */
export const SYMBOLS_FEATURE = "symbols.v1";

type SymbolsMeta = Omit<SymbolsListWire, "symbols">;

export interface SymbolsSnapshotMessage extends SymbolsMeta {
  type: "symbols.snapshot";
  rev: number;
  symbols: string[];
}

/** Applies to the list at `rev - 1`; the meta comes whole. */
export interface SymbolsUpdateMessage extends SymbolsMeta {
  type: "symbols.update";
  rev: number;
  added: string[];
  removed: string[];
}

export type SymbolsMessage = SymbolsSnapshotMessage | SymbolsUpdateMessage;
export type SymbolsClientMessage = { type: "symbols.subscribe" } | { type: "symbols.unsubscribe" };

export interface SymbolsState {
  /**
   * Whether the connected server pushes the list: null until the first welcome, then the
   * welcome's answer. Anything but true = keep polling `/api/symbols`.
   */
  available: boolean | null;
  /** The list and its meta, null until the first snapshot on this connection. */
  latest: SymbolsListWire | null;
  rev: number;
}

export const EMPTY_SYMBOLS: SymbolsState = { available: null, latest: null, rev: 0 };

export interface SymbolsChannel {
  get(): SymbolsState;
  subscribe(fn: () => void): () => void;
}

const strings = (v: unknown): string[] => (Array.isArray(v) ? v.filter((s): s is string => typeof s === "string") : []);

function metaOf(m: SymbolsMessage): SymbolsMeta {
  const { type: _t, rev: _r, symbols: _s, added: _a, removed: _d, ...meta } = m as SymbolsMessage & { symbols?: unknown; added?: unknown; removed?: unknown };
  return meta;
}

/**
 * One frame folded into the state. A snapshot replaces the list; an update applies only on top of
 * `rev - 1` (sets `resync` otherwise: the caller subscribes again and the answer is a snapshot).
 */
export function applySymbolsMessage(state: SymbolsState, m: SymbolsMessage): { state: SymbolsState; resync: boolean } {
  if (m.type === "symbols.snapshot") {
    return { state: { ...state, latest: { ...metaOf(m), symbols: strings(m.symbols).sort() }, rev: m.rev }, resync: false };
  }
  if (!state.latest || m.rev !== state.rev + 1) return { state, resync: true };
  const removed = new Set(strings(m.removed));
  const list = state.latest.symbols.filter((s) => !removed.has(s));
  const have = new Set(list);
  for (const s of strings(m.added)) if (!have.has(s)) list.push(s);
  list.sort();
  return { state: { ...state, latest: { ...metaOf(m), symbols: list }, rev: m.rev }, resync: false };
}

/**
 * Subscribed on demand like the other channels: the first listener opts in, the last opts out.
 * `setAvailable` comes from every welcome (shared ws.ts sends `symbols.subscribe` only to a server
 * that advertised the feature); `setConnected(false)` drops the list, so a hook falls back to its
 * own copy until the reconnect's snapshot.
 */
export function createSymbolsChannel(io: { subscribe(): void; unsubscribe(): void }): SymbolsChannel & { apply(m: SymbolsMessage): void; setAvailable(on: boolean): void; setConnected(open: boolean): void } {
  let state: SymbolsState = EMPTY_SYMBOLS;
  const listeners = new Set<() => void>();
  const emit = () => {
    for (const l of listeners) l();
  };
  return {
    get: () => state,
    subscribe(fn) {
      listeners.add(fn);
      if (listeners.size === 1) io.subscribe();
      return () => {
        if (!listeners.delete(fn) || listeners.size > 0) return;
        io.unsubscribe();
        state = { ...EMPTY_SYMBOLS, available: state.available };
      };
    },
    apply(m) {
      const next = applySymbolsMessage(state, m);
      state = next.state;
      if (next.resync && listeners.size > 0) io.subscribe();
      emit();
    },
    setAvailable(on) {
      if (state.available === on) return;
      state = { ...state, available: on };
      emit();
    },
    setConnected(open) {
      if (open || !state.latest) return;
      state = { ...state, latest: null, rev: 0 };
      emit();
    },
  };
}
