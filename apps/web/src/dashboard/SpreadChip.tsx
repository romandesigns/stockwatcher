import { cn, Tip } from "@stockwatcher/chart-web/ui";
import type { SpreadTone, SpreadView } from "@stockwatcher/shared";
import { Reveal } from "./Reveal.tsx";

/**
 * The cost-band fills (packages/shared spread.ts): status colours only, never the accent. A
 * normal spread is a quiet raised chip; a warning or danger one takes its status tint, so the
 * chip draws the eye only when the cost is the news. Fills, never a border.
 */
const LOOK: Record<SpreadTone, string> = {
  muted: "bg-bg-raised text-muted-foreground",
  warning: "bg-warning-bg text-warning",
  critical: "bg-critical-bg text-critical font-semibold",
};

/**
 * "Spread 0.42%" (owner, 2026-11-19): what the alert cost to trade at fire time, from the server's
 * NBBO (crates/server/src/spread.rs). The server sends it a moment after the row appears (it asks
 * for the quote once the alert fired), so the chip opens by animating its width and opacity
 * (Reveal) instead of pushing its neighbours over in one jump. No spread shows nothing. The
 * tooltip and the accessible name say the band in words ("costly to trade"), so colour is never
 * the only signal.
 *
 * In a row narrower than 18rem (the Fold's cover screen, a squeezed column) the word "Spread"
 * drops and the figure stays ("1.6%", the row's container query), so a full marks line still fits
 * without the row scrolling sideways; the accessible name keeps the whole sentence.
 */
export function SpreadChip({ spread, className }: { spread: SpreadView | null; className?: string }) {
  // Reveal keeps drawing the last open chip while it closes, so it shrinks away, never blanks.
  return (
    <Reveal show={spread !== null} axis="x" as="span" className={cn("shrink-0", className)}>
      {spread && (
        <Tip label={spread.aria}>
          <span role="img" aria-label={spread.aria} data-testid="spread-chip" data-band={spread.band} className={cn("inline-flex items-center rounded px-1.5 py-px font-mono text-[10px] whitespace-nowrap tabular-nums transition-colors duration-300 motion-reduce:transition-none", LOOK[spread.tone])}>
            <span className="hidden @[18rem]/row:inline">Spread&nbsp;</span>
            {spread.value}
          </span>
        </Tip>
      )}
    </Reveal>
  );
}
