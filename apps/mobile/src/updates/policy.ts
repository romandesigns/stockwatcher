// Pure decision logic for the foreground update check, kept free of React
// Native imports so `bun test` can exercise it directly.

/**
 * Minimum gap between two foreground checks. Flicking between apps fires
 * "active" repeatedly; one request per window is plenty, and EAS Update
 * counts every check against the project's bandwidth.
 */
export const FOREGROUND_CHECK_INTERVAL_MS = 15 * 60 * 1000;

export interface CheckState {
  /** Epoch ms of the last check that started, or null if none yet. */
  lastCheckedAt: number | null;
  /** A check or download is already running. */
  inFlight: boolean;
  /** An update is downloaded and waiting for a restart: nothing to fetch. */
  updatePending: boolean;
}

export function shouldCheckForUpdate(
  state: CheckState,
  now: number,
  minIntervalMs: number = FOREGROUND_CHECK_INTERVAL_MS,
): boolean {
  if (state.inFlight || state.updatePending) return false;
  if (state.lastCheckedAt === null) return true;
  // A clock moved backwards (manual change, timezone bug) must not block
  // checks until it catches up again.
  if (now < state.lastCheckedAt) return true;
  return now - state.lastCheckedAt >= minIntervalMs;
}
