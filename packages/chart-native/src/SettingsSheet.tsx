import { SETTINGS_NAV_ROOT, settingsBack, settingsNavReduce, type IconName } from "@stockwatcher/chart-core";
import { useEffect, useReducer, useRef, useState, type ReactNode } from "react";
import { AccessibilityInfo, Pressable, ScrollView, StyleSheet, Text, View, type HostInstance, type LayoutChangeEvent } from "react-native";
import Animated, { Easing, useAnimatedStyle, useSharedValue, withTiming } from "react-native-reanimated";
import { Sheet } from "./sheets.tsx";
import { Icon, UI } from "./ui.tsx";

/** One section of the sheet: the phone's twin of chart-web's SettingsDialogSection. */
export interface SettingsSheetSection {
  id: string;
  label: string;
  icon: IconName;
  /** One line under the section's heading, and under its label on the root list. */
  description?: string;
  /** Beside the section's heading, e.g. "Reset". */
  action?: ReactNode;
  render: () => ReactNode;
  /**
   * The panel brings its own ScrollView (ChartSettingsPanel, DrawingSettingsPanel). Otherwise the
   * sheet wraps it in one: the app's panels (Alerts, Data source, Trading account) are plain Views
   * and, unwrapped, anything past the sheet's height could not be reached (owner, 2026-11-18: "I
   * don't think I can scroll on the mobile setting screen").
   */
  scrolls?: boolean;
}

/** The push / pop slide: long enough to read as movement, short enough not to wait on. */
const SLIDE_MS = 250;
/** How far the root list drifts left under an incoming section (the iOS push's parallax). */
const ROOT_DRIFT = 0.25;

/**
 * The Settings sheet on the phone. SINGLE COLUMN (owner, 2026-10-18:
 * "Instead of two columns tab navigation settings modal on mobile, I think
 * we should make it a single column instead"), replacing the 15 / 85 rail
 * this sheet had that morning: the iOS / Android system Settings pattern.
 * chart-core's settingsLayout decides it (always single column in the phone
 * app; the web follows the same rule below 640 px), so the phone app and a
 * phone browser behave the same.
 *
 * - Root screen: the sheet's "Settings" title with a ✕, and one full-width
 *   list of the sections: per row the duotone icon on a raised tile, label,
 *   one-line description and a duotone chevron. Rows are filled (UI.row on
 *   the sheet's UI.panel), separated by gaps, never lines.
 * - Section screen: a tap slides the section in from the right at full
 *   width while the root drifts a quarter left and fades. Its header is a
 *   back button (duotone chevron-left, 44 pt target) + the section's title +
 *   its action ("Reset"), the description below; then the SAME panel
 *   component as before (ChartSettingsPanel, DrawingSettingsPanel), now
 *   with the full width.
 * - Back: the back button, or the Android hardware / gesture back (the
 *   Sheet's onBack, i.e. Modal's onRequestClose), pops to the root with the
 *   reverse slide; on the root, back closes (chart-core settingsBack). The
 *   backdrop tap and the ✕ always close. Android 14's predictive-back
 *   preview is not wired: RN's Modal does not expose it (see the workspace
 *   motion rules); the back itself animates through the same slide.
 * - Always opens on the root (chart-core settingsNavReduce "reset": one
 *   predictable starting point). The reset happens in the render that opens
 *   the sheet and snaps the slide without animating, so neither the open nor
 *   the close shows a section sliding away.
 * - Accessibility: rows are buttons named by their label with the
 *   description as the hint; the section's title is a header and takes
 *   screen-reader focus on a push, and the originating row gets it back on
 *   a pop. The screen not showing is hidden from accessibility and touch.
 * - There is no two-column branch here: settingsLayout(_, "phone") is
 *   always "single-column", so the phone never draws the rail.
 * - Motion: Reanimated on the UI thread, the same timing on both
 *   platforms; with reduced motion the slide is instant.
 */
