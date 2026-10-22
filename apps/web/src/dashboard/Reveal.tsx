import { cn } from "@stockwatcher/chart-web/ui";
import type { ReactNode } from "react";
import { useHeld } from "../lib/useHeld.ts";

/**
 * Content that opens and closes by animating its real size (owner, 2026-10-22: a panel or section
 * with 0 items is hidden, and every visible layout change animates). A one-track grid goes from
 * `1fr` to `0fr` (rows for `axis="y"`, columns for `"x"`) while it fades, so siblings move with it
 * instead of jumping. Reduced motion: the global rule in index.css makes it instant.
 *
 * While closing it keeps drawing the last children it had open, so a list that just emptied
 * shrinks away with its last rows instead of going blank first; closed content is inert and
 * hidden from assistive tech. Content that was never open is not rendered at all. The clip has a
 * small margin so focus rings at the content's edges are not cut off.
 */
export function Reveal({ show, axis = "y", as = "div", className, innerClassName, children }: { show: boolean; axis?: "x" | "y"; as?: "div" | "span"; className?: string; innerClassName?: string; children: ReactNode }) {
  const shown = useHeld<ReactNode>(children, show, null);
  // `span` inside phrasing content (a row that is a <button>): the same grid, valid markup.
  const Outer = as;
  const Inner = as;
  return (
    <Outer
      data-reveal={show ? "open" : "closed"}
      aria-hidden={show ? undefined : true}
      inert={!show}
      className={cn(
        "grid transition-[grid-template-rows,grid-template-columns,opacity,margin] duration-300 ease-out motion-reduce:transition-none",
        axis === "y" ? (show ? "grid-rows-[1fr]" : "grid-rows-[0fr]") : show ? "grid-cols-[1fr]" : "grid-cols-[0fr]",
        show ? "opacity-100" : "opacity-0",
        className,
      )}
    >
      <Inner className={cn("min-h-0 min-w-0 overflow-clip [overflow-clip-margin:4px]", as === "span" && "block", innerClassName)}>{shown}</Inner>
    </Outer>
  );
}
