import { cn, TOUCH_HIT_Y } from "@stockwatcher/chart-web/ui";
import {
  BOTTOM_PANELS,
  detectionItemCount,
  hodItemCount,
  moversCount,
  panelsSettled,
  rocketItemCount,
  rossItemCount,
  SIDE_PANELS,
  spansPair,
  visiblePanels,
  type BottomPanel,
  type DashboardPanel,
  type DetectionChannel,
  type FavoritesChannel,
  type HodChannel,
  type PressureChannel,
  type MoversChannel,
  type RocketChannel,
  type RossChannel,
  type SidePanel,
} from "@stockwatcher/shared";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent,
  type PointerEvent,
  type ReactNode,
  type RefObject,
} from "react";
import { FavoritesPanelView } from "../favorites/FavoritesPanel.tsx";
import { useFavoriteList } from "../favorites/useFavoriteList.ts";
import { useDetection } from "../feed/useDetection.ts";
import { useFavorites } from "../feed/useFavorites.ts";
import { useHod } from "../feed/useHod.ts";
import {
  usePressureBoard,
  usePressureStatus,
} from "../feed/usePressureBoard.ts";
import { useMovers } from "../feed/useMovers.ts";
import { useRocket } from "../feed/useRocket.ts";
import { useRossState } from "../feed/useRossState.ts";
import { useHeld } from "../lib/useHeld.ts";
import { DetectionPanelView } from "./DetectionPanel.tsx";
import { HaltWatcherView } from "./HaltWatcher.tsx";
import { HodPanelView } from "./HodPanel.tsx";
import { MoversPanelView } from "./MoversPanel.tsx";
import { RocketPanelView } from "./RocketPanel.tsx";
import { RossPanelView } from "./RossPanel.tsx";

/** How long the layout waits for every channel's first word before deciding anyway. */
export const SETTLE_MS = 1500;
/** How long a panel or group takes to open or close (the transitions below). */
const MOTION_MS = 300;
/** The bottom row's height until the owner drags it: about a third of a laptop screen. */
const BOTTOM_DEFAULT = "clamp(15rem, 32vh, 24rem)";
const BOTTOM_KEY = "stockwatcher.bottomRowHeight";
/** Drag limits: the row keeps a few rows visible, and never takes more than 65% from the charts. */
const BOTTOM_MIN = 160;
const BOTTOM_MAX_SHARE = 0.65;

/**
 * The panels to draw: the live set once every channel has said whether it is on (or after
 * {@link SETTLE_MS}), the last decided set until then. So the layout neither grows a panel per
 * channel as the first snapshots land nor collapses and reopens when a feed switch resets them.
 */
function useSettled<P extends DashboardPanel>(
  live: P[],
  settled: boolean,
  timedOut: boolean,
): P[] {
  // One array per distinct set, so holding it does not re-run the layout on every frame.
  const key = live.join(",");
  const stable = useMemo(() => (key ? (key.split(",") as P[]) : []), [key]);
  return useHeld(stable, settled || timedOut, NONE as P[]);
}
const NONE: DashboardPanel[] = [];

function useSettleTimeout(settled: boolean): boolean {
  const [timedOut, setTimedOut] = useState(false);
  useEffect(() => {
    if (settled) return;
    const id = setTimeout(() => setTimedOut(true), SETTLE_MS);
    return () => {
      clearTimeout(id);
      setTimedOut(false);
    };
  }, [settled]);
  return timedOut;
}

/**
 * The group's panels to keep drawn: the shown ones, plus the ones that just left for as long as
 * they take to animate away (`show: false`), each in the group's fixed order.
 */
function usePresence<P extends DashboardPanel>(
  order: readonly P[],
  shown: P[],
): { id: P; show: boolean }[] {
  const [prev, setPrev] = useState(shown);
  const [leaving, setLeaving] = useState<P[]>([]);
  if (prev !== shown) {
    const gone = prev.filter((p) => !shown.includes(p));
    setLeaving((l) => [...l.filter((p) => !shown.includes(p)), ...gone]);
    setPrev(shown);
  }
  useEffect(() => {
    if (leaving.length === 0) return;
    const id = setTimeout(() => setLeaving([]), MOTION_MS);
    return () => clearTimeout(id);
  }, [leaving]);
  return order
    .filter((p) => shown.includes(p) || leaving.includes(p))
    .map((p) => ({ id: p, show: shown.includes(p) }));
}

