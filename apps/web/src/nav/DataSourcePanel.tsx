import type { DashboardSource } from "@stockwatcher/app-state";
import type { IconName } from "@stockwatcher/chart-core";
import { Icon } from "@stockwatcher/chart-web";
import { cn } from "@stockwatcher/chart-web/ui";

const OPTIONS: readonly { value: DashboardSource; icon: IconName; name: string; description: string }[] = [
  {
    value: "live",
    icon: "live",
    name: "Live market",
    description: "Real trades from Alpaca as they happen: SIP from 4:00 to 20:00 ET, Blue Ocean overnight. Empty when nothing is trading.",
  },
  {
    value: "replay",
    icon: "replay",
    name: "Replay",
    description: "A recorded day from the archive, on the replay clock the chart page controls (the practice feature).",
  },
];

/** Settings → Data's choice list: the market the dashboard reads (see dataSource.tsx). */
export function DataSourcePanel({ source, onChange }: { source: DashboardSource; onChange: (s: DashboardSource) => void }) {
  return (
    <div role="radiogroup" aria-label="Dashboard data" className="flex flex-col gap-1">
      {OPTIONS.map((o) => {
        const on = o.value === source;
        return (
          <button
            key={o.value}
            type="button"
            role="radio"
            aria-checked={on}
            onClick={() => onChange(o.value)}
            className={cn(
              "flex items-start gap-3 rounded-lg px-3 py-2.5 text-left outline-none transition-colors hover:bg-bg-row focus-visible:ring-2 focus-visible:ring-ring [&_svg]:size-4",
              on && "bg-bg-row",
            )}
          >
            {/* The chosen one spends the accent on its icon and check only (60/30/10). */}
            <span className={cn("mt-0.5 shrink-0", on ? "text-primary" : "text-muted-foreground")}>
              <Icon name={o.icon} />
            </span>
            <span className="flex min-w-0 flex-1 flex-col gap-0.5">
              <span className="text-sm font-medium text-foreground">{o.name}</span>
              <span className="text-xs text-text-faint">{o.description}</span>
            </span>
            <span className={cn("mt-0.5 shrink-0 text-primary transition-opacity", on ? "opacity-100" : "opacity-0")}>
              <Icon name="check" />
            </span>
          </button>
        );
      })}
    </div>
  );
}
