// The client half of settings sync (docs/architecture/settings-sync.md; the server half is
// crates/server/src/settings.rs). Framework-free, so the web app, the desktop shell (the web
// app in Tauri) and the phone share one implementation; each app adds a ~10-line React hook.
//
// Model. Per namespace the client keeps what the SERVER last confirmed ({rev, value}) and a
// queue of its own PENDING edits (sent but not yet echoed, or not yet sent). What the UI shows is
// the confirmed value with the pending edits folded on top. That one rule gives every property
// the owner asked for:
// - optimistic: an edit shows at once, before any round trip;
// - echo suppression: the server echoes our own write (same opId) and we just drop it from the
//   queue; the shown value does not change, so nothing re-renders or flickers;
// - no lost latest action: a remote change that arrives while our own edit is still in flight
//   updates the confirmed value underneath, but our edit stays on top; the server applies ours
//   after it (it arrives later), so when our echo comes back both sides agree;
// - no ping-pong: applying a remote change never writes anything back;
// - offline: edits made with no connection queue up (coalesced, so at most one per namespace,
//   apart from a migration seed) and are persisted with the cache, so they survive a restart
//   and are sent after the next `settings.snapshot` (last writer wins, like any other write);
// - offline cache: the confirmed values are persisted too, so the app starts with the last
//   known settings before the socket connects and keeps working with no server (no transport
//   attached: everything stays local until one is).
//
// Migration. The first time a client runs this code it has settings in the old per-app keys
// (`sw-chart:*` on the web, `chart.*` preferences on the phone). The app passes them to `seed`,
// which queues them as `ifAbsent` sets: if the server has never had that namespace the first
// client to connect seeds it (so the owner's current desktop/web setup becomes the shared one);
// if it has one, the server's value wins and replaces the local one. Same rule for a server
// whose data was wiped: a client re-seeds what it last knew.
//
// Sending. The first change in a namespace goes out at once (a toggle reaches other devices
// "instantly"); further changes within `throttleMs` (default 200 ms) are coalesced into one
// write sent at the end of the window, so a drag or a slider makes at most ~5 writes a second
// and the final value is always flushed. Nothing is sent until the connection's snapshot has
// arrived, since only then is it known which seeds still apply.
//
// Never trust remote JSON: `view` runs the app's own parse/validate/migrate function over every
// value, local or remote, so junk from any device falls back to defaults field by field.

import type { SettingsClientMessage, SettingsServerMessage } from "./protocol.ts";

/** One queued edit. `set` replaces; `patch` sets/removes entries of an object value. */
export type SettingsOp =
  | { opId: string; kind: "set"; value: unknown; ifAbsent?: boolean }
  | { opId: string; kind: "patch"; set: Record<string, unknown>; remove: string[] };

/** Where the shown value last changed: this device, or another one / the server. */
export type SettingsOrigin = "local" | "remote";

export interface SettingsSyncOptions {
  /** The persisted cache from a previous run (the string `save` last wrote), or null. */
  initial: string | null;
  /** Persists the cache (localStorage / AsyncStorage). Called after every change. */
  save?: (raw: string) => void;
  /** Coalescing window for repeated changes of one namespace (ms). */
  throttleMs?: number;
  /** A write the server refused (or that was too large to send): logged by the app. */
  onReject?: (ns: string, code: string, message: string) => void;
  now?: () => number;
  setTimer?: (fn: () => void, ms: number) => unknown;
  clearTimer?: (handle: unknown) => void;
  /** Unique per edit; the default is random enough across devices. */
  newOpId?: () => string;
}

export interface SettingsView<T> {
  subscribe(onChange: () => void): () => void;
  /** The parsed value; the same object until the namespace's value changes (useSyncExternalStore). */
  get(): T;
  origin(): SettingsOrigin;
}

