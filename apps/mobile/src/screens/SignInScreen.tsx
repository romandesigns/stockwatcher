import { useState } from "react";
import { KeyboardAvoidingView, Platform, Pressable, StyleSheet, Text, TextInput, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { colors, fonts } from "../theme";

/**
 * Shown when the server refuses the connection for want of its access token (production sets
 * STOCKWATCHER_API_TOKEN). One field; the token goes to the keystore (expo-secure-store), never to
 * AsyncStorage, and is never shown back.
 */
export function SignInScreen({ rejected, onSubmit }: { rejected: boolean; onSubmit: (token: string) => void }) {
  const insets = useSafeAreaInsets();
  const [value, setValue] = useState("");
  const ok = value.trim().length > 0;
  return (
    <KeyboardAvoidingView style={[s.screen, { paddingTop: insets.top, paddingBottom: insets.bottom }]} behavior={Platform.OS === "ios" ? "padding" : undefined}>
      <View style={s.card}>
        <Text style={s.title} accessibilityRole="header">
          StockWatcher
        </Text>
        <Text style={s.body}>Enter your access token to connect to the server.</Text>
        <TextInput
          value={value}
          onChangeText={setValue}
          placeholder="Access token"
          placeholderTextColor={colors.textFaint}
          secureTextEntry
          autoCapitalize="none"
          autoCorrect={false}
          autoFocus
          returnKeyType="go"
          onSubmitEditing={() => ok && onSubmit(value.trim())}
          accessibilityLabel="Access token"
          style={s.input}
        />
        {rejected && <Text style={s.error}>That token wasn't accepted. Check it and try again.</Text>}
        <Pressable accessibilityRole="button" accessibilityState={{ disabled: !ok }} disabled={!ok} onPress={() => onSubmit(value.trim())} style={({ pressed }) => [s.button, !ok && { opacity: 0.5 }, pressed && { opacity: 0.85 }]}>
          <Text style={s.buttonText}>Sign in</Text>
        </Pressable>
      </View>
    </KeyboardAvoidingView>
  );
}

const s = StyleSheet.create({
  screen: { flex: 1, backgroundColor: colors.bg, justifyContent: "center", paddingHorizontal: 16 },
  card: { backgroundColor: colors.bgPanel, borderRadius: 16, padding: 20, gap: 12 },
  title: { color: colors.text, fontFamily: fonts.semibold, fontSize: 18 },
  body: { color: colors.textDim, fontSize: 13 },
  input: { height: 46, borderRadius: 12, backgroundColor: colors.bgRow, paddingHorizontal: 14, color: colors.text, fontSize: 15 },
  error: { color: colors.critical, fontSize: 12 },
  button: { height: 48, borderRadius: 12, backgroundColor: colors.accent, alignItems: "center", justifyContent: "center" },
  buttonText: { color: colors.accentFg, fontSize: 15, fontWeight: "600" },
});
