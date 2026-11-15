// A screen's params read live from its navigator's state, by route key (owner, 2026-11-15: a
// tapped notification left the open chart on the old ticker).
//
// Why the `route` prop is not enough: react-native-screen-transitions 4.0.0 (our blank stack)
// reuses a screen's previous descriptor *source* whenever its options are unchanged
// (helpers/build-blank-stack-state.js `resolveStableDescriptorSource` →
// state-equality.js `areDescriptorSourcesEquivalent`, which compares options only). The reused
// source carries React Navigation's `render` closure built for the OLD route, so a params-only
// update (openTicker.ts reusing the chart modal's route with a new ticker) reaches the navigation
// state, but the screen keeps rendering its first params. Reproduced on the emulator: after a
// tap, navigationRef showed {symbol: "TSLA"} while the modal still drew AAPL.
//
// useNavigationState subscribes to the navigator's own state events, so the screen re-renders
// with the route's current params even when the library hands it a stale render.

import { useNavigationState } from "@react-navigation/native";

import { paramsForKey, type StateLike } from "./routeParams";

/** `route.params`, kept current from the navigator's state (see the module notes). */
export function useLiveRouteParams<P extends object>(route: { key: string; params: P }): P {
  const live = useNavigationState((state) => paramsForKey<P>(state as StateLike, route.key));
  return live ?? route.params;
}
