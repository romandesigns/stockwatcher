// "Group repeat alerts" (owner, 2026-11-25, after AKAN 11-24's 47 pushes; server crates/server/src/
// alerts/runners.rs "Limits" 4a): one card per stock per day. Every `runners.notice` and silent
// `runners.grouped` frame carries the stock's `count` today; this keeps, per stock, that count and
// the latest notice, so a Runners row can say "12 alerts · latest Signals trigger 10:37" instead of
// the screen filling with pings. Pure: the web and the phone each hold one.

import type { RunnersNoticeMessage } from "./runnersNotify.ts";

export interface NoticeCard {
  symbol: string;
  /** Notices today (sent or grouped), the server's count when it sent one. */
  count: number;
  /** The latest notice's own title and body (the phone push's text). */
  title: string;
  body: string;
  kind: RunnersNoticeMessage["kind"];
  /** The latest notice's event time, epoch ms. */
  at: number;
  /** The ET date the card belongs to, "MM/DD/YYYY". */
  day: string;
}

const ET_DAY = new Intl.DateTimeFormat("en-US", { year: "numeric", month: "2-digit", day: "2-digit", timeZone: "America/New_York" });
const ET_TIME = new Intl.DateTimeFormat("en-US", { hour: "numeric", minute: "2-digit", hour12: false, timeZone: "America/New_York" });

export interface NoticeCards {
  /** Records one notice (sent or grouped); true when the stock's card changed. */
  record(m: RunnersNoticeMessage): boolean;
  /** The stock's card today, or null. */
  get(symbol: string, nowMs?: number): NoticeCard | null;
  subscribe(cb: () => void): () => void;
}

export function createNoticeCards(): NoticeCards {
  const cards = new Map<string, NoticeCard>();
  const listeners = new Set<() => void>();
  return {
    record(m) {
      if (!m.symbol || !m.at) return false;
      const day = ET_DAY.format(new Date(m.at));
      const prev = cards.get(m.symbol);
      const same = prev && prev.day === day;
      // The server's count when it sent one; otherwise count here.
      const count = m.count ?? (same ? prev.count + 1 : 1);
      if (same && m.at < prev.at && count <= prev.count) return false;
      const next: NoticeCard = { symbol: m.symbol, count: same ? Math.max(count, prev.count) : count, title: m.title, body: m.body, kind: m.kind, at: Math.max(m.at, same ? prev.at : 0), day };
      if (same && m.at < prev.at) {
        next.title = prev.title;
        next.body = prev.body;
        next.kind = prev.kind;
      }
      cards.set(m.symbol, next);
      for (const l of listeners) l();
      return true;
    },
    get(symbol, nowMs = Date.now()) {
      const c = cards.get(symbol);
      return c && c.day === ET_DAY.format(new Date(nowMs)) ? c : null;
    },
    subscribe(cb) {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
  };
}

/** A card's chip text: "12 alerts · 10:37" (null for a single alert: the row already says it). */
export function noticeCardLabel(c: NoticeCard | null): string | null {
  if (!c || c.count < 2) return null;
  return `${c.count} alerts · ${ET_TIME.format(new Date(c.at))}`;
}
