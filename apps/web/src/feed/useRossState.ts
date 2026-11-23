import { rossDemoPanel, rossDemoReadiness, type RossChannel, type RossState } from "@stockwatcher/shared";
import { useMemo, useState } from "react";
import { useRoss } from "./useRoss.ts";

/** The Ross state, flagged when it is the labelled demo panel. */
export type RossDemoState = RossState & { demo?: boolean };

/**
 * The Ross Panel's state as the dashboard layout reads it (to decide whether the panel has rows), with
 * the labelled demo mode (dev builds only, `?rossDemo`, or `?rossDemo=newsOptional`): synthetic members to see the cues before
 * the evidence sources admit real ones. Never in a production build.
 */
export function useRossState(channel: RossChannel | undefined): RossDemoState {
  const live = useRoss(channel);
  // `?rossDemo=newsOptional` shows the news-optional panel (ross-panel-v2) instead.
  const [demo] = useState(() => {
    if (!import.meta.env.DEV || typeof window === "undefined") return null;
    const q = new URLSearchParams(window.location.search);
    return q.has("rossDemo") ? { at: Date.now() * 1_000_000, newsOptional: q.get("rossDemo") === "newsOptional" } : null;
  });
  return useMemo(() => (demo !== null ? { ...live, enabled: true, panel: rossDemoPanel(demo.at, { newsOptional: demo.newsOptional }), readiness: rossDemoReadiness(demo.at), connected: true, demo: true } : live), [demo, live]);
}
