import { Icon } from "@stockwatcher/chart-web";
import { cn, Tip } from "@stockwatcher/chart-web/ui";
import type { ConfidenceView } from "@stockwatcher/shared";
import { Reveal } from "./Reveal.tsx";

/**
 * "High confidence" (owner, 2026-11-22; research v2 b-good-1, crates/server/src/bgood): the model's
 * top tier for this alert, decided 30 s after it fired. The chip therefore always arrives after its
 * row or log entry, so it opens by animating its width and opacity (Reveal), as the spread chip
 * does, instead of pushing its neighbours over in one jump.
 *
 * About 4 alerts in 10 are tagged, so the chip is a quiet raised fill with the two-tone research
 * mark (`insights`, every research detector's icon; the star is the favourites toggle) in the
 * text colour, never the accent (60/30/10: an accent on 40 % of the alerts would stop meaning
 * "look here"). Fills, never a border. The tooltip and the accessible name carry the sentence
 * (packages/shared confidence.ts), so the chip is never colour or an icon alone.
 *
 * Under 18rem of row (the Fold's cover screen, a squeezed column) the label shortens to "High" and
 * the mark stays, so a full marks line still fits without the row scrolling sideways.
 */
export function ConfidenceChip({ confidence, className, container = "row" }: { confidence: ConfidenceView | null; className?: string; container?: "row" | "log" }) {
  return (
    <Reveal show={confidence !== null} axis="x" as="span" className={cn("shrink-0", className)}>
      {confidence && (
        <Tip label={confidence.aria}>
          <span
            role="img"
            aria-label={confidence.aria}
            data-testid="confidence-chip"
            data-tier={confidence.tier}
            className="inline-flex items-center gap-1 rounded bg-bg-raised px-1.5 py-px font-sans text-[10px] font-medium whitespace-nowrap text-foreground [&_svg]:size-3"
          >
            <Icon name="insights" />
            {container === "row" ? (
              <>
                <span className="@[18rem]/row:hidden">{confidence.short}</span>
                <span className="hidden @[18rem]/row:inline">{confidence.label}</span>
              </>
            ) : (
              <>
                <span className="@lg/log:hidden">{confidence.short}</span>
                <span className="hidden @lg/log:inline">{confidence.label}</span>
              </>
            )}
          </span>
        </Tip>
      )}
    </Reveal>
  );
}
