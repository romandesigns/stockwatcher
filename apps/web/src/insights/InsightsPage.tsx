import { cn } from "@stockwatcher/chart-web/ui";
import type { NoticesChannel } from "@stockwatcher/shared";
import { useState } from "react";
import { ConnectionGate } from "../ConnectionGate.tsx";
import { NavToggle } from "../nav/NavToggle.tsx";
import { useHasRail } from "../nav/navRail.ts";
import { SettingsButton } from "../nav/settings.tsx";
import { NotificationLog } from "./NotificationLog.tsx";
import { ScorecardView } from "./ScorecardView.tsx";

/**
 * Insights (owner, 2026-10-26: "create an insight page where we can display a notification
 * log"): two tabs, the notification log (every notice the server decided, sent or held and why,
 * from the ledger) and the daily scorecard. It connects like every page, which is what brings the
 * sign-in when the server needs a token.
 */
export function InsightsPage() {
  return <ConnectionGate source="live">{(c) => <Insights notices={c.feed.notices} />}</ConnectionGate>;
}

type Tab = "notifications" | "scorecard";
const TABS: readonly { id: Tab; label: string }[] = [
  { id: "notifications", label: "Notifications" },
  { id: "scorecard", label: "Scorecard" },
];

function Insights({ notices }: { notices?: NoticesChannel }) {
  const hasRail = useHasRail();
  const [tab, setTab] = useState<Tab>("notifications");
  return (
    <div className="@container/insights flex h-full min-h-0 flex-col gap-4 p-3 text-foreground @3xl/insights:px-6 @3xl/insights:py-4">
      <header className="flex flex-wrap items-center gap-x-3 gap-y-2">
        {hasRail && <NavToggle />}
        <h1 className="m-0 text-base font-semibold tracking-tight">Insights</h1>
        <div role="tablist" aria-label="Insights" className="inline-flex rounded-[9px] bg-card p-0.5">
          {TABS.map((t) => (
            <button
              key={t.id}
              type="button"
              role="tab"
              id={`insights-tab-${t.id}`}
              aria-selected={tab === t.id}
              aria-controls={`insights-panel-${t.id}`}
              onClick={() => setTab(t.id)}
              className={cn(
                "inline-flex h-7 items-center rounded-[7px] px-2.5 text-[13px] font-semibold outline-none transition-colors duration-150 pointer-coarse:h-11 focus-visible:ring-2 focus-visible:ring-ring motion-reduce:transition-none",
                tab === t.id ? "bg-bg-raised text-foreground" : "text-muted-foreground hover:text-foreground",
              )}
            >
              {t.label}
            </button>
          ))}
        </div>
        {!hasRail && (
          <div className="ml-auto">
            <SettingsButton />
          </div>
        )}
      </header>
      <div role="tabpanel" id={`insights-panel-${tab}`} aria-labelledby={`insights-tab-${tab}`} className="flex min-h-0 flex-1 flex-col">
        {tab === "notifications" ? <NotificationLog notices={notices} /> : <ScorecardView />}
      </div>
    </div>
  );
}
