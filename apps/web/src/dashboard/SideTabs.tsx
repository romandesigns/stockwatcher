import { cn } from "@stockwatcher/chart-web/ui";
import { rocketFreshCount, type RocketChannel } from "@stockwatcher/shared";
import { useState, type ReactNode } from "react";
import { useRocket } from "../feed/useRocket.ts";
import { RocketPanel } from "./RocketPanel.tsx";

type Tab = "ross" | "rocket";
const KEY = "stockwatcher.sideTab";

function readTab(): Tab {
  try {
    return window.localStorage.getItem(KEY) === "rocket" ? "rocket" : "ross";
  } catch {
    return "ross";
  }
}

/**
 * The right-hand column's tabs (owner, 2026-10-21): **Ross | Rocket watch**, segmented and
 * compact, the choice remembered on this device. Rocket watch stays subscribed while the Ross tab
 * is open, so the tab can show a fresh flag (a dot and a count) without being opened.
 * With no Rocket watch channel (the fixture feed) the Ross panel shows alone, as before.
 */
export function SideTabs({ ross, rocket, onPick }: { ross: ReactNode; rocket: RocketChannel | undefined; onPick: (symbol: string) => void }) {
  const [tab, setTab] = useState<Tab>(readTab);
  if (!rocket) return <>{ross}</>;
  const pick = (t: Tab) => {
    setTab(t);
    try {
      window.localStorage.setItem(KEY, t);
    } catch {
      // A private window: the choice lasts this session.
    }
  };
  return (
    <div className="flex min-h-0 min-w-0 flex-col gap-2">
      <div role="tablist" aria-label="Side panel" className="inline-flex gap-0.5 self-start rounded-[9px] bg-card p-0.5">
        <TabButton active={tab === "ross"} onClick={() => pick("ross")}>
          Ross
        </TabButton>
        <TabButton active={tab === "rocket"} onClick={() => pick("rocket")}>
          Runners
          {tab !== "rocket" && <FreshBadge channel={rocket} />}
        </TabButton>
      </div>
      {tab === "ross" ? ross : <RocketPanel channel={rocket} onPick={onPick} />}
    </div>
  );
}

function TabButton({ active, onClick, children }: { active: boolean; onClick: () => void; children: ReactNode }) {
  return (
    <button
      type="button"
      role="tab"
      aria-selected={active}
      onClick={onClick}
      className={cn(
        "inline-flex h-7 items-center gap-1.5 rounded-[7px] px-2.5 text-[13px] font-semibold outline-none transition-colors duration-150 focus-visible:ring-2 focus-visible:ring-ring motion-reduce:transition-none",
        active ? "bg-bg-raised text-foreground" : "text-muted-foreground hover:text-foreground",
      )}
    >
      {children}
    </button>
  );
}

/** Flags under a minute old while the Ross tab is open: a dot and a count, nothing otherwise. */
function FreshBadge({ channel }: { channel: RocketChannel }) {
  const state = useRocket(channel);
  const n = rocketFreshCount(state);
  if (n === 0) return null;
  return (
    <span className="inline-flex items-center gap-1 font-mono text-[11px] text-primary" aria-label={`${n} new`}>
      <span className="size-1.5 animate-pulse rounded-full bg-primary motion-reduce:animate-none" aria-hidden />
      {n}
    </span>
  );
}
