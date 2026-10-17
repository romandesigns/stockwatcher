import { TIMEFRAMES, type Timeframe } from "@stockwatcher/chart-core";
import { useEffect, useRef } from "react";
import { StyleSheet, View } from "react-native";
import { GestureDetector, usePanGesture } from "react-native-gesture-handler";
import Animated, { Extrapolation, interpolate, ReduceMotion, useAnimatedStyle, useSharedValue, withTiming, type SharedValue } from "react-native-reanimated";
import { playHaptic, UI } from "./ui.tsx";

/** Row pitch: less than half the wheel, so the neighbours peek in above and below the current one. */
const ITEM = 15;
const HEIGHT = 36;
const WIDTH = 46;
/** Past either end the wheel gives a little, then stops: rubber band, not a wall. */
const OVERSCROLL = 0.35;

function Item({ label, index, pos }: { label: string; index: number; pos: SharedValue<number> }) {
  // Centre item full strength; neighbours fade and shrink a little, like a picker wheel's drum.
  const style = useAnimatedStyle(() => {
    const d = Math.abs(pos.value - index);
    return {
      opacity: interpolate(d, [0, 1, 1.6], [1, 0.32, 0], Extrapolation.CLAMP),
      transform: [{ scale: interpolate(d, [0, 1], [1, 0.82], Extrapolation.CLAMP) }],
    };
  });
  return <Animated.Text style={[s.item, style]}>{label}</Animated.Text>;
}

/**
 * The phone's timeframe picker as a small wheel: swipe up for the next
 * (longer) timeframe, down for the previous. The neighbours peek above and
 * below; each step clicks with a selection haptic; a release settles on the
 * nearest one. Screen readers adjust it with swipe up/down. One control the
 * size of a button, instead of four segments.
 */
export function TimeframeWheel({ value, onChange, haptics = true, reducedMotion = false }: { value: Timeframe; onChange: (tf: Timeframe) => void; haptics?: boolean; reducedMotion?: boolean }) {
  const index = Math.max(0, TIMEFRAMES.indexOf(value));
  const last = TIMEFRAMES.length - 1;
  const pos = useSharedValue(index);
  const start = useRef(index);
  const shown = useRef(index);
  const timing = { duration: reducedMotion ? 0 : 160, reduceMotion: ReduceMotion.System };

  // Follow the value when it changes from elsewhere (a restored preference, a symbol switch).
  useEffect(() => {
    shown.current = index;
    pos.value = withTiming(index, timing);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [index]);

  const settle = (to: number) => {
    const target = Math.min(last, Math.max(0, Math.round(to)));
    pos.value = withTiming(target, timing);
    if (target !== index) onChange(TIMEFRAMES[target]!);
  };

  const pan = usePanGesture({
    runOnJS: true,
    disableReanimated: true,
    activeOffsetY: [-4, 4],
    failOffsetX: [-14, 14],
    onActivate: () => {
      start.current = index;
    },
    onUpdate: (e) => {
      // Up (negative y) turns to longer timeframes.
      const raw = start.current - e.translationY / ITEM;
      const p = Math.min(last + OVERSCROLL, Math.max(-OVERSCROLL, raw));
      pos.value = p;
      const near = Math.min(last, Math.max(0, Math.round(p)));
      if (near !== shown.current) {
        shown.current = near;
        if (haptics) playHaptic("selection");
      }
    },
    onDeactivate: (e) => {
      // A flick carries one step further in its direction.
      const fling = Math.abs(e.velocityY) > 600 ? -Math.sign(e.velocityY) * 0.5 : 0;
      settle(start.current - e.translationY / ITEM + fling);
    },
  });

  const strip = useAnimatedStyle(() => ({ transform: [{ translateY: (HEIGHT - ITEM) / 2 - pos.value * ITEM }] }));

  return (
    <GestureDetector gesture={pan}>
      <View
        style={s.wheel}
        accessible
        accessibilityRole="adjustable"
        accessibilityLabel={`Timeframe ${value}`}
        accessibilityHint="Swipe up or down to change"
        accessibilityActions={[{ name: "increment" }, { name: "decrement" }]}
        onAccessibilityAction={(e) => settle(index + (e.nativeEvent.actionName === "increment" ? 1 : -1))}
      >
        <Animated.View style={strip}>
          {TIMEFRAMES.map((tf, i) => (
            <Item key={tf} label={tf} index={i} pos={pos} />
          ))}
        </Animated.View>
      </View>
    </GestureDetector>
  );
}

const s = StyleSheet.create({
  wheel: { width: WIDTH, height: HEIGHT, borderRadius: 9, backgroundColor: UI.row, overflow: "hidden" },
  // includeFontPadding off: Android pads text vertically, which pushed the centred label low.
  item: { height: ITEM, lineHeight: ITEM, includeFontPadding: false, textAlignVertical: "center", textAlign: "center", color: UI.text, fontSize: 13, fontWeight: "600", fontVariant: ["tabular-nums"] },
});
