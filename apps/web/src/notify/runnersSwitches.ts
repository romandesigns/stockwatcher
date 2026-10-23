import type { IconName } from "@stockwatcher/chart-core";
import type { RunnersNotifyKinds } from "@stockwatcher/shared";

/**
 * The phone's six switches, in its order and words (apps/mobile src/settings/rocketWatch.tsx),
 * so the three clients read the same.
 */
export const RUNNERS_SWITCHES: readonly { key: keyof RunnersNotifyKinds; icon: IconName; name: string; description: string }[] = [
  { key: "new", icon: "bell", name: "New flags", description: "Ticker, price and the top reasons as soon as Runners flags it." },
  { key: "breakout", icon: "gauge", name: "Breakouts", description: "When a flagged stock turns breaking out, with its % from the flag." },
  { key: "burst", icon: "trend", name: "Buying bursts", description: "When heavy buying is confirmed on the tape, with how far the burst already went." },
  { key: "running", icon: "gauge", name: "Running entries", description: "Every stock that starts running by its own range, with its move and money." },
  { key: "run", icon: "trend", name: "Run +20%", description: "A +20% leg within 15 minutes on real trades, with its window and start price." },
  { key: "fast", icon: "live", name: "Fast move", description: "+8% in a minute or +12% in 15 s on real trades, with the move and its money." },
];
