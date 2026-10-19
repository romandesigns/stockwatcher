// "Tap, then press and hold" (owner request 2026-10-18): on a ticker, a double tap whose second
// press is held copies the ticker to the clipboard. One recognizer for the web (pointer events)
// and the phone (Pressable in/out), so both behave the same.
//
// A ticker label's single tap already does something (opens the search, loads the ticker into a
// chart), and a second tap can only be told apart by waiting for it. So a single tap is reported
// `doubleTapMs` after its release, unless a second press starts in that window. The second press
// then copies once it has been held `holdMs`; released earlier (a plain double tap), it counts as
// one tap. A first press held long is still a tap: only the SECOND press holds to copy.

export interface TapHoldHandlers {
  onTap(): void;
  onHold(): void;
}

export interface TapHoldOptions {
  /** Window after the first release in which a second press starts the hold (ms). */
  doubleTapMs?: number;
  /** How long the second press must be held to copy (ms). */
  holdMs?: number;
  setTimer?: (fn: () => void, ms: number) => unknown;
  clearTimer?: (handle: unknown) => void;
}

export interface TapHold {
  down(): void;
  up(): void;
  /** The press left the target or was taken by a scroll: nothing fires. */
  cancel(): void;
  /** True right after a hold fired, until the next press: lets a host swallow the click that follows. */
  firedHold(): boolean;
}

export const TAP_HOLD_DEFAULTS = { doubleTapMs: 300, holdMs: 450 } as const;

export function createTapHold(handlers: TapHoldHandlers, opts: TapHoldOptions = {}): TapHold {
  const doubleTapMs = opts.doubleTapMs ?? TAP_HOLD_DEFAULTS.doubleTapMs;
  const holdMs = opts.holdMs ?? TAP_HOLD_DEFAULTS.holdMs;
  const setTimer = opts.setTimer ?? ((fn, ms) => setTimeout(fn, ms));
  const clearTimer = opts.clearTimer ?? ((h) => clearTimeout(h as ReturnType<typeof setTimeout>));
  let phase: "idle" | "first" | "waiting" | "second" | "fired" = "idle";
  let timer: unknown = null;
  let fired = false;
  const stop = () => {
    if (timer !== null) clearTimer(timer);
    timer = null;
  };
  return {
    down() {
      fired = false;
      if (phase === "waiting") {
        stop();
        phase = "second";
        timer = setTimer(() => {
          timer = null;
          phase = "fired";
          fired = true;
          handlers.onHold();
        }, holdMs);
      } else {
        stop();
        phase = "first";
      }
    },
    up() {
      if (phase === "first") {
        phase = "waiting";
        timer = setTimer(() => {
          timer = null;
          phase = "idle";
          handlers.onTap();
        }, doubleTapMs);
      } else if (phase === "second") {
        // A plain double tap: one tap.
        stop();
        phase = "idle";
        handlers.onTap();
      } else if (phase === "fired") {
        phase = "idle";
      }
    },
    cancel() {
      stop();
      phase = "idle";
    },
    firedHold: () => fired,
  };
}
