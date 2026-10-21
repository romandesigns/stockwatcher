import type { ConnectionStatus, MarketSource } from "@stockwatcher/shared";
import { useEffect, useState } from "react";

import { loadSettingsSync } from "../settings/sync";
import { appActions, loadAppStore } from "../state/store";
import { getAccessToken, setAccessToken } from "../storage/secure";
import { createMobileFeed, type MobileFeed } from "./serverFeed";

export interface MobileConnection {
  /** Null until the token, the synced cache and the app store have loaded. */
  ready: { feed: MobileFeed; token: string | null } | null;
  status: ConnectionStatus;
  /** Saves a token from the sign-in screen and reconnects with it. */
  signIn(token: string): void;
}

/**
 * A screen's connection to the server: the token from the keystore, the app store (the synced
 * cache and this phone's layout, so the screen opens with the last known state before the socket
 * connects) and a feed on its own socket. Each tab that shows market data owns one: the chart
 * tab's default slot plus the dashboard's 4 slots would exceed the server's 4 per connection,
 * and both tabs stay mounted. `source` is the server market the tab reads: `live` for the
 * dashboard, `replay` for the chart tab (the practice feature).
 */
export function useConnection(client: string, source: MarketSource): MobileConnection {
  const [ready, setReady] = useState<(NonNullable<MobileConnection["ready"]> & { source: MarketSource; client: string; attempt: number }) | null>(null);
  const [status, setStatus] = useState<ConnectionStatus>("connecting");
  // Each sign-in restarts the connection with the new token.
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let feed: MobileFeed | null = null;
    let alive = true;
    void Promise.all([getAccessToken().catch(() => null), loadSettingsSync(), loadAppStore()]).then(([token, sync]) => {
      if (!alive) return;
      feed = createMobileFeed(token, setStatus, sync, client, source, (m) => appActions().applyAlertsMessage(m));
      setReady({ feed, token, source, client, attempt });
    });
    return () => {
      alive = false;
      feed?.close();
    };
  }, [attempt, client, source]);
  return {
    // Effects load asynchronously. Never label the previous source's socket with the newly
    // selected source while its replacement is loading (the chart modal shares this feed).
    ready: ready?.source === source && ready.client === client && ready.attempt === attempt ? ready : null,
    status,
    signIn: (t) => {
      void setAccessToken(t).then(() => {
        setStatus("connecting");
        setAttempt((n) => n + 1);
      });
    },
  };
}
