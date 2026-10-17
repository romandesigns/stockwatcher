import { createBlankStackNavigator } from "react-native-screen-transitions/react-navigation";

import { motion } from "../motion/transitions";
import { useReducedMotion } from "../motion/useReducedMotion";
import { ChartScreen } from "../screens/ChartScreen";

// Blank stack, not native-stack: motion runs in our Reanimated worklets so
// a push looks the same on iOS and Android (CLAUDE.md "Screen transitions
// and motion"). Options come only from src/motion/transitions.ts.

export type RootStackParamList = {
  Home: undefined;
};

const Stack = createBlankStackNavigator<RootStackParamList>();

export function RootStack() {
  const m = motion(useReducedMotion());

  return (
    <Stack.Navigator initialRouteName="Home" screenOptions={m.screenPush}>
      <Stack.Screen name="Home" component={ChartScreen} options={m.root} />
    </Stack.Navigator>
  );
}
