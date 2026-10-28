import { formatEtTime12, type IconName } from "@stockwatcher/chart-core";
import { heldText, latencyText, noticeHeldReason, noticeKind, noticeMergedInto, noticeStatus, pathText, type NoticeGroup, type NoticeItemWire } from "@stockwatcher/shared";
import { BREAKOUT_LABEL, breakoutClockText, breakoutSummary, noticeChartContext, type NoticeChartContext } from "@stockwatcher/shared";

// One notification-log row on the phone (the web's NoticeRow, apps/web/src/insights): what the
// row shows, derived once so the screen stays layout only and the wording is tested.

const KIND_ICON: Record<string, IconName> = {
  "runners.burst": "pathBurst",
  "runners.fast": "pathFast",
  "runners.run": "pathRun",
  "runners.running": "pathRunning",
  "runners.new": "pathRunner",
  "runners.breakout": "pathRunner",
};
export const GROUP_ICON: Record<NoticeGroup, IconName> = { runners: "pathRunner", pullback: "trend", ross: "star", shelf: "levels", halt: "gauge", price: "bell" };

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
  /** Everything, for the screen reader and the long-press read-out. */
  a11y: string;
  evidence: string | null;
  noticeContext?: NoticeChartContext;
}

export function noticeRowView(item: NoticeItemWire): NoticeRowView {
  const kind = noticeKind(item.notice);
  const context = noticeChartContext(item);
  const label = context ? BREAKOUT_LABEL : kind.label;
  const cause = context ? `${breakoutSummary(context.breakout)} · ${breakoutClockText(context.breakout, (ms) => formatEtTime12(ms, true))} · decision ${item.at ? formatEtTime12(item.at, true) : "unavailable"} · rule ${context.breakout.rule} · ${context.breakout.source} / ${context.breakout.exact_state}` : item.notice === "runners.breakout" ? "Event time and comparison unavailable" : null;
  const evidence = [cause, item.entry_context?.length ? `Entry context (original alert): ${item.entry_context.join(" · ")}` : null].filter(Boolean).join(" · ") || null;
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
    icon: (item.notice && KIND_ICON[item.notice]) || GROUP_ICON[kind.group],
    sent,
    status: sent ? "Sent" : "Held",
    detail,
    phone,
    desk,
    evidence,
    noticeContext: context,
    a11y: [`${item.symbol ?? "Notice"}, ${label}`, sent ? "sent" : `held, ${heldText(reason, noticeMergedInto(item))}`, phone?.label, desk?.label, latency && sent ? latency : null, text, evidence].filter(Boolean).join(". "),
  };
}
