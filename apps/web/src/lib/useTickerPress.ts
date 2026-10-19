import { createTapHold } from "@stockwatcher/shared";
import { useEffect, useLayoutEffect, useMemo, useRef, type MouseEvent, type PointerEvent } from "react";
import { showToast } from "./toastStore.ts";

/** Copies a ticker and says so. Clipboard writes need a user gesture: the hold is one. */
export function copyTicker(symbol: string): void {
  void navigator.clipboard?.writeText(symbol).then(
    () => showToast(`Copied ${symbol}`),
    () => showToast("Couldn't copy: the browser refused the clipboard"),
  );
}

/**
 * Pointer handlers for a ticker label (owner request 2026-10-18): a tap does `onTap` (after the
 * short double-tap window, @stockwatcher/shared tapHold.ts); tap, then press and hold copies the
 * ticker with a "Copied" toast. Spread the result on the element. Keyboard activation (Enter,
 * Space) acts at once, as a button should. `touch-action: manipulation` and `select-none` on the
 * element keep the browser from zooming or selecting text on the double tap.
 */
export function useTickerPress(symbol: string, onTap: () => void) {
  const latest = useRef({ symbol, onTap });
  useLayoutEffect(() => {
    latest.current = { symbol, onTap };
  });
  const th = useMemo(() => createTapHold({ onTap: () => latest.current.onTap(), onHold: () => copyTicker(latest.current.symbol) }), []);
  useEffect(() => () => th.cancel(), [th]);
  return {
    onPointerDown: (e: PointerEvent) => {
      if (e.button === 0) th.down();
    },
    onPointerUp: () => th.up(),
    onPointerCancel: () => th.cancel(),
    // Leaving while pressed abandons the press; a touch lifting off fires pointerleave after
    // pointerup with no button held, which must not cancel the pending tap.
    onPointerLeave: (e: PointerEvent) => {
      if (e.buttons !== 0) th.cancel();
    },
    // Pointer clicks are the recognizer's; a keyboard "click" (detail 0) acts at once.
    onClick: (e: MouseEvent) => {
      if (e.detail === 0) latest.current.onTap();
    },
    // A touch press-and-hold would otherwise open the browser's context menu.
    onContextMenu: (e: MouseEvent) => e.preventDefault(),
  };
}