export interface DashboardPanelsProps {
  ross: RossChannel | undefined;
  rocket: RocketChannel | undefined;
  detection: DetectionChannel | undefined;
  favorites?: FavoritesChannel;
  pressure?: PressureChannel;
  movers?: MoversChannel;
  /** The HOD Momentum scanner (bottom row, beside the Ross Panel). */
  hod?: HodChannel;
  onPick: (symbol: string) => void;
  onEpisodePick: (symbol: string, episodeId: string, runEpoch: string) => void;
  onEpisodeClear?: () => void;
  /** The charts grid (or the live market's empty note): the layout's main area. */
  children: ReactNode;
}

/**
 * The dashboard's layout around the charts (owner, 2026-10-30 wireframe; web and desktop, the
 * phone app keeps its own). Laid out by the dashboard panel's own width (`@container/panel`):
 *
 * - **Wide (≥ @4xl/panel):** the charts in the middle; a **right column** (400px by default)
 *   with Favorites, Signals and Top Gainers stacked, all visible at once, sharing its height; and
 *   a **bottom row** under both, full width, with Ross Panel, Runners, Halt watcher and Hot stocks
 *   side by side at equal widths. Every panel scrolls inside its own area. Three kinds of boundary
 *   can be dragged (or moved with the arrow keys; double-click resets), each remembered on this
 *   device, never synced (owner, 2026-10-30): the bottom row's top edge (its height), the right
 *   column's left edge (its width, 280–640px, the charts keeping their 18rem), and the boundaries
 *   between the column's panels and between the bottom row's (their share of its height or width;
 *   a hidden panel's share goes to the others and comes back with it).
 * - **Narrower:** one column: the charts, then the right column's panels, then the bottom row's,
 *   each group two to a row where they fit (one on a phone), the lone last panel of an odd group
 *   spanning the pair so no group ends on a gap.
 *
 * A panel with 0 items is hidden (owner, 2026-10-22; panels.ts): off, not yet known and empty all
 * count as 0. The rest share the space evenly; a group with none closes (the charts take its
 * width or height). Each change animates by real size (flex-grow, grid tracks), and reduced motion
 * makes it instant. Nothing is decided until every channel has said whether it is on (or
 * {@link SETTLE_MS} passed), so the first snapshots do not open the layout a panel at a time. A
 * reconnect hides nothing: the channels keep their last rows, dimmed by each panel.
 *
 * Every channel stays subscribed while the dashboard is open, so a panel can appear the moment it
 * has rows. Top Gainers and Hot stocks come ranked from the server's movers channel; the whole
 * quote board is back to the ticker search alone (subscribed only while it is open).
 */
