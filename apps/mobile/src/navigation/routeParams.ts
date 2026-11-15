// Pure half of liveRouteParams.ts (no React Native import, so bun tests it with the real
// StackRouter): a route's current params, looked up by its key in its navigator's state.

export type StateLike = { routes: readonly { key: string; params?: unknown }[] } | undefined;

/** The params of the route with `key` in `state`, or undefined when it is not there. */
export function paramsForKey<P>(state: StateLike, key: string): P | undefined {
  const route = state?.routes.find((r) => r.key === key);
  return route?.params as P | undefined;
}
