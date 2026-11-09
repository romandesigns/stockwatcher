import { useEffect } from "react";

import { alertSoundFor } from "@stockwatcher/app-state";

import { initPush, pushToken } from "../notifications/push";
import { playDrawingTone } from "../notifications/setup";
import { loadAppStore } from "../state/store";
import { showDrawingFired, showFired } from "./actions";

/**
 * At the app's root: sets up phone push (initPush: the stored token, a fresh one when permission
 * was already given, a tapped notification opens the Alerts tab) and shows the in-app banner for
 * each fire heard live. Each tab's socket delivers the same `alert.fired`, so a fire is shown once
 * by its id; a reconnect's Activity snapshot is history and shows nothing.
 *
 * A drawing alert's fire (it carries a tone) sounds once: when its push went to the phones and this
 * phone is registered, the push itself rings on the tone's channel; otherwise the app plays the tone
 * as a local notification on that channel (app-state alertSoundFor decides whether it sounds at all:
 * the alert's Sound chip, the global switch, quiet hours). Its banner's "Got it" acknowledges it.
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
        if (!e || e === prev.lastFired || e.id === shown) return;
        shown = e.id;
        const tone = alertSoundFor(e, s.alerts.find((r) => r.id === e.alertId), s.alertPrefs);
        if (tone && (e.phone !== "sent" || !pushToken())) void playDrawingTone(tone, e.symbol, e.text, { alertId: e.alertId, symbol: e.symbol, eventId: e.id, url: "stockwatcher://alerts" });
        if (!e.app) return;
        if (e.tone) showDrawingFired(e.symbol, e.text, e.alertId);
        else showFired(e.symbol, e.text);
      });
    });
    return () => {
      alive = false;
      unsub?.();
    };
  }, []);
}