export function DashboardPanels({
  ross,
  rocket,
  detection,
  favorites,
  pressure,
  movers,
  hod,
  onPick,
  onEpisodePick,
  onEpisodeClear,
  children,
}: DashboardPanelsProps) {
  const rossState = useRossState(ross);
  const rocketState = useRocket(rocket);
  const detected = useDetection(detection);
  const favoritesState = useFavorites(favorites);
  // The owner's list counts from the synced setting, before any row arrives (panels.ts).
  const favoriteList = useFavoriteList();
  const pressureBoard = usePressureBoard(pressure);
  const pressureStatus = usePressureStatus(pressure);
  const moversState = useMovers(movers);
  const hodState = useHod(hod);
  // No wall-clock tick: the ages are market ages, and useRocket re-renders on each of the
  // server's clock frames (clock.v1, rocket.ts CLOCK_RENDER_STEP_MS), which the panels read.
  const runners = rocket ? rocketState : undefined;

  const counts: Record<DashboardPanel, number> = {
    favorites: favorites ? favoriteList.length : 0,
    signals: detection ? detectionItemCount(detected) : 0,
    gainers: movers ? moversCount(moversState, "gainers") : 0,
    ross: ross ? rossItemCount(rossState) : 0,
    rocket: rocket ? rocketItemCount(rocketState) : 0,
    halt: pressure ? pressureBoard.length : 0,
    hot: movers ? moversCount(moversState, "hot") : 0,
    hod: hod ? hodItemCount(hodState) : 0,
  };
  // The movers and the Halt watcher have no on/off: they have answered once their first frame
  // landed (the movers hold rows; the pressure status records a frame).
  const settled = panelsSettled([
    ross && rossState.enabled,
    rocket && rocketState.enabled,
    detection && detected.enabled,
    pressure &&
      (pressureStatus.connected || pressureStatus.asOf !== null ? true : null),
    movers && (moversState.rows !== null ? true : null),
    // An older server without the scanner never answers: it is off, not pending.
    hod && (!hodState.available ? true : hodState.policy !== null ? true : null),
  ]);
  const timedOut = useSettleTimeout(settled);
  const side = useSettled(
    visiblePanels(SIDE_PANELS, counts),
    settled,
    timedOut,
  );
  const bottom = useSettled(
    visiblePanels(BOTTOM_PANELS, counts),
    settled,
    timedOut,
  );
  const sideSlots = usePresence(SIDE_PANELS, side);
  const bottomSlots = usePresence(BOTTOM_PANELS, bottom);
  const sideOpen = side.length > 0;
  const bottomOpen = bottom.length > 0;

  const content = (p: DashboardPanel): ReactNode =>
    p === "favorites" ? (
      <FavoritesPanelView
        list={favoriteList}
        state={favoritesState}
        onPick={onPick}
        runners={runners}
      />
    ) : p === "signals" ? (
      <DetectionPanelView
        state={detected}
        onPick={onEpisodePick}
        onClear={onEpisodeClear}
      />
    ) : p === "gainers" ? (
      <MoversPanelView
        kind="gainers"
        state={moversState}
        runners={runners}
        onPick={onPick}
      />
    ) : p === "ross" ? (
      <RossPanelView
        state={rossState}
        detection={detection}
        onPick={onPick}
        onEpisodePick={onEpisodePick}
        runners={runners}
      />
    ) : p === "hod" ? (
      <HodPanelView state={hodState} onPick={onPick} />
    ) : p === "rocket" ? (
      <RocketPanelView state={rocketState} onPick={onPick} />
    ) : p === "halt" ? (
      <HaltWatcherView
        board={pressureBoard}
        status={pressureStatus}
        onPick={onPick}
      />
    ) : (
      <MoversPanelView
        kind="hot"
        state={moversState}
        runners={runners}
        onPick={onPick}
      />
    );

  const layoutRef = useRef<HTMLDivElement>(null);
  const resize = useBottomResize(layoutRef);
  const sideResize = useSideResize(layoutRef);
  const splits = useSplits(layoutRef, SPLITS_KEY, SIDE_PANELS, "y");
  const bottomSplits = useSplits(layoutRef, BOTTOM_SPLITS_KEY, BOTTOM_PANELS, "x");
  const sideW = sideResize.width ?? SIDE_DEFAULT;
  const style = {
    // The column's track is its content plus the 8px gap to the charts; its content keeps that
    // width while the track animates to 0, so the panels are clipped rather than reflowed.
    "--side-w": sideOpen ? `${sideW + SIDE_GAP}px` : "0px",
    "--side-content": `${sideW}px`,
    "--bottom-h": bottomOpen
      ? resize.height === null
        ? BOTTOM_DEFAULT
        : `${resize.height}px`
      : "0px",
  } as CSSProperties;
  // The first visible side panel has no boundary above it to drag.
  const firstSide = sideSlots.find((s) => s.show)?.id;
  const firstBottom = bottomSlots.find((s) => s.show)?.id;

  return (
    <div
      ref={layoutRef}
      data-dashboard-layout
      data-resizing={
        resize.dragging || sideResize.dragging || splits.dragging || bottomSplits.dragging || undefined
      }
      style={style}
      className={cn(
        // Narrow: one column, the charts first. Wide: charts | right column over the bottom row.
        "relative flex flex-1 flex-col",
        "@4xl/panel:grid @4xl/panel:grid-cols-[minmax(0,1fr)_var(--side-w)] @4xl/panel:grid-rows-[minmax(0,1fr)_var(--bottom-h)]",
        // The open/close of each group moves its track; while the owner drags a boundary, nothing lags.
        "transition-[grid-template-columns,grid-template-rows] duration-300 ease-out motion-reduce:transition-none data-resizing:transition-none",
      )}
    >
      {/* Narrow: the charts take most of the screen (18rem a row at least) and the panels follow
          below, so the panel scrolls. Wide: the chart rows shrink to fit (Dashboard's GRID), so
          nothing scrolls but each panel's own list. */}
      <div className="flex min-h-[max(18rem,70svh)] min-w-0 flex-1 flex-col @4xl/panel:col-start-1 @4xl/panel:row-start-1 @4xl/panel:min-h-auto">
        {children}
      </div>

      {sideOpen && (
        // The right column's left edge (the gap to the charts): drag to resize its width (wide
        // only), arrow keys too, double-click resets. Remembered on this device.
        <Separator
          orientation="vertical"
          label="Resize the right column"
          value={Math.round(sideW)}
          min={SIDE_MIN}
          max={SIDE_MAX}
          handlers={sideResize}
          className="@4xl/panel:col-start-2 @4xl/panel:row-start-1 @4xl/panel:w-2 @4xl/panel:justify-self-start @4xl/panel:self-stretch"
        />
      )}

      <PanelGroup group="side" open={sideOpen}>
        {sideSlots.map(({ id, show }, i) => (
          <PanelSlot
            key={id}
            panel={id}
            axis="side"
            show={show || !sideOpen}
            span={spansPair(i, sideSlots.length)}
            grow={splits.weight(id)}
            handle={
              show &&
              sideOpen &&
              id !== firstSide && (
                <Separator
                  orientation="horizontal"
                  label={`Resize ${SIDE_LABEL[id]} against the panel above`}
                  value={splits.share(sideSlots, id)}
                  min={0}
                  max={100}
                  handlers={splits.handlers(id, sideSlots)}
                  // In the slot's leading 8px gap, over its full width.
                  className="@4xl/panel:absolute @4xl/panel:inset-x-0 @4xl/panel:top-0"
                />
              )
            }
          >
            {content(id)}
          </PanelSlot>
        ))}
      </PanelGroup>

      {bottomOpen && (
        // The bottom row's top edge: drag to resize (wide only). A separator, so the keyboard and
        // assistive tech can reach it; its hit area grows to 44px on touch.
        <Separator
          orientation="horizontal"
          label="Resize the bottom panels"
          value={resize.height === null ? undefined : Math.round(resize.height)}
          min={BOTTOM_MIN}
          handlers={resize}
          className="@4xl/panel:col-[1/-1] @4xl/panel:row-start-2 @4xl/panel:self-start"
        />
      )}

      <PanelGroup group="bottom" open={bottomOpen}>
        {bottomSlots.map(({ id, show }, i) => (
          <PanelSlot
            key={id}
            panel={id}
            axis="bottom"
            show={show || !bottomOpen}
            span={spansPair(i, bottomSlots.length)}
            grow={bottomSplits.weight(id)}
            handle={
              show &&
              bottomOpen &&
              id !== firstBottom && (
                <Separator
                  orientation="vertical"
                  label={`Resize ${BOTTOM_LABEL[id]} against the panel on its left`}
                  value={bottomSplits.share(bottomSlots, id)}
                  min={0}
                  max={100}
                  handlers={bottomSplits.handlers(id, bottomSlots)}
                  // In the slot's leading 8px gap, over its full height.
                  className="@4xl/panel:absolute @4xl/panel:inset-y-0 @4xl/panel:left-0"
                />
              )
            }
          >
            {content(id)}
          </PanelSlot>
        ))}
      </PanelGroup>
    </div>
  );
}

