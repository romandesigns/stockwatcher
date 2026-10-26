import { Icon, UI } from "@stockwatcher/chart-native";
import { marksReadout, rossLinePaths, rowLineOverlay, type FlameMark, type PathKind, type RossLineView, type RossTone, type RowMarks, type StageMark, type TailMark } from "@stockwatcher/shared";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { StyleSheet, Text, View } from "react-native";
import Svg, { Circle, Line, Path, Rect } from "react-native-svg";

import { colors, fonts } from "../theme";
import { showToast } from "../ui/toast";

// The lighter rows on the phone (owner, 2026-10-26, approved design canvas "StockWatcher lighter
// rows"; the web's apps/web/src/dashboard/RowMarks.tsx, one derivation in shared rowMarks.ts).
// The phone has no hover, so pressing and holding a row (not after a tap: tap, then hold still
// copies the ticker) shows every mark's name and value in one read-out; each mark also carries
// it as its accessibility label.

const TONE: Record<RossTone, string> = {
  good: UI.good,
  critical: UI.critical,
  warning: UI.warning,
  accent: UI.accent,
  text: UI.text,
  muted: UI.textDim,
  dim: UI.textFaint,
};

const PATH_ICON: Record<PathKind, "pathRunner" | "pathBurst" | "pathFast" | "pathRun" | "pathRunning"> = {
  runner: "pathRunner",
  burst: "pathBurst",
  fast: "pathFast",
  run: "pathRun",
  running: "pathRunning",
};

/** A wall clock that ticks every `ms`, so a flame goes out and ages move without a new frame. */
export function useWall(ms = 15_000): number {
  const [wall, setWall] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setWall(Date.now()), ms);
    return () => clearInterval(id);
  }, [ms]);
  return wall;
}

/**
 * A row's press handlers (useTickerPress) plus press-and-hold for its marks' read-out. A hold
 * that follows a tap within the double-tap window is the copy gesture and shows nothing else.
 */
export function useReadoutPress<P extends { onPressIn: () => void; onPress: () => void }>(press: P, marks: RowMarks | null) {
  const lastTap = useRef(0);
  const downAt = useRef(0);
  const readout = marks ? marksReadout(marks) : "";
  return {
    ...press,
    onPressIn: () => {
      downAt.current = Date.now();
      press.onPressIn();
    },
    onPress: () => {
      lastTap.current = Date.now();
      press.onPress();
    },
    onLongPress: () => {
      if (!readout || downAt.current - lastTap.current < 450) return;
      showToast(readout, { ms: 6000, lines: 8 });
    },
    delayLongPress: 450,
  };
}

/** "Look now": the flame before the ticker. */
export function Flame({ flame }: { flame: FlameMark | null }) {
  if (!flame?.lit) return null;
  return (
    <View accessible accessibilityRole="image" accessibilityLabel={flame.label} testID="row-flame">
      <Icon name="flame" color={colors.flame} size={15} />
    </View>
  );
}

/**
 * The row's second line: pillars, relative volume, news, the Runners path, the demand zone the
 * price is in (Runners rows), then the setup.
 */
export function MarksLine({ marks: m, extra }: { marks: RowMarks; extra?: ReactNode }) {
  const { pillars, rvol, news, path, zone, stage } = m;
  if (!pillars && !rvol && !news && !path && !zone && !stage && !extra) return null;
  return (
    <View style={styles.line} testID="row-marks">
      {pillars && (
        <View accessible accessibilityRole="image" accessibilityLabel={pillars.label} testID="mark-pillars">
          {/* Filled = passes, a ring = missed: SVG marks, not bordered views (house rule). */}
          <Svg width={pillars.dots.length * 10 - 3} height={7} viewBox={`0 0 ${pillars.dots.length * 10 - 3} 7`}>
            {pillars.dots.map((d, i) => (d.state === "pass" ? <Circle key={i} cx={3.5 + i * 10} cy={3.5} r={3.5} fill={UI.textDim} /> : <Circle key={i} cx={3.5 + i * 10} cy={3.5} r={2.8} fill="none" stroke={UI.textFaint} strokeWidth={1.4} />))}
          </Svg>
        </View>
      )}
      {rvol && (
        <View accessible accessibilityRole="image" accessibilityLabel={rvol.label} testID="mark-rvol">
          <Svg width={13} height={12} viewBox="0 0 13 12">
            <Rect x={0} y={7} width={3} height={5} rx={1} fill={UI.textDim} />
            <Rect x={5} y={4} width={3} height={8} rx={1} fill={rvol.bars >= 2 ? UI.textDim : UI.raised} />
            <Rect x={10} y={0} width={3} height={12} rx={1} fill={rvol.bars >= 3 ? UI.textDim : UI.raised} />
          </Svg>
        </View>
      )}
      {news && (
        <View accessible accessibilityRole="image" accessibilityLabel={news.label} testID="mark-news">
          <Icon name="news" color={news.dilution ? UI.warning : UI.textDim} size={14} />
        </View>
      )}
      {path && (
        <View accessible accessibilityRole="image" accessibilityLabel={path.label} style={styles.path} testID="mark-path">
          <Icon name={PATH_ICON[path.kind]} color={UI.textDim} size={13} />
          <Text style={styles.age}>{path.age}</Text>
        </View>
      )}
      {zone && (
        // The Demand zone tool's icon, as on the web; fainter once price has been back in it.
        <View accessible accessibilityRole="image" accessibilityLabel={zone.label} style={zone.tested ? styles.tested : undefined} testID="mark-zone">
          <Icon name="demand" color={UI.textDim} size={14} />
        </View>
      )}
      {extra}
      {stage && <Stage stage={stage} />}
    </View>
  );
}

