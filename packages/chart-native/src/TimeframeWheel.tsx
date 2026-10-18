import { TIMEFRAMES, type Timeframe } from "@stockwatcher/chart-core";
import { useEffect, useRef } from "react";
import { StyleSheet, View } from "react-native";
import { GestureDetector, usePanGesture } from "react-native-gesture-handler";
import Animated, { Extrapolation, interpolate, ReduceMotion, useAnimatedStyle, useSharedValue, withTiming, type SharedValue } from "react-native-reanimated";
import Svg, { Defs, LinearGradient, Rect, Stop } from "react-native-svg";
import { playHaptic, UI } from "./ui.tsx";

/** Finger travel for one step: a short swipe turns one notch. */
const STEP_PX = 18;
const HEIGHT = 36;
/** Label row pitch inside the window; only the current one shows, the others slide past out of sight. */
const LABEL = 18;
/** Past either end the wheel gives a little, then stops: rubber band, not a wall. */
const OVERSCROLL = 0.35;
/** The wheel graphic: a slim drum with ridges. */
const DRUM_W = 10;
const DRUM_H = 24;
const RIDGE = 3.5;
const RIDGES = Math.ceil(DRUM_H / RIDGE) + 2;

function Label({ text, index, pos }: { text: string; index: number; pos: SharedValue<number> }) {
  // Only the current label is visible; a neighbour fades in as the wheel turns towards it.
  const style = useAnimatedStyle(() => ({ opacity: interpolate(Math.abs(pos.value - index), [0, 0.6], [1, 0], Extrapolation.CLAMP) }));
  return <Animated.Text style={[s.label, style]}>{text}</Animated.Text>;
}

/**
 * The phone's timeframe picker, a scroll wheel: swipe up for the next
 * (longer) timeframe, down for the previous. It looks like a mouse wheel:
 * a slim ridged drum, shaded at its ends, whose ridges roll with the finger,
 * beside the current timeframe, which slides as it changes. Each step clicks
 * with a selection haptic; a release settles on the nearest timeframe; a
 * flick carries one more. Screen readers adjust it with swipe up/down. It is
 * a toolbar button in size, so it sits with the other controls, but has no
 * surface: it blends into the app background (owner, 2026-10-18).
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
      const raw = start.current - e.translationY / STEP_PX;
      const p = Math.min(last + OVERSCROLL, Math.max(-OVERSCROLL, raw));
      pos.value = p;
      const near = Math.min(last, Math.max(0, Math.round(p)));
      if (near !== shown.current) {
        shown.current = near;
        if (haptics) playHaptic("selection");
      }
    },
    onDeactivate: (e) => {
      const fling = Math.abs(e.velocityY) > 600 ? -Math.sign(e.velocityY) * 0.5 : 0;
      settle(start.current - e.translationY / STEP_PX + fling);
    },
  });

  // Labels slide with the turn: up for longer timeframes.
  const labels = useAnimatedStyle(() => ({ transform: [{ translateY: (HEIGHT - LABEL) / 2 - pos.value * LABEL }] }));
  // Ridges roll the same way as the finger, wrapping every ridge spacing, so the drum looks like it turns.
  const ridges = useAnimatedStyle(() => {
    const travel = -pos.value * STEP_PX * 0.5;
    const phase = ((travel % RIDGE) + RIDGE) % RIDGE;
    return { transform: [{ translateY: phase - RIDGE }] };
  });

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
        <View style={s.window}>
          <Animated.View style={labels}>
            {TIMEFRAMES.map((tf, i) => (
              <Label key={tf} text={tf} index={i} pos={pos} />
            ))}
          </Animated.View>
        </View>
        <View style={s.drum}>
          <Animated.View style={ridges}>
            {Array.from({ length: RIDGES }, (_, i) => (
              <View key={i} style={s.ridge} />
            ))}
          </Animated.View>
          {/* Curvature: the drum darkens towards its ends, as a wheel turning away from you does. */}
          <Svg style={StyleSheet.absoluteFill} width={DRUM_W} height={DRUM_H}>
            <Defs>
              <LinearGradient id="drum" x1="0" y1="0" x2="0" y2="1">
                <Stop offset="0" stopColor={UI.row} stopOpacity={1} />
                <Stop offset="0.32" stopColor={UI.row} stopOpacity={0} />
                <Stop offset="0.68" stopColor={UI.row} stopOpacity={0} />
                <Stop offset="1" stopColor={UI.row} stopOpacity={1} />
              </LinearGradient>
            </Defs>
            <Rect x={0} y={0} width={DRUM_W} height={DRUM_H} fill="url(#drum)" />
          </Svg>
        </View>
      </View>
    </GestureDetector>
  );
}

const s = StyleSheet.create({
  // A toolbar button's size, but no surface of its own (owner, 2026-10-18): it sits on the app
  // background, and the drum alone says "wheel".
  wheel: { height: HEIGHT, flexDirection: "row", alignItems: "center", gap: 8, paddingLeft: 12, paddingRight: 9, borderRadius: 9 },
  window: { width: 26, height: HEIGHT, overflow: "hidden" },
  // includeFontPadding off: Android pads text vertically, which pushes a centred label low.
  label: { height: LABEL, lineHeight: LABEL, includeFontPadding: false, textAlignVertical: "center", textAlign: "center", color: UI.text, fontSize: 12, fontWeight: "600", fontVariant: ["tabular-nums"] },
  drum: { width: DRUM_W, height: DRUM_H, borderRadius: DRUM_W / 2, backgroundColor: UI.raised, overflow: "hidden" },
  ridge: { height: RIDGE, borderBottomWidth: 1, borderBottomColor: "rgba(228,232,237,0.28)" },
});
