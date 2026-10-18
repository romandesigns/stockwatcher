import { Icon, UI } from "@stockwatcher/chart-native";
import { useEffect, useSyncExternalStore } from "react";
import { StyleSheet, Text, View } from "react-native";
import Animated, { FadeIn, FadeOut, ReduceMotion } from "react-native-reanimated";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { colors, fonts } from "../theme";
import { dismissToast, getToast, subscribeToast } from "./toast";

/** Fully visible, then the fade (ms). */
const SHOW_MS = 1300;
const FADE_MS = 300;
/** Clear of the main nav's tab bar (48 pt items + padding). */
const ABOVE_TAB_BAR = 72;

/**
 * The current toast at the bottom centre, above the tab bar: it fades in, stays ~1.3 s and fades
 * out (no movement; with reduced motion it simply appears and goes). Polite live region, so
 * TalkBack/VoiceOver read it. Rendered once, at the app's root.
 */
export function Toaster() {
  const insets = useSafeAreaInsets();
  const toast = useSyncExternalStore(subscribeToast, getToast);
  useEffect(() => {
    if (!toast) return;
    const t = setTimeout(() => dismissToast(toast.id), SHOW_MS);
    return () => clearTimeout(t);
  }, [toast]);
  return (
    <View pointerEvents="none" style={[styles.layer, { bottom: insets.bottom + ABOVE_TAB_BAR }]}>
      {toast && (
        <Animated.View
          key={toast.id}
          entering={FadeIn.duration(150).reduceMotion(ReduceMotion.System)}
          exiting={FadeOut.duration(FADE_MS).reduceMotion(ReduceMotion.System)}
          accessibilityLiveRegion="polite"
          accessibilityRole="alert"
          style={styles.pill}
        >
          <Icon name="check" color={colors.accent} size={14} />
          <Text style={styles.text}>{toast.text}</Text>
        </Animated.View>
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  layer: { position: "absolute", left: 0, right: 0, alignItems: "center" },
  pill: { flexDirection: "row", alignItems: "center", gap: 8, backgroundColor: UI.raised, borderRadius: 999, paddingHorizontal: 14, paddingVertical: 9, boxShadow: "0 6px 18px rgba(0,0,0,0.35)" },
  text: { color: colors.text, fontFamily: fonts.semibold, fontSize: 14 },
});
