import { Icon } from "@stockwatcher/chart-web";
import { cn, Tip } from "@stockwatcher/chart-web/ui";
import type { QuoteWarningView } from "@stockwatcher/shared";
import { Reveal } from "./Reveal.tsx";

/**
 * "No live quote" (owner, 2026-11-22; research v2 W2-REPORT §2, packages/shared quoteWarning.ts):
 * the alert fired with no usable bid/ask in the minute before it, and such alerts were GOOD about
 * half as often (11.5 % vs 21.5 %). The server learns it when it asks for the fire-time quote, a
 * moment after the row appears, so the chip opens by animating its width and opacity (Reveal), as
 * the spread chip does, instead of pushing its neighbours over in one jump. It takes the spread
 * chip's place: a row has one or the other, never both.
 *
 * A caution, so the warning status tint and the two-tone `quoteOff` mark, never the accent
 * (60/30/10). Fills, never a border. The tooltip and the accessible name carry the sentence, so
 * the chip is never colour or an icon alone. Under 18rem of row (the Fold's cover screen, a
 * squeezed column) or a narrow log it says "No quote".
 */
export function NoQuoteChip({ warning, className, container = "row" }: { warning: QuoteWarningView | null; className?: string; container?: "row" | "log" }) {
  return (
    <Reveal show={warning !== null} axis="x" as="span" className={cn("shrink-0", className)}>
      {warning && (
        <Tip label={warning.aria}>
          <span role="img" aria-label={warning.aria} data-testid="no-quote-chip" className="inline-flex items-center gap-1 rounded bg-warning-bg px-1.5 py-px font-sans text-[10px] font-medium whitespace-nowrap text-warning [&_svg]:size-3">
            <Icon name="quoteOff" />
            {container === "row" ? (
              <>
                <span className="@[18rem]/row:hidden">{warning.short}</span>
                <span className="hidden @[18rem]/row:inline">{warning.label}</span>
              </>
            ) : (
              <>
                <span className="@lg/log:hidden">{warning.short}</span>
                <span className="hidden @lg/log:inline">{warning.label}</span>
              </>
            )}
          </span>
        </Tip>
      )}
    </Reveal>
  );
}
