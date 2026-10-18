import { StyleSheet, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { colors, fonts } from "../theme";

/**
 * The main page (owner request 2026-10-18), the first tab. A placeholder until the live dashboard
 * is built: it will consume only live data (docs/CONVENTIONS.md "Product direction"; the
 * constraints to plan around are in docs/architecture/chart-packages.md).
 */
export function DashboardScreen() {
  const insets = useSafeAreaInsets();
  return (
    <View style={[styles.screen, { paddingTop: insets.top + 8 }]}>
      <Text accessibilityRole="header" style={styles.title}>
        Dashboard
      </Text>
      <Text style={styles.body}>Main page. The live market dashboard will be built here.</Text>
    </View>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: colors.bg, paddingHorizontal: 16, gap: 4 },
  title: { color: colors.text, fontFamily: fonts.semibold, fontSize: 20 },
  body: { color: colors.textDim, fontFamily: fonts.regular, fontSize: 14 },
});