const SIDE_LABEL: Record<SidePanel, string> = {
  favorites: "Favorites",
  signals: "Signals",
  gainers: "Top Gainers",
};

const BOTTOM_LABEL: Record<BottomPanel, string> = {
  ross: "Ross Panel",
  hod: "HOD Momentum",
  rocket: "Runners",
  halt: "Halt watcher",
  hot: "Hot stocks",
};

/** What a drag hook hands its separator. */
interface DragHandlers {
  dragging: boolean;
  onPointerDown: (e: PointerEvent<HTMLDivElement>) => void;
  onPointerMove: (e: PointerEvent<HTMLDivElement>) => void;
  onPointerUp: (e: PointerEvent<HTMLDivElement>) => void;
  onKeyDown: (e: KeyboardEvent<HTMLDivElement>) => void;
  reset: () => void;
}

/**
 * A layout boundary the owner can drag (wide layout only; hidden when the panels stack). An
 * invisible 8px hit area in the gap between two regions, never a border or divider: a short pill
 * fills in on hover, focus or drag. Its hit area grows to 44px on touch. `orientation` is the
 * boundary line's (ARIA): `horizontal` resizes heights, `vertical` widths.
 */
function Separator({
  orientation,
  label,
  value,
  min,
  max,
  handlers: h,
  className,
}: {
  orientation: "horizontal" | "vertical";
  label: string;
  value?: number;
  min?: number;
  max?: number;
  handlers: DragHandlers;
  className?: string;
}) {
  const across = orientation === "vertical";
  return (
    <div
      role="separator"
      aria-orientation={orientation}
      aria-label={label}
      aria-valuenow={value}
      aria-valuemin={min}
      aria-valuemax={max}
      tabIndex={0}
      onPointerDown={h.onPointerDown}
      onPointerMove={h.onPointerMove}
      onPointerUp={h.onPointerUp}
      onPointerCancel={h.onPointerUp}
      onKeyDown={h.onKeyDown}
      onDoubleClick={h.reset}
      data-dragging={h.dragging || undefined}
      className={cn(
        "group relative z-10 hidden touch-none outline-none @4xl/panel:flex @4xl/panel:items-center @4xl/panel:justify-center",
        across
          ? cn("w-2 cursor-col-resize", TOUCH_HIT_X)
          : cn("h-2 cursor-row-resize", TOUCH_HIT_Y),
        className,
      )}
    >
      <span
        className={cn(
          "rounded-full bg-transparent transition-colors duration-150 group-hover:bg-bg-raised group-focus-visible:bg-ring group-data-dragging:bg-bg-raised motion-reduce:transition-none",
          across ? "h-10 w-1" : "h-1 w-10",
        )}
        aria-hidden
      />
    </div>
  );
}

