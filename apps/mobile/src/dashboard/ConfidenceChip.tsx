import { Icon, UI } from "@stockwatcher/chart-native";
import type { ConfidenceView } from "@stockwatcher/shared";
import { useEffect, useState } from "react";
import { StyleSheet, Text } from "react-native";
import Animated, { useAnimatedStyle, useSharedValue, withTiming } from "react-native-reanimated";

import { collapseTiming } from "../motion/transitions";
import { fonts } from "../theme";

/** The widest the chip gets (icon + "High confidence" at 10 dp): the open width's animation target. */
const MAX_W = 112;

/**
 * "High confidence" (owner, 2026-11-22; research v2 b-good-1, crates/server/src/bgood): the model's
 * top tier for this alert, decided 30 s after it fired, the web's ConfidenceChip on the phone. It
 * always arrives after its row, so it opens by animating its real width and opacity on the motion
 * module's collapse timing (reduced motion snaps), as SpreadChip does; a layout animation, not a
 * Reanimated layout transition, so taps on the row land where it is drawn.
 *
 * About 4 alerts in 10 are tagged, so the chip is a quiet raised fill with the two-tone research
 * mark (`insights`, every research detector's icon; the star is the favourites toggle) in the
 * text colour, never the accent (60/30/10). No borders. `compact` (the Fold's cover screen, a
 * narrow row measured by the caller) says "High" instead of "High confidence"; the accessible name
 * keeps the whole sentence (packages/shared confidence.ts).
 */
export function ConfidenceChip({ confidence, reducedMotion, compact = false }: { confidence: ConfidenceView | null; reducedMotion: boolean; compact?: boolean }) {
  const open = confidence !== null;
  const progress = useSharedValue(open ? 1 : 0);
  useEffect(() => {
    progress.set(reducedMotion ? (open ? 1 : 0) : withTiming(open ? 1 : 0, collapseTiming));
  }, [open, reducedMotion, progress]);
  // The last tag, drawn while the chip closes ("state from the previous render").
  const [held, setHeld] = useState<ConfidenceView | null>(confidence);
  if (open && held !== confidence) setHeld(confidence);
  const animated = useAnimatedStyle(() => ({ maxWidth: progress.get() * MAX_W, opacity: progress.get() }));
  const shown = open ? confidence : held;
  if (!shown) return null;
  return (
    <Animated.View
      style={[styles.clip, animated]}
      accessible={open}
      accessibilityElementsHidden={!open}
      importantForAccessibility={open ? "auto" : "no-hide-descendants"}
      accessibilityLabel={shown.aria}
      testID="confidence-chip"
    >
      <Animated.View style={styles.chip}>
        <Icon name="insights" color={UI.text} size={11} />
        <Text numberOfLines={1} style={styles.text}>
          {compact ? shown.short : shown.label}
        </Text>
      </Animated.View>
    </Animated.View>
  );
}

const styles = StyleSheet.create({
  clip: { flexShrink: 0, overflow: "hidden" },
  chip: { flexDirection: "row", alignItems: "center", gap: 3, borderRadius: 4, paddingHorizontal: 5, paddingVertical: 1, backgroundColor: UI.raised },
  text: { fontSize: 10, color: UI.text, fontFamily: fonts.regular },
});
