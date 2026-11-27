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
  nextReplaySpeed,
  prevAlert,
  REPLAY_ALERT_COLOR,
  REPLAY_BAR_DESKTOP_MIN,
  REPLAY_BAR_EXPANDED_KEY,
  REPLAY_SPEEDS,
  replayBarLayout,
  replayDetailsInline,
  replayScrubberInline,
  replaySpeedLabel,
  SCRUBBER_BARS,
  SCRUBBER_MICRO_BARS,
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
import { useCallback, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Icon } from "./Icon.tsx";
import { ReplayScrubber } from "./ReplayScrubber.tsx";
import { Button } from "./ui/button.tsx";
import { DropdownMenu, DropdownMenuContent, DropdownMenuLabel, DropdownMenuRadioGroup, DropdownMenuRadioItem, DropdownMenuTrigger } from "./ui/dropdown-menu.tsx";
import { Popover, PopoverContent, PopoverTrigger } from "./ui/popover.tsx";
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
  /**
   * The compact bar's "more" panel, controlled (tests, catalogs). Uncontrolled, the bar keeps it
   * itself and remembers it in this browser (localStorage, REPLAY_BAR_EXPANDED_KEY).
   */
  expanded?: boolean;
  onExpandedChange?: (expanded: boolean) => void;
  className?: string;
}

/**
 * Which layout this element's width calls for, re-measured on resize (a container query in JS:
 * the rows differ, not just sizes). The one-row desktop tier's width depends on its content (a
 * long price, "· any" on the alert jumps, the day while loading), so a width threshold alone can
 * squeeze it: when the row overflows at some width, the bar uses the compact arrangement up to
 * that width instead (`desktopMin` only grows, so it settles and never flips back and forth).
 */
function useLayout(forced: ReplayBarLayout | undefined) {
  const ref = useRef<HTMLDivElement | null>(null);
  // The root element itself as state: the desktop and the compact bars are different elements,
  // so the observer must follow the swap (watching the unmounted one froze the layout).
  const [el, setEl] = useState<HTMLDivElement | null>(null);
  const attach = useCallback((node: HTMLDivElement | null) => {
    ref.current = node;
    setEl(node);
  }, []);
  const rowRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState<number | null>(null);
  const [desktopMin, setDesktopMin] = useState(REPLAY_BAR_DESKTOP_MIN);
  // First fallback: the same one row without the "Jump to" label and with tighter gaps.
  const [roomyMin, setRoomyMin] = useState(REPLAY_BAR_DESKTOP_MIN);
  useLayoutEffect(() => {
    if (!el || forced) return;
    const set = () => setWidth(Math.round(el.getBoundingClientRect().width));
    set();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(set);
    ro.observe(el);
    return () => ro.disconnect();
  }, [el, forced]);
  const auto: ReplayBarLayout = width === null || width >= desktopMin ? "desktop" : replayBarLayout(Math.min(width, REPLAY_BAR_DESKTOP_MIN - 1));
  useLayoutEffect(() => {
    if (forced || auto !== "desktop" || width === null) return;
    const row = rowRef.current;
    if (!row || row.scrollWidth <= row.clientWidth + 1) return;
    if (width >= roomyMin) setRoomyMin(width + 1);
    else setDesktopMin(width + 1);
  });
  const tight = !forced && width !== null && width < roomyMin;
  return [ref, attach, rowRef, forced ?? auto, tight, width] as const;
}

const EXPANDED_STORAGE_KEY = `sw-${REPLAY_BAR_EXPANDED_KEY}`;

/** The compact bar's expanded flag: controlled when the host passes it, else this browser's (it can throw or come back empty in a private window: then it starts collapsed). */
function useExpanded(controlled: boolean | undefined, onChange: ((v: boolean) => void) | undefined) {
  const [own, setOwn] = useState(() => {
    try {
      return typeof localStorage !== "undefined" && localStorage.getItem(EXPANDED_STORAGE_KEY) === "1";
    } catch {
      return false;
    }
  });
  const value = controlled ?? own;
  const set = (v: boolean) => {
    if (controlled === undefined) {
      setOwn(v);
      try {
        localStorage.setItem(EXPANDED_STORAGE_KEY, v ? "1" : "0");
      } catch {
        // Storage blocked: the panel still opens and closes, it just is not remembered.
      }
    }
    onChange?.(v);
  };
  return [value, set] as const;
}

