// The Insights page's notification log (owner, 2026-10-26: "create an insight page where we can
// display a notification log"): GET /api/notices (crates/server/src/notices.rs) parsed, grouped
// into the page's filter groups and put in plain words. Shared by the web and the phone so both
// say the same thing about the same notice.

import { parseBreakoutNotice, type BreakoutNotice } from "./breakout.ts";

/** One path's outcome: the desk (desktop and web) or the phones. */
export interface NoticePathWire {
  /** `sent` | `held` (Runners notices); a price alert's phone: `sent` | `off` | `held`. */
  decision: string | null;
  /** The limit's code when held: no_device, quiet_hours, stale, symbol_limit, duplicate, merged. */
  held: string | null;
  devices: number | null;
  /**
   * What it was folded into when several Runners paths fired on one stock within half a minute
   * (server alerts/runners.rs, "Merging"): present on a merged decision, and on a send that only
   * reached the devices the first push missed.
   */
  merged_into?: NoticeMergedWire | null;
  /** Devices that already had that first push. */
  merged_devices?: number | null;
}

export interface NoticeMergedWire {
  event_id: string;
  /** The first push's data kind (`runners.running`). */
  notice: string;
  /** Seconds between the first push and this one. */
  secs: number;
}

export interface NoticeItemWire {
  event_id: string;
  /** Epoch ms of the first decision, or recorded pushed/failed fallback when no decision exists. */
  at?: number;
  symbol?: string;
  price?: number;
  /** The data kind the clients see (`runners.burst`, `ross.crossing`, …; `price.alert`). */
  notice?: string;
  title?: string;
  body?: string;
  breakout?: BreakoutNotice;
  /** Frozen reasons from the original alert; never current-bar cause measurements. */
  entry_context?: string[];
  desk?: NoticePathWire;
  phone?: NoticePathWire;
  latency?: { trade_to_board_ms?: number; board_to_queue_ms?: number; queue_to_expo_ms?: number; trade_to_expo_ms?: number };
  /** Expo refused it (token-redacted detail). */
  failed?: string | null;
}

export interface NoticeLogWire {
  day: string;
  days: string[];
  items: NoticeItemWire[];
  truncated: boolean;
}

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);
const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);
const num = (v: unknown): number | undefined => (typeof v === "number" && Number.isFinite(v) ? v : undefined);

function path(v: unknown): NoticePathWire | undefined {
  if (!isObj(v)) return undefined;
  return { decision: str(v.decision) ?? null, held: str(v.held) ?? null, devices: num(v.devices) ?? null };
}

/** A /api/notices body, or null when it is not one. Malformed items are dropped. */
export function parseNoticeLog(raw: unknown): NoticeLogWire | null {
  if (!isObj(raw) || typeof raw.day !== "string" || !Array.isArray(raw.items)) return null;
  const items: NoticeItemWire[] = [];
  for (const r of raw.items) {
    if (!isObj(r) || typeof r.event_id !== "string") continue;
    const lat = isObj(r.latency) ? r.latency : undefined;
    items.push({
      event_id: r.event_id,
      at: num(r.at),
      symbol: str(r.symbol),
      price: num(r.price),
      notice: str(r.notice),
      title: str(r.title),
      body: str(r.body),
      breakout: r.notice === "runners.breakout" ? parseBreakoutNotice(r.breakout) : undefined,
      entry_context: Array.isArray(r.entry_context) ? r.entry_context.filter((x): x is string => typeof x === "string").slice(0, 32) : undefined,
      desk: path(r.desk),
      phone: path(r.phone),
      latency: lat && {
        trade_to_board_ms: num(lat.trade_to_board_ms),
        board_to_queue_ms: num(lat.board_to_queue_ms),
        queue_to_expo_ms: num(lat.queue_to_expo_ms),
        trade_to_expo_ms: num(lat.trade_to_expo_ms),
      },
      failed: str(r.failed) ?? null,
    });
  }
  return { day: raw.day, days: Array.isArray(raw.days) ? raw.days.filter((d): d is string => typeof d === "string") : [], items, truncated: raw.truncated === true };
}

/** The page's kind filters, in chip order. */
export type NoticeGroup = "runners" | "pullback" | "ross" | "shelf" | "halt" | "price";
export const NOTICE_GROUPS: readonly { id: NoticeGroup; label: string }[] = [
  { id: "runners", label: "Runners" },
  { id: "pullback", label: "Pullback" },
  { id: "ross", label: "Ross / Signals" },
  { id: "shelf", label: "Shelf / Follow-ups" },
  { id: "halt", label: "Halt" },
  { id: "price", label: "Price alerts" },
];

const KIND: Record<string, { group: NoticeGroup; label: string }> = {
  "runners.new": { group: "runners", label: "Runners flag" },
  "runners.burst": { group: "runners", label: "Buying burst" },
  "runners.fast": { group: "runners", label: "Fast move" },
  "runners.run": { group: "runners", label: "Run +20%" },
  "runners.running": { group: "runners", label: "Running" },
  "runners.breakout": { group: "runners", label: "Breakout" },
  "runners.pullback": { group: "pullback", label: "Pullback" },
  "ross.crossing": { group: "ross", label: "Ross crossing" },
  "ross.trigger": { group: "ross", label: "Ross at trigger" },
  "signals.crossing": { group: "ross", label: "Signals crossing" },
  "signals.trigger": { group: "ross", label: "Signals at trigger" },
  "runners.shelf": { group: "shelf", label: "Volume shelf" },
  "runners.followup": { group: "shelf", label: "Follow-up" },
  "halt.watch": { group: "halt", label: "Halt watcher" },
  "price.alert": { group: "price", label: "Price alert" },
};

