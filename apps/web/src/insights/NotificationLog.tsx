import { formatEtTime12, type IconName } from "@stockwatcher/chart-core";
import { Icon } from "@stockwatcher/chart-web";
import { cn, Collapsible, CollapsibleContent, CollapsibleTrigger, Input, OptionSelect, ScrollArea, Tip } from "@stockwatcher/chart-web/ui";
import { filterNotices, noticeDayOptions, latencyText, NOTICE_GROUPS, noticeEvidence, noticeHeldReason, noticeMergedInto, heldText, noticeIcon, noticeKind, noticeStatus, noticeSummary, pathText, type NoticeGroup, type NoticeItemWire, type NoticePathWire, type NoticeStatus } from "@stockwatcher/shared";
import { useMemo, useState } from "react";
import { openTickerChart } from "../nav/openTicker.ts";
import { BREAKOUT_LABEL, noticeChartContext } from "@stockwatcher/shared";
import { useNoticeLog } from "./useNoticeLog.ts";

// The filters are icons, their names in tooltips and for screen readers (owner, 2026-10-31).
const STATUS: readonly { id: NoticeStatus | null; label: string; icon: "menu" | "check" | "pause" }[] = [
  { id: null, label: "All", icon: "menu" },
  { id: "sent", label: "Sent", icon: "check" },
  { id: "held", label: "Held", icon: "pause" },
];

/**
 * The notification log: every notice the server decided today (or on a chosen day), newest first,
 * with what happened on the phones and on desktop/web, and why when it was held. A row opens the
 * stock's chart. Read from the ledger (crates/server/src/notices.rs), written since 2026-10-26.
 */
