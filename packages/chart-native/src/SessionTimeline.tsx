import { Canvas, Picture } from "@shopify/react-native-skia";
import { DEFAULT_CHART_THEME, formatEtTime, layoutTimeline, sessionOf, timelineTimeAt, type ChartTheme, type SessionBand, type Size, type VolumeBucket } from "@stockwatcher/chart-core";
import { useMemo, useState } from "react";
import { Pressable, StyleSheet, View, type LayoutChangeEvent, type StyleProp, type ViewStyle } from "react-native";
import { recordPicture } from "./drawSkia.ts";
import { playHaptic } from "./ui.tsx";

const SESSION_NAME = { overnight: "overnight", pre: "premarket", regular: "regular session", after: "after hours" } as const;

/**
 * The full-day session timeline (overnight, premarket, regular, after-hours
 * bands, a volume minimap and the replay head), drawn from chart-core's
 * layoutTimeline like chart-web's. With onSeek, a tap jumps the replay to
 * that time (a light haptic confirms it), and screen readers can swipe
 * up/down to step 5 minutes.
 */
export function SessionTimeline({
  bands,
  buckets,
  bucketMs,
  now,
  variant = "compact",
  theme = DEFAULT_CHART_THEME,
  onSeek,
  style,
}: {
  bands: readonly SessionBand[];
  buckets: readonly VolumeBucket[];
  bucketMs: number;
  now: number | null;
  variant?: "full" | "compact";
  theme?: ChartTheme;
  onSeek?: (t: number) => void;
  style?: StyleProp<ViewStyle>;
}) {
  const [size, setSize] = useState<Size>({ width: 0, height: 0 });
  const onLayout = (e: LayoutChangeEvent) => {
    const { width, height } = e.nativeEvent.layout;
    setSize((s) => (s.width === width && s.height === height ? s : { width, height }));
  };
  const picture = useMemo(
    () => (size.width > 0 && size.height > 0 && bands.length ? recordPicture(layoutTimeline({ bands, buckets, bucketMs, now, variant }, theme, size), size.width, size.height) : null),
    [bands, buckets, bucketMs, now, variant, theme, size],
  );
  const label = now === null ? "Session timeline" : `Session timeline: ${formatEtTime(now, true)} Eastern, ${SESSION_NAME[sessionOf(now)]}`;
  const start = bands[0]?.start ?? 0;
  const end = bands[bands.length - 1]?.end ?? start;
  const step = (d: number) => onSeek?.(Math.min(end, Math.max(start, (now ?? start) + d * 5 * 60_000)));

  const body = (
    <View style={StyleSheet.absoluteFill} onLayout={onLayout}>
      {picture && (
        <Canvas style={StyleSheet.absoluteFill}>
          <Picture picture={picture} />
        </Canvas>
      )}
    </View>
  );
  return (
    <View style={[{ height: variant === "full" ? 76 : 56 }, style]}>
      {onSeek ? (
        <Pressable
          style={StyleSheet.absoluteFill}
          accessibilityRole="adjustable"
          accessibilityLabel={label}
          accessibilityHint="Tap a time to jump the replay there"
          accessibilityActions={[{ name: "increment" }, { name: "decrement" }]}
          onAccessibilityAction={(e) => step(e.nativeEvent.actionName === "increment" ? 1 : -1)}
          onPress={(e) => {
            const t = timelineTimeAt(bands, size.width, e.nativeEvent.locationX);
            if (t === null) return;
            playHaptic("selection");
            onSeek(t);
          }}
        >
          {body}
        </Pressable>
      ) : (
        <View style={StyleSheet.absoluteFill} accessible accessibilityRole="image" accessibilityLabel={label}>
          {body}
        </View>
      )}
    </View>
  );
}
