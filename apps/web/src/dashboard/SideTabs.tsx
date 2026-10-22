import { cn } from "@stockwatcher/chart-web/ui";
import { detectionAttention, rocketFreshCount, type DetectionChannel, type RocketChannel } from "@stockwatcher/shared";
import { useState, type ReactNode } from "react";
import { useDetection } from "../feed/useDetection.ts";
import { useRocket } from "../feed/useRocket.ts";
import { DetectionPanel } from "./DetectionPanel.tsx";
import { RocketPanel } from "./RocketPanel.tsx";

type Tab = "ross" | "rocket" | "signals";
const KEY = "stockwatcher.sideTab";

function readTab(): Tab {
  try {
    const t = window.localStorage.getItem(KEY);
    return t === "rocket" || t === "signals" ? t : "ross";
  } catch {
    return "ross";
  }
}

/**
 * The right-hand column's tabs (owner, 2026-10-21): **Ross | Runners | Signals**, segmented and
 * compact, the choice remembered on this device. Runners and Signals stay subscribed while
 * another tab is open, so their tab can show a count without being opened: Runners its fresh
 * flags, Signals (design canvas "Detection Panel", 2026-10-22) the rows at the trigger or crossing
 * it, with a pulsing dot. The Signals tab exists only while the server's detector is on (the
 * canvas's "off" state: the column is exactly as before). With neither, the Ross panel shows alone.
 */
export function SideTabs({ ross, rocket, detection, onPick, onEpisodePick }: { ross: ReactNode; rocket: RocketChannel | undefined; detection: DetectionChannel | undefined; onPick: (symbol: string) => void; onEpisodePick: (symbol: string, episodeId: string, runEpoch: string) => void }) {
  const [stored, setStored] = useState<Tab>(readTab);
  const detected = useDetection(detection);
  const signals = !!detection && detected.enabled === true;
  const available: Tab[] = [...(ross ? (["ross"] as const) : []), ...(rocket ? (["rocket"] as const) : []), ...(signals ? (["signals"] as const) : [])];
  // A remembered tab that is not offered now (detector off, fixture feed) falls back to the
  // first one without forgetting the choice.
  const tab = available.includes(stored) ? stored : available[0];
  const content = (t: Tab | undefined) => (t === "ross" ? ross : t === "rocket" && rocket ? <RocketPanel channel={rocket} onPick={onPick} /> : t === "signals" ? <DetectionPanel channel={detection} onPick={onEpisodePick} /> : null);
  if (available.length <= 1) return <>{content(tab)}</>;
  const pick = (t: Tab) => {
    setStored(t);
    try {
      window.localStorage.setItem(KEY, t);
    } catch {
      // A private window: the choice lasts this session.
    }
  };
  const attention = signals ? detectionAttention(detected.snapshot) : 0;
  return (
    <div className="flex min-h-0 min-w-0 flex-col gap-2">
      <div role="tablist" aria-label="Side panel" className="inline-flex gap-0.5 self-start rounded-[9px] bg-card p-0.5">
        {ross && (
          <TabButton id="ross" active={tab === "ross"} onClick={() => pick("ross")}>
            Ross
          </TabButton>
        )}
        {rocket && (
          <TabButton id="rocket" active={tab === "rocket"} onClick={() => pick("rocket")}>
            Runners
            {tab !== "rocket" && <FreshBadge channel={rocket} />}
          </TabButton>
        )}
        {signals && (
          <TabButton id="signals" active={tab === "signals"} onClick={() => pick("signals")}>
            Signals
            {attention > 0 && <CountBadge n={attention} label={`${attention} at the trigger or crossing it`} />}
          </TabButton>
        )}
      </div>
      <div role="tabpanel" id={`side-panel-${tab}`} aria-labelledby={`side-tab-${tab}`} className="flex min-h-0 min-w-0 flex-col">
        {content(tab)}
      </div>
    </div>
  );
}

function TabButton({ id, active, onClick, children }: { id: Tab; active: boolean; onClick: () => void; children: ReactNode }) {
  return (
    <button
      type="button"
      role="tab"
      id={`side-tab-${id}`}
      aria-controls={`side-panel-${id}`}
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

/** A pulsing dot and a count, nothing at zero. */
function CountBadge({ n, label }: { n: number; label: string }) {
  return (
    <span className="inline-flex items-center gap-1 font-mono text-[11px] text-primary" aria-label={label}>
      <span className="size-1.5 animate-pulse rounded-full bg-primary motion-reduce:animate-none" aria-hidden />
      {n}
    </span>
  );
}

/** Flags under a minute old while another tab is open: a dot and a count, nothing otherwise. */
function FreshBadge({ channel }: { channel: RocketChannel }) {
  const state = useRocket(channel);
  const n = rocketFreshCount(state);
  if (n === 0) return null;
  return <CountBadge n={n} label={`${n} new`} />;
}
