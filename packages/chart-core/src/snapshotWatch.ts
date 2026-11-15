// Notices a chart whose first frame is overdue (the empty chart, 2026-11-15).
//
// A chart.subscribe is answered at once by its chart.snapshot (crates/server/src/ws.rs: an empty
// one even while the symbol's history is still loading). So a slot that has heard nothing a few
// seconds after subscribing was almost certainly sent into a connection that died without
// closing; the feed then checks the connection (shared ws.ts `checkConnection`), which reconnects
// a dead one at once and the reconnect resubscribes every open chart. Shared by the web and phone
// feeds so both recover alike.

/** How long a subscription may wait for its snapshot before the feed checks the connection. */
export const SNAPSHOT_TIMEOUT_MS = 5_000;

export interface SnapshotWatch {
  /** A subscription was (re)sent for `id`: its snapshot is now awaited. */
  expect(id: string): void;
  /** `id`'s snapshot arrived. */
  seen(id: string): void;
  /** `id` is no longer shown. */
  forget(id: string): void;
  clear(): void;
}

export function createSnapshotWatch(onOverdue: (id: string) => void, timeoutMs = SNAPSHOT_TIMEOUT_MS): SnapshotWatch {
  const timers = new Map<string, ReturnType<typeof setTimeout>>();
  const forget = (id: string) => {
    const t = timers.get(id);
    if (t !== undefined) clearTimeout(t);
    timers.delete(id);
  };
  return {
    expect(id) {
      forget(id);
      timers.set(
        id,
        setTimeout(() => {
          timers.delete(id);
          onOverdue(id);
        }, timeoutMs),
      );
    },
    seen: forget,
    forget,
    clear() {
      for (const t of timers.values()) clearTimeout(t);
      timers.clear();
    },
  };
}
