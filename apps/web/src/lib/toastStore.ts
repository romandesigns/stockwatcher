// The toast's state, apart from its component so the component file only exports components
// (React fast refresh). One toast at a time: a new one replaces the current one.
//
// Two kinds (2026-10-19, price alerts): a plain confirmation that fades by itself ("Copied AAL"),
// and one with an action (an alert's Undo, a fired alert's View) that stays for UNDO_MS so the
// action can be reached, with the bell for alert news.

export interface ToastAction {
  label: string;
  run: () => void;
}

export interface Toast {
  id: number;
  text: string;
  icon?: "check" | "bell";
  action?: ToastAction;
  /** How long it is fully visible (ms); defaults by kind. */
  ms?: number;
}

/** A toast with an action (Undo) stays this long: the design's "Toast → Undo (5 s)". */
export const UNDO_MS = 5000;

let current: Toast | null = null;
let nextId = 1;
const listeners = new Set<() => void>();
const emit = () => listeners.forEach((l) => l());

/** Shows a one-line confirmation ("Copied AAL") that fades out by itself, or one with an action. */
export function showToast(text: string, opts: Omit<Toast, "id" | "text"> = {}): void {
  current = { id: nextId++, text, ...opts };
  emit();
}

export const getToast = (): Toast | null => current;

/** Clears the toast if it is still `id` (a newer one is left alone). */
export function dismissToast(id: number): void {
  if (current?.id !== id) return;
  current = null;
  emit();
}

/** Runs the current toast's action (its button, or Ctrl+Z for an Undo) and dismisses it. False when there is none. */
export function runToastAction(label?: string): boolean {
  const t = current;
  if (!t?.action || (label && t.action.label !== label)) return false;
  dismissToast(t.id);
  t.action.run();
  return true;
}

export function subscribeToast(cb: () => void): () => void {
  listeners.add(cb);
  return () => listeners.delete(cb);
}
