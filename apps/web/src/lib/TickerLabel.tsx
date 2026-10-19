import { cn, TOUCH_HIT } from "@stockwatcher/chart-web/ui";
import { forwardRef, type ComponentPropsWithoutRef } from "react";
import { useTickerPress } from "./useTickerPress.ts";

/**
 * A ticker shown as a button: a tap does `onTap` (open the search, load it into a chart), tap
 * then press and hold copies it (useTickerPress). Forwards its ref and extra props, so a Tip can
 * wrap it. It looks like text; on a coarse pointer TOUCH_HIT gives the finger a 44 px area around it.
 */
export const TickerLabel = forwardRef<HTMLButtonElement, { symbol: string; onTap: () => void; className?: string } & Omit<ComponentPropsWithoutRef<"button">, "onClick">>(function TickerLabel(
  { symbol, onTap, className, ...rest },
  ref,
) {
  const press = useTickerPress(symbol, onTap);
  return (
    <button
      ref={ref}
      type="button"
      {...rest}
      {...press}
      className={cn("pointer-events-auto relative cursor-pointer touch-manipulation rounded-md font-semibold outline-none select-none hover:text-primary focus-visible:ring-2 focus-visible:ring-ring", TOUCH_HIT, className)}
    >
      {symbol}
    </button>
  );
});
