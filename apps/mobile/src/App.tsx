import { DarkTheme, NavigationContainer, type LinkingOptions, type Theme } from "@react-navigation/native";
import * as Linking from "expo-linking";
import { StatusBar } from "expo-status-bar";
import { GestureHandlerRootView } from "react-native-gesture-handler";
import { SafeAreaProvider } from "react-native-safe-area-context";

import { AlertSheet } from "./alerts/AlertSheet";
import { useFiredBanner } from "./alerts/useFiredBanner";
import { ErrorBoundary } from "./ErrorBoundary";
import { flushNavigation, navigationRef } from "./navigation/ref";
import { RootStack, type RootStackParamList } from "./navigation/RootStack";
import { colors } from "./theme";
import { Toaster } from "./ui/Toaster";
import { useForegroundUpdates } from "./updates/useForegroundUpdates";

// Deep links: stockwatcher:// opens the dashboard, stockwatcher://chart the chart (scheme set in
// app.config.ts).
const linking: LinkingOptions<RootStackParamList> = {
  prefixes: [Linking.createURL("/")],
  config: {
    screens: {
      Home: {
        screens: {
          Dashboard: "",
          Chart: "chart",
          Alerts: "alerts",
        },
      },
    },
  },
};

// React Navigation's theme mapped onto src/theme.ts, so its containers
// never flash its own default colours between screens.
const navigationTheme: Theme = {
  ...DarkTheme,
  colors: {
    ...DarkTheme.colors,
    primary: colors.accent,
    background: colors.bg,
    card: colors.bgPanel,
    text: colors.text,
    // No borders (owner rule): the hairlines React Navigation draws under
    // headers and above tab bars take the panel colour, so they vanish.
    border: colors.bgPanel,
    notification: colors.critical,
  },
};

export default function App() {
  // Checks for OTA updates whenever the app returns to the foreground.
  // Renders nothing; the downloaded update applies on the next cold start.
  useForegroundUpdates();
  // Price alerts: the phone's push token (no prompt at launch) and the in-app banner on a fire.
  useFiredBanner();

  return (
    <GestureHandlerRootView style={{ flex: 1, backgroundColor: colors.bg }}>
      <SafeAreaProvider>
        <ErrorBoundary>
          <NavigationContainer ref={navigationRef} linking={linking} theme={navigationTheme} onReady={flushNavigation}>
            <RootStack />
          </NavigationContainer>
        </ErrorBoundary>
        {/* The full alert sheet (chart "More options", the Alerts tab), once for every screen. */}
        <AlertSheet />
        {/* Over every screen and above the tab bar ("Copied AAL", an alert's Undo, a fired alert). */}
        <Toaster />
        <StatusBar style="light" />
      </SafeAreaProvider>
    </GestureHandlerRootView>
  );
}
