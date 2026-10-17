import { formatEtDate, formatEtTime, sessionOf, type Clock } from "@stockwatcher/chart-core";
import { Button } from "./ui/button.tsx";
import { cn } from "./ui/utils.ts";

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
}

export const DEFAULT_SPEEDS = [1, 2, 10, 60, 0] as const;

export const speedLabel = (s: number) => (s === 0 ? "Max" : `${s}×`);

const SESSION_LABEL = { overnight: "Overnight", pre: "Premarket", regular: "Regular", after: "After hours" } as const;

function PlayIcon({ playing }: { playing: boolean }) {
  return playing ? (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
      <rect x="6" y="5" width="4" height="14" />
      <rect x="14" y="5" width="4" height="14" />
    </svg>
  ) : (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
      <path d="M7 5v14l12-7z" />
    </svg>
  );
}

/**
 * The replay transport, one design on every screen (owner: simplify across
 * the board): play/pause, the virtual clock with its session, then on the
 * right a step button (shown only while paused, the only time it acts) and
 * one speed chip that cycles 1× → 2× → 10× → 60× → Max. "full" (desktop)
 * adds the replay date and uses slightly smaller controls. In live mode the
 * transport is disabled: live time cannot be paused.
 */
export function ReplayControls({ clock, onPlayPause, onStep, onSpeedChange, speeds = DEFAULT_SPEEDS, variant = "full", className }: ReplayControlsProps) {
  const live = clock?.mode === "live";
  const playing = !!clock?.playing;
  const speed = clock?.speed ?? speeds[0] ?? 1;
  const time = clock ? formatEtTime(clock.now, true) : "--:--:--";
  const session = clock ? SESSION_LABEL[sessionOf(clock.now)] : "";
  const sub = clock && variant === "full" ? `${session} · ${formatEtDate(clock.now)}` : session;
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
      </span>
      {!playing && !live && clock && (
        <Button
          className={cn(size, "shrink-0 rounded-lg transition-opacity duration-150 starting:opacity-0")}
          size="icon"
          variant="secondary"
          aria-label="Step one event"
          onClick={onStep}
        >
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden="true">
            <path d="M18 5v14" />
            <path d="M6 6l9 6-9 6z" />
          </svg>
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
