import { cn, TOUCH_HIT_Y } from "@stockwatcher/chart-web/ui";
import {
  BOTTOM_PANELS,
  detectionItemCount,
  EMPTY_QUOTE_BOARD,
  hotStocks,
  panelsSettled,
  rocketItemCount,
  rossItemCount,
  SIDE_PANELS,
  spansPair,
  topGainers,
  visiblePanels,
  type BottomPanel,
  type DashboardPanel,
  type DetectionChannel,
  type FavoritesChannel,
  type PressureChannel,
  type QuoteChannel,
  type RocketChannel,
  type RossChannel,
  type SidePanel,
} from "@stockwatcher/shared";
import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent, type PointerEvent, type ReactNode, type RefObject } from "react";
import { FavoritesPanelView } from "../favorites/FavoritesPanel.tsx";
import { useFavoriteList } from "../favorites/useFavoriteList.ts";
import { useDetection } from "../feed/useDetection.ts";
import { useFavorites } from "../feed/useFavorites.ts";
import { usePressureBoard, usePressureStatus } from "../feed/usePressureBoard.ts";
import { useQuoteBoard } from "../feed/useQuoteBoard.ts";
import { useRocket } from "../feed/useRocket.ts";
import { useRossState } from "../feed/useRossState.ts";
import { useHeld } from "../lib/useHeld.ts";
import { useTick } from "../lib/useTick.ts";
import { DetectionPanelView } from "./DetectionPanel.tsx";
import { HaltWatcherView } from "./HaltWatcher.tsx";
import { MoversPanelView } from "./MoversPanel.tsx";
import { RocketPanelView } from "./RocketPanel.tsx";
import { RossPanelView } from "./RossPanel.tsx";

