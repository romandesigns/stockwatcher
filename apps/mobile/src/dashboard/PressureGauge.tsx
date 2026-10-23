import { easeOut } from "@stockwatcher/chart-core";
import { UI } from "@stockwatcher/chart-native";
import { GAUGE, gaugeCaption, gaugeGeometry, gaugeTone } from "@stockwatcher/shared";
import { useEffect, useRef, useState } from "react";
import Svg, { Path, Text as SvgText } from "react-native-svg";

import { fonts } from "../theme";

const SWEEP_MS = 300;

/** The value sliding to a new reading (ease-out), or jumping with reduced motion. */
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

const FILL = { good: UI.good, critical: UI.critical, halted: UI.textFaint } as const;

/**
 * The Halt watcher's gauge for React Native (react-native-svg; the web's PressureGauge, design
 * canvas "Halt Watcher" option A "fill"): a radial half ring filled in proportion to a 0–100
 * `value` (the halt risk since 2026-10-20; the prop was `pressure` before), in the colour of the
 * side the build-up pushes (green toward limit up, red toward limit down), the reading inside the
 * ring and "limit-up/limit-down risk" under it. Halted: a grey fill, the frozen value shown.
 */
export function PressureGauge({ value, side, halted, label, width, reducedMotion, compact = false, stale = false }: { value: number; side: "bull" | "bear"; halted: boolean; label: string; width: number; reducedMotion: boolean; /** The one-row strip's mini ring: no caption, the reading larger (a caption is unreadable at ~56 px). */ compact?: boolean; stale?: boolean }) {
  const shown = useSweep(Math.max(0, Math.min(100, Math.round(value))), reducedMotion || stale);
  const g = gaugeGeometry(shown);
  const neutral = halted || stale;
  const tone = gaugeTone(side, neutral);
  const ink = neutral ? UI.textFaint : UI.text;
  return (
    <Svg width={width} height={(width * GAUGE.height) / GAUGE.width} viewBox={`0 0 ${GAUGE.width} ${GAUGE.height}`} accessible accessibilityRole="image" accessibilityLabel={label}>
      <Path d={g.track} fill="none" stroke={UI.raised} strokeWidth={GAUGE.stroke} strokeLinecap="round" />
      {g.value && <Path d={g.value} fill="none" stroke={FILL[tone]} strokeOpacity={neutral ? 0.55 : 1} strokeWidth={GAUGE.stroke} strokeLinecap="round" />}
      {compact ? (
        <SvgText x={GAUGE.cx} y={101} textAnchor="middle" fill={ink} fontSize={52} fontFamily={fonts.semibold}>
          {String(g.label)}
        </SvgText>
      ) : (
        <>
          <SvgText x={GAUGE.cx} y={90} textAnchor="middle" fill={ink} fontSize={36} fontFamily={fonts.semibold}>
            {String(g.label)}
          </SvgText>
          <SvgText x={GAUGE.cx} y={110} textAnchor="middle" fill={UI.textDim} fontSize={12} fontFamily={fonts.regular}>
            {stale ? "last known risk" : gaugeCaption(side, halted)}
          </SvgText>
        </>
      )}
    </Svg>
  );
}