/** TOUCH_HIT_Y's twin for a vertical boundary: the tap area grows sideways to 44px on touch. */
const TOUCH_HIT_X =
  "pointer-coarse:after:absolute pointer-coarse:after:inset-y-0 pointer-coarse:after:inset-x-[min(0px,calc((100%_-_44px)/2))]";

/**
 * One group of panels: the right column or the bottom row. Wide, it fills its grid track (which
 * animates to 0 when the group closes, the charts growing into it) and keeps its content's size
 * while it moves, so the panels are clipped rather than reflowed. Narrow, it opens and closes by
 * its real height, and lays its panels out two to a row where they fit.
 */
function PanelGroup({
  group,
  open,
  children,
}: {
  group: "side" | "bottom";
  open: boolean;
  children: ReactNode;
}) {
  const side = group === "side";
  return (
    <div
      data-panel-group={group}
      data-open={open ? "open" : "closed"}
      aria-hidden={open ? undefined : true}
      inert={!open}
      className={cn(
        "grid min-h-0 min-w-0 transition-[grid-template-rows,opacity] duration-300 ease-out motion-reduce:transition-none",
        open ? "grid-rows-[1fr] opacity-100" : "grid-rows-[0fr] opacity-0",
        "@4xl/panel:grid-rows-[1fr]",
        side
          ? "@4xl/panel:col-start-2 @4xl/panel:row-start-1"
          : "@4xl/panel:col-[1/-1] @4xl/panel:row-start-2",
      )}
    >
      <div
        className={cn(
          "min-h-0 min-w-0 overflow-clip [overflow-clip-margin:4px]",
          "pt-2",
          side ? "@4xl/panel:pt-0 @4xl/panel:pl-2" : "@4xl/panel:pt-2",
        )}
      >
        <div
          className={cn(
            // Narrow: a grid of up to two columns by the panel's width; one on a phone.
            "grid grid-cols-1 gap-2 @2xl/panel:grid-cols-2",
            // Wide: a flex line whose items share it by their weights; each brings its own leading
            // gap (so a closing panel takes its gap with it), cancelled once here.
            side
              ? "@4xl/panel:-mt-2 @4xl/panel:flex @4xl/panel:h-[calc(100%+0.5rem)] @4xl/panel:w-(--side-content) @4xl/panel:flex-col @4xl/panel:gap-0"
              : "@4xl/panel:-ml-2 @4xl/panel:flex @4xl/panel:h-full @4xl/panel:w-[calc(100%+0.5rem)] @4xl/panel:flex-row @4xl/panel:gap-0",
          )}
        >
          {children}
        </div>
      </div>
    </div>
  );
}

/**
 * One panel in its group. It enters from nothing and leaves to nothing by its real size: wide by
 * flex-grow (its neighbours sliding to share the space), narrow by its height. While leaving it
 * keeps drawing its last rows. Narrow, a panel is capped in height and scrolls inside.
 *
 * `grow` is its share of its group (the owner's dragged splits): a hidden panel grows to 0
 * and the others share its space by their own weights, and its weight applies again when it
 * returns. Flex-grow means nothing in the narrow grid, so it only acts wide. `handle` is the
 * draggable boundary before it (in its leading gap: above it in the column, on its left in the
 * bottom row), drawn only while it is shown.
 */
