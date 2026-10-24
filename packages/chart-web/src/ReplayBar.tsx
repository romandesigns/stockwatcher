import {
  alertDots,
  alertJumpScope,
  alertLabel,
  alertsForSymbol,
  formatDayKey,
  formatEtDate,
  formatEtTime,
  formatSymbolCount,
  nextAlert,
  prevAlert,
  REPLAY_ALERT_COLOR,
  REPLAY_BAR_DESKTOP_MIN,
  REPLAY_SPEEDS,
  replayBarLayout,
  replaySpeedLabel,
  SCRUBBER_BARS,
  SCRUBBER_TICK_HOURS,
  scrubberBars,
  scrubberSegments,
  scrubberSpan,
  scrubberTicks,
  sessionProgress,
  type Candle,
  type Clock,
  type ReplayAlert,
  type ReplayBarLayout,
  type ReplayDayOption,
  type SessionBand,
} from "@stockwatcher/chart-core";
import { useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Icon } from "./Icon.tsx";
import { ReplayScrubber } from "./ReplayScrubber.tsx";
import { Button } from "./ui/button.tsx";
import { DropdownMenu, DropdownMenuContent, DropdownMenuLabel, DropdownMenuRadioGroup, DropdownMenuRadioItem, DropdownMenuTrigger } from "./ui/dropdown-menu.tsx";
import { ToggleGroup, ToggleGroupItem } from "./ui/toggle-group.tsx";
import { cn, TOUCH_HIT } from "./ui/utils.ts";

export interface ReplayBarProps {
  clock: Clock | null;
  onPlayPause: (play: boolean) => void;
  onSpeedChange: (speed: number) => void;
  /** Jump the replay (`replay.control {action: "seek"}`); the server rebuilds and resends. */
  onSeek: (t: number) => void;
  /** The replay day's session bands (chart-core sessionBands). */
  bands: readonly SessionBand[];
  /** What feeds the volume histogram: the shown chart's candles. */
  candles: readonly Pick<Candle, "t" | "v">[];
  /** The shown ticker and its last price (formatted), for the ticker button. */
  symbol: string;
  price: string;
  /** The ticker button: opens the ticker search, whose pick replaces the chart. */
  onTickerClick: () => void;
  /** Every Runners entry of the replay day (GET /api/replay/alerts), all tickers. */
  alerts?: readonly ReplayAlert[];
  /** An "any ticker" alert jump also opens that ticker (see chart-core alertJumpScope). */
  onOpenTicker?: (symbol: string) => void;
  /** The archive days the server can switch to; with none the date is plain text. */
  days?: readonly ReplayDayOption[];
  /** The day being replayed, "YYYY-MM-DD". */
  day?: string | null;
  onDayChange?: (day: string) => void;
  /** The day menu opened or closed; opening is the moment to refresh `days`. */
  onDayMenuOpenChange?: (open: boolean) => void;
  /**
   * A picked day the server is still switching to: the whole-market rebuild takes a minute or
   * two, and without this the menu closed on the old date as if the pick was ignored (owner,
   * 2026-10-23). The date reads "Loading Oct 22…" until `day` becomes it.
   */
  pendingDay?: string | null;
  /** Force an artboard (tests, catalogs); otherwise the bar's own width picks it (replayBarLayout). */
  layout?: ReplayBarLayout;
  className?: string;
}

/**
 * Which layout this element's width calls for, re-measured on resize (a container query in JS:
 * the rows differ, not just sizes). The one-row desktop tier's width depends on its content (a
 * long price, "· any" on the alert jumps, the day while loading), so a width threshold alone can
 * squeeze it: when the row overflows at some width, the bar uses the two-row arrangement up to
 * that width instead (`desktopMin` only grows, so it settles and never flips back and forth).
 */
function useLayout(forced: ReplayBarLayout | undefined) {
  const ref = useRef<HTMLDivElement>(null);
  const rowRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState<number | null>(null);
  const [desktopMin, setDesktopMin] = useState(REPLAY_BAR_DESKTOP_MIN);
  // First fallback: the same one row without the "Jump to" label and with tighter gaps.
  const [roomyMin, setRoomyMin] = useState(REPLAY_BAR_DESKTOP_MIN);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el || forced) return;
    const set = () => setWidth(Math.round(el.getBoundingClientRect().width));
    set();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(set);
    ro.observe(el);
    return () => ro.disconnect();
  }, [forced]);
  const auto: ReplayBarLayout = width === null || width >= desktopMin ? "desktop" : replayBarLayout(Math.min(width, REPLAY_BAR_DESKTOP_MIN - 1));
  useLayoutEffect(() => {
    if (forced || auto !== "desktop" || width === null) return;
    const row = rowRef.current;
    if (!row || row.scrollWidth <= row.clientWidth + 1) return;
    if (width >= roomyMin) setRoomyMin(width + 1);
    else setDesktopMin(width + 1);
  });
  const tight = !forced && width !== null && width < roomyMin;
  return [ref, rowRef, forced ?? auto, tight] as const;
}

