import { rossDemoPanel, type RossChannel, type RossState } from "@stockwatcher/shared";
import { useMemo, useState } from "react";
import { useRoss } from "./useRoss.ts";

/** The Ross state, flagged when it is the labelled demo panel. */
export type RossDemoState = RossState & { demo?: boolean };

/**
 * The Ross Panel's state as the side column reads it (to decide whether the panel has rows), with
 * the labelled demo mode (dev builds only, `?rossDemo`): synthetic members to see the cues before
 * the evidence sources admit real ones. Never in a production build.
 */
export function useRossState(channel: RossChannel | undefined): RossDemoState {
  const live = useRoss(channel);
  const [demoAt] = useState(() => (import.meta.env.DEV && typeof window !== "undefined" && new URLSearchParams(window.location.search).has("rossDemo") ? Date.now() * 1_000_000 : null));
  return useMemo(() => (demoAt !== null ? { ...live, enabled: true, panel: rossDemoPanel(demoAt), connected: true, demo: true } : live), [demoAt, live]);
}