function PanelSlot({
  panel,
  axis,
  show,
  span,
  grow,
  handle,
  children,
}: {
  panel: SidePanel | BottomPanel;
  axis: "side" | "bottom";
  show: boolean;
  span: boolean;
  grow?: number;
  handle?: ReactNode;
  children: ReactNode;
}) {
  const shown = useHeld<ReactNode>(children, show, null);
  return (
    <div
      data-slot-panel={panel}
      data-show={show ? "open" : "closed"}
      aria-hidden={show ? undefined : true}
      inert={!show}
      style={grow !== undefined ? { flexGrow: show ? grow : 0 } : undefined}
      className={cn(
        "relative grid min-h-0 min-w-0 transition-[grid-template-rows,flex-grow,opacity,padding] duration-300 ease-out motion-reduce:transition-none",
        show ? "grid-rows-[1fr] opacity-100" : "grid-rows-[0fr] opacity-0",
        "starting:grid-rows-[0fr] starting:opacity-0",
        span && "@2xl/panel:col-span-2",
        "@4xl/panel:basis-0 @4xl/panel:grid-rows-[1fr] @4xl/panel:starting:grow-0",
        show ? "@4xl/panel:grow" : "@4xl/panel:grow-0",
        axis === "side"
          ? show
            ? "@4xl/panel:pt-2"
            : "@4xl/panel:pt-0"
          : show
            ? "@4xl/panel:pl-2"
            : "@4xl/panel:pl-0",
        axis === "side"
          ? "@4xl/panel:starting:pt-0"
          : "@4xl/panel:starting:pl-0",
        // While the owner drags a split the panels follow the pointer without easing.
        "in-data-resizing:transition-none",
      )}
    >
      {handle}
      <div className="flex max-h-[26rem] min-h-0 min-w-0 flex-col overflow-clip [overflow-clip-margin:4px] *:min-h-0 *:flex-1 @4xl/panel:max-h-none">
        {shown}
      </div>
    </div>
  );
}

// ---- Remembered sizes (per device: screen layout is never synced across devices) -------------

/** The right column's content width until the owner drags it (the old side tabs' 400px). */
const SIDE_DEFAULT = 400;
/** The gap between the charts and the column (its track holds both). */
const SIDE_GAP = 8;
/** Drag limits: a panel row stays readable at 280px; past 640px the charts lose too much. */
const SIDE_MIN = 280;
const SIDE_MAX = 640;
/** The charts keep at least their 18rem minimum beside the column. */
const CHARTS_MIN = 288;
const SIDE_KEY = "stockwatcher.sideColumnWidth";
/** The right column's panels' weights (`{favorites: 1.4, …}`); absent = equal. */
const SPLITS_KEY = "stockwatcher.sideColumnSplits";
/** The bottom row's panels' weights, the same way. */
const BOTTOM_SPLITS_KEY = "stockwatcher.bottomRowSplits";
/** A panel keeps at least this much of its group's axis (its title and a couple of rows) when dragged. */
const SPLIT_MIN = 96;

/** A remembered number (px), or null when absent, unreadable or under `min`. */
function readNumber(key: string, min: number): number | null {
  try {
    const raw = window.localStorage.getItem(key);
    const v = raw === null ? Number.NaN : Number(raw);
    return Number.isFinite(v) && v >= min ? v : null;
  } catch {
    return null;
  }
}

function writeNumber(key: string, v: number | null): void {
  try {
    if (v === null) window.localStorage.removeItem(key);
    else window.localStorage.setItem(key, String(Math.round(v)));
  } catch {
    // A private window: the size lasts this session.
  }
}

/**
 * One draggable length along an axis, shared by the bottom row's height and the right column's
 * width: it follows the pointer (one layout per frame, transitions off while dragging), clamped by
 * `clamp`, and is remembered under `key` on this device. Arrow keys move it 16px; double-click goes
 * back to the default (null). `grow` turns a pointer delta into growth (dragging up makes the
 * bottom row taller; dragging left makes the column wider).
 */
