import { etParts, formatEtDate, formatEtTime, sessionOf, type Clock } from "@stockwatcher/chart-core";
import { Button } from "./ui/button.tsx";
import { ToggleGroup, ToggleGroupItem } from "./ui/toggle-group.tsx";
import { cn } from "./ui/utils.ts";

export interface ReplayControlsProps {
  clock: Clock | null;
  onPlayPause: (play: boolean) => void;
  /** Advance exactly one event. */
  onStep: () => void;
  onSpeedChange: (speed: number) => void;
  /** Multipliers; 0 = as fast as possible ("Max"). */
  speeds?: readonly number[];
  /** "full": speed chips (desktop). "compact": one button cycles speed (phone). */
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
 * Play/pause, step one event, replay speed, and the virtual clock in ET.
 * In live mode the transport is disabled: live time cannot be paused.
 */
export function ReplayControls({ clock, onPlayPause, onStep, onSpeedChange, speeds = DEFAULT_SPEEDS, variant = "full", className }: ReplayControlsProps) {
  const live = clock?.mode === "live";
  const playing = !!clock?.playing;
  const speed = clock?.speed ?? speeds[0] ?? 1;
  const time = clock ? formatEtTime(clock.now, true) : "--:--:--";
  const sub = clock ? (variant === "full" ? `ET · ${etParts(clock.now).abbr} · ${formatEtDate(clock.now)}` : `ET · ${SESSION_LABEL[sessionOf(clock.now)]}`) : "ET";
  const size = variant === "full" ? "size-8" : "size-9";
  const nextSpeed = () => {
    const i = speeds.indexOf(speed);
    onSpeedChange(speeds[(i + 1) % speeds.length] ?? speeds[0]!);
  };

  return (
    <div className={cn("flex flex-wrap items-center gap-2", className)}>
      <Button className={cn(size, "rounded-lg")} size="icon" disabled={!clock || live} aria-label={playing ? "Pause replay" : "Play replay"} onClick={() => onPlayPause(!playing)}>
        <PlayIcon playing={playing} />
      </Button>
      {variant === "full" && (
        <Button className={cn(size, "rounded-lg")} size="icon" variant="secondary" disabled={!clock || live} aria-label="Step one event" onClick={onStep}>
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden="true">
            <path d="M18 5v14" />
            <path d="M6 6l9 6-9 6z" />
          </svg>
        </Button>
      )}
      <span className={cn("font-mono tabular-nums", variant === "full" ? "text-sm" : "text-[13px]")} aria-live="off">
        {time}
      </span>
      <span className={cn("text-muted-foreground", "text-[10px]")}>{sub}</span>
      {variant === "full" ? (
        <ToggleGroup aria-label="Replay speed" size="sm" spacing={0.5} variant="segmented" value={[String(speed)]} onValueChange={(v) => v[0] !== undefined && onSpeedChange(Number(v[0]))} className="ml-auto" disabled={!clock || live}>
          {speeds.map((s) => (
            <ToggleGroupItem key={s} value={String(s)} className="min-w-9 px-2 font-mono text-[11px]">
              {speedLabel(s)}
            </ToggleGroupItem>
          ))}
        </ToggleGroup>
      ) : (
        <Button variant="secondary" className="ml-auto h-9 min-w-11 rounded-lg font-mono text-xs" disabled={!clock || live} aria-label={`Replay speed ${speedLabel(speed)}, change`} onClick={nextSpeed}>
          {speedLabel(speed)}
        </Button>
      )}
    </div>
  );
}
