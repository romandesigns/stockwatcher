import { DarkTheme, NavigationContainer, type LinkingOptions, type Theme } from "@react-navigation/native";
import * as Linking from "expo-linking";
import { StatusBar } from "expo-status-bar";
import { GestureHandlerRootView } from "react-native-gesture-handler";
import { SafeAreaProvider } from "react-native-safe-area-context";

import { ErrorBoundary } from "./ErrorBoundary";
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

  return (
    <GestureHandlerRootView style={{ flex: 1, backgroundColor: colors.bg }}>
      <SafeAreaProvider>
        <ErrorBoundary>
          <NavigationContainer linking={linking} theme={navigationTheme}>
            <RootStack />
          </NavigationContainer>
        </ErrorBoundary>
        {/* Over every screen and above the tab bar ("Copied AAL"). */}
        <Toaster />
        <StatusBar style="light" />
      </SafeAreaProvider>
    </GestureHandlerRootView>
  );
}
