import type { PriceScaleType } from "@stockwatcher/chart-core";
import { useMemo, type RefObject } from "react";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
} from "./ui/dropdown-menu.tsx";

export const PRICE_SCALE_LABELS: Readonly<Record<PriceScaleType, string>> = {
  linear: "Linear (price)",
  percent: "Linear (%)",
  log: "Logarithmic",
};

export interface PriceAxisMenuProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Viewport (client) px to open at: the right-click or long-press point. */
  anchor: { x: number; y: number } | null;
  scaleType: PriceScaleType;
  onScaleTypeChange: (type: PriceScaleType) => void;
  /** Whether the range is auto-scaled now (the item then reads as current). */
  autoScaled: boolean;
  onAutoScale: () => void;
  /** Where focus returns on close (the chart, so keyboard users keep their place). */
  finalFocus?: RefObject<HTMLElement | null>;
}

/**
 * The price axis menu, Robinhood Legend's right-click y-axis menu cut to
 * what this chart has: auto-scale, and Linear (price) / Linear (%) /
 * Logarithmic. It opens at a point rather than under a trigger (a virtual
 * anchor), and it is a Base UI menu, so arrow keys, Enter, Escape and
 * type-ahead work and focus returns to the chart.
 */
export function PriceAxisMenu({ open, onOpenChange, anchor, scaleType, onScaleTypeChange, autoScaled, onAutoScale, finalFocus }: PriceAxisMenuProps) {
  const virtual = useMemo(() => {
    if (!anchor) return null;
    const { x, y } = anchor;
    return { getBoundingClientRect: () => DOMRect.fromRect({ x, y, width: 0, height: 0 }) };
  }, [anchor]);

  return (
    <DropdownMenu open={open && virtual !== null} onOpenChange={(next) => onOpenChange(next)}>
      <DropdownMenuContent anchor={virtual} side="bottom" align="end" sideOffset={2} finalFocus={finalFocus} aria-label="Price axis">
        <DropdownMenuItem onClick={onAutoScale}>
          Auto-scale price axis
          {autoScaled && <span className="text-xs text-muted-foreground">(on)</span>}
          <DropdownMenuShortcut>A</DropdownMenuShortcut>
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuRadioGroup value={scaleType} onValueChange={(v) => onScaleTypeChange(v as PriceScaleType)}>
          <DropdownMenuLabel>Scale</DropdownMenuLabel>
          {(Object.keys(PRICE_SCALE_LABELS) as PriceScaleType[]).map((t) => (
            <DropdownMenuRadioItem key={t} value={t} closeOnClick>
              {PRICE_SCALE_LABELS[t]}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
