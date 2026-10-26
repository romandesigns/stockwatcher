import { cn } from "@stockwatcher/chart-web/ui";
import { detectionAttention, detectionItemCount, panelsSettled, pickSideTab, rocketFreshCount, rocketItemCount, rossItemCount, visibleSideTabs, type DetectionChannel, type FavoritesChannel, type RocketChannel, type RossChannel, type SideTab } from "@stockwatcher/shared";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { useDetection } from "../feed/useDetection.ts";
import { useFavorites } from "../feed/useFavorites.ts";
import { FavoritesPanelView } from "../favorites/FavoritesPanel.tsx";
import { useFavoriteList } from "../favorites/useFavoriteList.ts";
import { useRocket } from "../feed/useRocket.ts";
import { useRossState } from "../feed/useRossState.ts";
import { DetectionPanelView } from "./DetectionPanel.tsx";
import { Reveal } from "./Reveal.tsx";
import { useHeld } from "../lib/useHeld.ts";
import { useTick } from "../lib/useTick.ts";
import { RocketPanelView } from "./RocketPanel.tsx";
import { RossPanelView } from "./RossPanel.tsx";

const KEY = "stockwatcher.sideTab";
/** How long the column waits for every channel's first word before deciding anyway. */
export const SETTLE_MS = 1500;

function readTab(): SideTab {
  try {
    const t = window.localStorage.getItem(KEY);
    return t === "rocket" || t === "signals" || t === "favorites" ? t : "ross";
  } catch {
    return "ross";
  }
}

/**
 * The tabs to draw: the live set once every channel has said whether it is on (or after
 * {@link SETTLE_MS}), the last decided set until then. So the column neither grows a tab per
 * channel as the first snapshots land nor collapses and reopens when a feed switch resets them.
 */
function useSettledTabs(live: SideTab[], settled: boolean): SideTab[] {
  const [timedOut, setTimedOut] = useState(false);
  useEffect(() => {
    if (settled) return;
    const id = setTimeout(() => setTimedOut(true), SETTLE_MS);
    return () => {
      clearTimeout(id);
      setTimedOut(false);
    };
  }, [settled]);
  // One array per distinct set, so holding it does not re-run the column on every frame.
  const key = live.join(",");
  const stable = useMemo(() => (key ? (key.split(",") as SideTab[]) : []), [key]);
  return useHeld(stable, settled || timedOut, NONE);
}
const NONE: SideTab[] = [];

/**
 * The right-hand column (owner, 2026-10-21; Ross split out 2026-10-26: "I want the ross panel to
 * be in its own panel"): the **Ross Panel** as its own card on top, always in view while it has
 * members, and under it the tabs **Runners | Signals | Favorites**, segmented and compact, the
 * choice remembered on this device. Ross takes its rows' height up to about half the column and
 * scrolls inside beyond that, so the tabs below always keep room. All three stay subscribed whatever tab is open,
 * so a tab can show a count without being opened: Runners its fresh flags, Signals (design canvas
 * "Detection Panel", 2026-10-22) the rows at the trigger or crossing it, with a pulsing dot.
 *
 * A panel with 0 items is hidden (owner, 2026-10-22): its tab is not offered, and off, not yet
 * known and empty all count as 0. If the open tab empties, the first non-empty one opens (for
 * this visit; the remembered choice is kept for the next). A lone tab draws no tab strip. With
 * every panel empty the whole column closes and the charts take its width. Each of those changes
 * animates by real size (Reveal); reduced motion makes them instant. A reconnect does not hide
 * anything: the channels keep their last rows (dimmed by each panel) until the new connection
 * says otherwise.
 */
