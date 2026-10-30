import { brokerLabel, brokerOf, buyingPower, freshNotice, type PaperState } from "@stockwatcher/chart-core";
import { useEffect, useState } from "react";
import { Icon } from "./Icon.tsx";
import { Tip } from "./ui/tooltip.tsx";
import { cn } from "./ui/utils.ts";

/**
 * Which account an order goes to, everywhere orders are placed or shown (owner, 2026-10-29):
 * "Simulator" (warning tones, as the old SIMULATED chip) or "Alpaca paper" (good tones while its
 * trade stream is live, warning while it is not). Two-tone icon, token colours, no border.
 */
export function AccountBadge({ state, className }: { state: PaperState | null | undefined; className?: string }) {
  const broker = brokerOf(state);
  const alpaca = broker === "alpaca_paper";
  const down = alpaca && state?.link !== undefined && state.link.state !== "live";
  const title = alpaca
    ? down
      ? `Alpaca paper account: ${state?.link?.message || "not connected"}`
      : "Orders go to your Alpaca paper account (paper money, real market)"
    : "Simulated fills on our own server; nothing leaves the app";
  return (
    <Tip label={title}>
    <span
      aria-label={`${brokerLabel(broker)}${down ? ", offline" : ""}. ${title}`}
      className={cn(
        "inline-flex shrink-0 items-center gap-1 rounded px-1.5 py-0.5 text-[9px] font-semibold uppercase tracking-wider [&_svg]:size-3",
        alpaca && !down ? "bg-good-bg text-good" : "bg-warning-bg text-warning",
        className,
      )}
    >
      <Icon name={alpaca ? "trade" : "gauge"} />
      {brokerLabel(broker)}
      {down && <span className="font-medium normal-case tracking-normal">· offline</span>}
    </span>
    </Tip>
  );
}

const usd = (v: number) => `$${v.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;

/** Buying power, shown only for Alpaca paper (the simulator does not enforce one). */
export function BuyingPower({ state, className }: { state: PaperState | null | undefined; className?: string }) {
  const bp = buyingPower(state);
  if (bp === null) return null;
  return (
    <p className={cn("m-0 flex items-baseline justify-between gap-2 text-[11px] text-text-faint", className)}>
      <span>Buying power</span>
      <span className="font-mono tabular-nums text-muted-foreground">{usd(bp)}</span>
    </p>
  );
}

/**
 * The latest refusal from Alpaca paper (a rejected order, a refused modify or cancel), shown
 * once for a few seconds over the chart. The simulator never sends one.
 */
export function BrokerNotice({ state, className }: { state: PaperState | null | undefined; className?: string }) {
  // Anything already in the first state this chart sees is old news.
  const [seenAt, setSeenAt] = useState(() => state?.notice?.at ?? 0);
  const notice = freshNotice(state, seenAt, Date.now());
  useEffect(() => {
    if (!notice) return;
    const t = setTimeout(() => setSeenAt(notice.at), 6000);
    return () => clearTimeout(t);
  }, [notice]);
  if (!notice) return null;
  return (
    <div
      role="status"
      className={cn(
        "flex max-w-[min(26rem,calc(100%-1rem))] items-start gap-2 rounded-lg bg-popover px-3 py-2 text-xs text-popover-foreground shadow-lg transition-[opacity,translate] duration-150 starting:translate-y-1 starting:opacity-0 [&_svg]:mt-0.5 [&_svg]:size-3.5 [&_svg]:shrink-0",
        className,
      )}
    >
      <span className="text-critical">
        <Icon name="bell" />
      </span>
      <span className="min-w-0 flex-1">{notice.text}</span>
      <button type="button" aria-label="Dismiss" onClick={() => setSeenAt(notice.at)} className="-m-1 inline-grid place-items-center rounded p-1 text-muted-foreground pointer-coarse:min-h-11 pointer-coarse:min-w-11 outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring">
        ✕
      </button>
    </div>
  );
}