function Stage({ stage: s }: { stage: StageMark }) {
  const c = TONE[s.tone];
  return (
    <View accessible accessibilityRole="image" accessibilityLabel={s.label} style={styles.stage} testID="mark-stage">
      <Svg width={12} height={12} viewBox="0 0 12 12">
        {s.stage === "forming" && <Circle cx={6} cy={6} r={2.5} fill="none" stroke={c} strokeWidth={1.5} />}
        {(s.stage === "armed" || s.stage === "near") && <Circle cx={6} cy={6} r={4.2} fill={s.stage === "near" ? c : "none"} fillOpacity={0.35} stroke={c} strokeWidth={1.5} />}
        {(s.stage === "crossing" || s.stage === "triggered") && <Path d="M6 1.5 10.5 10h-9z" fill={c} fillOpacity={s.stage === "crossing" ? 1 : 0.4} stroke={c} strokeWidth={1.5} strokeLinejoin="round" />}
        {s.stage === "invalidated" && <Path d="M3 3l6 6M9 3 3 9" stroke={c} strokeWidth={1.5} strokeLinecap="round" />}
      </Svg>
      {s.meter !== null && (
        <View style={styles.meter}>
          <View style={[styles.meterFill, { width: `${Math.round(s.meter * 100)}%`, backgroundColor: c }]} />
          <View style={styles.meterTrigger} />
        </View>
      )}
      {s.trigger ? <Text style={styles.trigger}>{s.trigger}</Text> : null}
    </View>
  );
}

const TAIL: Record<TailMark["tone"], string> = { good: UI.good, critical: UI.critical, muted: UI.textDim };

/** The row chart with the last minute as a thick tail and the trigger as a dotted level. */
export function RowLine({ line, tail, trigger, width, height = 26, color }: { line: RossLineView | null; tail: TailMark | null; trigger?: number | null; width: number; height?: number; color: string }) {
  if (!line) return <View style={{ width, height }} />;
  const p = rossLinePaths(line, width, height);
  const o = rowLineOverlay(line, width, height, trigger);
  return (
    <View style={{ width, height }} accessible={!!tail} accessibilityRole={tail ? "image" : undefined} accessibilityLabel={tail?.label} testID="row-line">
      <Svg width={width} height={height} viewBox={`0 0 ${width} ${height}`}>
        {p.baseY !== null && <Line x1={0} x2={width} y1={p.baseY} y2={p.baseY} stroke={UI.textFaint} strokeWidth={1} strokeDasharray="1.5 2.5" />}
        {o.triggerY !== null && <Line x1={0} x2={width} y1={o.triggerY} y2={o.triggerY} stroke={UI.textDim} strokeOpacity={0.6} strokeWidth={1} strokeDasharray="2 3" />}
        <Path d={p.area} fill={color} fillOpacity={0.14} />
        <Path d={p.line} fill="none" stroke={color} strokeWidth={1.5} strokeLinejoin="round" />
        {tail && o.tail && <Path d={o.tail} fill="none" stroke={TAIL[tail.tone]} strokeWidth={2.6} strokeLinecap="round" strokeLinejoin="round" />}
      </Svg>
    </View>
  );
}

const styles = StyleSheet.create({
  line: { flexDirection: "row", alignItems: "center", gap: 11, minHeight: 16 },
  path: { flexDirection: "row", alignItems: "center", gap: 4 },
  tested: { opacity: 0.6 },
  age: { color: UI.textFaint, fontSize: 11, fontVariant: ["tabular-nums"], fontFamily: fonts.regular },
  stage: { marginLeft: "auto", flexDirection: "row", alignItems: "center", gap: 6 },
  meter: { width: 48, height: 6, borderRadius: 3, backgroundColor: UI.raised, justifyContent: "center" },
  meterFill: { position: "absolute", left: 0, top: 0, bottom: 0, borderRadius: 3, opacity: 0.55 },
  meterTrigger: { position: "absolute", right: 0, top: -2, width: 2, height: 10, borderRadius: 1, backgroundColor: UI.text },
  trigger: { color: UI.textDim, fontSize: 11, fontVariant: ["tabular-nums"], fontFamily: fonts.regular },
});
