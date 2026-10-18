import { formatDayKey, formatEtDate, formatEtTime, formatSymbolCount, sessionOf, type Clock, type ReplayDayOption } from "@stockwatcher/chart-core";
import { Button } from "./ui/button.tsx";
import { DropdownMenu, DropdownMenuContent, DropdownMenuLabel, DropdownMenuRadioGroup, DropdownMenuRadioItem, DropdownMenuTrigger } from "./ui/dropdown-menu.tsx";
import { cn } from "./ui/utils.ts";
import { Icon } from "./Icon.tsx";

export interface ReplayControlsProps {
  clock: Clock | null;
  onPlayPause: (play: boolean) => void;
  /** Advance exactly one event. */
  onStep: () => void;
  onSpeedChange: (speed: number) => void;
  /** Multipliers; 0 = as fast as possible ("Max"). */
  speeds?: readonly number[];
  /** "full": desktop sizing, with the replay date. "compact": phone sizing. The controls are the same. */
  variant?: "full" | "compact";
  className?: string;
  /** The archive days the server can switch to (chart-core `replayDayOptions`). With none (the
   *  in-browser fixture, live mode, an older server) the date is plain text, not a menu. */
  days?: readonly ReplayDayOption[];
  /** The day being replayed, "YYYY-MM-DD" (chart-core `currentReplayDay`). */
  day?: string | null;
  /** A day was picked: send `replay.control {action: "day", value}`. */
  onDayChange?: (day: string) => void;
  /** The day menu opened or closed; opening is the moment to refresh `days`. */
  onDayMenuOpenChange?: (open: boolean) => void;
}

export const DEFAULT_SPEEDS = [1, 2, 10, 60, 0] as const;

export const speedLabel = (s: number) => (s === 0 ? "Max" : `${s}×`);

const SESSION_LABEL = { overnight: "Overnight", pre: "Premarket", regular: "Regular", after: "After hours" } as const;

/** Play and pause share one duotone family (soft body, crisp outline) with step, so the transport reads as a set. */
function PlayIcon({ playing }: { playing: boolean }) {
  return <Icon name={playing ? "pause" : "play"} />;
}

/**
 * The replay date as a menu of the archive's days, most recent first. It sits
 * where the date always was and reads as the same small dim text until hovered
 * (no chip, no border: the bar stays quiet); the chevron is the only hint. The
 * check on the current day is the menu's one accent. Fixture days say so in the
 * warning tone, as the source chip does, so synthetic data never passes for a
 * real session.
 */
function ReplayDayMenu({ day, days, onDayChange, onOpenChange }: { day: string; days: readonly ReplayDayOption[]; onDayChange: (day: string) => void; onOpenChange?: (open: boolean) => void }) {
  const current = days.find((d) => d.value === day);
  return (
    <DropdownMenu onOpenChange={(open) => onOpenChange?.(open)}>
      <DropdownMenuTrigger
        aria-label={`Replay day ${current?.longLabel ?? formatDayKey(day)}, change`}
        className="-mx-1 inline-flex shrink-0 cursor-pointer items-center gap-0.5 rounded px-1 text-[11px] text-muted-foreground outline-none transition-colors duration-150 hover:bg-bg-row hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring data-popup-open:bg-bg-row data-popup-open:text-foreground"
      >
        {formatDayKey(day)}
        <Icon name="chevronDown" width={10} height={10} />
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

/**
 * The replay transport, one design on every screen (owner: simplify across
 * the board): play/pause, the virtual clock with its session, then on the
 * right a step button (shown only while paused, the only time it acts) and
 * one speed chip that cycles 1× → 2× → 10× → 60× → Max. "full" (desktop)
 * adds the replay date and uses slightly smaller controls. In live mode the
 * transport is disabled: live time cannot be paused.
 *
 * When the server offers days, the date is the day picker (ReplayDayMenu), on
 * both variants: the phone needs to pick a day as much as the desktop does.
 * Picking one changes the whole replay (the server rebuilds as for a seek), so
 * it is a deliberate two-step menu rather than a cycling chip like speed.
 */
export function ReplayControls({ clock, onPlayPause, onStep, onSpeedChange, speeds = DEFAULT_SPEEDS, variant = "full", className, days, day, onDayChange, onDayMenuOpenChange }: ReplayControlsProps) {
  const live = clock?.mode === "live";
  const playing = !!clock?.playing;
  const speed = clock?.speed ?? speeds[0] ?? 1;
  const time = clock ? formatEtTime(clock.now, true) : "--:--:--";
  const session = clock ? SESSION_LABEL[sessionOf(clock.now)] : "";
  // The day menu needs days to offer, a day to mark, somewhere to send the pick, and replay mode.
  const pick = !live && clock && day && days?.length && onDayChange ? { day, days, onDayChange } : null;
  const sub = clock && variant === "full" && !pick ? `${session} · ${day ? formatDayKey(day) : formatEtDate(clock.now)}` : session;
  const size = variant === "full" ? "size-8" : "size-9";
  const nextSpeed = () => {
    const i = speeds.indexOf(speed);
    onSpeedChange(speeds[(i + 1) % speeds.length] ?? speeds[0]!);
  };

  return (
    <div className={cn("flex items-center gap-2", className)}>
      <Button className={cn(size, "shrink-0 rounded-lg")} size="icon" disabled={!clock || live} aria-label={playing ? "Pause replay" : "Play replay"} onClick={() => onPlayPause(!playing)}>
        <PlayIcon playing={playing} />
      </Button>
      {/* The clock takes the free width, so step and speed stay pinned right and nothing shifts when step appears. */}
      <span className="flex min-w-0 flex-1 items-baseline gap-2" aria-label={clock ? `${time} Eastern, ${session}` : "Clock not started"}>
        <span className={cn("font-mono tabular-nums", variant === "full" ? "text-sm" : "text-[13px]")} aria-live="off">
          {time}
        </span>
        <span className="truncate text-[11px] text-muted-foreground">{sub}</span>
        {pick && (
          <>
            <span className="text-[11px] text-muted-foreground" aria-hidden="true">
              ·
            </span>
            <ReplayDayMenu day={pick.day} days={pick.days} onDayChange={pick.onDayChange} onOpenChange={onDayMenuOpenChange} />
          </>
        )}
      </span>
      {!playing && !live && clock && (
        <Button
          className={cn(size, "shrink-0 rounded-lg transition-opacity duration-150 starting:opacity-0")}
          size="icon"
          variant="secondary"
          aria-label="Step one event"
          onClick={onStep}
        >
          <Icon name="step" />
        </Button>
      )}
      <Button
        variant="secondary"
        className={cn("shrink-0 rounded-lg font-mono text-xs", variant === "full" ? "h-8 min-w-11" : "h-9 min-w-11")}
        disabled={!clock || live}
        aria-label={`Replay speed ${speedLabel(speed)}, change`}
        onClick={nextSpeed}
      >
        {speedLabel(speed)}
      </Button>
    </div>
  );
}
