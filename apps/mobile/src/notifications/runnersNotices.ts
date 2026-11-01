import type { RunnersNoticeMessage } from "@stockwatcher/shared";
import * as Notifications from "expo-notifications";
import { Platform } from "react-native";

import { pushToken } from "./push";
import { createPhoneNotices, noticeBannerData } from "./runnersNoticeModel";
import { runnersPushPrefs } from "./runnersPush";
import { ALERTS_CHANNEL_ID } from "./setup";

// The phone's one notice handler (runnersNoticeModel.ts): the dashboard's connection feeds it
// (src/feed/serverFeed.ts) and the Runners card asks it whether to diff panels (RocketPanel.tsx).

function ring(m: RunnersNoticeMessage): void {
  void Notifications.scheduleNotificationAsync({
    content: { title: m.title, body: m.body, sound: true, data: noticeBannerData(m) },
    // Now, on the alerts channel (Android: its sound and importance).
    trigger: Platform.OS === "android" ? { channelId: ALERTS_CHANNEL_ID } : null,
    // Not allowed (never asked, or refused): nothing to ring; the card still shows the row.
  }).catch(() => {});
}

export const phoneNotices = createPhoneNotices({ prefs: runnersPushPrefs, token: pushToken, ring });
