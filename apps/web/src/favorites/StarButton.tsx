import { Icon } from "@stockwatcher/chart-web";
import { cn, Tip, TOUCH_HIT } from "@stockwatcher/chart-web/ui";
import { getAppStore } from "../state/store.ts";
import { useFavoriteList } from "./useFavoriteList.ts";

/**
 * Adds a ticker to the Favorites panel, or takes it off (owner, 2026-10-26). The list is the
 * synced `favorites` setting, so the star turns on every device at once. On, it spends the accent
 * (an active state, and a small one); off, it is a quiet outline. Never inside another button:
 * hosts put it beside a row, not in it.
 */
export function StarButton({ symbol, className, quiet = false }: { symbol: string; className?: string; quiet?: boolean }) {
  const on = useFavoriteList().includes(symbol);
  const label = on ? `Remove ${symbol} from favorites` : `Add ${symbol} to favorites`;
  return (
    <Tip label={on ? "In favorites" : "Add to favorites"}>
      <button
        type="button"
        aria-label={label}
        aria-pressed={on}
        onClick={(e) => {
          e.stopPropagation();
          getAppStore().getState().toggleFavorite(symbol);
        }}
        onPointerDown={(e) => e.stopPropagation()}
        className={cn(
          "relative inline-grid size-7 shrink-0 place-items-center rounded-md outline-none transition-colors duration-150 focus-visible:ring-2 focus-visible:ring-ring motion-reduce:transition-none [&_svg]:size-3.5",
          // `quiet`: inside the Favorites panel every row is on, so its stars stay neutral and the
          // accent keeps meaning something there.
          on && !quiet ? "text-primary hover:bg-primary/10" : on ? "text-muted-foreground hover:bg-bg-raised hover:text-foreground" : "text-text-faint hover:bg-bg-raised hover:text-foreground",
          TOUCH_HIT,
          className,
        )}
      >
        <Icon name={on ? "starOn" : "star"} />
      </button>
    </Tip>
  );
}
