// The Insights page's notification log (owner, 2026-10-26: "create an insight page where we can
// display a notification log"): GET /api/notices (crates/server/src/notices.rs) parsed, grouped
// into the page's filter groups and put in plain words. Shared by the web and the phone so both
// say the same thing about the same notice.

import { breakoutClockText, breakoutPlainLine, breakoutSummary, noticeChartContext, parseBreakoutNotice, type BreakoutNotice } from "./breakout.ts";
import { RUNNERS_NOTICE_KINDS, runnersNoticeIcon, type AlertIcon, type RunnersNoticeKind } from "./runnersNotify.ts";

/** One path's outcome: the desk (desktop and web) or the phones. */
export interface NoticePathWire {
  /** `sent` | `held` (Runners notices); a price alert's phone: `sent` | `off` | `held`. */
  decision: string | null;
  /**
   * The limit's code when held: no_device, quiet_hours, stale, duplicate, switched_off (its
   * detector switched off at runtime, crates/server/src/alerts/detectors.rs). Days before 2026-10-30
   * also have symbol_limit and merged (the per-stock gap and merge since removed: server
   * alerts/runners.rs, "Every distinct event is delivered"); they are still worded.
   */
  held: string | null;
  devices: number | null;
  /**
   * What it was folded into when several Runners paths fired on one stock within half a minute
   * (the server's merge, removed 2026-10-30): present on old days' merged decisions, and on a send
   * that only reached the devices the first push missed.
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
  /**
   * b-good-1's tag of this notice, 30 s after it (crates/server/src/bgood; the ledger's
   * `confidence` line): `tier` `high` | `none`, the model's `score`, `held` `switched_off` when its
   * runtime switch was off (decided, not shown), `at` epoch ms. Absent: not a B0-kind notice, not
   * scored yet, or an older server. Drawn with confidence.ts `confidenceView(tier, held)`.
   */
  confidence?: { tier: string | null; score: number | null; held: string | null; at: number | null };
  /**
   * The row as the server assembled it (`notices.rows.v1`, crates/server/src/notices.rs): sent or
   * held, the hold's code and the same in plain words. Absent from older servers, whose rows the
   * clients still derive (`noticeStatus`, `noticeHeldReason`, `heldText`).
   */
  status?: NoticeStatus;
  reason?: string | null;
  reason_text?: string | null;
}

/** The day's sent / held counts and the held ones by plain reason, most common first. */
export interface NoticeSummary {
  sent: number;
  held: number;
  reasons: { reason: string; text: string; n: number }[];
}

export interface NoticeLogWire {
  day: string;
  days: string[];
  items: NoticeItemWire[];
  truncated: boolean;
  /** The server's summary over the WHOLE day (`notices.rows.v1`); absent from older servers. */
  summary?: NoticeSummary;
}

/**
 * The day menu: `today` first (the server's ET date, which has no ledger file yet on a weekend or
 * before the first notice), then every day with a file, newest first. Picking `today` means "no
 * day" (`null`, so the log follows the date when it turns); any other entry is that day. The
 * newest file is NOT today: treating it so on Saturday turned a pick of Friday back into Saturday's
 * empty log, and Friday could not be opened at all (owner, 2026-10-31, INO on the 30th).
 */
export function noticeDayOptions(days: readonly string[], today: string | null): { value: string; label: string }[] {
  const all = today ? [today, ...days.filter((d) => d !== today)] : [...days];
  return all.map((d) => ({ value: d, label: d === today ? `Today · ${d}` : d }));
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
      ...(r.status === "sent" || r.status === "held" ? { status: r.status, reason: str(r.reason) ?? null, reason_text: str(r.reason_text) ?? null } : {}),
    });
  }
  const summary = parseNoticeSummary(raw.summary);
  return { day: raw.day, days: Array.isArray(raw.days) ? raw.days.filter((d): d is string => typeof d === "string") : [], items, truncated: raw.truncated === true, ...(summary ? { summary } : {}) };
}

/** A server `summary` (`notices.rows.v1`), or undefined when absent or malformed. */
export function parseNoticeSummary(v: unknown): NoticeSummary | undefined {
  if (!isObj(v) || num(v.sent) === undefined || num(v.held) === undefined || !Array.isArray(v.reasons)) return undefined;
  const reasons = v.reasons.filter(isObj).flatMap((r) => (typeof r.reason === "string" && num(r.n) !== undefined ? [{ reason: r.reason, text: str(r.text) ?? r.reason.replace(/_/g, " "), n: num(r.n)! }] : []));
  return { sent: num(v.sent)!, held: num(v.held)!, reasons };
}

/** A row's status: the server's when it assembled the row, else derived as before (older servers). */
export function rowStatus(item: NoticeItemWire): NoticeStatus {
  return item.status ?? noticeStatus(item);
}

