import { Icon } from "@stockwatcher/chart-web";
import { Tip, TOUCH_HIT, cn } from "@stockwatcher/chart-web/ui";
import { useContext } from "react";
import { NavRailContext } from "./navRail.ts";

/**
 * Collapse / expand the main nav rail, at the far left of every page's top header (owner,
 * 2026-10-18: moved out of the rail's footer, where Settings now sits). Renders nothing without a
 * rail. A quiet 28 px square chip, like the header's other controls; 40 px with a 44 px touch area
 * on a coarse pointer (tablets).
 */
export function NavToggle() {
  const nav = useContext(NavRailContext);
  if (!nav) return null;
  const label = nav.collapsed ? "Expand menu" : "Collapse menu";
  return (
    <Tip label={label}>
      <button
        type="button"
        aria-label={label}
        aria-expanded={!nav.collapsed}
        onClick={nav.toggle}
        className={cn("relative inline-grid size-7 shrink-0 place-items-center rounded-lg text-muted-foreground outline-none transition-colors hover:bg-bg-row hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring pointer-coarse:size-10 [&_svg]:size-4", TOUCH_HIT)}
      >
        <Icon name={nav.collapsed ? "sidebarExpand" : "sidebarCollapse"} />
      </button>
    </Tip>
  );
}
