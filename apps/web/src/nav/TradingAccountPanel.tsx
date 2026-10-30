import { alpacaUnavailable, type BrokerStatus, type LiveAccount } from "@stockwatcher/app-state";
import type { IconName } from "@stockwatcher/chart-core";
import { Icon } from "@stockwatcher/chart-web";
import { cn } from "@stockwatcher/chart-web/ui";

const OPTIONS: readonly { value: LiveAccount; icon: IconName; name: string; description: string }[] = [
  {
    value: "simulator",
    icon: "gauge",
    name: "Simulator",
    description: "Orders fill on our own server against the live tape. Nothing leaves the app.",
  },
  {
    value: "alpaca_paper",
    icon: "trade",
    name: "Alpaca paper",
    description: "Orders go to your Alpaca paper account: paper money, real market rules. Outside 9:30–16:00 ET only limit orders are accepted.",
  },
];

/** The server's word on the Alpaca paper route, as a short line under its option. */
function statusLine(s: BrokerStatus): { text: string; tone: "good" | "warning" | "faint" } | null {
  if (alpacaUnavailable(s)) return { text: `Not configured. ${s.message}`, tone: "warning" };
  if (s.state === "live") return { text: "Connected", tone: "good" };
  if (s.state === "connecting" || s.state === "down") return { text: s.message || "Connecting…", tone: "faint" };
  return null;
}

/**
 * Settings → Trading's choice list: which account the LIVE market's chart orders go to. Replay
 * always uses the simulator. Same radio-row pattern as Settings → Data; the chosen row spends the
 * accent on its icon and check only (60/30/10).
 */
export function TradingAccountPanel({ account, status, onChange }: { account: LiveAccount; status: BrokerStatus; onChange: (a: LiveAccount) => void }) {
  return (
    <div role="radiogroup" aria-label="Live market account" className="flex flex-col gap-1">
      {OPTIONS.map((o) => {
        const on = o.value === account;
        const line = o.value === "alpaca_paper" ? statusLine(status) : null;
        return (
          <button
            key={o.value}
            type="button"
            role="radio"
            aria-checked={on}
            onClick={() => onChange(o.value)}
            className={cn(
              "flex items-start gap-3 rounded-lg px-3 py-2.5 text-left outline-none transition-colors hover:bg-bg-row focus-visible:ring-2 focus-visible:ring-ring pointer-coarse:min-h-11 [&_svg]:size-4",
              on && "bg-bg-row",
            )}
          >
            <span className={cn("mt-0.5 shrink-0", on ? "text-primary" : "text-muted-foreground")}>
              <Icon name={o.icon} />
            </span>
            <span className="flex min-w-0 flex-1 flex-col gap-0.5">
              <span className="text-sm font-medium text-foreground">{o.name}</span>
              <span className="text-xs text-text-faint">{o.description}</span>
              {line && <span className={cn("text-xs", line.tone === "good" ? "text-good" : line.tone === "warning" ? "text-warning" : "text-text-faint")}>{line.text}</span>}
            </span>
            <span className={cn("mt-0.5 shrink-0 text-primary transition-opacity", on ? "opacity-100" : "opacity-0")}>
              <Icon name="check" />
            </span>
          </button>
        );
      })}
      <p className="m-0 px-3 pt-1 text-xs text-text-faint">Applies to the live market on every device. Replay practice always uses the simulator.</p>
    </div>
  );
}
