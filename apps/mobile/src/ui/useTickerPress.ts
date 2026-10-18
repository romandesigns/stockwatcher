import { playHaptic } from "@stockwatcher/chart-native";
import { createTapHold } from "@stockwatcher/shared";
import * as Clipboard from "expo-clipboard";
import { useEffect, useLayoutEffect, useMemo, useRef } from "react";

import { showToast } from "./toast";

/** Copies a ticker, with a success haptic and a "Copied" toast. */
export function copyTicker(symbol: string): void {
  Clipboard.setStringAsync(symbol).then(
    () => {
      playHaptic("success");
      showToast(`Copied ${symbol}`);
    },
    () => showToast("Couldn't copy"),
  );
}

/**
 * Pressable handlers for a ticker label (owner request 2026-10-18; the web's useTickerPress):
 * a tap does `onTap` after the short double-tap window (@stockwatcher/shared tapHold.ts); tap,
 * then press and hold copies the ticker. Spread on a Pressable. A press the responder takes away
 * (the list starts scrolling) ends with onPressOut and no onPress: that cancels, so a scroll never
 * loads or copies anything.
 */
export function useTickerPress(symbol: string, onTap: () => void) {
  const latest = useRef({ symbol, onTap });
  useLayoutEffect(() => {
    latest.current = { symbol, onTap };
  });
  const th = useMemo(() => createTapHold({ onTap: () => latest.current.onTap(), onHold: () => copyTicker(latest.current.symbol) }), []);
  useEffect(() => () => th.cancel(), [th]);
  const completed = useRef(false);
  return {
    onPressIn: () => {
      completed.current = false;
      th.down();
    },
    onPress: () => {
      completed.current = true;
      th.up();
    },
    // RN calls onPressOut before onPress; wait a tick to see whether the press completed.
    onPressOut: () => {
      setTimeout(() => {
        if (!completed.current) th.cancel();
      }, 0);
    },
  };
}
