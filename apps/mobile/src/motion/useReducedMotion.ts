import { useEffect, useState } from "react";
import { AccessibilityInfo } from "react-native";

/**
 * The OS "reduce motion" setting, kept live: unlike Reanimated's
 * useReducedMotion (read once at startup), this follows the user changing
 * it while the app runs.
 */
export function useReducedMotion(): boolean {
  const [reduced, setReduced] = useState(false);

  useEffect(() => {
    let mounted = true;
    void AccessibilityInfo.isReduceMotionEnabled().then((value) => {
      if (mounted) setReduced(value);
    });
    const sub = AccessibilityInfo.addEventListener("reduceMotionChanged", setReduced);
    return () => {
      mounted = false;
      sub.remove();
    };
  }, []);

  return reduced;
}