export function NotificationLog() {
  const [day, setDay] = useState<string | null>(null);
  // The server's today, kept from the default load for when another day is open.
  const [today, setToday] = useState<string | null>(null);
  const [groups, setGroups] = useState<ReadonlySet<NoticeGroup>>(new Set());
  const [status, setStatus] = useState<NoticeStatus | null>(null);
  const [ticker, setTicker] = useState("");
  const load = useNoticeLog(day);
  const log = load.state === "ok" ? load.log : null;
  const items = useMemo(() => (log ? filterNotices(log.items, { groups, status, ticker }) : []), [log, groups, status, ticker]);
  const summary = useMemo(() => noticeSummary(log?.items ?? []), [log]);
  const toggleGroup = (g: NoticeGroup) =>
    setGroups((prev) => {
      const next = new Set(prev);
      if (next.has(g)) next.delete(g);
      else next.add(g);
      return next;
    });
  const todayDay = day === null ? (log?.day ?? null) : today;
  const dayOptions = useMemo(() => noticeDayOptions(log?.days ?? [], todayDay), [log, todayDay]);
  const pickDay = (d: string) => {
    if (day === null && log) setToday(log.day);
    setDay(d === todayDay ? null : d);
  };

  return (
    <section aria-labelledby="notification-log-title" className="@container/log flex min-h-0 flex-1 flex-col gap-3">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <h2 id="notification-log-title" className="panel-title">
          Notification log
        </h2>
        {log && dayOptions.length > 0 && <OptionSelect aria-label="Day" options={dayOptions} value={day ?? log.day} onValueChange={pickDay} size="sm" className="w-auto" />}
        <p className="m-0 text-xs text-muted-foreground" aria-live="polite">
          {log ? <SummaryLine sent={summary.sent} held={summary.held} reasons={summary.reasons} /> : null}
        </p>
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <div role="group" aria-label="Kinds" className="flex flex-wrap gap-1">
          {NOTICE_GROUPS.map((g) => (
            <Tip key={g.id} label={`${g.label} only`}>
              <button
                type="button"
                aria-label={`${g.label} only`}
                aria-pressed={groups.has(g.id)}
                onClick={() => toggleGroup(g.id)}
                className={cn(
                  "inline-grid size-7 place-items-center rounded-full outline-none transition-colors duration-150 pointer-coarse:size-11 focus-visible:ring-2 focus-visible:ring-ring motion-reduce:transition-none [&_svg]:size-3.5",
                  groups.has(g.id) ? "bg-bg-raised text-foreground" : "bg-card text-muted-foreground hover:text-foreground",
                )}
              >
                <Icon name={g.icon} />
              </button>
            </Tip>
          ))}
        </div>
        <div role="radiogroup" aria-label="Status" className="inline-flex rounded-[9px] bg-card p-0.5">
          {STATUS.map((s) => (
            <Tip key={s.label} label={s.label}>
              <button
                type="button"
                role="radio"
                aria-label={s.label}
                aria-checked={status === s.id}
                onClick={() => setStatus(s.id)}
                className={cn("inline-grid h-6 w-8 place-items-center rounded-[7px] outline-none pointer-coarse:h-10 pointer-coarse:w-11 focus-visible:ring-2 focus-visible:ring-ring [&_svg]:size-3.5", status === s.id ? "bg-bg-raised text-foreground" : "text-muted-foreground hover:text-foreground")}
              >
                <Icon name={s.icon} />
              </button>
            </Tip>
          ))}
        </div>
        <Input aria-label="Filter by ticker" placeholder="Ticker" value={ticker} onChange={(e) => setTicker(e.target.value.toUpperCase())} className="h-8 w-28 font-mono uppercase pointer-coarse:h-11" />
      </div>

      {load.state === "loading" && <p className="m-0 text-[13px] text-text-faint">Loading…</p>}
      {load.state === "error" && <p className="m-0 text-[13px] text-warning">{load.message}</p>}
      {log && log.items.length === 0 && (
        <p className="m-0 max-w-md text-[13px] text-text-faint">
          No notifications recorded {day === null ? "yet today" : `on ${log.day}`}. The log starts on October 26, 2026: every notice the server decides, sent or held, with the reason.
        </p>
      )}
      {log && log.items.length > 0 && items.length === 0 && <p className="m-0 text-[13px] text-text-faint">Nothing matches these filters.</p>}

      {items.length > 0 && (
        <ScrollArea className="min-h-0 flex-1" viewportClassName="flex flex-col gap-1">
          <ul className="m-0 flex list-none flex-col gap-1 p-0">
            {items.map((i) => (
              <li key={i.event_id}>
                <NoticeRow item={i} />
              </li>
            ))}
          </ul>
          {log?.truncated && <p className="m-0 mt-2 text-[11px] text-text-faint">Showing the newest 2,000 of the day.</p>}
        </ScrollArea>
      )}
    </section>
  );
}

function SummaryLine({ sent, held, reasons }: { sent: number; held: number; reasons: { reason: string; text: string; n: number }[] }) {
  return (
    <>
      <span className="font-mono text-foreground">{sent}</span> sent · <span className="font-mono text-foreground">{held}</span> held
      {reasons.length > 0 && <span className="text-text-faint"> ({reasons.map((r) => `${r.n} ${r.text}`).join(", ")})</span>}
    </>
  );
}

/**
 * One notice: time, ticker and kind, its text, one plain evidence line, and what happened on each
 * path. A breakout's measured record (every clock, rule, source) sits behind "Details" (owner,
 * 2026-10-30: no debug walls), which is its own button beside the row's open-the-chart one.
 */
