import { Icon, UI } from "@stockwatcher/chart-native";
import type { QuoteWarningView } from "@stockwatcher/shared";
import { useEffect, useState } from "react";
import { StyleSheet, Text } from "react-native";
import Animated, { useAnimatedStyle, useSharedValue, withTiming } from "react-native-reanimated";

import { collapseTiming } from "../motion/transitions";
import { colors, fonts } from "../theme";

/** The widest the chip gets (icon + "No live quote" at 10 dp): the open width's animation target. */
const MAX_W = 104;

/**
 * "No live quote" (owner, 2026-11-22; research v2 W2-REPORT §2, packages/shared quoteWarning.ts),
 * the web's NoQuoteChip on the phone: the alert fired with no usable bid/ask, and such alerts were
 * GOOD about half as often (11.5 % vs 21.5 %). It arrives a moment after its row (the server asks
 * for the quote once the alert fired), so it opens by animating its real width and opacity on the
 * motion module's collapse timing (reduced motion snaps), as SpreadChip does; a layout animation,
 * not a Reanimated layout transition, so taps on the row land where it is drawn.
 *
 * A caution: the warning status tint and the two-tone `quoteOff` mark, never the accent
 * (60/30/10). No borders. `compact` (the Fold's cover screen, a narrow row measured by the caller)
 * says "No quote"; the accessible name keeps the whole sentence.
 */
export function NoQuoteChip({ warning, reducedMotion, compact = false }: { warning: QuoteWarningView | null; reducedMotion: boolean; compact?: boolean }) {
  const open = warning !== null;
  const progress = useSharedValue(open ? 1 : 0);
  useEffect(() => {
    progress.set(reducedMotion ? (open ? 1 : 0) : withTiming(open ? 1 : 0, collapseTiming));
  }, [open, reducedMotion, progress]);
  // The last warning, drawn while the chip closes ("state from the previous render").
  const [held, setHeld] = useState<QuoteWarningView | null>(warning);
  if (open && held !== warning) setHeld(warning);
  const animated = useAnimatedStyle(() => ({ maxWidth: progress.get() * MAX_W, opacity: progress.get() }));
  const shown = open ? warning : held;
  if (!shown) return null;
  return (
    <Animated.View
      style={[styles.clip, animated]}
      accessible={open}
      accessibilityElementsHidden={!open}
      importantForAccessibility={open ? "auto" : "no-hide-descendants"}
      accessibilityLabel={shown.aria}
      testID="no-quote-chip"
    >
      <Animated.View style={styles.chip}>
        <Icon name="quoteOff" color={UI.warning} size={11} />
        <Text numberOfLines={1} style={styles.text}>
          {compact ? shown.short : shown.label}
        </Text>
      </Animated.View>
    </Animated.View>
  );
}

const styles = StyleSheet.create({
  clip: { flexShrink: 0, overflow: "hidden" },
  chip: { flexDirection: "row", alignItems: "center", gap: 3, borderRadius: 4, paddingHorizontal: 5, paddingVertical: 1, backgroundColor: colors.warningBg },
  text: { fontSize: 10, color: UI.warning, fontFamily: fonts.regular },
});