export function SettingsSheet({ open, onClose, sections, reducedMotion }: { open: boolean; onClose: () => void; sections: readonly SettingsSheetSection[]; reducedMotion?: boolean }) {
  const [nav, dispatch] = useReducer(settingsNavReduce, SETTINGS_NAV_ROOT);
  // Reset on every open, during render (React's "adjust state when a prop changes" pattern), and mark
  // it so the effect below snaps the slide instead of animating it.
  const snap = useRef(false);
  const [wasOpen, setWasOpen] = useState(open);
  if (open !== wasOpen) {
    setWasOpen(open);
    if (open) {
      dispatch({ type: "reset" });
      snap.current = true;
    }
  }
  // The section stays rendered while it slides back out after a pop, so remember the last one pushed.
  const [shown, setShown] = useState<string | null>(nav.section);
  if (nav.section !== null && nav.section !== shown) setShown(nav.section);
  const section = sections.find((x) => x.id === shown);
  const inSection = nav.section !== null;

  // 0 = root showing, 1 = section showing. Widths come from layout, so the slide is exactly one screen.
  const [width, setWidth] = useState(0);
  const at = useSharedValue(0);
  useEffect(() => {
    const target = inSection ? 1 : 0;
    at.value = snap.current || reducedMotion ? target : withTiming(target, { duration: SLIDE_MS, easing: Easing.out(Easing.cubic) });
    snap.current = false;
  }, [inSection, reducedMotion, at]);
  const rootStyle = useAnimatedStyle(() => ({ opacity: 1 - at.value, transform: [{ translateX: -at.value * width * ROOT_DRIFT }] }));
  const sectionStyle = useAnimatedStyle(() => ({ transform: [{ translateX: (1 - at.value) * width }] }));

  // Screen-reader focus: the section's title on a push, the row that opened it on a pop.
  const headingRef = useRef<HostInstance>(null);
  const rowRefs = useRef(new Map<string, HostInstance>());
  useEffect(() => {
    const target = nav.section !== null ? headingRef.current : nav.returnTo !== null ? rowRefs.current.get(nav.returnTo) : null;
    if (target) AccessibilityInfo.sendAccessibilityEvent(target, "focus");
  }, [nav]);

  const back = () => (settingsBack(nav) === "pop" ? dispatch({ type: "pop" }) : onClose());
  const close = (
    <Pressable onPress={onClose} accessibilityRole="button" accessibilityLabel="Close settings" hitSlop={8} style={({ pressed }) => [s.close, pressed && { backgroundColor: UI.row }]}>
      <Icon name="clear" color={UI.textDim} size={16} />
    </Pressable>
  );

  return (
    <Sheet open={open} onClose={onClose} onBack={back} title="Settings" badge={close} reducedMotion={reducedMotion} tall>
      <View style={s.body} onLayout={(e: LayoutChangeEvent) => setWidth(e.nativeEvent.layout.width)}>
        {/* Root: the list of sections. */}
        <Animated.View
          style={[s.screen, rootStyle]}
          pointerEvents={inSection ? "none" : "auto"}
          accessibilityElementsHidden={inSection}
          importantForAccessibility={inSection ? "no-hide-descendants" : "auto"}
        >
          {/* Scrolls too: on the Fold's cover screen in landscape the list is taller than the sheet. */}
          <ScrollView showsVerticalScrollIndicator={false} showsHorizontalScrollIndicator={false} contentContainerStyle={s.list}>
            {sections.map((sec) => (
            <Pressable
              key={sec.id}
              ref={(el) => {
                if (el) rowRefs.current.set(sec.id, el);
                else rowRefs.current.delete(sec.id);
              }}
              accessibilityRole="button"
              accessibilityLabel={sec.label}
              accessibilityHint={sec.description}
              onPress={() => dispatch({ type: "push", id: sec.id })}
              style={({ pressed }) => [s.row, pressed && { backgroundColor: UI.raised }]}
            >
              <View style={s.tile}>
                <Icon name={sec.icon} color={UI.text} size={18} />
              </View>
              <View style={{ flex: 1, gap: 2 }}>
                <Text style={s.rowLabel}>{sec.label}</Text>
                {sec.description ? (
                  <Text style={s.desc} numberOfLines={1}>
                    {sec.description}
                  </Text>
                ) : null}
              </View>
              <Icon name="chevronRight" color={UI.textDim} size={14} />
            </Pressable>
            ))}
          </ScrollView>
        </Animated.View>
        {/* Section: slides in from the right over the root, on the sheet's own colour so it covers it. */}
        <Animated.View
          style={[s.screen, s.section, sectionStyle, width === 0 && { opacity: 0 }]}
          pointerEvents={inSection ? "auto" : "none"}
          accessibilityElementsHidden={!inSection}
          importantForAccessibility={inSection ? "auto" : "no-hide-descendants"}
        >
          {section && (
            <>
              <View style={s.head}>
                <Pressable onPress={() => dispatch({ type: "pop" })} accessibilityRole="button" accessibilityLabel="Back to Settings" style={({ pressed }) => [s.back, pressed && { backgroundColor: UI.row }]}>
                  <Icon name="chevronLeft" color={UI.textDim} size={16} />
                </Pressable>
                <Text ref={headingRef} style={s.title} accessibilityRole="header" numberOfLines={1}>
                  {section.label}
                </Text>
                {section.action}
              </View>
              {section.description ? <Text style={[s.desc, s.sectionDesc]}>{section.description}</Text> : null}
              {section.scrolls ? (
                <View style={{ flex: 1 }}>{section.render()}</View>
              ) : (
                <ScrollView style={{ flex: 1 }} showsVerticalScrollIndicator={false} showsHorizontalScrollIndicator={false} keyboardShouldPersistTaps="handled" contentContainerStyle={{ paddingBottom: 8 }}>
                  {section.render()}
                </ScrollView>
              )}
            </>
          )}
        </Animated.View>
      </View>
    </Sheet>
  );
}

const s = StyleSheet.create({
  // Clipped, so the section waiting off to the right never shows past the sheet's edge.
  body: { flex: 1, overflow: "hidden" },
  screen: { ...StyleSheet.absoluteFill, gap: 6 },
  list: { gap: 6, paddingBottom: 8 },
  section: { backgroundColor: UI.panel, gap: 0 },
  close: { marginLeft: "auto", width: 32, height: 32, borderRadius: 8, alignItems: "center", justifyContent: "center" },
  row: { flexDirection: "row", alignItems: "center", gap: 12, minHeight: 60, paddingHorizontal: 12, paddingVertical: 10, borderRadius: 12, backgroundColor: UI.row },
  tile: { width: 36, height: 36, borderRadius: 9, alignItems: "center", justifyContent: "center", backgroundColor: UI.raised },
  rowLabel: { color: UI.text, fontSize: 14, fontWeight: "500" },
  head: { flexDirection: "row", alignItems: "center", gap: 4, minHeight: 44, marginLeft: -10 },
  back: { width: 44, height: 44, borderRadius: 10, alignItems: "center", justifyContent: "center" },
  title: { flex: 1, color: UI.text, fontSize: 15, fontWeight: "600" },
  desc: { color: UI.textFaint, fontSize: 11 },
  sectionDesc: { marginBottom: 10 },
});