/** How long the layout waits for every channel's first word before deciding anyway. */
export const SETTLE_MS = 1500;
/** How long a panel or group takes to open or close (the transitions below). */
const MOTION_MS = 300;
/** The right column's content width (the old side tabs' 400px), plus its 8px gap to the charts. */
const SIDE_W = "408px";
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
function useSettled<P extends DashboardPanel>(live: P[], settled: boolean, timedOut: boolean): P[] {
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
function usePresence<P extends DashboardPanel>(order: readonly P[], shown: P[]): { id: P; show: boolean }[] {
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
  return order.filter((p) => shown.includes(p) || leaving.includes(p)).map((p) => ({ id: p, show: shown.includes(p) }));
}

export interface DashboardPanelsProps {
  ross: RossChannel | undefined;
  rocket: RocketChannel | undefined;
  detection: DetectionChannel | undefined;
  favorites?: FavoritesChannel;
  pressure?: PressureChannel;
  quotes?: QuoteChannel;
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
 * - **Wide (≥ @4xl/panel):** the charts in the middle; a **right column** (400px, as the old side tabs)
 *   with Favorites, Signals and Top Gainers stacked, all visible at once, sharing its height; and
 *   a **bottom row** under both, full width, with Ross Panel, Runners, Halt watcher and Hot stocks
 *   side by side at equal widths. Every panel scrolls inside its own area. The row's height can be
 *   dragged at its top edge (or set with the arrow keys there); double-click resets it.
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
 * has rows, including the quote board behind Top Gainers and Hot stocks (it used to be subscribed
 * only while the ticker search was open).
 */
export function DashboardPanels({ ross, rocket, detection, favorites, pressure, quotes, onPick, onEpisodePick, onEpisodeClear, children }: DashboardPanelsProps) {
  const rossState = useRossState(ross);
  const rocketState = useRocket(rocket);
  const detected = useDetection(detection);
  const favoritesState = useFavorites(favorites);
  // The owner's list counts from the synced setting, before any row arrives (panels.ts).
  const favoriteList = useFavoriteList();
  const pressureBoard = usePressureBoard(pressure);
  const pressureStatus = usePressureStatus(pressure);
  const board = useQuoteBoard(quotes, true);
  const gainers = useMemo(() => topGainers(board), [board]);
  const hot = useMemo(() => hotStocks(board), [board]);
  const wall = useTick(15_000);
  const runners = rocket ? rocketState : undefined;

  const counts: Record<DashboardPanel, number> = {
    favorites: favorites ? favoriteList.length : 0,
    signals: detection ? detectionItemCount(detected) : 0,
    gainers: gainers.length,
    ross: ross ? rossItemCount(rossState) : 0,
    rocket: rocket ? rocketItemCount(rocketState) : 0,
    halt: pressure ? pressureBoard.length : 0,
    hot: hot.length,
  };
  // The quote board and the Halt watcher have no on/off: they have answered once their first frame
  // landed (the board leaves its shared empty instance; the pressure status records a frame).
  const settled = panelsSettled([
    ross && rossState.enabled,
    rocket && rocketState.enabled,
    detection && detected.enabled,
    pressure && (pressureStatus.connected || pressureStatus.asOf !== null ? true : null),
    quotes && (board !== EMPTY_QUOTE_BOARD ? true : null),
  ]);
  const timedOut = useSettleTimeout(settled);
  const side = useSettled(visiblePanels(SIDE_PANELS, counts), settled, timedOut);
  const bottom = useSettled(visiblePanels(BOTTOM_PANELS, counts), settled, timedOut);
  const sideSlots = usePresence(SIDE_PANELS, side);
  const bottomSlots = usePresence(BOTTOM_PANELS, bottom);
  const sideOpen = side.length > 0;
  const bottomOpen = bottom.length > 0;

  const content = (p: DashboardPanel): ReactNode =>
    p === "favorites" ? (
      <FavoritesPanelView list={favoriteList} state={favoritesState} onPick={onPick} runners={runners} wallNow={wall} />
    ) : p === "signals" ? (
      <DetectionPanelView state={detected} onPick={onEpisodePick} onClear={onEpisodeClear} />
    ) : p === "gainers" ? (
      <MoversPanelView kind="gainers" rows={gainers} runners={runners} wallNow={wall} onPick={onPick} />
    ) : p === "ross" ? (
      <RossPanelView state={rossState} detection={detection} onPick={onPick} onEpisodePick={onEpisodePick} runners={runners} wallNow={wall} />
    ) : p === "rocket" ? (
      <RocketPanelView state={rocketState} wallNow={wall} onPick={onPick} />
    ) : p === "halt" ? (
      <HaltWatcherView board={pressureBoard} status={pressureStatus} onPick={onPick} />
    ) : (
      <MoversPanelView kind="hot" rows={hot} runners={runners} wallNow={wall} onPick={onPick} />
    );

  const layoutRef = useRef<HTMLDivElement>(null);
  const resize = useBottomResize(layoutRef);
  const style = { "--side-w": sideOpen ? SIDE_W : "0px", "--bottom-h": bottomOpen ? (resize.height === null ? BOTTOM_DEFAULT : `${resize.height}px`) : "0px" } as CSSProperties;

  return (
    <div
      ref={layoutRef}
      data-dashboard-layout
      data-resizing={resize.dragging || undefined}
      style={style}
      className={cn(
        // Narrow: one column, the charts first. Wide: charts | right column over the bottom row.
        "relative flex flex-1 flex-col",
        "@4xl/panel:grid @4xl/panel:grid-cols-[minmax(0,1fr)_var(--side-w)] @4xl/panel:grid-rows-[minmax(0,1fr)_var(--bottom-h)]",
        // The open/close of each group moves its track; while the owner drags the row, nothing lags.
        "transition-[grid-template-columns,grid-template-rows] duration-300 ease-out motion-reduce:transition-none data-resizing:transition-none",
      )}
    >
      {/* The charts keep their own minimum height (18rem a row), so with too little room the panel
          scrolls rather than squeezing them; on a narrow panel they take most of the screen and
          the panels follow below. */}
      <div className="flex min-h-[max(18rem,70svh)] min-w-0 flex-1 flex-col @4xl/panel:col-start-1 @4xl/panel:row-start-1 @4xl/panel:min-h-auto">{children}</div>

      <PanelGroup group="side" open={sideOpen}>
        {sideSlots.map(({ id, show }, i) => (
          <PanelSlot key={id} panel={id} axis="side" show={show || !sideOpen} span={spansPair(i, sideSlots.length)}>
            {content(id)}
          </PanelSlot>
        ))}
      </PanelGroup>

      {bottomOpen && (
        // The bottom row's top edge: drag to resize (wide only). A separator, so the keyboard and
        // assistive tech can reach it; its hit area grows to 44px on touch.
        <div
          role="separator"
          aria-orientation="horizontal"
          aria-label="Resize the bottom panels"
          aria-valuenow={resize.height ?? undefined}
          tabIndex={0}
          onPointerDown={resize.onPointerDown}
          onPointerMove={resize.onPointerMove}
          onPointerUp={resize.onPointerUp}
          onPointerCancel={resize.onPointerUp}
          onKeyDown={resize.onKeyDown}
          onDoubleClick={resize.reset}
          data-dragging={resize.dragging || undefined}
          className={cn(
            "group relative z-10 hidden h-2 cursor-row-resize touch-none outline-none @4xl/panel:col-[1/-1] @4xl/panel:row-start-2 @4xl/panel:flex @4xl/panel:items-center @4xl/panel:justify-center @4xl/panel:self-start",
            TOUCH_HIT_Y,
          )}
        >
          <span className="h-1 w-10 rounded-full bg-transparent transition-colors duration-150 group-hover:bg-bg-raised group-focus-visible:bg-ring group-data-dragging:bg-bg-raised motion-reduce:transition-none" aria-hidden />
        </div>
      )}

      <PanelGroup group="bottom" open={bottomOpen}>
        {bottomSlots.map(({ id, show }, i) => (
          <PanelSlot key={id} panel={id} axis="bottom" show={show || !bottomOpen} span={spansPair(i, bottomSlots.length)}>
            {content(id)}
          </PanelSlot>
        ))}
      </PanelGroup>
    </div>
  );
}

/**
 * One group of panels: the right column or the bottom row. Wide, it fills its grid track (which
 * animates to 0 when the group closes, the charts growing into it) and keeps its content's size
 * while it moves, so the panels are clipped rather than reflowed. Narrow, it opens and closes by
 * its real height, and lays its panels out two to a row where they fit.
 */
function PanelGroup({ group, open, children }: { group: "side" | "bottom"; open: boolean; children: ReactNode }) {
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
        side ? "@4xl/panel:col-start-2 @4xl/panel:row-start-1" : "@4xl/panel:col-[1/-1] @4xl/panel:row-start-2",
      )}
    >
      <div className={cn("min-h-0 min-w-0 overflow-clip [overflow-clip-margin:4px]", "pt-2", side ? "@4xl/panel:pt-0 @4xl/panel:pl-2" : "@4xl/panel:pt-2")}>
        <div
          className={cn(
            // Narrow: a grid of up to two columns by the panel's width; one on a phone.
            "grid grid-cols-1 gap-2 @2xl/panel:grid-cols-2",
            // Wide: a flex line whose items share it evenly; each brings its own leading gap (so a
            // closing panel takes its gap with it), cancelled once here.
            side ? "@4xl/panel:-mt-2 @4xl/panel:flex @4xl/panel:h-[calc(100%+0.5rem)] @4xl/panel:w-[400px] @4xl/panel:flex-col @4xl/panel:gap-0" : "@4xl/panel:-ml-2 @4xl/panel:flex @4xl/panel:h-full @4xl/panel:w-[calc(100%+0.5rem)] @4xl/panel:flex-row @4xl/panel:gap-0",
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
 */
function PanelSlot({ panel, axis, show, span, children }: { panel: SidePanel | BottomPanel; axis: "side" | "bottom"; show: boolean; span: boolean; children: ReactNode }) {
  const shown = useHeld<ReactNode>(children, show, null);
  return (
    <div
      data-slot-panel={panel}
      data-show={show ? "open" : "closed"}
      aria-hidden={show ? undefined : true}
      inert={!show}
      className={cn(
        "grid min-h-0 min-w-0 transition-[grid-template-rows,flex-grow,opacity,padding] duration-300 ease-out motion-reduce:transition-none",
        show ? "grid-rows-[1fr] opacity-100" : "grid-rows-[0fr] opacity-0",
        "starting:grid-rows-[0fr] starting:opacity-0",
        span && "@2xl/panel:col-span-2",
        "@4xl/panel:basis-0 @4xl/panel:grid-rows-[1fr] @4xl/panel:starting:grow-0",
        show ? "@4xl/panel:grow" : "@4xl/panel:grow-0",
        axis === "side" ? (show ? "@4xl/panel:pt-2" : "@4xl/panel:pt-0") : show ? "@4xl/panel:pl-2" : "@4xl/panel:pl-0",
        axis === "side" ? "@4xl/panel:starting:pt-0" : "@4xl/panel:starting:pl-0",
      )}
    >
      <div className="flex max-h-[26rem] min-h-0 min-w-0 flex-col overflow-clip [overflow-clip-margin:4px] *:min-h-0 *:flex-1 @4xl/panel:max-h-none">{shown}</div>
    </div>
  );
}

/** The bottom row's dragged height (px; null = the default clamp), kept on this device. */
function readHeight(): number | null {
  try {
    const v = Number(window.localStorage.getItem(BOTTOM_KEY));
    return Number.isFinite(v) && v >= BOTTOM_MIN ? v : null;
  } catch {
    return null;
  }
}

function writeHeight(v: number | null): void {
  try {
    if (v === null) window.localStorage.removeItem(BOTTOM_KEY);
    else window.localStorage.setItem(BOTTOM_KEY, String(Math.round(v)));
  } catch {
    // A private window: the height lasts this session.
  }
}

/**
 * Dragging the boundary between the charts and the bottom row. The height follows the pointer
 * (one layout per frame, transitions off while dragging), clamped so both keep usable room, and
 * is remembered on this device. Arrow keys move it 16px; double-click goes back to the default.
 */
function useBottomResize(layoutRef: RefObject<HTMLDivElement | null>) {
  const [height, setHeight] = useState<number | null>(() => (typeof window === "undefined" ? null : readHeight()));
  const [dragging, setDragging] = useState(false);
  const drag = useRef<{ startY: number; startH: number; frame: number; next: number } | null>(null);
  const clamp = useCallback(
    (h: number) => {
      const total = layoutRef.current?.getBoundingClientRect().height ?? 0;
      const max = Math.max(BOTTOM_MIN, total * BOTTOM_MAX_SHARE);
      return Math.round(Math.min(max, Math.max(BOTTOM_MIN, h)));
    },
    [layoutRef],
  );
  const current = useCallback(() => {
    const el = layoutRef.current?.querySelector<HTMLElement>('[data-panel-group="bottom"]');
    return el?.getBoundingClientRect().height ?? BOTTOM_MIN;
  }, [layoutRef]);
  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    drag.current = { startY: e.clientY, startH: current(), frame: 0, next: current() };
    setDragging(true);
  };
  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (!d) return;
    // Dragging up makes the row taller.
    d.next = clamp(d.startH + (d.startY - e.clientY));
    if (d.frame) return;
    d.frame = requestAnimationFrame(() => {
      d.frame = 0;
      setHeight(d.next);
    });
  };
  const onPointerUp = (e: PointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (!d) return;
    if (d.frame) cancelAnimationFrame(d.frame);
    drag.current = null;
    e.currentTarget.releasePointerCapture?.(e.pointerId);
    setHeight(d.next);
    writeHeight(d.next);
    setDragging(false);
  };
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key !== "ArrowUp" && e.key !== "ArrowDown") return;
    e.preventDefault();
    const next = clamp((height ?? current()) + (e.key === "ArrowUp" ? 16 : -16));
    setHeight(next);
    writeHeight(next);
  };
  const reset = () => {
    setHeight(null);
    writeHeight(null);
  };
  return { height, dragging, onPointerDown, onPointerMove, onPointerUp, onKeyDown, reset };
}
