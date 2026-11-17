import type { SettingsDialogSection } from "@stockwatcher/chart-web";
import { RunnersNotifyPanel } from "./RunnersNotifyPanel.tsx";
import { getRunnersNotifier } from "./runnersNotify.ts";

/**
 * Settings → Alerts on the web and the desktop shell (owner, 2026-10-23): this device's system
 * notifications for the Runners, Ross, Signals and Halt watcher panels (runnersNotify.ts), as the
 * phone gets them. Per device, never synced, like the phone's. The id stays "runners" so a
 * remembered open section still finds it.
 */
export function runnersNotifySection(): SettingsDialogSection {
  const notifier = getRunnersNotifier();
  return {
    id: "runners",
    label: "Alerts",
    icon: "bell",
    description: notifier.desktop ? "System notifications on this computer for Runners, Ross, Signals and the Halt watcher, as the phone gets them, and a sound for exchange halts." : "Browser notifications in this tab for Runners, Ross, Signals and the Halt watcher, as the phone gets them, and a sound for exchange halts.",
    render: () => <RunnersNotifyPanel notifier={notifier} />,
  };
}
