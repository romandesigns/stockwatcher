import { Icon, UI } from "@stockwatcher/chart-native";
import { useEffect, useSyncExternalStore } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";
import Animated, { FadeIn, FadeOut, ReduceMotion } from "react-native-reanimated";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { colors, fonts } from "../theme";
import { dismissToast, getToast, runToastAction, subscribeToast, UNDO_MS } from "./toast";

/** A plain toast's time fully visible, then the fade (ms). */
const SHOW_MS = 1300;
const FADE_MS = 300;
/** Clear of the main nav's tab bar (48 pt items + padding). */
const ABOVE_TAB_BAR = 72;

/**
 * The current toast at the bottom centre, above the tab bar: it fades in, stays (~1.3 s, or 5 s
 * with an action such as an alert's Undo) and fades out (no movement; with reduced motion it
 * simply appears and goes). Polite live region, so TalkBack/VoiceOver read it. Only the action
 * button takes touches; the rest lets them through. Rendered once, at the app's root.
 */
export function Toaster() {
  const insets = useSafeAreaInsets();
  const toast = useSyncExternalStore(subscribeToast, getToast);
  useEffect(() => {
    if (!toast) return;
    const t = setTimeout(() => dismissToast(toast.id), toast.ms ?? (toast.action ? UNDO_MS : SHOW_MS));
    return () => clearTimeout(t);
  }, [toast]);
  return (
    <View pointerEvents="box-none" style={[styles.layer, { bottom: insets.bottom + ABOVE_TAB_BAR }]}>
      {toast && (
        <Animated.View
          key={toast.id}
          pointerEvents="box-none"
          entering={FadeIn.duration(150).reduceMotion(ReduceMotion.System)}
          exiting={FadeOut.duration(FADE_MS).reduceMotion(ReduceMotion.System)}
          accessibilityLiveRegion="polite"
          accessibilityRole="alert"
          style={[styles.pill, toast.action && styles.withAction]}
        >
          <Icon name={toast.icon ?? "check"} color={colors.accent} size={14} />
          <Text style={styles.text} numberOfLines={2}>
            {toast.text}
          </Text>
          {toast.action && (
            <Pressable accessibilityRole="button" onPress={() => runToastAction()} hitSlop={8} style={({ pressed }) => [styles.action, pressed && { backgroundColor: UI.row }]}>
              <Text style={styles.actionText}>{toast.action.label}</Text>
            </Pressable>
          )}
        </Animated.View>
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  layer: { position: "absolute", left: 16, right: 16, alignItems: "center" },
  pill: { flexDirection: "row", alignItems: "center", gap: 8, maxWidth: "100%", backgroundColor: UI.raised, borderRadius: 999, paddingHorizontal: 14, paddingVertical: 9, boxShadow: "0 6px 18px rgba(0,0,0,0.35)" },
  withAction: { paddingVertical: 5, paddingRight: 5 },
  text: { flexShrink: 1, color: colors.text, fontFamily: fonts.semibold, fontSize: 14 },
  // 36 pt tall inside the pill, 44+ with its hit slop.
  action: { height: 36, paddingHorizontal: 12, borderRadius: 18, alignItems: "center", justifyContent: "center" },
  actionText: { color: colors.accent, fontFamily: fonts.semibold, fontSize: 14 },
});