/** The notice's filter group and its name ("Buying burst"); an unknown kind is a Runners one by its raw name. */
export function noticeKind(notice: string | undefined): { group: NoticeGroup; label: string } {
  if (notice && KIND[notice]) return KIND[notice];
  return { group: "runners", label: notice ?? "Notice" };
}

/**
 * Overall status of a notice: the phone's when the phones saw it (the question the log answers is
 * "why didn't my phone buzz?": SRTM's pullback on 2026-10-26 reached the desktop but was held for
 * the phone), else the desk's.
 */
export type NoticeStatus = "sent" | "held";
export function noticeStatus(item: NoticeItemWire): NoticeStatus {
  const p = item.phone?.decision ? item.phone : item.desk;
  return p?.decision === "sent" ? "sent" : "held";
}

/** What the held path's notice was merged into (the phone's if held, else the desk's), or null. */
export function noticeMergedInto(item: NoticeItemWire): NoticeMergedWire | null {
  if (item.phone?.decision === "held") return item.phone.merged_into ?? null;
  if (item.desk?.decision === "held") return item.desk.merged_into ?? null;
  return null;
}

/** The hold reason that matters: the phone's if held, else the desk's. */
export function noticeHeldReason(item: NoticeItemWire): string | null {
  if (item.phone?.decision === "held" || item.phone?.decision === "off") return item.phone.held ?? (item.phone.decision === "off" ? "off" : null);
  if (item.desk?.decision === "held") return item.desk.held;
  return null;
}

const HELD: Record<string, string> = {
  symbol_limit: "same stock notified moments earlier",
  no_device: "no device wanted it",
  quiet_hours: "quiet hours",
  stale: "too old by the time it was decided",
  duplicate: "already sent",
  off: "phone alerts off for it",
  merged: "merged into an earlier alert",
};

/**
 * A hold reason in plain words ("same stock notified moments earlier"). A merged one names what it
 * was folded into when known: "merged into the Running alert 1 s earlier".
 */
export function heldText(reason: string | null, merged?: NoticeMergedWire | null): string {
  if (reason === "merged" && merged) return `merged into the ${noticeKind(merged.notice).label} alert ${mergedAgo(merged.secs)}`;
  return reason ? (HELD[reason] ?? reason.replace(/_/g, " ")) : "held";
}

const mergedAgo = (secs: number) => (Number.isFinite(secs) && secs >= 1 ? `${Math.round(secs)} s earlier` : "moments earlier");

/** Short status for a path's indicator: "sent", "held: quiet hours", or null when the path never saw it. */
export function pathText(p: NoticePathWire | undefined): string | null {
  if (!p || !p.decision) return null;
  if (p.decision === "sent") {
    const to = `sent${p.devices ? ` to ${p.devices} device${p.devices === 1 ? "" : "s"}` : ""}`;
    // A send that only reached the devices the stock's first push missed.
    return p.merged_into && p.merged_devices ? `${to} · ${p.merged_devices} already had the ${noticeKind(p.merged_into.notice).label} alert` : to;
  }
  return `${p.decision === "off" ? "not sent" : "held"}: ${heldText(p.held ?? (p.decision === "off" ? "off" : null), p.merged_into)}`;
}

/** The latency line for a tooltip ("trade → phone 1.04 s · board 0.10 s · queue 0.84 s · Expo 0.11 s"), or null. */
export function latencyText(item: NoticeItemWire): string | null {
  const l = item.latency;
  if (!l) return null;
  const s = (ms: number | undefined) => (ms === undefined ? null : `${(ms / 1000).toFixed(2)} s`);
  const parts = [s(l.trade_to_expo_ms) && `trade → push ${s(l.trade_to_expo_ms)}`, s(l.trade_to_board_ms) && `detect ${s(l.trade_to_board_ms)}`, s(l.board_to_queue_ms) && `queue ${s(l.board_to_queue_ms)}`, s(l.queue_to_expo_ms) && `push service ${s(l.queue_to_expo_ms)}`].filter(Boolean);
  return parts.length ? parts.join(" · ") : null;
}

export interface NoticeFilter {
  groups: ReadonlySet<NoticeGroup>;
  /** null = both. */
  status: NoticeStatus | null;
  /** Ticker prefix, upper case; "" = all. */
  ticker: string;
}

/** The items that pass the page's filters (empty group set = every group). */
export function filterNotices(items: readonly NoticeItemWire[], f: NoticeFilter): NoticeItemWire[] {
  const t = f.ticker.trim().toUpperCase();
  return items.filter((i) => (f.groups.size === 0 || f.groups.has(noticeKind(i.notice).group)) && (f.status === null || noticeStatus(i) === f.status) && (!t || (i.symbol ?? "").toUpperCase().startsWith(t)));
}

/** The summary line's counts: sent, held, and held by plain reason, most common first. */
export function noticeSummary(items: readonly NoticeItemWire[]): { sent: number; held: number; reasons: { reason: string; text: string; n: number }[] } {
  let sent = 0;
  const by = new Map<string, number>();
  for (const i of items) {
    if (noticeStatus(i) === "sent") sent++;
    else {
      const r = noticeHeldReason(i) ?? "held";
      by.set(r, (by.get(r) ?? 0) + 1);
    }
  }
  const reasons = [...by].map(([reason, n]) => ({ reason, text: heldText(reason), n })).sort((a, b) => b.n - a.n);
  return { sent, held: items.length - sent, reasons };
}
