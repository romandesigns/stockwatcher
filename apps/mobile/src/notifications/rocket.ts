import { isBurstRow, isFastRow, isRunningRow, isRunRow, type RocketRowWire } from "@stockwatcher/shared";
import * as Notifications from "expo-notifications";
import { Platform } from "react-native";

import { pushToken } from "./push";
import { ringRunnersLocally } from "./route";
import { runnersPushPrefs } from "./runnersPush";
import { ALERTS_CHANNEL_ID } from "./setup";

// Runners' local banner from diffing panels: only the stand-in for a server OLDER than
// `runners.notice.v1` (owner, 2026-10-21; push 2026-10-22). With a push token the server rings
// this phone for each new flag even with the app closed (src/notifications/runnersPush.ts,
// Settings → Runners), so the dashboard stays quiet: two banners for one flag would be noise.
// Without one (permission refused, a build without FCM) a current server sends the notices
// themselves, which runnersNotices.ts rings with the push's own text, the same as the desktop
// and web; only against an older server does the dashboard ring a banner here for each new row it
// sees. A local notification, not a push: nothing reaches the server. Delete with the old servers.

/**
 * One banner (with the channel's sound) per new flag or buying burst, when this phone has no
 * push; flags under the "new flags" switch, bursts under "buying bursts" (the push's kinds).
 */
export function notifyRocketFlags(rows: RocketRowWire[]): void {
  const prefs = runnersPushPrefs();
  const token = pushToken();
  // Running, "Run +20%" and "Fast move" entries under their own switches (2026-10-22), like the push.
  const ring = rows.filter((r) => ringRunnersLocally(isBurstRow(r) ? prefs.pushBurst : isRunningRow(r) ? prefs.pushRunning : isRunRow(r) ? prefs.pushRun : isFastRow(r) ? prefs.pushFast : prefs.pushNew, token));
  for (const r of ring.slice(0, 3)) {
    const price = `$${r.p.toFixed(r.p >= 1 ? 2 : 4)}`;
    const burst = isBurstRow(r) ? r.burst : null;
    void Notifications.scheduleNotificationAsync({
      content: {
        title: burst
          ? `Buying burst: ${r.s} ${price}${burst.gain_pct != null ? ` (+${Math.round(burst.gain_pct)}% since start)` : ""}`
          : isRunningRow(r)
            ? `Running: ${r.s} ${price}`
            : isRunRow(r)
              ? `Run +20%: ${r.s} ${price}`
              : isFastRow(r)
                ? `Fast move: ${r.s} ${price}`
                : `Runners: ${r.s} ${price}`,
        body: r.reasons.length ? r.reasons.slice(0, 2).join(" · ") : burst ? "Heavy buying confirmed (experimental)" : "Flagged (experimental)",
        sound: true,
        // A tap opens the chart, like the push (src/notifications/route.ts).
        data: { symbol: r.s, kind: burst ? "runners.burst" : isRunningRow(r) ? "runners.running" : isRunRow(r) ? "runners.run" : isFastRow(r) ? "runners.fast" : "runners.new", alert_id: r.id },
      },
      // Now, on the alerts channel (Android: its sound and importance).
      trigger: Platform.OS === "android" ? { channelId: ALERTS_CHANNEL_ID } : null,
      // Not allowed (never asked, or refused): nothing to ring; the card still shows the flag.
    }).catch(() => {});
  }
}
