// The phone's toast state (the web's toastStore.ts): one line that fades by itself ("Copied AAL"),
// or one with an action (an alert's Undo, a fired alert's View) that stays UNDO_MS so it can be
// reached; a new one replaces the current one. Rendered by <Toaster /> at the app's root.

export interface ToastAction {
  label: string;
  run: () => void;
}

export interface Toast {
  id: number;
  text: string;
  icon?: "check" | "bell";
  /**
   * The practice screen's source chip before the text (owner, 2026-11-23): "Practice" for what
   * the replay decided, "Live" for a real alert arriving meanwhile, so the two are never confused.
   */
  tag?: "practice" | "live";
  action?: ToastAction;
  ms?: number;
  /** Lines the text may take (default 2): a row's press-and-hold read-out of its marks needs more. */
  lines?: number;
}

/** The design's "Toast → Undo (5 s)". */
export const UNDO_MS = 5000;

let current: Toast | null = null;
let nextId = 1;
const listeners = new Set<() => void>();
const emit = () => listeners.forEach((l) => l());

export function showToast(text: string, opts: Omit<Toast, "id" | "text"> = {}): void {
  current = { id: nextId++, text, ...opts };
  emit();
}

export const getToast = (): Toast | null => current;

export function dismissToast(id: number): void {
  if (current?.id !== id) return;
  current = null;
  emit();
}

/** Runs the current toast's action and dismisses it; false when there is none. */
export function runToastAction(): boolean {
  const t = current;
  if (!t?.action) return false;
  dismissToast(t.id);
  t.action.run();
  return true;
}

export function subscribeToast(cb: () => void): () => void {
  listeners.add(cb);
  return () => listeners.delete(cb);
}
