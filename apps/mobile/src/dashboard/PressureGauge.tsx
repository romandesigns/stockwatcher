import { easeOut } from "@stockwatcher/chart-core";
import { UI } from "@stockwatcher/chart-native";
import { GAUGE, GAUGE_GRADIENT, gaugeColors, gaugeGeometry } from "@stockwatcher/shared";
import { useEffect, useId, useRef, useState } from "react";
import Svg, { Circle, Defs, Line, LinearGradient, Path, Stop, Text as SvgText } from "react-native-svg";

import { fonts } from "../theme";

const SWEEP_MS = 300;

/** The pressure sliding to a new reading (ease-out), or jumping with reduced motion. */
function useSweep(target: number, reducedMotion: boolean): number {
  const [value, setValue] = useState(target);
  const current = useRef(target);
  useEffect(() => {
    // Reduced motion shows the target as is (below); only remember it for a later sweep.
    if (reducedMotion) {
      current.current = target;
      return;
    }
    const from = current.current;
    if (from === target) return;
    const start = performance.now();
    let frame = 0;
    const step = () => {
      const v = easeOut(from, target, performance.now() - start, SWEEP_MS);
      current.current = v;
      setValue(v);
      if (v !== target) frame = requestAnimationFrame(step);
    };
    frame = requestAnimationFrame(step);
    return () => cancelAnimationFrame(frame);
  }, [target, reducedMotion]);
  return reducedMotion ? target : value;
}

/**
 * The Halt watcher's 240° speedometer for React Native (react-native-svg; the web's
 * PressureGauge, wavykit `pressure-gauge`): track, value arc, needle, 0–100 under the hub. The
 * side's gradient spans the whole dial (shared GAUGE_GRADIENT), so the arc lightens as the
 * pressure builds; halted, the arc is grey and the needle dim.
 */
export function PressureGauge({ pressure, side, halted, label, width, reducedMotion, compact = false }: { pressure: number; side: "bull" | "bear"; halted: boolean; label: string; width: number; reducedMotion: boolean; /** The one-row strip's mini dial: no needle, the value large inside the arc (unreadable under a needle at ~56 px). */ compact?: boolean }) {
  // react-native-svg resolves url(#id) per document, but ids must still be unique per gauge.
  const id = `hw${useId().replace(/[^a-zA-Z0-9]/g, "")}`;
  const shown = useSweep(Math.max(0, Math.min(100, Math.round(pressure))), reducedMotion);
  const g = gaugeGeometry(shown);
  const colors = gaugeColors(side, halted);
  const ink = halted ? UI.textFaint : UI.text;
  return (
    <Svg width={width} height={(width * GAUGE.height) / GAUGE.width} viewBox={`0 0 ${GAUGE.width} ${GAUGE.height}`} accessible accessibilityRole="image" accessibilityLabel={label}>
      {colors && (
        <Defs>
          <LinearGradient id={id} gradientUnits="userSpaceOnUse" x1={GAUGE_GRADIENT.x1} y1={0} x2={GAUGE_GRADIENT.x2} y2={0}>
            <Stop offset="0" stopColor={colors.from} />
            <Stop offset="1" stopColor={colors.to} />
          </LinearGradient>
        </Defs>
      )}
      <Path d={g.track} fill="none" stroke={UI.raised} strokeWidth={GAUGE.stroke} strokeLinecap="round" />
      {g.value && <Path d={g.value} fill="none" stroke={colors ? `url(#${id})` : UI.textFaint} strokeOpacity={colors ? 1 : 0.55} strokeWidth={GAUGE.stroke} strokeLinecap="round" />}
      {compact ? (
        <SvgText x={GAUGE.cx} y={118} textAnchor="middle" fill={ink} fontSize={58} fontFamily={fonts.semibold}>
          {String(g.label)}
        </SvgText>
      ) : (
        <>
          <Line x1={GAUGE.cx} y1={GAUGE.cy} x2={g.needle.x} y2={g.needle.y} stroke={ink} strokeWidth={3} strokeLinecap="round" />
          <Circle cx={GAUGE.cx} cy={GAUGE.cy} r={6} fill={ink} />
          <SvgText x={GAUGE.cx} y={132} textAnchor="middle" fill={UI.text} fontSize={26} fontFamily={fonts.semibold}>
            {String(g.label)}
          </SvgText>
        </>
      )}
    </Svg>
  );
}
