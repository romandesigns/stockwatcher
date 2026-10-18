import { createBlankStackNavigator } from "react-native-screen-transitions/react-navigation";

import type { NavigatorScreenParams } from "@react-navigation/native";

import { motion } from "../motion/transitions";
import { useReducedMotion } from "../motion/useReducedMotion";
import { MainTabs, type MainTabParamList } from "./MainTabs";

// Blank stack, not native-stack: motion runs in our Reanimated worklets so
// a push looks the same on iOS and Android (CLAUDE.md "Screen transitions
// and motion"). Options come only from src/motion/transitions.ts.

export type RootStackParamList = {
  /** The main navigation's tabs (dashboard, chart). Pushed screens go beside it in this stack. */
  Home: NavigatorScreenParams<MainTabParamList> | undefined;
};

const Stack = createBlankStackNavigator<RootStackParamList>();

export function RootStack() {
  const m = motion(useReducedMotion());

  return (
    <Stack.Navigator initialRouteName="Home" screenOptions={m.screenPush}>
      <Stack.Screen name="Home" component={MainTabs} options={m.root} />
    </Stack.Navigator>
  );
}
