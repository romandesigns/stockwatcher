import { useEffect } from "react";

import { initPush } from "../notifications/push";
import { loadAppStore } from "../state/store";
import { showFired } from "./actions";

/**
 * At the app's root: sets up phone push (initPush: the stored token, a fresh one when permission
 * was already given, a tapped notification opens the Alerts tab) and shows the in-app banner for
 * each fire heard live. Each tab's socket delivers the same `alert.fired`, so a fire is shown once
 * by its id; a reconnect's Activity snapshot is history and shows nothing.
 */
export function useFiredBanner(): void {
  useEffect(() => {
    void initPush();
    let unsub: (() => void) | null = null;
    let alive = true;
    let shown: string | null = null;
    void loadAppStore().then((store) => {
      if (!alive) return;
      unsub = store.subscribe((s, prev) => {
        const e = s.lastFired;
        if (!e || e === prev.lastFired || e.id === shown || !e.app) return;
        shown = e.id;
        showFired(e.symbol, e.text);
      });
    });
    return () => {
      alive = false;
      unsub?.();
    };
  }, []);
}
