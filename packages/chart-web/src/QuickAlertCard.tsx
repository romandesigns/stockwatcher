import { formatPrice } from "@stockwatcher/chart-core";
import { useMemo, type RefObject } from "react";
import { Popover, PopoverContent } from "./ui/popover.tsx";
import { cn } from "./ui/utils.ts";

export interface QuickAlertDraft {
  price: number;
  dir: "above" | "below";
}

export interface QuickAlertCardProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Viewport (client) px: where the hold finished, the bell, or Alt+A's crosshair. */
  anchor: { x: number; y: number } | null;
  symbol: string;
  draft: QuickAlertDraft | null;
  /** Last trade, for "+8.2%" next to the price. */
  last: number | null;
  /** The defaults a new alert gets, one line ("Once · this week · in app + phone"). */
  defaultsText?: string;
  onCreate: (draft: QuickAlertDraft) => void;
  /** "More…": the host's full form, prefilled with this draft. */
  onMore?: (draft: QuickAlertDraft) => void;
  finalFocus?: RefObject<HTMLElement | null>;
}

/**
 * The quick alert card (approved design, "Web and desktop: alerts by hand"): opened at the exact
 * price pressed (hold 0.5 s), the bell's or the crosshair's, with the direction already decided
 * (above the last price = crosses above). One action creates it with the defaults; "More…" hands
 * the draft to the full form. Enter creates (the form's submit), Esc closes (the popover's own).
 */
export function QuickAlertCard({ open, onOpenChange, anchor, symbol, draft, last, defaultsText = "Once · this week · in app + phone", onCreate, onMore, finalFocus }: QuickAlertCardProps) {
  const virtual = useMemo(() => {
    if (!anchor) return null;
    const { x, y } = anchor;
    return { getBoundingClientRect: () => DOMRect.fromRect({ x, y, width: 0, height: 0 }) };
  }, [anchor]);
  const pct = draft && last ? ((draft.price - last) / last) * 100 : null;
  return (
    <Popover open={open && virtual !== null && draft !== null} onOpenChange={(o) => onOpenChange(o)}>
      <PopoverContent anchor={virtual} side="bottom" align="start" sideOffset={10} finalFocus={finalFocus} aria-label={draft ? `New alert for ${symbol} at ${formatPrice(draft.price)}` : "New alert"} className="w-56">
        {draft && (
          <form
            className="flex flex-col gap-2"
            onSubmit={(e) => {
              e.preventDefault();
              onCreate(draft);
              onOpenChange(false);
            }}
          >
            <span className="text-[12.5px]">
              Alert when <span className="font-mono">{symbol}</span> crosses <b className="font-semibold">{draft.dir}</b>
            </span>
            <span className="font-mono text-lg font-semibold tabular-nums">
              {formatPrice(draft.price)}
              {pct !== null && <span className={cn("ml-1.5 text-[11.5px] font-normal", pct >= 0 ? "text-good" : "text-critical")}>{`${pct >= 0 ? "+" : "−"}${Math.abs(pct).toFixed(1)}%`}</span>}
            </span>
            <span className="text-[11.5px] text-muted-foreground">{defaultsText}</span>
            <div className="flex gap-1.5">
              {/* The one accent on the card: the primary action. */}
              <button type="submit" autoFocus className="h-[30px] flex-1 rounded-lg bg-primary pointer-coarse:h-11 text-xs font-semibold text-primary-foreground outline-none hover:bg-primary/90 focus-visible:ring-2 focus-visible:ring-ring">
                Create
              </button>
              {onMore && (
                <button
                  type="button"
                  onClick={() => {
                    onOpenChange(false);
                    onMore(draft);
                  }}
                  className="h-[30px] rounded-lg bg-bg-raised px-2.5 pointer-coarse:h-11 pointer-coarse:px-4 text-xs font-medium text-foreground outline-none hover:bg-bg-row focus-visible:ring-2 focus-visible:ring-ring"
                >
                  More…
                </button>
              )}
            </div>
          </form>
        )}
      </PopoverContent>
    </Popover>
  );
}
