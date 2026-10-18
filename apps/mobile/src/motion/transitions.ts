// The app's one motion vocabulary (CLAUDE.md "Screen transitions and
// motion"). Screens never write their own spring numbers or interpolators:
// navigators import a named spec from here, and a new kind of motion means
// a new spec in this file, never an inline one.
//
// Every spec is built by `motion(reducedMotion)`, so the reduced-motion
// fallback (a short cross-fade, no movement, no gesture) is decided in one
// place instead of per screen.

import { interpolate } from "react-native-reanimated";
import Transition, {
  type ScreenStyleInterpolator,
  type TransitionSpec,
} from "react-native-screen-transitions";
import type { BottomTabNavigationOptions } from "@react-navigation/bottom-tabs";
import type { BlankStackNavigationOptions } from "react-native-screen-transitions/react-navigation";

export type MotionSpec = BlankStackNavigationOptions;
/** A tab switch is the bottom-tab navigator's own JS-driven scene animation (Animated, not the OS), so it is identical on iOS and Android too. */
export type TabMotionSpec = Pick<BottomTabNavigationOptions, "animation">;

// --- timing ----------------------------------------------------------------

const navSpring: TransitionSpec = {
  open: Transition.Specs.DefaultSpec,
  close: Transition.Specs.DefaultSpec,
};

const fadeTiming: TransitionSpec = {
  open: { duration: 180 },
  close: { duration: 150 },
};

// --- interpolators -----------------------------------------------------------
// `progress` runs 0 -> 1 while a screen enters and 1 -> 2 while the next
// screen covers it.

const slideFromRight: ScreenStyleInterpolator = ({ progress, layouts }) => {
  "worklet";
  const width = layouts.screen.width;
  return {
    content: {
      style: {
        transform: [
          {
            // The covered screen drifts a third of the way left, so a push
            // and its back read as one continuous movement.
            translateX: interpolate(progress, [0, 1, 2], [width, 0, -width * 0.3]),
          },
        ],
      },
    },
  };
};

const slideFromBottom: ScreenStyleInterpolator = ({ progress, layouts }) => {
  "worklet";
  const height = layouts.screen.height;
  return {
    content: {
      style: {
        transform: [{ translateY: interpolate(progress, [0, 1, 2], [height, 0, 0]) }],
      },
    },
  };
};

const crossFadeInterpolator: ScreenStyleInterpolator = ({ progress }) => {
  "worklet";
  return {
    content: {
      style: { opacity: interpolate(progress, [0, 1, 2], [0, 1, 0]) },
    },
  };
};

// --- specs -------------------------------------------------------------------

const crossFade: MotionSpec = {
  screenStyleInterpolator: crossFadeInterpolator,
  transitionSpec: fadeTiming,
  // No swipe-dismiss under reduced motion: a gesture that drives no visible
  // movement is confusing. Hardware/gesture back still pops the screen.
  gestureEnabled: false,
};

export interface MotionVocabulary {
  /** The root screen: nothing animates on first mount. */
  root: MotionSpec;
  /** Push onto the stack (list -> detail). */
  screenPush: MotionSpec;
  /** A modal sheet over the current screen. */
  modalPresent: MotionSpec;
  /**
   * Switching main-nav tabs: a short cross-fade. Tabs are siblings, not a hierarchy, so nothing
   * slides (a slide would read as push/back). A cross-fade is already the reduced-motion form, so
   * it stays the same there.
   */
  tabSwitch: TabMotionSpec;
}

export function motion(reducedMotion: boolean): MotionVocabulary {
  if (reducedMotion) {
    return { root: {}, screenPush: crossFade, modalPresent: crossFade, tabSwitch: { animation: "fade" } };
  }
  return {
    root: {},
    tabSwitch: { animation: "fade" },
    screenPush: {
      screenStyleInterpolator: slideFromRight,
      transitionSpec: navSpring,
      // iOS interactive edge-swipe back. Android back (hardware/gesture)
      // pops through the same spec; predictive back is off until a pushed
      // screen exists (app.config.ts).
      gestureEnabled: true,
      gestureDirection: { gesture: "horizontal", area: "edge" },
    },
    modalPresent: {
      screenStyleInterpolator: slideFromBottom,
      transitionSpec: navSpring,
      gestureEnabled: true,
      gestureDirection: "vertical",
    },
  };
}
