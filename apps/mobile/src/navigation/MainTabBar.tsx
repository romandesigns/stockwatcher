import type { BottomTabBarProps } from "@react-navigation/bottom-tabs";
import type { IconName } from "@stockwatcher/chart-core";
import { Icon, playHaptic } from "@stockwatcher/chart-native";
import { Pressable, StyleSheet, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { colors, fonts } from "../theme";

// The main navigation's bottom bar: wavykit's tab-bar (H:\wavystack\wavykit
// registry/components/tab-bar/native.tsx), ported onto this app's theme instead of
// `kit add`ed (stockwatcher predates wavykit's tokens). Same rules as the kit and the web's
// NavTabBar: items share the width, icon over label; the active item is a row-surface pill
// behind an accent duotone icon with a foreground label, the only accent the bar spends; idle is
// dim. Roles are tablist / tab, which is how iOS and Android announce a bottom bar. The panel
// surface is the bar's boundary: no top border (owner rule).

/** Each tab's icon, by route name (MainTabs registers the routes). The chart tab is the replay practice feature, hence the replay icon (owner, 2026-10-18). */
const ICONS: Readonly<Record<string, IconName>> = { Dashboard: "dashboard", Chart: "replay", Alerts: "bell", Settings: "settings" };

export function MainTabBar({ state, descriptors, navigation }: BottomTabBarProps) {
  const insets = useSafeAreaInsets();
  return (
    <View accessibilityRole="tablist" accessibilityLabel="Main" style={[styles.bar, { paddingBottom: 4 + insets.bottom }]}>
      {state.routes.map((route, index) => {
        const active = state.index === index;
        const label = descriptors[route.key]?.options.title ?? route.name;
        const onPress = () => {
          const event = navigation.emit({ type: "tabPress", target: route.key, canPreventDefault: true });
          if (!active && !event.defaultPrevented) {
            playHaptic("selection");
            navigation.navigate(route.name, route.params);
          }
        };
        return (
          <Pressable
            key={route.key}
            onPress={onPress}
            onLongPress={() => navigation.emit({ type: "tabLongPress", target: route.key })}
            accessibilityRole="tab"
            accessibilityState={{ selected: active }}
            accessibilityLabel={label}
            style={styles.item}
          >
            {({ pressed }) => (
              <>
                <View style={[styles.pill, (active || pressed) && { backgroundColor: colors.bgRow }]}>
                  <Icon name={ICONS[route.name] ?? "chart"} color={active ? colors.accent : colors.textDim} size={20} />
                </View>
                {/* The app bundles Geist regular and semibold only: the active label takes the heavier one. */}
                <Text numberOfLines={1} style={[styles.label, { color: active ? colors.text : colors.textDim, fontFamily: active ? fonts.semibold : fonts.regular }]}>
                  {label}
                </Text>
              </>
            )}
          </Pressable>
        );
      })}
    </View>
  );
}

const styles = StyleSheet.create({
  bar: { flexDirection: "row", gap: 4, paddingTop: 4, paddingHorizontal: 4, backgroundColor: colors.bgPanel },
  // 48 pt tall at least: a comfortable thumb target.
  item: { flex: 1, minWidth: 0, minHeight: 48, alignItems: "center", justifyContent: "center", gap: 2, paddingVertical: 4 },
  // Half the height: Android drew a 999 radius on this small view as square corners.
  pill: { width: 48, height: 28, borderRadius: 14, alignItems: "center", justifyContent: "center" },
  label: { fontSize: 11 },
});