export function NoticeRow({ item }: { item: NoticeItemWire }) {
  const kind = noticeKind(item.notice);
  const context = noticeChartContext(item);
  const label = context ? BREAKOUT_LABEL : kind.label;
  const { line, details } = noticeEvidence(item, (ms) => formatEtTime12(ms, true));
  const sent = noticeStatus(item) === "sent";
  // "merged into the Running alert 1 s earlier" when several paths fired at once (shared noticeLog).
  const held = heldText(noticeHeldReason(item), noticeMergedInto(item));
  const latency = latencyText(item);
  const text = item.body ? `${item.title ?? ""} · ${item.body.replace(/[\r\n]+/g, " · ")}` : (item.title ?? "");
  const row = (
    <button
      type="button"
      onClick={() => item.symbol && openTickerChart(item.symbol, context)}
      disabled={!item.symbol}
      aria-label={`${item.symbol ?? "Notice"}: ${label}, ${sent ? "sent" : `held, ${held}`}. ${line ?? ""}. ${text}. Open the chart.`}
      className="grid w-full grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-x-3 gap-y-1 rounded-lg bg-card px-3 py-2 text-left outline-none transition-colors duration-150 hover:bg-bg-row focus-visible:ring-2 focus-visible:ring-ring motion-reduce:transition-none @2xl/log:grid-cols-[6.5rem_minmax(8rem,12rem)_minmax(0,1fr)_auto]"
    >
      <Tip label={context ? "History decision time" : "History time: first decision or recorded fallback"}><span className="font-mono text-xs text-muted-foreground">{item.at ? formatEtTime12(item.at, true) : "—"}</span></Tip>
      <span className="flex min-w-0 items-center gap-2">
        <span className="font-mono text-[13px] font-bold">{item.symbol ?? "—"}</span>
        <Tip label={label}>
          <span className="inline-flex items-center gap-1 text-xs text-muted-foreground [&_svg]:size-3.5">
            <Icon name={noticeIcon(item.notice)} />
            <span className="truncate">{label}</span>
          </span>
        </Tip>
      </span>
      <span className="col-span-3 row-start-2 min-w-0 text-xs text-text-faint @2xl/log:col-span-1 @2xl/log:col-start-3 @2xl/log:row-start-1">
        <span className="block truncate">{item.failed ? `${text} · push service refused it` : text}</span>
        {line && <span className="mt-0.5 block truncate text-muted-foreground">{line}</span>}
      </span>
      <span className="col-start-3 row-start-1 flex items-center gap-2 justify-self-end @2xl/log:col-start-4">
        <PathMark icon="screen" name="Desktop and web" path={item.desk} />
        <PathMark icon="phone" name="Phones" path={item.phone} extra={latency} />
        <Tip label={latency && sent ? latency : sent ? "Sent" : `Held: ${held}`}>
          <span className={cn("rounded-full px-2 py-0.5 text-[11px] font-medium whitespace-nowrap", sent ? "bg-bg-row text-muted-foreground" : "bg-warning/15 text-warning")}>
            {sent ? (
              "Sent"
            ) : (
              <>
                Held<span className="hidden @3xl/log:inline"> · {held}</span>
              </>
            )}
          </span>
        </Tip>
      </span>
    </button>
  );
  if (details.length === 0) return row;
  return (
    <div className="rounded-lg bg-card">
      {row}
      <Collapsible className="px-3 pb-1.5">
        <CollapsibleTrigger aria-label={`Details for ${item.symbol ?? "this notice"}`} className="h-7 w-auto px-1 text-[11px] text-muted-foreground hover:text-foreground pointer-coarse:h-11">
          Details
        </CollapsibleTrigger>
        <CollapsibleContent>
          <ul className="m-0 flex list-none flex-col gap-0.5 p-0 pb-1 pl-5 text-[11px] text-text-faint">
            {details.map((d) => (
              <li key={d} className="break-words">
                {d}
              </li>
            ))}
          </ul>
        </CollapsibleContent>
      </Collapsible>
    </div>
  );
}

/** A path's indicator (phone, desktop): quiet when sent, warning when held, absent when the path never saw it. */
function PathMark({ icon, name, path, extra }: { icon: IconName; name: string; path: NoticePathWire | undefined; extra?: string | null }) {
  const text = pathText(path);
  if (!text) return null;
  const sent = path?.decision === "sent";
  const label = `${name}: ${text}${sent && extra ? ` (${extra})` : ""}`;
  return (
    <Tip label={label}>
      <span role="img" aria-label={label} className={cn("inline-flex [&_svg]:size-3.5", sent ? "text-muted-foreground" : "text-warning")}>
        <Icon name={icon} />
      </span>
    </Tip>
  );
}
