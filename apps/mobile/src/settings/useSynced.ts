import { diffDrawings, drawingsFromDoc, drawingsNs, type Drawing } from "@stockwatcher/chart-core";
import type { SettingsOrigin, SettingsSync } from "@stockwatcher/shared";
import { useCallback, useMemo, useSyncExternalStore } from "react";

// React bindings for the synced settings: a copy of the web app's
// (apps/web/src/settings/useSynced.ts); keep the two the same. Ten lines each, not worth a
// React dependency in @stockwatcher/shared.

/**
 * One synced namespace as React state: the parsed value (re-rendered when any device changes
 * it), a setter (optimistic, synced), and where the current value came from. `parse` must be a
 * stable function (a module-level parser such as parseLayers): it validates every value, local
 * or remote.
 */
export function useSynced<T>(sync: SettingsSync, ns: string, parse: (raw: unknown) => T): [T, (value: T) => void, SettingsOrigin] {
  const view = useMemo(() => sync.view(ns, parse), [sync, ns, parse]);
  const value = useSyncExternalStore(view.subscribe, view.get);
  const set = useCallback((v: T) => sync.set(ns, v), [sync, ns]);
  return [value, set, view.origin()];
}

/**
 * One symbol's drawings as a list, written back as a per-drawing patch (only what was added,
 * changed or removed), so an edit on this device never overwrites another device's edit of a
 * different drawing.
 */
export function useSyncedDrawings(sync: SettingsSync, symbol: string): [Drawing[], (next: Drawing[]) => void] {
  const ns = drawingsNs(symbol);
  const [drawings] = useSynced(sync, ns, drawingsFromDoc);
  const set = useCallback(
    (next: Drawing[]) => {
      if (!symbol) return;
      const p = diffDrawings(sync.get(ns), next);
      if (p) sync.patch(ns, p.set, p.remove);
    },
    [sync, ns, symbol],
  );
  return [drawings, set];
}
