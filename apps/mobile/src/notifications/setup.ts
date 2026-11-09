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
  await ensureDrawingChannels();
}

export type DrawingTone = "up" | "down" | "stop";

/**
 * Drawing alerts' channels, one per tone (owner decision 2026-11-09: distinct tones for up-touch,
 * down-touch and stop). Android fixes a channel's sound when it is created, so each tone is its
 * own channel with its own bundled WAV (rendered by scripts/tones.ts from app-state's
 * ALERT_TONES, the web's score; listed in app.config.ts's expo-notifications `sounds`). The
 * server sends a sounding drawing alert's push on these ids (crates/server/src/alerts mod.rs
 * `drawing_channel`; muted ones go on ALERTS_CHANNEL_ID). A channel keeps its first sound: to
 * change a tone on phones that have it, change the id here and on the server together.
 * Volume is the phone's notification volume; Do Not Disturb applies as for any notification.
 */
export const DRAWING_CHANNELS: Readonly<Record<DrawingTone, { id: string; sound: string; name: string; description: string }>> = {
  up: { id: "drawing-up", sound: "drawing_up.wav", name: "Drawing alerts: price came up", description: "A rising tone: the price came up into a line you drew" },
  down: { id: "drawing-down", sound: "drawing_down.wav", name: "Drawing alerts: price came down", description: "A falling tone: the price came down into a line you drew" },
  stop: { id: "drawing-stop", sound: "drawing_stop.wav", name: "Drawing alerts: stop touched", description: "A low triple: the price touched a position's stop" },
};

export async function ensureDrawingChannels(): Promise<void> {
  if (Platform.OS !== "android") return;
  for (const c of Object.values(DRAWING_CHANNELS)) {
    await Notifications.setNotificationChannelAsync(c.id, {
      name: c.name,
      description: c.description,
      importance: Notifications.AndroidImportance.HIGH,
      sound: c.sound,
      lightColor: colors.accent,
      vibrationPattern: [0, 250, 120, 250],
    });
  }
}

/**
 * Plays a drawing alert's tone on this phone now, as a local notification on the tone's channel
 * (the only sound path the app has without an audio module; the system plays the channel's
 * bundled tone at the notification volume). Used for a fire heard in the app when no push brings
 * the tone itself, and for the Alerts tab's test buttons. Silently nothing without permission (never asks).
 */
export async function playDrawingTone(tone: DrawingTone, title: string, body: string, data: Record<string, unknown> = {}): Promise<void> {
  const c = DRAWING_CHANNELS[tone];
  try {
    // Never the first ask: creating a channel prompts on Android 13+, and a fire is no moment for it
    // (the Alerts tab's "Turn on phone alerts" is).
    if (!(await Notifications.getPermissionsAsync()).granted) return;
    await ensureDrawingChannels();
    await Notifications.scheduleNotificationAsync({ content: { title, body, sound: c.sound, data }, trigger: Platform.OS === "android" ? { channelId: c.id } : null });
  } catch {
    // No permission, or a build without notifications: the banner still shows.
  }
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