export interface SettingsSync {
  /** The shown (confirmed + pending) value; undefined when nothing was ever stored. */
  get(ns: string): unknown;
  origin(ns: string): SettingsOrigin;
  /** Every namespace with a value (e.g. to find which symbols have drawings). */
  namespaces(): string[];
  /** Replaces a namespace's value (optimistic; sent at once or at the end of the window). */
  set(ns: string, value: unknown): void;
  /** Sets/removes entries of an object-valued namespace; no-op when both are empty. */
  patch(ns: string, set: Record<string, unknown>, remove?: readonly string[]): void;
  /** Migration: queues `value` as an `ifAbsent` seed, only if this namespace is unknown here. */
  seed(ns: string, value: unknown): void;
  /** Calls `fn(ns)` whenever a namespace's shown value changes. */
  subscribe(fn: (ns: string) => void): () => void;
  /** A parsed, memoized view of one namespace (`parse` gets undefined when nothing is stored). */
  view<T>(ns: string, parse: (raw: unknown) => T): SettingsView<T>;
  /** The connection is up (after `welcome`): writes go through `send` once the snapshot arrives. */
  attach(send: (msg: SettingsClientMessage) => boolean): void;
  /** The connection dropped: in-flight edits are kept and resent after the next snapshot. */
  detach(): void;
  /** A settings.* frame from the server. */
  handle(msg: SettingsServerMessage): void;
  /** Sends every waiting edit now (e.g. on page hide), if connected. */
  flush(): void;
  /** Edits not yet confirmed by the server (tests, a "saving…" hint). */
  pendingCount(): number;
  /** Stops timers; nothing more is sent. */
  close(): void;
}

/** The largest frame sent; the server's WebSocket refuses messages over 64 KiB (ws.rs). */
export const MAX_SETTINGS_FRAME = 60_000;

interface Pending {
  op: SettingsOp;
  sent: boolean;
}

interface NsState {
  server: { rev: number; value: unknown } | null;
  pending: Pending[];
  shown: unknown;
  shownJson: string;
  origin: SettingsOrigin;
  timer: unknown;
  lastSentAt: number;
}

interface Persisted {
  v: 1;
  docs: Record<string, { rev: number; value: unknown }>;
  pending: Record<string, SettingsOp[]>;
}

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

/** `value` with a patch applied (a non-object value counts as `{}`, as on the server). */
export function applyPatch(value: unknown, set: Readonly<Record<string, unknown>>, remove: readonly string[]): Record<string, unknown> {
  const out: Record<string, unknown> = isObject(value) ? { ...value } : {};
  for (const k of remove) delete out[k];
  for (const [k, v] of Object.entries(set)) out[k] = v;
  return out;
}

/** The value the UI shows: the confirmed one with the pending edits folded on top. */
function fold(server: NsState["server"], pending: readonly Pending[]): unknown {
  let v: unknown = server ? server.value : undefined;
  let exists = server !== null;
  for (const { op } of pending) {
    if (op.kind === "set") {
      // A seed only applies where the server has nothing (the server's own rule).
      if (op.ifAbsent && exists) continue;
      v = op.value;
    } else v = applyPatch(v, op.set, op.remove);
    exists = true;
  }
  return v;
}

/**
 * Canonical JSON (object keys sorted), for "did the shown value change?". The server's JSON keeps
 * keys sorted while a local edit has them in insertion order; without this our own echo would
 * look like a change and re-render every view of the namespace.
 */
export function canonicalJson(v: unknown): string {
  if (v === undefined) return "";
  return JSON.stringify(v, (_k, x: unknown) => {
    if (!isObject(x)) return x;
    const out: Record<string, unknown> = {};
    for (const k of Object.keys(x).sort()) out[k] = x[k];
    return out;
  });
}
const json = canonicalJson;

function parsePersisted(raw: string | null): Persisted | null {
  if (!raw) return null;
  try {
    const p = JSON.parse(raw) as Partial<Persisted>;
    if (p?.v !== 1 || !isObject(p.docs) || !isObject(p.pending)) return null;
    return p as Persisted;
  } catch {
    return null;
  }
}