function useLength(
  key: string,
  min: number,
  clamp: (v: number) => number,
  current: () => number,
  grow: (
    start: { x: number; y: number },
    e: { clientX: number; clientY: number },
  ) => number,
  keys: { more: string; less: string },
) {
  const [length, setLength] = useState<number | null>(() =>
    typeof window === "undefined" ? null : readNumber(key, min),
  );
  const [dragging, setDragging] = useState(false);
  const drag = useRef<{
    x: number;
    y: number;
    start: number;
    frame: number;
    next: number;
  } | null>(null);
  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    const start = current();
    drag.current = { x: e.clientX, y: e.clientY, start, frame: 0, next: start };
    setDragging(true);
  };
  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (!d) return;
    d.next = clamp(d.start + grow(d, e));
    if (d.frame) return;
    d.frame = requestAnimationFrame(() => {
      d.frame = 0;
      setLength(d.next);
    });
  };
  const onPointerUp = (e: PointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (!d) return;
    if (d.frame) cancelAnimationFrame(d.frame);
    drag.current = null;
    e.currentTarget.releasePointerCapture?.(e.pointerId);
    setLength(d.next);
    writeNumber(key, d.next);
    setDragging(false);
  };
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key !== keys.more && e.key !== keys.less) return;
    e.preventDefault();
    const next = clamp(
      (length ?? current()) + (e.key === keys.more ? 16 : -16),
    );
    setLength(next);
    writeNumber(key, next);
  };
  const reset = () => {
    setLength(null);
    writeNumber(key, null);
  };
  return {
    length,
    dragging,
    onPointerDown,
    onPointerMove,
    onPointerUp,
    onKeyDown,
    reset,
  };
}

/** The bottom row's dragged height (px; null = the default clamp), kept on this device. */
function useBottomResize(layoutRef: RefObject<HTMLDivElement | null>) {
  const clamp = useCallback(
    (h: number) => {
      const total = layoutRef.current?.getBoundingClientRect().height ?? 0;
      const max = Math.max(BOTTOM_MIN, total * BOTTOM_MAX_SHARE);
      return Math.round(Math.min(max, Math.max(BOTTOM_MIN, h)));
    },
    [layoutRef],
  );
  const current = () =>
    layoutRef.current
      ?.querySelector<HTMLElement>('[data-panel-group="bottom"]')
      ?.getBoundingClientRect().height ?? BOTTOM_MIN;
  const l = useLength(
    BOTTOM_KEY,
    BOTTOM_MIN,
    clamp,
    current,
    (s, e) => s.y - e.clientY,
    { more: "ArrowUp", less: "ArrowDown" },
  );
  return { ...l, height: l.length };
}

/** The right column's dragged content width (px; null = 400), kept on this device. */
function useSideResize(layoutRef: RefObject<HTMLDivElement | null>) {
  const clamp = useCallback(
    (w: number) => {
      const total = layoutRef.current?.getBoundingClientRect().width ?? 0;
      const max =
        total > 0
          ? Math.max(
              SIDE_MIN,
              Math.min(SIDE_MAX, total - CHARTS_MIN - SIDE_GAP),
            )
          : SIDE_MAX;
      return Math.round(Math.min(max, Math.max(SIDE_MIN, w)));
    },
    [layoutRef],
  );
  const current = () => {
    const el = layoutRef.current?.querySelector<HTMLElement>(
      '[data-panel-group="side"]',
    );
    return el ? el.getBoundingClientRect().width - SIDE_GAP : SIDE_DEFAULT;
  };
  const l = useLength(
    SIDE_KEY,
    SIDE_MIN,
    clamp,
    current,
    (s, e) => s.x - e.clientX,
    { more: "ArrowLeft", less: "ArrowRight" },
  );
  // A remembered width from a wider screen is clamped to this one's room when drawn.
  const width = l.length === null ? null : Math.min(l.length, SIDE_MAX);
  return { ...l, width };
}

type Weights<P extends string> = Partial<Record<P, number>>;

function readWeights<P extends string>(key: string, panels: readonly P[]): Weights<P> {
  try {
    const raw = JSON.parse(
      window.localStorage.getItem(key) ?? "{}",
    ) as Record<string, unknown>;
    const out: Weights<P> = {};
    for (const p of panels) {
      const v = raw[p];
      if (typeof v === "number" && Number.isFinite(v) && v > 0.05 && v < 20)
        out[p] = v;
    }
    return out;
  } catch {
    return {};
  }
}

function writeWeights<P extends string>(key: string, w: Weights<P>): void {
  try {
    if (Object.keys(w).length === 0) window.localStorage.removeItem(key);
    else window.localStorage.setItem(key, JSON.stringify(w));
  } catch {
    // A private window: the splits last this session.
  }
}

