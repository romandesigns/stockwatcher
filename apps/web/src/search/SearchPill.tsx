import { Icon } from "@stockwatcher/chart-web";
import { cn } from "@stockwatcher/chart-web/ui";
import { forwardRef } from "react";

const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);

/**
 * The header's ticker quick-open pill (VS Code's Ctrl+P look): centred in a page header, it opens
 * TickerSearch, which drops down into this pill's exact footprint (pass its ref as `anchor`).
 * Shared by the chart page and the dashboard so both headers look and behave the same.
 */
export const SearchPill = forwardRef<HTMLButtonElement, { onOpen: () => void; className?: string }>(function SearchPill({ onOpen, className }, ref) {
  return (
    <button
      ref={ref}
      type="button"
      onClick={onOpen}
      className={cn(
        "flex h-8 w-(--search-w) items-center gap-2 rounded-lg bg-card px-3 text-xs text-text-faint outline-none hover:bg-bg-row hover:text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring [&_svg]:size-3.5",
        className,
      )}
    >
      <Icon name="search" />
      <span>Search tickers</span>
      <kbd className="ml-auto rounded bg-bg-row px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground">{isMac ? "⌘P" : "Ctrl P"}</kbd>
    </button>
  );
});
