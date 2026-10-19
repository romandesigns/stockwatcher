import { createBottomTabNavigator } from "@react-navigation/bottom-tabs";

import { motion } from "../motion/transitions";
import { useReducedMotion } from "../motion/useReducedMotion";
import { ChartScreen } from "../screens/ChartScreen";
import { AlertsScreen } from "../screens/AlertsScreen";
import { DashboardScreen } from "../screens/DashboardScreen";
import { MainTabBar } from "./MainTabBar";

// The main navigation (owner request 2026-10-18; Alerts added 2026-10-19): the dashboard is the main page and the first
// tab; the chart is the second, Alerts the third. Android back on the chart returns to the dashboard
// (backBehavior "firstRoute"), then leaves the app, so both platforms' back works without extra
// wiring. Each screen draws its own header, so the navigator's is off. The tab-switch motion
// comes only from src/motion/transitions.ts.

export type MainTabParamList = {
  Dashboard: undefined;
  Chart: undefined;
  Alerts: undefined;
};

const Tab = createBottomTabNavigator<MainTabParamList>();

export function MainTabs() {
  const m = motion(useReducedMotion());
  return (
    <Tab.Navigator
      initialRouteName="Dashboard"
      backBehavior="firstRoute"
      tabBar={(props) => <MainTabBar {...props} />}
      screenOptions={{ headerShown: false, ...m.tabSwitch }}
    >
      <Tab.Screen name="Dashboard" component={DashboardScreen} options={{ title: "Dashboard" }} />
      <Tab.Screen name="Chart" component={ChartScreen} options={{ title: "Chart" }} />
      {/* Price alerts (2026-10-19): the list and Activity; the same tab-switch spec as the others. */}
      <Tab.Screen name="Alerts" component={AlertsScreen} options={{ title: "Alerts" }} />
    </Tab.Navigator>
  );
}
