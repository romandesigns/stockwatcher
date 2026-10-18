import { resolveSettingsSection, settingsRailMode, type IconName } from "@stockwatcher/chart-core";
import { useEffect, useState, type ReactNode } from "react";
import { Pressable, StyleSheet, Text, useWindowDimensions, View } from "react-native";
import Animated, { FadeIn, FadeOut, ReduceMotion, useAnimatedStyle, useSharedValue, withTiming } from "react-native-reanimated";
import { Sheet } from "./sheets.tsx";
import { Icon, UI } from "./ui.tsx";

/** One tab of the sheet: the phone's twin of chart-web's SettingsDialogSection. */
export interface SettingsSheetSection {
  id: string;
  label: string;
  icon: IconName;
  description?: string;
  /** Beside the panel's heading, e.g. "Reset". */
  action?: ReactNode;
  render: () => ReactNode;
}

const TAB = 48;
const TAB_GAP = 4;
const RAIL_PAD = 4;
/** Sheet's horizontal padding (sheets.tsx), so the rail rule sees the sheet's inner width. */
const SHEET_PAD = 16;
/** The connected tab's corner radius, and the size of its folder-tab notches. */
const TAB_R = 9;

/**
 * The Settings dialog on the phone (owner request 2026-10-18): the same
 * sections as the web, in the existing Sheet, full height (`tall`), with
 * the same two columns: a 15 % rail and an 85 % panel.
 *
 * Why the split is kept rather than a top tab row: 15 % of a 390 pt phone
 * (inside the sheet's padding) is ~54 pt, more than the 44 pt minimum
 * touch target, so the rail holds full-size icon tabs (48 pt tall). It has
 * no room for words, so it shows icons only: each tab's label is its
 * accessible name, and the panel's heading names the active section in
 * words. chart-core's settingsRailMode makes that call from the width, the
 * same rule the web dialog uses; only on a screen too narrow for a 44 pt
 * rail (under ~293 pt, no current phone) do the tabs become a top row.
 *
 * Active tab: CONNECTED to the panel (owner, 2026-10-18: "connect the
 * active tab with the associated section, have them both have the same
 * background"), as on the web. It wears the panel's colour (the sheet's,
 * UI.panel, which the panel shows through), is rounded only on the side away
 * from the panel, and runs to the rail's inner edge (the rail has no padding
 * on that side and nothing sits between rail and panel), so tab and panel
 * read as one shape. Two Notch squares at its panel-side corners curve the
 * rail into it, folder-tab style (side rail only; the top-row fallback,
 * which no current phone reaches, keeps just the rounded top). The icon is
 * the duotone in accent (the sheet's one accent); idle tabs are dim, on the
 * rail. The tab slides between positions and the panel fades in on a
 * switch; under reduced motion both are instant. The tab chosen is kept
 * while the app runs (this component stays mounted with the screen).
 */
