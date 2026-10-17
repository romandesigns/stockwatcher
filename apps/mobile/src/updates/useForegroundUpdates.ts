import * as Updates from "expo-updates";
import { useCallback, useEffect, useRef, useState } from "react";
import { AppState, type AppStateStatus } from "react-native";

import { shouldCheckForUpdate } from "./policy";

// Launch is covered by app config (checkAutomatically ON_LOAD,
// fallbackToCacheTimeout 0: start on the cached bundle, never wait).
// This hook adds the check on return to foreground, so a phone that keeps
// the app alive for days still picks up OTA updates.
//
// A found update downloads in the background and is applied on the next
// cold start. `restart` exists for a future "Restart to update" prompt; no
// UI calls it yet (boilerplate: Hello world only).

export interface ForegroundUpdates {
  /** A newer update is downloaded and will run after a restart. */
  updateReady: boolean;
  /** Reload into the downloaded update now. Only call on user consent. */
  restart: () => Promise<void>;
}

export function useForegroundUpdates(): ForegroundUpdates {
  const [updateReady, setUpdateReady] = useState(false);
  const lastCheckedAt = useRef<number | null>(null);
  const inFlight = useRef(false);
  const pending = useRef(false);

  const check = useCallback(async () => {
    const now = Date.now();
    if (
      !shouldCheckForUpdate(
        { lastCheckedAt: lastCheckedAt.current, inFlight: inFlight.current, updatePending: pending.current },
        now,
      )
    ) {
      return;
    }
    lastCheckedAt.current = now;
    inFlight.current = true;
    try {
      const result = await Updates.checkForUpdateAsync();
      if (!result.isAvailable) return;
      const fetched = await Updates.fetchUpdateAsync();
      if (fetched.isNew) {
        pending.current = true;
        setUpdateReady(true);
      }
    } catch (err) {
      // Offline, server error or a rejected manifest: the running bundle is
      // fine, so log and try again on a later foreground.
      console.warn("[updates] foreground check failed", err);
    } finally {
      inFlight.current = false;
    }
  }, []);

  useEffect(() => {
    // The Updates API rejects in development builds (the bundle comes from
    // Metro) and when updates are disabled; there is nothing to check.
    if (__DEV__ || !Updates.isEnabled) return;

    const onChange = (next: AppStateStatus) => {
      if (next === "active") void check();
    };
    const sub = AppState.addEventListener("change", onChange);
    return () => sub.remove();
  }, [check]);

  const restart = useCallback(async () => {
    await Updates.reloadAsync();
  }, []);

  return { updateReady, restart };
}
