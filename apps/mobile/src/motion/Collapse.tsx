/**
 * Content that opens and closes by animating its real height (owner, 2026-10-22: a card or
 * section with 0 items is not shown, and the layout animates when one appears or disappears).
 *
 * Entering/exiting keyframes animate opacity and transforms only, so on their own a card that
 * appears or disappears still shoves everything below it in one frame. Here the children are
 * measured at their natural height in an absolutely positioned layer (so the collapsed parent
 * cannot squeeze the measurement) and the clipping parent's height follows a 0..1 progress,
 * fading as it goes. Because it is layout, not a transform, siblings move with it and taps land
 * where things are drawn: no Reanimated layout transition, which on Android left taps on a moved
 * Pressable hitting stale frames (fired twice or not at all).
 *
 * While closing it keeps drawing the last children it had open, so a list that just emptied
 * shrinks away with its last rows instead of going blank first. Content that was never open is
 * not rendered. Reduced motion: it snaps, like every spec in transitions.ts.
 *
 * `gap` is the parent's gap: a closed Collapse would still take one, so it is cancelled by a
 * negative margin and given back as padding while open.
 */
import { useEffect, useState, type ReactNode } from "react";
import { View } from "react-native";
import Animated, { useAnimatedStyle, useSharedValue, withTiming } from "react-native-reanimated";

import { collapseTiming } from "./transitions";

export function Collapse({ open, reducedMotion, gap = 0, children }: { open: boolean; reducedMotion: boolean; gap?: number; children: ReactNode }) {
  const height = useSharedValue(0);
  const progress = useSharedValue(open ? 1 : 0);
  useEffect(() => {
    progress.set(reducedMotion ? (open ? 1 : 0) : withTiming(open ? 1 : 0, collapseTiming));
  }, [open, reducedMotion, progress]);
  // The last open children, drawn while closing ("state from the previous render").
  const [held, setHeld] = useState<ReactNode>(open ? children : null);
  if (open && held !== children) setHeld(children);
  const animated = useAnimatedStyle(() => ({ height: height.get() * progress.get(), opacity: progress.get() }));
  return (
    <Animated.View
      style={[{ overflow: "hidden", marginBottom: -gap }, animated]}
      // Closed content is still mounted while it shrinks; keep it out of the reader's way.
      accessibilityElementsHidden={!open}
      importantForAccessibility={open ? "auto" : "no-hide-descendants"}
      pointerEvents={open ? "auto" : "none"}
    >
      <View style={{ position: "absolute", top: 0, left: 0, right: 0, paddingBottom: gap }} onLayout={(e) => height.set(e.nativeEvent.layout.height)}>
        {open ? children : held}
      </View>
    </Animated.View>
  );
}
