import { createBlankStackNavigator } from "react-native-screen-transitions/react-navigation";

import type { NavigatorScreenParams } from "@react-navigation/native";

import { motion } from "../motion/transitions";
import { useReducedMotion } from "../motion/useReducedMotion";
import { InsightsTickerScreen } from "../screens/InsightsTickerScreen";
import { TickerChartScreen } from "../screens/TickerChartScreen";
import type { ChartParams } from "./openTicker";
import { MainTabs, type MainTabParamList } from "./MainTabs";

// Blank stack, not native-stack: motion runs in our Reanimated worklets so
// a push looks the same on iOS and Android (CLAUDE.md "Screen transitions
// and motion"). Options come only from src/motion/transitions.ts.

export type RootStackParamList = {
  /** The main navigation's tabs (dashboard, chart). Pushed screens go beside it in this stack. */
  Home: NavigatorScreenParams<MainTabParamList> | undefined;
  /** A ticker's chart, presented as a modal over everything (owner, 2026-10-19). */
  TickerChart: ChartParams;
  /** The Insights ticker page for one symbol-day (owner, 2026-10-31), pushed over the Insights tab. */
  InsightsTicker: { symbol: string; day: string };
};

const Stack = createBlankStackNavigator<RootStackParamList>();

export function RootStack() {
  const m = motion(useReducedMotion());

  return (
    <Stack.Navigator initialRouteName="Home" screenOptions={m.screenPush}>
      <Stack.Screen name="Home" component={MainTabs} options={m.root} />
      <Stack.Screen name="TickerChart" component={TickerChartScreen} options={m.chartModal} />
      {/* The app's first pushed screen: the shared push (iOS edge swipe back, Android back through the same spec). */}
      <Stack.Screen name="InsightsTicker" component={InsightsTickerScreen} options={m.screenPush} />
    </Stack.Navigator>
  );
}