export function SettingsSheet({ open, onClose, sections, reducedMotion }: { open: boolean; onClose: () => void; sections: readonly SettingsSheetSection[]; reducedMotion?: boolean }) {
  const { width } = useWindowDimensions();
  const mode = settingsRailMode(width - 2 * SHEET_PAD);
  const top = mode === "top";
  const [stored, setStored] = useState<string | null>(null);
  const tab = resolveSettingsSection(stored, sections.map((s) => s.id));
  const index = Math.max(0, sections.findIndex((s) => s.id === tab));
  const active = sections[index];

  // The raised surface behind the active tab, sliding along the rail.
  const at = useSharedValue(index);
  useEffect(() => {
    at.value = withTiming(index, { duration: reducedMotion ? 0 : 200 });
  }, [index, reducedMotion, at]);
  const indicator = useAnimatedStyle(() => (top ? { transform: [{ translateX: at.value * (TAB + TAB_GAP) }] } : { transform: [{ translateY: at.value * (TAB + TAB_GAP) }] }));

  const fade = (ms: number) => (reducedMotion ? undefined : FadeIn.duration(ms).reduceMotion(ReduceMotion.System));
  return (
    <Sheet open={open} onClose={onClose} title="Settings" reducedMotion={reducedMotion} tall>
      <View style={[s.body, { flexDirection: top ? "column" : "row" }]}>
        <View style={[s.rail, top ? s.railTop : s.railSide]} accessibilityRole="tablist">
          <Animated.View pointerEvents="none" style={[s.indicator, top ? s.indicatorTop : s.indicatorSide, indicator]}>
            {!top && (
              <>
                <Notch at="above" />
                <Notch at="below" />
              </>
            )}
          </Animated.View>
          {sections.map((sec) => {
            const on = sec.id === tab;
            return (
              <Pressable
                key={sec.id}
                accessibilityRole="tab"
                accessibilityState={{ selected: on }}
                accessibilityLabel={sec.label}
                onPress={() => setStored(sec.id)}
                style={({ pressed }) => [s.tab, top ? s.tabTop : s.tabSide, pressed && !on && { backgroundColor: UI.row }]}
              >
                <Icon name={sec.icon} color={on ? UI.accent : UI.textDim} size={18} />
              </Pressable>
            );
          })}
        </View>
        {active && (
          // Keyed by tab, so a switch mounts the new panel and it fades in (the old one fades out).
          <Animated.View key={active.id} entering={fade(160)} exiting={reducedMotion ? undefined : FadeOut.duration(100).reduceMotion(ReduceMotion.System)} style={[s.panel, top ? { paddingTop: 10 } : { paddingLeft: 12 }]} accessibilityRole="none">
            <View style={s.head}>
              <View style={{ flex: 1, gap: 2 }}>
                <Text style={s.title} accessibilityRole="header">
                  {active.label}
                </Text>
                {active.description ? <Text style={s.desc}>{active.description}</Text> : null}
              </View>
              {active.action}
            </View>
            <View style={{ flex: 1 }}>{active.render()}</View>
          </Animated.View>
        )}
      </View>
    </Sheet>
  );
}

/**
 * One folder-tab notch: a TAB_R square just outside the connected tab's
 * panel-side corner, in the panel colour, holding a rail-coloured square
 * with the opposite corner rounded by TAB_R. That rounded square is exactly
 * a quarter circle centred on the rail side, so what shows is the rail's
 * edge curving into the tab. A child of the indicator, so it slides with it.
 */
function Notch({ at }: { at: "above" | "below" }) {
  return (
    <View style={[s.notch, at === "above" ? { top: -TAB_R } : { bottom: -TAB_R }]}>
      <View style={[s.notchRail, at === "above" ? { borderBottomRightRadius: TAB_R } : { borderTopRightRadius: TAB_R }]} />
    </View>
  );
}

const s = StyleSheet.create({
  body: { flex: 1 },
  // Recessed: the page tone on the sheet's panel colour, so the rail separates by contrast (no border).
  // No padding on the panel side, so the connected tab reaches the panel.
  rail: { backgroundColor: UI.bg, borderRadius: 12, padding: RAIL_PAD, gap: TAB_GAP },
  railSide: { width: "15%", minWidth: 44 + RAIL_PAD, alignSelf: "flex-start", paddingRight: 0 },
  railTop: { flexDirection: "row", alignSelf: "flex-start", paddingBottom: 0 },
  indicator: { position: "absolute", height: TAB, backgroundColor: UI.panel },
  indicatorSide: { left: RAIL_PAD, right: 0, top: RAIL_PAD, borderTopLeftRadius: TAB_R, borderBottomLeftRadius: TAB_R },
  indicatorTop: { left: RAIL_PAD, top: RAIL_PAD, width: TAB, borderTopLeftRadius: TAB_R, borderTopRightRadius: TAB_R },
  notch: { position: "absolute", right: 0, width: TAB_R, height: TAB_R, backgroundColor: UI.panel },
  notchRail: { flex: 1, backgroundColor: UI.bg },
  tab: { height: TAB, alignItems: "center", justifyContent: "center" },
  tabSide: { borderTopLeftRadius: TAB_R, borderBottomLeftRadius: TAB_R },
  tabTop: { width: TAB, borderTopLeftRadius: TAB_R, borderTopRightRadius: TAB_R },
  panel: { flex: 1, gap: 10 },
  head: { flexDirection: "row", alignItems: "flex-start", gap: 8 },
  title: { color: UI.text, fontSize: 15, fontWeight: "600" },
  desc: { color: UI.textFaint, fontSize: 11 },
});