/** A held row's reason in plain words: the server's when it sent one, else derived (older servers). */
export function rowHeldText(item: NoticeItemWire): string {
  if (item.status) return item.reason_text ?? "held";
  return heldText(noticeHeldReason(item), noticeMergedInto(item));
}

/** The log's summary line: the server's (the whole day), else counted here from the rows an older server sent. */
export function logSummary(log: Pick<NoticeLogWire, "items" | "summary"> | null): NoticeSummary {
  return log?.summary ?? noticeSummary(log?.items ?? []);
}

/** The page's kind filters, in chip order, each with its icon (a group's chip, and the icon of a kind the log does not know). */
export type NoticeGroup = "runners" | "pullback" | "ross" | "shelf" | "halt" | "shadow" | "price";
export const NOTICE_GROUPS: readonly { id: NoticeGroup; label: string; icon: AlertIcon }[] = [
  { id: "runners", label: "Runners", icon: "pathRunner" },
  { id: "pullback", label: "Pullback", icon: runnersNoticeIcon("runners.pullback") },
  { id: "ross", label: "Ross / Signals", icon: "star" },
  { id: "shelf", label: "Shelf / Follow-ups", icon: runnersNoticeIcon("runners.shelf") },
  { id: "halt", label: "Halt", icon: runnersNoticeIcon("halt.watch") },
  { id: "shadow", label: "Shadow", icon: runnersNoticeIcon("shadow.n1") },
  { id: "price", label: "Price alerts", icon: "bell" },
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
  "halt.bull": { group: "halt", label: "Halt risk · buying" },
  "halt.resumed_up": { group: "halt", label: "Resumed up" },
  // The N1 shadow detector (crates/server/src/n1_live): labelled a shadow wherever it shows.
  "shadow.n1": { group: "shadow", label: "N1 · shadow" },
  "shadow.f1u": { group: "shadow", label: "F1 · first pullback" },
  // Research v2's x-float-1 (crates/server/src/xfloat_live): a Runners-pipeline stream.
  "v2.xfloat": { group: "runners", label: "Float rotation" },
  // The v2 finalists (crates/server/src/gap_ctx), research like the shadow kinds.
  "v2.xgap": { group: "shadow", label: "Pre-market high" },
  "v2.xctx": { group: "shadow", label: "Day 2 · above yesterday's high" },
  "model.pick": { group: "shadow", label: "Model pick" },
  "price.alert": { group: "price", label: "Price alert" },
};

const isRunnersKind = (n: string): n is RunnersNoticeKind => (RUNNERS_NOTICE_KINDS as readonly string[]).includes(n);

/**
 * A notice's icon in the log: exactly its Settings → Alerts switch's (`ALERT_SWITCH_ICON` in
 * runnersNotify.ts, one table for both screens on web and phone); a price alert's bell; an
 * unknown kind its group's.
 */
export function noticeIcon(notice: string | undefined): AlertIcon {
  if (notice && isRunnersKind(notice)) return runnersNoticeIcon(notice);
  const group = noticeKind(notice).group;
  return NOTICE_GROUPS.find((g) => g.id === group)?.icon ?? "bell";
}

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
  // A detector switched off at runtime (crates/server/src/alerts/detectors.rs): it ran, and its
  // would-be alert was only recorded (the `run` cut by default).
  switched_off: "detector switched off (recorded only)",
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

/**
 * What a log row says beyond its notice text (owner, 2026-10-30: no debug walls): `line` is one
 * plain line with the key numbers ("New high $8.81 above alert $8.68 · volume 80× normal"), or
 * null; `details` is the full technical record (measured bar, every clock, rule and source,
 * the original alert's reasons), shown only behind "Details" (web) or a press (phone). Display
 * only: the ledger keeps every field as it was written. `time` formats epoch ms.
 */
export function noticeEvidence(item: NoticeItemWire, time: (ms: number) => string): { line: string | null; details: string[] } {
  const details: string[] = [];
  let line: string | null = null;
  const context = noticeChartContext(item);
  if (context) {
    const b = context.breakout;
    line = breakoutPlainLine(b);
    details.push(breakoutSummary(b), breakoutClockText(b, time), `Decision ${item.at ? time(item.at) : "unavailable"}`, `Rule ${b.rule} · ${b.source} / ${b.exact_state}`);
  } else if (item.notice === "runners.breakout") {
    details.push("Event time and comparison unavailable");
  }
  if (item.entry_context?.length) details.push(`Entry context (original alert): ${item.entry_context.join(" · ")}`);
  return { line, details };
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
  return items.filter((i) => (f.groups.size === 0 || f.groups.has(noticeKind(i.notice).group)) && (f.status === null || rowStatus(i) === f.status) && (!t || (i.symbol ?? "").toUpperCase().startsWith(t)));
}

/**
 * The summary line's counts: sent, held, and held by plain reason, most common first. Only for an
 * older server: one with `notices.rows.v1` sends the day's summary itself (`logSummary`).
 */
export function noticeSummary(items: readonly NoticeItemWire[]): NoticeSummary {
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
