// Runners system notifications on the desktop and the web (owner, 2026-10-23: "the same
// notifications as the phone, and a click takes me to the chart"). The server decides once
// (crates/server/src/alerts/runners.rs `desk_frame`, the phone's limits with their own count) and
// sends every socket a `runners.notice` carrying the phone push's own title and body; a client
// that sent `runners.notify` shows it when its switch for that kind is on, once per `id`.

/** The kinds a Runners notice can be about (the phone push's `data.kind`). */
export const RUNNERS_NOTICE_KINDS = ["runners.new", "runners.breakout", "runners.burst", "runners.running", "runners.run", "runners.fast"] as const;
export type RunnersNoticeKind = (typeof RUNNERS_NOTICE_KINDS)[number];

/** server `runners.notice`: one notice the desk limits let through. */
export interface RunnersNoticeMessage {
  type: "runners.notice";
  /** `runners-<kind>-<alertId>`: what a client dedupes on (a row's flag and its breakout share `alertId`). */
  id: string;
  alertId: string;
  symbol: string;
  kind: RunnersNoticeKind;
  /** Exactly the phone push's title and body. */
  title: string;
  body: string;
  /** The phone push's data `{symbol, kind, alert_id}`. */
  data?: Record<string, unknown>;
  /** When it happened, epoch ms. */
  at: number;
}

/** One client's switches, the phone's names (Settings → Runners). */
export interface RunnersNotifyKinds {
  new: boolean;
  breakout: boolean;
  burst: boolean;
  running: boolean;
  run: boolean;
  fast: boolean;
}

/** The phone's defaults (crates/server/src/alerts/push.rs `register`): everything but breakouts. */
export const DEFAULT_RUNNERS_NOTIFY: Readonly<RunnersNotifyKinds> = Object.freeze({ new: true, breakout: false, burst: true, running: true, run: true, fast: true });

/** client `runners.notify`: this connection shows Runners notifications for these kinds (`enabled: false` stops). */
export interface RunnersNotifyMessage extends Partial<RunnersNotifyKinds> {
  type: "runners.notify";
  enabled?: boolean;
}

/** A stored or partial value as a full set of switches (anything unreadable takes the default). */
export function runnersNotifyKinds(v: unknown): RunnersNotifyKinds {
  const o = v && typeof v === "object" ? (v as Record<string, unknown>) : {};
  const out = { ...DEFAULT_RUNNERS_NOTIFY };
  for (const k of Object.keys(out) as (keyof RunnersNotifyKinds)[]) if (typeof o[k] === "boolean") out[k] = o[k];
  return out;
}

/** The switch a notice's kind answers to. */
export function runnersNotifyKey(kind: RunnersNoticeKind): keyof RunnersNotifyKinds {
  return kind.slice("runners.".length) as keyof RunnersNotifyKinds;
}

/** A ticker as the wire spells it (the phone's route.ts rule). */
const SYMBOL = /^[A-Z][A-Z0-9.-]{0,11}$/;

/** A frame read defensively: null for anything that is not a well-formed notice. */
export function parseRunnersNotice(v: unknown): RunnersNoticeMessage | null {
  if (!v || typeof v !== "object") return null;
  const m = v as Record<string, unknown>;
  if (m.type !== "runners.notice") return null;
  const str = (k: string) => (typeof m[k] === "string" ? (m[k] as string) : "");
  const kind = str("kind") as RunnersNoticeKind;
  const symbol = str("symbol").trim().toUpperCase();
  const id = str("id");
  if (!id || !SYMBOL.test(symbol) || !RUNNERS_NOTICE_KINDS.includes(kind) || !str("title")) return null;
  return { type: "runners.notice", id, alertId: str("alertId"), symbol, kind, title: str("title"), body: str("body"), data: m.data && typeof m.data === "object" ? (m.data as Record<string, unknown>) : undefined, at: typeof m.at === "number" ? m.at : 0 };
}

/**
 * The ids already shown, so the same client never shows one twice (a reconnect, a second socket,
 * a frame replayed). Bounded: the oldest go first. `load`/`save` persist it (the web keeps it in
 * localStorage, so a reload or a second tab does not show it again either).
 */
export interface NoticeDedupe {
  /** True the first time `id` is seen (and records it); false after. */
  claim(id: string): boolean;
}

export function createNoticeDedupe(opts: { max?: number; load?: () => string[]; save?: (ids: string[]) => void } = {}): NoticeDedupe {
  const max = opts.max ?? 500;
  // The session's own memory: the ids even when storage fails (private mode, quota).
  const mem = new Set<string>();
  return {
    claim(id) {
      if (mem.has(id)) return false;
      // Re-read every time: another tab may have claimed it since.
      let ids: string[] = [];
      try {
        ids = opts.load?.() ?? [];
      } catch {
        // Unreadable storage: this session's memory still holds.
      }
      mem.add(id);
      if (ids.includes(id)) return false;
      ids.push(id);
      try {
        opts.save?.(ids.slice(-max));
      } catch {
        // Full or blocked storage: as above.
      }
      if (mem.size > max) mem.delete(mem.values().next().value as string);
      return true;
    },
  };
}

/** Whether this client shows `notice`: its switch is on and it was not shown before. */
export function shouldShowNotice(notice: RunnersNoticeMessage, kinds: RunnersNotifyKinds, dedupe: NoticeDedupe): boolean {
  return kinds[runnersNotifyKey(notice.kind)] && dedupe.claim(notice.id);
}