function validOp(op: unknown): op is SettingsOp {
  if (!isObject(op) || typeof op.opId !== "string") return false;
  if (op.kind === "set") return "value" in op;
  return op.kind === "patch" && isObject(op.set) && Array.isArray(op.remove) && op.remove.every((k) => typeof k === "string");
}

let opSeq = 0;
const randomTag = () => Math.random().toString(36).slice(2, 10);

export function createSettingsSync(opts: SettingsSyncOptions): SettingsSync {
  const throttleMs = opts.throttleMs ?? 200;
  const now = opts.now ?? (() => Date.now());
  const setTimer = opts.setTimer ?? ((fn, ms) => setTimeout(fn, ms));
  const clearTimer = opts.clearTimer ?? ((h) => clearTimeout(h as ReturnType<typeof setTimeout>));
  const tag = randomTag();
  const newOpId = opts.newOpId ?? (() => `${tag}-${now().toString(36)}-${(opSeq++).toString(36)}`);

  const states = new Map<string, NsState>();
  const listeners = new Set<(ns: string) => void>();
  let send: ((msg: SettingsClientMessage) => boolean) | null = null;
  /** The current connection's snapshot has arrived: writes may go out. */
  let ready = false;
  let closed = false;

  const state = (ns: string): NsState => {
    let st = states.get(ns);
    if (!st) {
      st = { server: null, pending: [], shown: undefined, shownJson: "", origin: "local", timer: null, lastSentAt: -Infinity };
      states.set(ns, st);
    }
    return st;
  };

  // Restore the cache: confirmed values and unconfirmed edits (all unsent: the connection that
  // carried them is gone, so they go out again after the next snapshot).
  const restored = parsePersisted(opts.initial);
  if (restored) {
    for (const [ns, d] of Object.entries(restored.docs)) {
      if (isObject(d) && typeof d.rev === "number") state(ns).server = { rev: d.rev, value: d.value };
    }
    for (const [ns, ops] of Object.entries(restored.pending)) {
      if (Array.isArray(ops)) state(ns).pending = ops.filter(validOp).map((op) => ({ op, sent: false }));
    }
    for (const st of states.values()) {
      st.shown = fold(st.server, st.pending);
      st.shownJson = json(st.shown);
    }
  }

  const persist = () => {
    if (!opts.save) return;
    const out: Persisted = { v: 1, docs: {}, pending: {} };
    for (const [ns, st] of states) {
      if (st.server) out.docs[ns] = st.server;
      if (st.pending.length) out.pending[ns] = st.pending.map((p) => p.op);
    }
    try {
      opts.save(JSON.stringify(out));
    } catch {
      // Private mode / quota: the settings still sync, they just are not cached on this device.
    }
  };

  /** Recomputes what the UI shows; tells listeners when it changed. */
  const refresh = (ns: string, origin: SettingsOrigin) => {
    const st = state(ns);
    const next = fold(st.server, st.pending);
    const nextJson = json(next);
    if (nextJson === st.shownJson) return;
    st.shown = next;
    st.shownJson = nextJson;
    st.origin = origin;
    for (const l of listeners) l(ns);
  };

  const toMessage = (ns: string, op: SettingsOp): SettingsClientMessage =>
    op.kind === "set"
      ? { type: "settings.set", ns, value: op.value, opId: op.opId, ...(op.ifAbsent ? { ifAbsent: true as const } : {}) }
      : { type: "settings.patch", ns, opId: op.opId, ...(Object.keys(op.set).length ? { set: op.set } : {}), ...(op.remove.length ? { remove: op.remove } : {}) };

  /** Sends every unsent edit of `ns`, in order. */
  const sendNs = (ns: string) => {
    const st = state(ns);
    if (st.timer !== null) {
      clearTimer(st.timer);
      st.timer = null;
    }
    if (!send || !ready || closed) return;
    let dropped = false;
    // A copy: a too-large edit is spliced out while iterating.
    for (const p of st.pending.slice()) {
      if (p.sent) continue;
      const msg = toMessage(ns, p.op);
      const text = JSON.stringify(msg);
      if (text.length > MAX_SETTINGS_FRAME) {
        // The server would drop the connection over it; refuse it here, like the server would.
        st.pending.splice(st.pending.indexOf(p), 1);
        dropped = true;
        opts.onReject?.(ns, "too_large", "settings value too large to send");
        continue;
      }
      if (!send(msg)) return;
      p.sent = true;
      st.lastSentAt = now();
    }
    if (dropped) {
      refresh(ns, "local");
      persist();
    }
  };

  /** A local edit was queued: send now, or at the end of the namespace's window. */
  const schedule = (ns: string) => {
    if (!send || !ready || closed) return;
    const st = state(ns);
    if (st.timer !== null) return;
    const wait = st.lastSentAt + throttleMs - now();
    if (wait <= 0) sendNs(ns);
    else st.timer = setTimer(() => {
      st.timer = null;
      sendNs(ns);
    }, wait);
  };

  /** Queues an edit, coalescing it into the last one if that has not been sent yet. */
  const enqueue = (ns: string, op: SettingsOp) => {
    const st = state(ns);
    const last = st.pending[st.pending.length - 1];
    // A seed is never merged into: it may be dropped (the server has a value), and the edit
    // made on top of it must survive that. A sent edit is the server's business already.
    if (last && !last.sent && !(last.op.kind === "set" && last.op.ifAbsent)) {
      if (op.kind === "set") last.op = op;
      else if (last.op.kind === "set") last.op = { ...last.op, opId: op.opId, value: applyPatch(last.op.value, op.set, op.remove) };
      else {
        const set = { ...last.op.set };
        const remove = new Set(last.op.remove);
        for (const k of op.remove) {
          delete set[k];
          remove.add(k);
        }
        for (const [k, v] of Object.entries(op.set)) {
          set[k] = v;
          remove.delete(k);
        }
        last.op = { opId: op.opId, kind: "patch", set, remove: [...remove] };
      }
    } else st.pending.push({ op, sent: false });
    refresh(ns, "local");
    persist();
    schedule(ns);
  };

  const onSnapshot = (docs: readonly { ns: string; rev: number; value: unknown }[]) => {
    ready = true;
    const listed = new Set<string>();
    for (const d of docs) {
      if (typeof d?.ns !== "string" || typeof d.rev !== "number") continue;
      listed.add(d.ns);
      state(d.ns).server = { rev: d.rev, value: d.value };
    }
    for (const [ns, st] of states) {
      // Known here but not on the server (its data was wiped, or this is a namespace it never
      // got): re-seed what this device last knew, under any edits made since.
      if (!listed.has(ns) && st.server) {
        st.pending.unshift({ op: { opId: newOpId(), kind: "set", value: st.server.value, ifAbsent: true }, sent: false });
        st.server = null;
      }
      // Whatever was in flight on the old connection may or may not have arrived: send it again.
      // Every op is idempotent (a set or patch re-applied gives the same value; a seed finds the
      // value it seeded), so a duplicate costs one rev, never data.
      for (const p of st.pending) p.sent = false;
      // A seed for a namespace the server has is moot: the server's value wins.
      if (st.server) st.pending = st.pending.filter((p) => !(p.op.kind === "set" && p.op.ifAbsent));
    }
    for (const ns of states.keys()) refresh(ns, "remote");
    persist();
    for (const ns of states.keys()) sendNs(ns);
  };

  const onChanged = (ns: string, rev: number, value: unknown, opId: string | undefined) => {
    const st = state(ns);
    const i = opId === undefined ? -1 : st.pending.findIndex((p) => p.op.opId === opId);
    if (i >= 0) st.pending.splice(i, 1);
    // `>=`: an ifAbsent seed that found a value is answered with the current rev, unchanged.
    if (!st.server || rev > st.server.rev || (i >= 0 && rev === st.server.rev)) st.server = { rev, value };
    refresh(ns, i >= 0 ? "local" : "remote");
    persist();
  };

  const onRejected = (ns: string, opId: string, code: string, message: string) => {
    const st = state(ns);
    const p = st.pending.find((x) => x.op.opId === opId);
    if (p) {
      // The disk write failed: keep the edit and try again after the next reconnect. Anything
      // else will never be accepted: drop it, and the UI falls back to the server's value.
      if (code === "settings_unavailable") p.sent = false;
      else st.pending.splice(st.pending.indexOf(p), 1);
    }
    opts.onReject?.(ns, code, message);
    refresh(ns, "local");
    persist();
  };

  // Parsed values per namespace AND parse function, so two views of one namespace with different
  // parsers never evict each other (useSyncExternalStore needs a stable object per value).
  const views = new Map<string, Map<(raw: unknown) => unknown, { json: string; value: unknown }>>();

  return {
    get: (ns) => states.get(ns)?.shown,
    origin: (ns) => states.get(ns)?.origin ?? "local",
    namespaces: () => [...states].filter(([, st]) => st.shown !== undefined).map(([ns]) => ns),
    set: (ns, value) => enqueue(ns, { opId: newOpId(), kind: "set", value }),
    patch(ns, set, remove = []) {
      if (!Object.keys(set).length && !remove.length) return;
      enqueue(ns, { opId: newOpId(), kind: "patch", set: { ...set }, remove: [...remove] });
    },
    seed(ns, value) {
      const st = states.get(ns);
      if (value === undefined || (st && (st.server || st.pending.length))) return;
      enqueue(ns, { opId: newOpId(), kind: "set", value, ifAbsent: true });
    },
    subscribe(fn) {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
    view<T>(ns: string, parse: (raw: unknown) => T): SettingsView<T> {
      return {
        subscribe: (onChange) => {
          const fn = (changed: string) => changed === ns && onChange();
          listeners.add(fn);
          return () => listeners.delete(fn);
        },
        get: () => {
          const st = states.get(ns);
          const j = st?.shownJson ?? "";
          let byParse = views.get(ns);
          if (!byParse) views.set(ns, (byParse = new Map()));
          const cached = byParse.get(parse);
          if (cached && cached.json === j) return cached.value as T;
          const value = parse(st?.shown);
          byParse.set(parse, { json: j, value });
          return value;
        },
        origin: () => states.get(ns)?.origin ?? "local",
      };
    },
    attach(fn) {
      send = fn;
      ready = false;
    },
    detach() {
      send = null;
      ready = false;
      for (const st of states.values()) {
        if (st.timer !== null) clearTimer(st.timer);
        st.timer = null;
        for (const p of st.pending) p.sent = false;
      }
    },
    handle(msg) {
      if (closed) return;
      if (msg.type === "settings.snapshot") onSnapshot(Array.isArray(msg.docs) ? msg.docs : []);
      else if (msg.type === "settings.changed") {
        if (typeof msg.ns === "string" && typeof msg.rev === "number") onChanged(msg.ns, msg.rev, msg.value, typeof msg.opId === "string" ? msg.opId : undefined);
      } else if (msg.type === "settings.rejected") onRejected(String(msg.ns), String(msg.opId), String(msg.code), String(msg.message));
    },
    flush() {
      for (const ns of states.keys()) sendNs(ns);
    },
    pendingCount: () => [...states.values()].reduce((n, st) => n + st.pending.length, 0),
    close() {
      closed = true;
      for (const st of states.values()) if (st.timer !== null) clearTimer(st.timer);
    },
  };
}