function DayButton({ day, days, onDayChange, onOpenChange, pendingDay, clock, className }: { day: string | null | undefined; days?: readonly ReplayDayOption[]; onDayChange?: (day: string) => void; onOpenChange?: (open: boolean) => void; pendingDay?: string | null; clock: Clock | null; className: string }) {
  const chip = cn("inline-flex shrink-0 items-center gap-2 rounded-[10px] bg-secondary px-3 text-[13px] text-foreground", className);
  const current = days?.find((d) => d.value === day);
  const loading = pendingDay && pendingDay !== day ? pendingDay : null;
  const live = clock?.mode === "live";
  if (!clock || live || !day || !days?.length || !onDayChange) {
    // Nothing to pick (live, an older server): the date as a quiet chip.
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
 * The replay bar. On the desktop (≥ 1100 px of bar): "B · Two-tier scrubber"
 * (owner-approved 2026-10-24), one row of play/pause (the accent), the clock with
 * its session and the time into it, the day picker, the ticker (opens the
 * search), the jumps (04:00, the 09:30 open, the previous and next alert) and the
 * speed segments, over the full scrubber (ReplayScrubber).
 *
 * Narrower ("fold" 600–1099, "cover" < 600) the two-tier bar took half a phone's
 * height (owner, 2026-10-24), so it rests as ONE compact row instead: play, Prev
 * and Next alert (the main practice action, one tap away), the clock, the thin
 * scrubber (inline on "fold", a full-width strip over the row on "cover", where
 * the row leaves it too little room), a speed chip that cycles and "more". "More"
 * opens the rest in a popover over the chart (so the chart never relayouts): day,
 * ticker, Jump to, the speed segments and the full scrubber. From 900 px the day
 * and ticker also sit in the row. The open state is remembered in this browser.
 *
 * Alert jumps follow the shown ticker's alerts that day; when it has none they
 * cover every ticker ("· any") and a jump opens that ticker, so practice can
 * always start where an alert fired (chart-core alertJumpScope).
 */
export function ReplayBar(props: ReplayBarProps) {
  const { clock, onPlayPause, onSpeedChange, onSeek, bands, candles, symbol, price, onTickerClick, alerts = [], onOpenTicker, pendingDay, className } = props;
  const [ref, attach, rowRef, layout, tight, width] = useLayout(props.layout);
  const [expanded, setExpanded] = useExpanded(props.expanded, props.onExpandedChange);
  const compact = layout !== "desktop";
  const live = clock?.mode === "live";
  const playing = !!clock?.playing;
  const controllable = !!clock && !live;
  const span = useMemo(() => scrubberSpan(bands), [bands]);
  const segments = useMemo(() => scrubberSegments(bands), [bands]);
  const bars = useMemo(() => (span ? scrubberBars(candles, span, SCRUBBER_BARS[layout]) : []), [candles, span, layout]);
  const microBars = useMemo(() => (span && compact ? scrubberBars(candles, span, SCRUBBER_MICRO_BARS) : []), [candles, span, compact]);
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
  const any = scope.any ? " · any" : "";
  const jumps: Jump[] = [
    { key: "start", label: "04:00", aria: "Jump to 04:00, the pre-market open", dot: "var(--text-faint)", target: span?.start ?? null },
    { key: "open", label: "09:30 open", aria: "Jump to 09:30, the regular open", dot: "var(--text-dim)", target: open },
    {
      key: "prev",
      label: compact ? `‹ Prev alert${any}` : `Prev alert${any}`,
      aria: prev ? `Previous alert: ${alertLabel(prev, true)}` : "No earlier alert",
      dot: REPLAY_ALERT_COLOR.fast,
      target: prev?.t ?? null,
      ...(scope.any && prev ? { symbol: prev.symbol } : {}),
    },
    {
      key: "next",
      label: compact ? `Next alert${any} ›` : `Next alert${any}`,
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
      className="size-11 shrink-0 rounded-xl pointer-coarse:size-11 [&_svg]:size-[18px]"
      size="icon"
      disabled={!controllable}
      aria-label={playing ? "Pause replay" : "Play replay"}
      onClick={() => onPlayPause(!playing)}
    >
      <Icon name={playing ? "pause" : "play"} />
    </Button>
  );
  const control = compact ? "h-11 rounded-xl" : "h-9";
  const dayButton = (extra?: string) => <DayButton day={props.day} days={props.days} onDayChange={props.onDayChange} onOpenChange={props.onDayMenuOpenChange} pendingDay={pendingDay} clock={clock} className={cn(control, extra)} />;
  const tickerButton = (extra?: string) => (
    <Button variant="secondary" className={cn(control, "gap-2 rounded-[10px] px-3 text-[13px] font-normal", compact && "rounded-xl", extra)} aria-label={`${symbol || "No ticker"}${price ? `, ${price}` : ""}: search tickers`} onClick={onTickerClick}>
      <Icon name="search" />
      <span className="font-semibold">{symbol || "—"}</span>
      {price && <span className="truncate text-muted-foreground tabular-nums">${price}</span>}
    </Button>
  );
  const jumpButtons = jumps.map((j) => (
    <Button
      key={j.key}
      variant={compact ? "secondary" : "ghost"}
      disabled={!controllable || j.target === null}
      aria-label={j.aria}
      onClick={() => jump(j)}
      className={cn("gap-1.5 px-2.5 text-xs font-normal text-foreground/85", compact ? "h-10 rounded-full px-3" : "h-8 rounded-[9px]")}
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
      className={cn("p-[3px] data-[variant=segmented]:bg-secondary data-[variant=segmented]:p-[3px]", compact ? "w-full rounded-xl" : "rounded-[10px]")}
    >
      {REPLAY_SPEEDS.map((s) => (
        <ToggleGroupItem
          key={s}
          value={String(s)}
          aria-label={`Speed ${replaySpeedLabel(s)}`}
          className={cn("text-xs font-semibold tabular-nums", compact ? "h-[38px] flex-1 rounded-[9px] text-[13px]" : "h-[30px] min-w-[38px] rounded-lg px-2")}
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

  if (!compact) {
    return (
      <div ref={attach} data-layout={layout} className={cn("flex flex-col gap-3 px-4.5 pt-3.5 pb-3", className)}>
        <div ref={rowRef} className={cn("flex items-center whitespace-nowrap [&>*]:shrink-0", tight ? "gap-2" : "gap-3")}>
          {play}
          <div className="flex min-w-0 items-baseline gap-2.5" aria-label={clock ? `${time} Eastern, ${progress}` : "Clock not started"} role="group">
            <span className="font-mono text-2xl font-medium tabular-nums leading-none" aria-live="off">
              {time}
            </span>
            <span className="truncate text-[13px] text-muted-foreground">{progress}</span>
          </div>
          {dayButton()}
          {tickerButton()}
          <span className="flex-1" />
          {!tight && <span className="text-xs text-text-faint">Jump to</span>}
          <div className="flex items-center gap-0.5">{jumpButtons}</div>
          {speeds}
        </div>
        {scrubber}
      </div>
    );
  }

  // ---- The compact bar ----
  const inline = replayScrubberInline(layout);
  const details = inline && width !== null && replayDetailsInline(width);
  const alertButton = (j: Jump, icon: "alertPrev" | "alertNext") => (
    <Button variant="secondary" size="icon" className="size-11 shrink-0 rounded-xl pointer-coarse:size-11 [&_svg]:size-[18px]" disabled={!controllable || j.target === null} aria-label={j.aria} onClick={() => jump(j)}>
      <Icon name={icon} />
      {/* "· any": the jump crosses to another ticker (no alerts on this one today). */}
      {scope.any && (
        <span className="absolute inset-x-0 bottom-[3px] text-center text-[8px] leading-none font-medium text-muted-foreground" aria-hidden="true">
          any
        </span>
      )}
    </Button>
  );
  const clockBlock = (
    <div className={cn("flex min-w-0 flex-col justify-center gap-1", inline ? "shrink-0" : "flex-1")} aria-label={clock ? `${time} Eastern, ${progress}` : "Clock not started"} role="group">
      <span className="font-mono text-[15px] font-medium leading-none tabular-nums" aria-live="off">
        {time}
      </span>
      <span className={cn("truncate text-[10px] leading-none text-muted-foreground", inline && "max-w-[8.5rem]")}>{progress}</span>
    </div>
  );
  const micro = span ? (
    <ReplayScrubber span={span} segments={segments} bars={microBars} ticks={[]} dots={dots} now={now} onSeek={onSeek} disabled={!controllable} tooltip="drag" variant="micro" className={inline ? "h-11 flex-1" : "h-6"} />
  ) : (
    <div className={cn("grid place-items-center text-[10px] text-text-faint", inline ? "h-11 flex-1" : "h-6")}>{clock ? "No session for this day" : "Waiting for the replay clock…"}</div>
  );
  const speedChip = (
    <Button
      variant="secondary"
      className="h-11 min-w-11 shrink-0 rounded-xl px-2 text-[13px] font-semibold tabular-nums pointer-coarse:h-11"
      disabled={!controllable}
      aria-label={`Speed ${replaySpeedLabel(speed)}, change to ${replaySpeedLabel(nextReplaySpeed(speed))}`}
      onClick={() => onSpeedChange(nextReplaySpeed(speed))}
    >
      {replaySpeedLabel(speed)}
    </Button>
  );
  const more = (
    <PopoverTrigger
      aria-label="More replay controls"
      className="relative inline-flex size-11 shrink-0 cursor-pointer items-center justify-center rounded-xl text-muted-foreground outline-none transition-colors duration-150 hover:bg-bg-row hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring data-popup-open:bg-bg-row data-popup-open:text-foreground [&_svg]:size-[18px]"
    >
      <Icon name="more" />
    </PopoverTrigger>
  );
  const row = (
    <div className="flex items-center gap-1.5">
      {play}
      {alertButton(jumps[2]!, "alertPrev")}
      {alertButton(jumps[3]!, "alertNext")}
      <span className="w-0.5 shrink-0" />
      {clockBlock}
      {details && (
        <>
          {dayButton("ml-1.5")}
          {tickerButton()}
        </>
      )}
      {inline ? <div className="mx-1.5 flex min-w-0 flex-1">{micro}</div> : null}
      {speedChip}
      {more}
    </div>
  );

  return (
    // Controlled: the trigger toggles it; Escape and a press outside the bar close it. A press on
    // the bar itself (play, Prev/Next, the thin scrubber) leaves it open, so the panel can stay up
    // while practising, and focus leaving it (to the ticker search) does not close it either.
    <Popover
      open={expanded}
      onOpenChange={(o, d) => {
        if (!o && d.reason === "focus-out") return;
        if (!o && d.reason === "outside-press" && d.event.target instanceof Node && ref.current?.contains(d.event.target)) return;
        setExpanded(o);
      }}
    >
      <div ref={attach} data-layout={layout} data-expanded={expanded || undefined} className={cn("flex flex-col", inline ? "px-2.5 py-2.5" : "px-2 pb-1.5", className)}>
        {!inline && <div className="px-1">{micro}</div>}
        {row}
      </div>
      <PopoverContent
        anchor={ref}
        side="top"
        align="end"
        sideOffset={6}
        aria-label="Replay controls"
        className={cn("shadow-[0_-8px_28px_rgb(0_0_0/0.4)]", layout === "cover" ? "w-(--anchor-width)" : "w-[min(560px,var(--anchor-width))]")}
      >
        <div className="flex flex-col gap-3">
          {!details && (
            <div className="flex gap-2">
              {dayButton("min-w-0 flex-1")}
              {tickerButton("min-w-0 flex-1 justify-start")}
            </div>
          )}
          <div className="flex flex-col gap-1.5">
            <span className="text-xs text-text-faint">Jump to</span>
            <div className="flex flex-wrap gap-1.5">{jumpButtons}</div>
          </div>
          {speeds}
          {scrubber}
        </div>
      </PopoverContent>
    </Popover>
  );
}
