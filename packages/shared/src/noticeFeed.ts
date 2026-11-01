// The Insights page's notification log, pushed by the server (`notices.v1`;
// crates/server/src/notices.rs, "Pushed"). The open log used to refetch the whole day from
// GET /api/notices every 15 s on every client; now today's log is one `notices.snapshot` and then a
// `notices.append` per ledger line, carrying that notice's whole item, upserted here by event_id.
// A past day stays a one-shot fetch (its file no longer changes), and against a server without the
// feature the hooks keep polling as before.

import { parseNoticeLog, parseNoticeSummary, type NoticeItemWire, type NoticeLogWire } from "./noticeLog.ts";

/** The server pushes today's notification log (`notices.subscribe`); see the module comment. */
export const NOTICES_FEATURE = "notices.v1";

/**
 * The server assembles the rows (crates/server/src/notices.rs `FEATURE_ROWS`): each item's
 * `status` / `reason` / `reason_text` and each body's day `summary`. The fields are additive and
 * the clients read them whenever present, so this only documents the contract.
 */
export const NOTICES_ROWS_FEATURE = "notices.rows.v1";

/** The rows a day holds at most, as the server caps them (notices.rs MAX_ITEMS). */
export const NOTICE_LOG_MAX_ITEMS = 2000;

export interface NoticesSnapshotMessage {
  type: "notices.snapshot";
  day: string;
  days: string[];
  items: unknown[];
  truncated: boolean;
  /** The day's summary (`notices.rows.v1`). */
  summary?: unknown;
}

/** One ledger line's notice, regrouped from all its lines so far: replaces the item with its event_id. */
export interface NoticesAppendMessage {
  type: "notices.append";
  day: string;
  items: unknown[];
  /** The day's summary after this line (`notices.rows.v1`). */
  summary?: unknown;
}

export type NoticesMessage = NoticesSnapshotMessage | NoticesAppendMessage;
export type NoticesClientMessage = { type: "notices.subscribe"; day?: string } | { type: "notices.unsubscribe" };

export interface NoticesState {
  /** Whether the connected server pushes the log: null until the first welcome. Not true = poll. */
  available: boolean | null;
  /** Today's log on this connection, null until its snapshot. */
  log: NoticeLogWire | null;
}

export const EMPTY_NOTICES: NoticesState = { available: null, log: null };

export interface NoticesChannel {
  get(): NoticesState;
  subscribe(fn: () => void): () => void;
}

/**
 * Upserts appended items into `log`: an item already there is replaced in place (its notice got
 * another line: the phone decision, the push latency), a new one goes in newest first by `at`.
 * An append for another day than the log's is ignored: the server follows the date with a new
 * snapshot, not appends.
 */
export function applyNoticesMessage(log: NoticeLogWire | null, m: NoticesMessage): NoticeLogWire | null {
  if (m.type === "notices.snapshot") return parseNoticeLog(m) ?? log;
  if (!log || m.day !== log.day) return log;
  const incoming = parseNoticeLog({ day: m.day, items: m.items })?.items ?? [];
  // The server's tally after this line covers the whole day: it replaces, never adds up here.
  const summary = parseNoticeSummary(m.summary) ?? log.summary;
  if (incoming.length === 0) return summary === log.summary ? log : { ...log, summary };
  const items = [...log.items];
  let truncated = log.truncated;
  for (const it of incoming) {
    const i = items.findIndex((x) => x.event_id === it.event_id);
    if (i >= 0) {
      items[i] = it;
      continue;
    }
    const at = it.at ?? 0;
    const j = items.findIndex((x: NoticeItemWire) => (x.at ?? 0) < at);
    if (j < 0) items.push(it);
    else items.splice(j, 0, it);
    if (items.length > NOTICE_LOG_MAX_ITEMS) {
      items.length = NOTICE_LOG_MAX_ITEMS;
      truncated = true;
    }
  }
  return { ...log, items, truncated, ...(summary ? { summary } : {}) };
}

/**
 * Today's log, subscribed on demand: the first listener opts in (shared ws.ts sends
 * `notices.subscribe` only to a server whose welcome lists the feature), the last opts out.
 * `setConnected(false)` drops the log, so a hook shows its own copy until the reconnect's snapshot.
 */
export function createNoticesChannel(io: { subscribe(): void; unsubscribe(): void }): NoticesChannel & { apply(m: NoticesMessage): void; setAvailable(on: boolean): void; setConnected(open: boolean): void } {
  let state: NoticesState = EMPTY_NOTICES;
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
        state = { ...EMPTY_NOTICES, available: state.available };
      };
    },
    apply(m) {
      const log = applyNoticesMessage(state.log, m);
      if (log === state.log) return;
      state = { ...state, log };
      emit();
    },
    setAvailable(on) {
      if (state.available === on) return;
      state = { ...state, available: on };
      emit();
    },
    setConnected(open) {
      if (open || !state.log) return;
      state = { ...state, log: null };
      emit();
    },
  };
}