function DayButton({ day, days, onDayChange, onOpenChange, pendingDay, clock, className }: { day: string | null | undefined; days?: readonly ReplayDayOption[]; onDayChange?: (day: string) => void; onOpenChange?: (open: boolean) => void; pendingDay?: string | null; clock: Clock | null; className: string }) {
  const chip = cn("inline-flex shrink-0 items-center gap-2 rounded-[10px] bg-secondary px-3 text-[13px] text-foreground", className);
  const current = days?.find((d) => d.value === day);
  const loading = pendingDay && pendingDay !== day ? pendingDay : null;
  const live = clock?.mode === "live";
  if (!clock || live || !day || !days?.length || !onDayChange) {
    // Nothing to pick (live, the in-browser fixture, an older server): the date as a quiet chip.
    const text = day ? (current?.label ?? formatDayKey(day)) : clock ? formatEtDate(clock.now) : "—";
    return (
      <span className={cn(chip, "text-muted-foreground")} aria-label={`Replay date ${current?.longLabel ?? text}`}>
        <Icon name="calendar" />
        {text}
      </span>
    );
  }
  return (
    <DropdownMenu onOpenChange={(open) => onOpenChange?.(open)}>
      <DropdownMenuTrigger
        aria-label={loading ? `Replay day ${current?.longLabel ?? formatDayKey(day)}, loading ${formatDayKey(loading)}` : `Replay day ${current?.longLabel ?? formatDayKey(day)}, change`}
        className={cn(chip, "relative cursor-pointer outline-none transition-colors duration-150 hover:bg-bg-raised focus-visible:ring-2 focus-visible:ring-ring data-popup-open:bg-bg-raised", TOUCH_HIT)}
      >
        <Icon name="calendar" />
        {loading ? <span className="motion-safe:animate-pulse">Loading {formatDayKey(loading)}…</span> : <span className="tabular-nums">{current?.label ?? formatDayKey(day)}</span>}
        <span className="flex-1" />
        <Icon name="chevronDown" width={12} height={12} className="text-muted-foreground" />
      </DropdownMenuTrigger>
      <DropdownMenuContent side="top" align="start" aria-label="Replay day" className="min-w-56">
        <DropdownMenuRadioGroup value={day} onValueChange={(v) => typeof v === "string" && v !== day && onDayChange(v)}>
          <DropdownMenuLabel>Replay day</DropdownMenuLabel>
          {days.map((d) => (
            <DropdownMenuRadioItem key={d.value} value={d.value} closeOnClick aria-label={`${d.longLabel}${d.fixture ? ", fixture data" : ""}, ${formatSymbolCount(d.symbols)}`}>
              <span className="flex-1 tabular-nums">{d.label}</span>
              {d.fixture && <span className="rounded bg-warning-bg px-1 text-[10px] text-warning">Fixture</span>}
              <span className="font-mono text-[10px] text-text-faint tabular-nums">{d.symbols.toLocaleString("en-US")}</span>
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

interface Jump {
  key: string;
  label: string;
  aria: string;
  dot: string;
  target: number | null;
  /** An "any ticker" alert jump opens that ticker too. */
  symbol?: string;
}

/**
 * The replay bar, "B · Two-tier scrubber" (owner-approved 2026-10-24). The top
 * tier: play/pause (the accent), the clock with its session and the time into
 * it, the day picker, the ticker (opens the search), the jumps (04:00, the
 * 09:30 open, the previous and next alert) and the speed segments. The lower
 * tier: the scrubber (ReplayScrubber). Three arrangements, picked by the bar's
 * own width (replayBarLayout): "desktop" one row over the scrubber, "fold"
 * (the Fold unfolded and narrow windows, 600–1099 px) two rows around it, "cover" (< 600 px)
 * stacked with 44 px targets.
 *
 * Alert jumps follow the shown ticker's alerts that day; when it has none they
 * cover every ticker ("· any") and a jump opens that ticker, so practice can
 * always start where an alert fired (chart-core alertJumpScope).
 */
export function ReplayBar(props: ReplayBarProps) {
  const { clock, onPlayPause, onSpeedChange, onSeek, bands, candles, symbol, price, onTickerClick, alerts = [], onOpenTicker, pendingDay, className } = props;
  const [ref, rowRef, layout, tight] = useLayout(props.layout);
  const live = clock?.mode === "live";
  const playing = !!clock?.playing;
  const controllable = !!clock && !live;
  const span = useMemo(() => scrubberSpan(bands), [bands]);
  const segments = useMemo(() => scrubberSegments(bands), [bands]);
  const bars = useMemo(() => (span ? scrubberBars(candles, span, SCRUBBER_BARS[layout]) : []), [candles, span, layout]);
  const ticks = useMemo(() => {
    if (!span) return [];
    const all = scrubberTicks(span, SCRUBBER_TICK_HOURS[layout]);
    // The desktop's hourly labels each start at their hour, so 20:00 would hang off the end.
    return layout === "desktop" ? all.slice(0, -1) : all;
  }, [span, layout]);
  const dots = useMemo(() => (span ? alertDots(alertsForSymbol(alerts, symbol), span) : []), [alerts, symbol, span]);
  const scope = useMemo(() => alertJumpScope(alerts, symbol), [alerts, symbol]);
  const now = clock?.now ?? null;
  const time = clock ? formatEtTime(clock.now, true) : "--:--:--";
  const progress = clock && bands.length ? sessionProgress(clock.now, bands).text : "";
  const open = bands.find((b) => b.kind === "regular")?.start ?? null;
  const prev = now === null ? null : prevAlert(scope.alerts, now);
  const next = now === null ? null : nextAlert(scope.alerts, now);
  const phone = layout !== "desktop";
  const any = scope.any ? " · any" : "";
  const jumps: Jump[] = [
    { key: "start", label: "04:00", aria: "Jump to 04:00, the pre-market open", dot: "var(--text-faint)", target: span?.start ?? null },
    { key: "open", label: "09:30 open", aria: "Jump to 09:30, the regular open", dot: "var(--text-dim)", target: open },
    {
      key: "prev",
      label: phone ? `‹ Prev alert${any}` : `Prev alert${any}`,
      aria: prev ? `Previous alert: ${alertLabel(prev, true)}` : "No earlier alert",
      dot: REPLAY_ALERT_COLOR.fast,
      target: prev?.t ?? null,
      ...(scope.any && prev ? { symbol: prev.symbol } : {}),
    },
    {
      key: "next",
      label: phone ? `Next alert${any} ›` : `Next alert${any}`,
      aria: next ? `Next alert: ${alertLabel(next, true)}` : "No later alert",
      dot: REPLAY_ALERT_COLOR.fast,
      target: next?.t ?? null,
      ...(scope.any && next ? { symbol: next.symbol } : {}),
    },
  ];
  const jump = (j: Jump) => {
    if (j.target === null) return;
    if (j.symbol) onOpenTicker?.(j.symbol);
    onSeek(j.target);
  };

  const play = (
    <Button
      className={cn("shrink-0", layout === "desktop" ? "size-11 rounded-xl" : layout === "fold" ? "size-12 rounded-[13px]" : "size-[52px] rounded-[14px]", "[&_svg]:size-[18px]")}
      size="icon"
      disabled={!controllable}
      aria-label={playing ? "Pause replay" : "Play replay"}
      onClick={() => onPlayPause(!playing)}
    >
      <Icon name={playing ? "pause" : "play"} />
    </Button>
  );
  const clockBlock = (
    <div className={cn("flex min-w-0", layout === "desktop" ? "items-baseline gap-2.5" : "flex-col gap-0.5")} aria-label={clock ? `${time} Eastern, ${progress}` : "Clock not started"} role="group">
      <span className={cn("font-mono font-medium tabular-nums leading-none", layout === "fold" ? "text-[22px]" : "text-2xl")} aria-live="off">
        {time}
      </span>
      <span className={cn("truncate text-muted-foreground", layout === "desktop" ? "text-[13px]" : "text-xs")}>{progress}</span>
    </div>
  );
  const control = layout === "desktop" ? "h-9" : "h-11 rounded-xl";
  const dayButton = <DayButton day={props.day} days={props.days} onDayChange={props.onDayChange} onOpenChange={props.onDayMenuOpenChange} pendingDay={pendingDay} clock={clock} className={cn(control, layout === "cover" && "min-w-0 flex-1")} />;
  const tickerButton = (
    <Button variant="secondary" className={cn(control, "gap-2 rounded-[10px] px-3 text-[13px] font-normal", layout === "desktop" ? "" : "rounded-xl", layout === "cover" && "min-w-0 flex-1 justify-start")} aria-label={`${symbol || "No ticker"}${price ? `, ${price}` : ""}: search tickers`} onClick={onTickerClick}>
      <Icon name="search" />
      <span className="font-semibold">{symbol || "—"}</span>
      {price && <span className="truncate text-muted-foreground tabular-nums">${price}</span>}
    </Button>
  );
  const jumpButtons = jumps.map((j) => (
    <Button
      key={j.key}
      variant={layout === "desktop" ? "ghost" : "secondary"}
      disabled={!controllable || j.target === null}
      aria-label={j.aria}
      onClick={() => jump(j)}
      className={cn("gap-1.5 px-2.5 text-xs font-normal text-foreground/85", layout === "desktop" ? "h-8 rounded-[9px]" : layout === "fold" ? "h-10 rounded-full px-3" : "h-9 rounded-full px-3")}
    >
      <span className="size-1.5 shrink-0 rounded-full" style={{ background: j.dot }} aria-hidden="true" />
      {j.label}
    </Button>
  ));
  const speed = clock?.speed ?? 1;
  const speeds = (
    <ToggleGroup
      aria-label="Replay speed"
      variant="segmented"
      size="sm"
      spacing={0.5}
      value={[String(speed)]}
      onValueChange={(v) => v[0] && onSpeedChange(Number(v[0]))}
      disabled={!controllable}
      className={cn("p-[3px] data-[variant=segmented]:bg-secondary data-[variant=segmented]:p-[3px]", layout === "cover" ? "w-full rounded-xl" : "rounded-[10px]")}
    >
      {REPLAY_SPEEDS.map((s) => (
        <ToggleGroupItem
          key={s}
          value={String(s)}
          aria-label={`Speed ${replaySpeedLabel(s)}`}
          className={cn("text-xs font-semibold tabular-nums", layout === "desktop" ? "h-[30px] min-w-[38px] rounded-lg px-2" : layout === "fold" ? "h-[34px] min-w-10 rounded-[9px] px-2" : "h-[38px] flex-1 rounded-[9px] text-[13px]")}
        >
          {replaySpeedLabel(s)}
        </ToggleGroupItem>
      ))}
    </ToggleGroup>
  );
  const scrubber = span ? (
    <ReplayScrubber
      span={span}
      segments={segments}
      bars={bars}
      ticks={ticks}
      dots={dots}
      now={now}
      onSeek={onSeek}
      disabled={!controllable}
      labels={layout === "cover" ? "short" : "long"}
      tooltip={layout === "desktop" ? "hover" : "drag"}
      className={layout === "cover" ? "h-[92px]" : "h-24"}
    />
  ) : (
    <div className={cn("grid place-items-center rounded-lg bg-background/55 text-xs text-text-faint", layout === "cover" ? "h-[92px]" : "h-24")}>{clock ? "No session for this day" : "Waiting for the replay clock…"}</div>
  );

  let body: ReactNode;
  if (layout === "desktop") {
    body = (
      <>
        <div ref={rowRef} className={cn("flex items-center whitespace-nowrap [&>*]:shrink-0", tight ? "gap-2" : "gap-3")}>
          {play}
          {clockBlock}
          {dayButton}
          {tickerButton}
          <span className="flex-1" />
          {!tight && <span className="text-xs text-text-faint">Jump to</span>}
          <div className="flex items-center gap-0.5">{jumpButtons}</div>
          {speeds}
        </div>
        {scrubber}
      </>
    );
  } else if (layout === "fold") {
    body = (
      <>
        <div className="flex items-center gap-3">
          {play}
          {clockBlock}
          <span className="flex-1" />
          {dayButton}
          {tickerButton}
        </div>
        {scrubber}
        <div className="flex items-center gap-1.5">
          {jumpButtons}
          <span className="flex-1" />
          {speeds}
        </div>
      </>
    );
  } else {
    body = (
      <>
        <div className="flex items-center gap-3">
          {play}
          {clockBlock}
        </div>
        <div className="flex gap-2">
          {dayButton}
          {tickerButton}
        </div>
        {scrubber}
        <div className="flex flex-wrap gap-1.5">{jumpButtons}</div>
        {speeds}
      </>
    );
  }
  return (
    <div ref={ref} data-layout={layout} className={cn("flex flex-col gap-3", className)}>
      {body}
    </div>
  );
}
