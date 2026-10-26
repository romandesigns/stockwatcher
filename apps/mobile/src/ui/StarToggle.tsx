import { Icon, UI } from "@stockwatcher/chart-native";
import { Pressable, StyleSheet } from "react-native";

import { appActions, useApp } from "../state/store";

/**
 * Adds a ticker to the Favorites card, or takes it off (owner, 2026-10-26); the web's StarButton.
 * The list is the synced `favorites` setting, so every device follows. On, the star spends the
 * accent (an active state); off, it is a quiet outline. A 44 dp target round a 16 dp icon. As a
 * Pressable inside a row's Pressable it takes its own press, so the row does not open.
 */
export function StarToggle({ symbol, size = 16, quiet = false }: { symbol: string; size?: number; quiet?: boolean }) {
  const on = useApp((s) => s.favorites.includes(symbol));
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityState={{ selected: on }}
      accessibilityLabel={on ? `Remove ${symbol} from favorites` : `Add ${symbol} to favorites`}
      onPress={() => appActions().toggleFavorite(symbol)}
      hitSlop={10}
      style={({ pressed }) => [styles.box, pressed && { backgroundColor: UI.raised }]}
    >
      {/* `quiet`: on the Favorites card every row is on, so its stars stay neutral (60/30/10). */}
      <Icon name={on ? "starOn" : "star"} color={on && !quiet ? UI.accent : on ? UI.textDim : UI.textFaint} size={size} />
    </Pressable>
  );
}

const styles = StyleSheet.create({
  box: { width: 28, height: 28, borderRadius: 8, alignItems: "center", justifyContent: "center" },
});
