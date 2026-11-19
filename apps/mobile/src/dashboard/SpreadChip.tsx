import { UI } from "@stockwatcher/chart-native";
import type { SpreadTone, SpreadView } from "@stockwatcher/shared";
import { useEffect, useState } from "react";
import { StyleSheet, Text } from "react-native";
import Animated, { useAnimatedStyle, useSharedValue, withTiming } from "react-native-reanimated";

import { collapseTiming } from "../motion/transitions";
import { colors, fonts } from "../theme";

/**
 * The cost-band colours (packages/shared spread.ts): status colours only, never the accent. A
 * normal spread is a quiet raised chip; a warning or danger one takes its status tint, so the
 * chip draws the eye only when the cost is the news. Fills, never a border.
 */
const LOOK: Record<SpreadTone, { bg: string; fg: string }> = {
  muted: { bg: UI.raised, fg: UI.textDim },
  warning: { bg: colors.warningBg, fg: UI.warning },
  critical: { bg: colors.criticalBg, fg: UI.critical },
};

/** The widest the chip gets ("Spread 12.3%" at 10 dp): the open width's animation target. */
const MAX_W = 96;

/**
 * "Spread 0.42%" (owner, 2026-11-19): what the alert cost to trade at fire time, from the server's
 * NBBO (crates/server/src/spread.rs). The server sends it a moment after the row appears (it asks
 * for the quote once the alert fired), so the chip opens by animating its real width and opacity
 * (the web's Reveal, on the motion module's collapse timing; reduced motion snaps) instead of
 * squeezing the momentum word in one frame. A layout animation, not a Reanimated layout
 * transition, so taps on the row land where it is drawn. No spread shows nothing; the band is
 * also in the accessible name ("costly to trade"), so colour is never the only signal.
 */
export function SpreadChip({ spread, reducedMotion }: { spread: SpreadView | null; reducedMotion: boolean }) {
  const open = spread !== null;
  const progress = useSharedValue(open ? 1 : 0);
  useEffect(() => {
    progress.set(reducedMotion ? (open ? 1 : 0) : withTiming(open ? 1 : 0, collapseTiming));
  }, [open, reducedMotion, progress]);
  // The last spread, drawn while the chip closes ("state from the previous render").
  const [held, setHeld] = useState<SpreadView | null>(spread);
  if (open && held !== spread) setHeld(spread);
  const animated = useAnimatedStyle(() => ({ maxWidth: progress.get() * MAX_W, opacity: progress.get() }));
  const shown = open ? spread : held;
  if (!shown) return null;
  const look = LOOK[shown.tone];
  return (
    <Animated.View style={[styles.clip, animated]} accessible={open} accessibilityElementsHidden={!open} importantForAccessibility={open ? "auto" : "no-hide-descendants"} accessibilityLabel={shown.aria}>
      <Text numberOfLines={1} style={[styles.chip, { backgroundColor: look.bg, color: look.fg, fontFamily: shown.band === "danger" ? fonts.semibold : fonts.regular }]}>
        {shown.label}
      </Text>
    </Animated.View>
  );
}

const styles = StyleSheet.create({
  clip: { flexShrink: 0, overflow: "hidden" },
  chip: { borderRadius: 4, paddingHorizontal: 5, paddingVertical: 1, fontSize: 10, fontVariant: ["tabular-nums"], overflow: "hidden" },
});
