import Constants from "expo-constants";
import * as Device from "expo-device";
import * as Notifications from "expo-notifications";
import { Platform } from "react-native";

import { colors } from "../theme";

// Wired but deliberately NOT called on launch (boilerplate scope). On
// Android 13+ creating the first channel is what triggers the system
// permission prompt, so even the channel waits until a feature asks for
// alerts in context, e.g. after the user turns on a watchlist alert.

/** Must match `defaultChannel` of the expo-notifications plugin (app.config.ts). */
export const ALERTS_CHANNEL_ID = "alerts";

/** How a notification behaves while the app is in the foreground. */
export function configureForegroundPresentation(): void {
  Notifications.setNotificationHandler({
    handleNotification: async () => ({
      shouldShowBanner: true,
      shouldShowList: true,
      shouldPlaySound: true,
      shouldSetBadge: false,
    }),
  });
}

export async function ensureAlertsChannel(): Promise<void> {
  if (Platform.OS !== "android") return;
  await Notifications.setNotificationChannelAsync(ALERTS_CHANNEL_ID, {
    name: "Stock alerts",
    importance: Notifications.AndroidImportance.HIGH,
    lightColor: colors.accent,
    vibrationPattern: [0, 200, 120, 200],
  });
}

export type PushRegistration =
  | { status: "granted"; expoPushToken: string }
  | { status: "denied" }
  | { status: "unsupported"; reason: string };

/**
 * Creates the channel, asks for permission and returns the Expo push token
 * for the server to target. Call it from a user action, never on launch.
 */
export async function registerForPushNotifications(): Promise<PushRegistration> {
  if (!Device.isDevice) {
    return { status: "unsupported", reason: "Push needs a physical device." };
  }

  await ensureAlertsChannel();

  const current = await Notifications.getPermissionsAsync();
  const final = current.granted ? current : await Notifications.requestPermissionsAsync();
  if (!final.granted) return { status: "denied" };

  const projectId = Constants.expoConfig?.extra?.eas?.projectId as string | undefined;
  if (!projectId) {
    return { status: "unsupported", reason: "EAS project id missing from app config." };
  }
  const token = await Notifications.getExpoPushTokenAsync({ projectId });
  return { status: "granted", expoPushToken: token.data };
}
