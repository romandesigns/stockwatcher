import * as Updates from "expo-updates";
import { Component, type ReactNode } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";

import { colors } from "./theme";

/**
 * Last line of defence, the phone's twin of the web's: an error anywhere in
 * the screen shows a short note and a reload button instead of an empty
 * screen (2026-10-17: a menu threw on open and blanked the web app).
 */
export class ErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state = { error: null as Error | null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error) {
    console.error("StockWatcher crashed:", error);
  }

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <View style={s.screen} accessibilityRole="alert">
        <Text style={s.title}>Something went wrong on this screen.</Text>
        <Text style={s.message}>{this.state.error.message}</Text>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel="Reload"
          // A full reload (fresh JS state); in a dev client without updates it falls back to remounting.
          onPress={() => Updates.reloadAsync().catch(() => this.setState({ error: null }))}
          style={({ pressed }) => [s.button, pressed && { opacity: 0.85 }]}
        >
          <Text style={s.buttonText}>Reload</Text>
        </Pressable>
      </View>
    );
  }
}

const s = StyleSheet.create({
  screen: { flex: 1, alignItems: "center", justifyContent: "center", gap: 12, padding: 24, backgroundColor: colors.bg },
  title: { color: colors.text, fontSize: 15, fontWeight: "600", textAlign: "center" },
  message: { color: colors.textDim, fontSize: 12, textAlign: "center" },
  button: { height: 40, paddingHorizontal: 18, borderRadius: 10, backgroundColor: colors.accent, alignItems: "center", justifyContent: "center" },
  buttonText: { color: colors.accentFg, fontSize: 14, fontWeight: "600" },
});