/**
 * The splits between the panels of one group: the right column's stacked panels (`axis: "y"`,
 * heights) or the bottom row's side-by-side ones (`axis: "x"`, widths; owner, 2026-10-31: "allow
 * resizing between the panels sharing the same parent container"). Each panel has a weight (its
 * flex-grow; 1 by default): dragging the boundary before a panel moves space between it and the
 * visible panel before it, keeping their combined weight, so the others never move. Hidden panels
 * keep their weights, so the stored proportions return with them. Arrow keys move 16px;
 * double-click makes every panel equal again. Kept on this device under `key`.
 */
function useSplits<P extends string>(
  layoutRef: RefObject<HTMLDivElement | null>,
  key: string,
  panels: readonly P[],
  axis: "x" | "y",
) {
  const [weights, setWeights] = useState<Weights<P>>(() =>
    typeof window === "undefined" ? {} : readWeights(key, panels),
  );
  const [dragging, setDragging] = useState(false);
  const drag = useRef<{
    at: number;
    before: P;
    after: P;
    sA: number;
    sB: number;
    sum: number;
    frame: number;
    next: Weights<P>;
  } | null>(null);
  const pos = (e: { clientX: number; clientY: number }) =>
    axis === "x" ? e.clientX : e.clientY;
  const [more, less] =
    axis === "x" ? ["ArrowRight", "ArrowLeft"] : ["ArrowDown", "ArrowUp"];
  const weight = (p: P) => weights[p] ?? 1;
  const slotContent = (p: P) => {
    // The panel itself, not its slot: the slot's leading gap is not the panel's size.
    const r = layoutRef.current
      ?.querySelector<HTMLElement>(`[data-slot-panel="${p}"] > div:last-child`)
      ?.getBoundingClientRect();
    return (axis === "x" ? r?.width : r?.height) ?? 0;
  };
  const beforeOf = (p: P, slots: readonly { id: P; show: boolean }[]) => {
    const shown = slots.filter((s) => s.show).map((s) => s.id);
    const i = shown.indexOf(p);
    return i > 0 ? shown[i - 1]! : null;
  };
  /** The weights after moving `d` px from `after` to `before` (positive: `after` shrinks). */
  const moved = (
    before: P,
    after: P,
    sA: number,
    sB: number,
    sum: number,
    d: number,
    base: Weights<P>,
  ): Weights<P> => {
    const total = sA + sB;
    if (total <= 0) return base;
    const min = Math.min(SPLIT_MIN, total / 2);
    const a = Math.min(total - min, Math.max(min, sA + d));
    return {
      ...base,
      [before]: (sum * a) / total,
      [after]: (sum * (total - a)) / total,
    };
  };
  const handlers = (
    p: P,
    slots: readonly { id: P; show: boolean }[],
  ): DragHandlers => ({
    dragging,
    onPointerDown(e) {
      const before = beforeOf(p, slots);
      if (e.button !== 0 || !before) return;
      e.currentTarget.setPointerCapture(e.pointerId);
      drag.current = {
        at: pos(e),
        before,
        after: p,
        sA: slotContent(before),
        sB: slotContent(p),
        sum: weight(before) + weight(p),
        frame: 0,
        next: weights,
      };
      setDragging(true);
    },
    onPointerMove(e) {
      const d = drag.current;
      if (!d) return;
      d.next = moved(d.before, d.after, d.sA, d.sB, d.sum, pos(e) - d.at, weights);
      if (d.frame) return;
      d.frame = requestAnimationFrame(() => {
        d.frame = 0;
        setWeights(d.next);
      });
    },
    onPointerUp(e) {
      const d = drag.current;
      if (!d) return;
      if (d.frame) cancelAnimationFrame(d.frame);
      drag.current = null;
      e.currentTarget.releasePointerCapture?.(e.pointerId);
      setWeights(d.next);
      writeWeights(key, d.next);
      setDragging(false);
    },
    onKeyDown(e) {
      const before = beforeOf(p, slots);
      if (!before || (e.key !== more && e.key !== less)) return;
      e.preventDefault();
      const next = moved(
        before,
        p,
        slotContent(before),
        slotContent(p),
        weight(before) + weight(p),
        e.key === more ? 16 : -16,
        weights,
      );
      setWeights(next);
      writeWeights(key, next);
    },
    reset() {
      setWeights({});
      writeWeights(key, {});
    },
  });
  /** The panel before's share of the pair, percent (the separator's value). */
  const share = (slots: readonly { id: P; show: boolean }[], p: P) => {
    const before = beforeOf(p, slots);
    if (!before) return 50;
    return Math.round((100 * weight(before)) / (weight(before) + weight(p)));
  };
  return { weight, handlers, share, dragging };
}
