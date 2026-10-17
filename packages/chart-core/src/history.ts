// Undo / redo for the chart's drawings (owner, 2026-10-17: one mis-drag
// should not cost a careful drawing). A plain past/future stack of whole
// snapshots: drawings are small, and whole snapshots cannot drift the way
// replayed edits can. Only committed changes are recorded (a drag records
// once, on release), so one undo undoes one thing the user did.

export interface History<T> {
  past: T[];
  future: T[];
}

export const HISTORY_LIMIT = 100;

export function emptyHistory<T>(): History<T> {
  return { past: [], future: [] };
}

/** Records `current` before it is replaced by a new state; clears redo. */
export function record<T>(h: History<T>, current: T): History<T> {
  const past = [...h.past, current];
  return { past: past.length > HISTORY_LIMIT ? past.slice(past.length - HISTORY_LIMIT) : past, future: [] };
}

/** The state to go back to, and the history after; null when there is nothing to undo. */
export function undo<T>(h: History<T>, current: T): { state: T; history: History<T> } | null {
  const prev = h.past.at(-1);
  if (prev === undefined) return null;
  return { state: prev, history: { past: h.past.slice(0, -1), future: [current, ...h.future] } };
}

export function redo<T>(h: History<T>, current: T): { state: T; history: History<T> } | null {
  const next = h.future[0];
  if (next === undefined) return null;
  return { state: next, history: { past: [...h.past, current], future: h.future.slice(1) } };
}