export function SideTabs({ ross, rocket, detection, favorites, onPick, onEpisodePick, onEpisodeClear }: { ross: RossChannel | undefined; rocket: RocketChannel | undefined; detection: DetectionChannel | undefined; favorites?: FavoritesChannel; onPick: (symbol: string) => void; onEpisodePick: (symbol: string, episodeId: string, runEpoch: string) => void; onEpisodeClear?: () => void }) {
  const [picked, setPicked] = useState<SideTab>(readTab);
  const rossState = useRossState(ross);
  const rocketState = useRocket(rocket);
  const detected = useDetection(detection);
  const favoritesState = useFavorites(favorites);
  // The owner's list counts from the synced setting, before any row arrives (panels.ts).
  const favoriteList = useFavoriteList();
  const wall = useTick(15_000);
  const live = visibleSideTabs({
    ross: ross ? rossItemCount(rossState) : 0,
    rocket: rocket ? rocketItemCount(rocketState) : 0,
    signals: detection ? detectionItemCount(detected) : 0,
    favorites: favorites ? favoriteList.length : 0,
  });
  const settled = panelsSettled([ross && rossState.enabled, rocket && rocketState.enabled, detection && detected.enabled]);
  // Settled together, so neither Ross nor a tab appears before every channel has answered; then
  // Ross is its own card and the rest are the tabs.
  const shownPanels = useSettledTabs(live, settled);
  const rossShown = shownPanels.includes("ross");
  const visible = useMemo<SideTab[]>(() => shownPanels.filter((t) => t !== "ross"), [shownPanels]);
  const tab = pickSideTab(visible, picked);
  // The open tab emptied: the fallback stays open when the first one refills, so nothing switches
  // under the reader. Not written to storage: the device's choice is the user's, not the market's.
  if (tab && tab !== picked) setPicked(tab);
  const pick = (t: SideTab) => {
    setPicked(t);
    try {
      window.localStorage.setItem(KEY, t);
    } catch {
      // A private window: the choice lasts this session.
    }
  };
  const content = (t: SideTab | undefined): ReactNode =>
    t === "rocket" ? (
      <RocketPanelView state={rocketState} wallNow={wall} onPick={onPick} />
    ) : t === "signals" ? (
      <DetectionPanelView state={detected} onPick={onEpisodePick} onClear={onEpisodeClear} />
    ) : t === "favorites" ? (
      <FavoritesPanelView list={favoriteList} state={favoritesState} onPick={onPick} />
    ) : null;
  const has = (t: SideTab) => visible.includes(t);
  // Only from current data: while reconnecting the rows are last known, so nothing claims attention.
  const attention = detected.enabled === true && detected.connected && !detected.resyncing ? detectionAttention(detected.snapshot) : 0;
  const fresh = tab !== "rocket" ? rocketFreshCount(rocketState, wall) : 0;
  const open = visible.length > 0 || rossShown;
  return (
    // The column: beside the charts on a wide panel (its width animates to 0 when closed, the
    // charts' grid growing into it), under them on a narrow one (its height does). Its content
    // keeps a fixed 400px width there, so it is clipped, never reflowed, while it moves.
    <div
      data-side-column={open ? "open" : "closed"}
      aria-hidden={open ? undefined : true}
      inert={!open}
      className={cn(
        "grid min-h-0 min-w-0 shrink-0 overflow-clip [overflow-clip-margin:4px] transition-[grid-template-rows,width,margin,opacity] duration-300 ease-out motion-reduce:transition-none",
        open ? "mt-2 grid-rows-[1fr] opacity-100 @4xl/panel:mt-0 @4xl/panel:ml-2 @4xl/panel:w-[400px]" : "mt-0 grid-rows-[0fr] opacity-0 @4xl/panel:ml-0 @4xl/panel:w-0",
      )}
    >
      <div className="flex min-h-0 min-w-0 flex-col @4xl/panel:w-[400px]">
        <Reveal show={rossShown} className="max-h-[48%] shrink-0" innerClassName={cn("flex min-h-0 flex-col", visible.length > 0 && "pb-2")}>
          <RossPanelView state={rossState} detection={detection} onPick={onPick} onEpisodePick={onEpisodePick} />
        </Reveal>
        <Reveal show={visible.length > 1} innerClassName="pb-2">
          <div role="tablist" aria-label="Side panel" className="inline-flex rounded-[9px] bg-card p-0.5">
            
            <TabReveal show={has("rocket")}>
              <TabButton id="rocket" active={tab === "rocket"} onClick={() => pick("rocket")}>
                Runners
                {fresh > 0 && <CountBadge n={fresh} label={`${fresh} new`} />}
              </TabButton>
            </TabReveal>
            <TabReveal show={has("signals")}>
              <TabButton id="signals" active={tab === "signals"} onClick={() => pick("signals")}>
                Signals
                {attention > 0 && <CountBadge n={attention} label={`${attention} at the trigger or crossing it`} />}
              </TabButton>
            </TabReveal>
            <TabReveal show={has("favorites")}>
              <TabButton id="favorites" active={tab === "favorites"} onClick={() => pick("favorites")}>
                Favorites
              </TabButton>
            </TabReveal>
          </div>
        </Reveal>
        <SidePanel tab={tab} lone={visible.length <= 1}>
          {content(tab)}
        </SidePanel>
      </div>
    </div>
  );
}

/** The open tab's panel; a tab change (picked, or the open one emptied) fades the new panel in. */
function SidePanel({ tab, lone, children }: { tab: SideTab | undefined; lone: boolean; children: ReactNode }) {
  // Closing column: keep drawing the last panel while it shrinks away (Reveal's rule).
  const shownTab = useHeld(tab, tab !== undefined, undefined);
  const shown = useHeld(children, tab !== undefined, null);
  if (!shownTab) return null;
  return (
    <div
      key={shownTab}
      {...(lone ? {} : { role: "tabpanel", id: `side-panel-${shownTab}`, "aria-labelledby": `side-tab-${shownTab}` })}
      className="flex min-h-0 min-w-0 animate-panel-in flex-col"
    >
      {shown}
    </div>
  );
}

/** A tab that slides open or shut sideways, so its neighbours move instead of jumping. */
function TabReveal({ show, children }: { show: boolean; children: ReactNode }) {
  return (
    <Reveal show={show} axis="x">
      <div className="whitespace-nowrap">{children}</div>
    </Reveal>
  );
}

function TabButton({ id, active, onClick, children }: { id: SideTab; active: boolean; onClick: () => void; children: ReactNode }) {
  return (
    <button
      type="button"
      role="tab"
      id={`side-tab-${id}`}
      aria-controls={active ? `side-panel-${id}` : undefined}
      aria-selected={active}
      onClick={onClick}
      className={cn(
        "inline-flex h-7 items-center gap-1.5 rounded-[7px] px-2.5 text-[13px] font-semibold outline-none transition-colors duration-150 focus-visible:ring-2 focus-visible:ring-ring motion-reduce:transition-none",
        active ? "bg-bg-raised text-foreground" : "text-muted-foreground hover:text-foreground",
      )}
    >
      {children}
    </button>
  );
}

/** A pulsing dot and a count, nothing at zero. */
function CountBadge({ n, label }: { n: number; label: string }) {
  return (
    <span className="inline-flex items-center gap-1 font-mono text-[11px] text-primary" aria-label={label}>
      <span className="size-1.5 animate-pulse rounded-full bg-primary motion-reduce:animate-none" aria-hidden />
      {n}
    </span>
  );
}
