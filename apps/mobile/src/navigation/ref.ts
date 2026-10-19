import { createNavigationContainerRef } from "@react-navigation/native";

import type { RootStackParamList } from "./RootStack";

/**
 * The app's navigation, reachable outside a screen: a fired alert's banner ("View") and a tapped
 * phone notification open the Alerts tab from wherever the person is.
 */
export const navigationRef = createNavigationContainerRef<RootStackParamList>();

/** Opens a ticker's chart modal (it covers the dashboard, so nothing can open a second one over it). */
export function openTickerChart(symbol: string): void {
  if (navigationRef.isReady()) navigationRef.navigate("TickerChart", { symbol });
}

export function navigateToAlerts(): void {
  if (navigationRef.isReady()) navigationRef.navigate("Home", { screen: "Alerts" });
}
