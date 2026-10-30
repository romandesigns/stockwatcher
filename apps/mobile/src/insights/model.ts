import { formatEtTime12, type IconName } from "@stockwatcher/chart-core";
import { heldText, latencyText, noticeEvidence, noticeHeldReason, noticeIcon, noticeKind, noticeMergedInto, noticeStatus, pathText, type NoticeItemWire } from "@stockwatcher/shared";
import { BREAKOUT_LABEL, noticeChartContext, type NoticeChartContext } from "@stockwatcher/shared";

// One notification-log row on the phone (the web's NoticeRow, apps/web/src/insights): what the
// row shows, derived once so the screen stays layout only and the wording is tested.

export interface NoticeRowView {
  key: string;
  time: string;
  symbol: string | null;
  kind: string;
  icon: IconName;
  sent: boolean;
  /** "Sent" or "Held". */
  status: string;
  /** Second line: the hold reason in plain words when held, else the notice text. */
  detail: string;
  /** Per path: label for the indicator, and whether that path sent it; null when it never saw it. */
  phone: { label: string; sent: boolean } | null;
  desk: { label: string; sent: boolean } | null;
  /** Everything plain, for the screen reader and the read-out sheet. */
  a11y: string;
  /** One plain evidence line with the key numbers ("New high $8.81 above alert $8.68 · volume 80× normal"), or null. */
  line: string | null;
  /** The full technical record, only in the read-out sheet ("Details" or a long press; owner, 2026-10-30: no debug walls). */
  details: string[];
  noticeContext?: NoticeChartContext;
}

export function noticeRowView(item: NoticeItemWire): NoticeRowView {
  const kind = noticeKind(item.notice);
  const context = noticeChartContext(item);
  const label = context ? BREAKOUT_LABEL : kind.label;
  // Shared with the web row (packages/shared noticeEvidence): one plain line, the rest as details.
  const { line, details } = noticeEvidence(item, (ms) => formatEtTime12(ms, true));
  const sent = noticeStatus(item) === "sent";
  const reason = noticeHeldReason(item);
  const text = item.body ? `${item.title ?? ""} · ${item.body}` : (item.title ?? "");
  const path = (name: string, p: NoticeItemWire["phone"]) => {
    const t = pathText(p);
    return t ? { label: `${name}: ${t}`, sent: p?.decision === "sent" } : null;
  };
  const phone = path("Phones", item.phone);
  const desk = path("Desktop and web", item.desk);
  const latency = latencyText(item);
  const detail = sent ? (item.failed ? `${text} · push service refused it` : text) : `Held: ${heldText(reason, noticeMergedInto(item))}`;
  return {
    key: item.event_id,
    time: item.at ? formatEtTime12(item.at, true) : "—",
    symbol: item.symbol ?? null,
    kind: label,
    // The Settings → Alerts switch's icon (one shared table, packages/shared ALERT_SWITCH_ICON).
    icon: noticeIcon(item.notice),
    sent,
    status: sent ? "Sent" : "Held",
    detail,
    phone,
    desk,
    line,
    details,
    noticeContext: context,
    a11y: [`${item.symbol ?? "Notice"}, ${label}`, sent ? "sent" : `held, ${heldText(reason, noticeMergedInto(item))}`, phone?.label, desk?.label, latency && sent ? latency : null, text, line].filter(Boolean).join(". "),
  };
}
