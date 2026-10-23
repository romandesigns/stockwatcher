import type { SettingsDialogSection } from "@stockwatcher/chart-web";
import { RunnersNotifyPanel } from "./RunnersNotifyPanel.tsx";
import { getRunnersNotifier } from "./runnersNotify.ts";

/**
 * Settings → Runners on the web and the desktop shell (owner, 2026-10-23): this device's Runners
 * system notifications (runnersNotify.ts). Per device, never synced, like the phone's.
 */
export function runnersNotifySection(): SettingsDialogSection {
  const notifier = getRunnersNotifier();
  return {
    id: "runners",
    label: "Runners",
    icon: "bell",
    description: notifier.desktop ? "System notifications on this computer for the Runners board, as the phone gets them." : "Browser notifications in this tab for the Runners board, as the phone gets them.",
    render: () => <RunnersNotifyPanel notifier={notifier} />,
  };
}
