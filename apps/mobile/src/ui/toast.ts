// The phone's toast state (the web's toastStore.ts): one line ("Copied AAL") that fades out by
// itself; a new one replaces the current one. Rendered by <Toaster /> at the app's root.

export interface Toast {
  id: number;
  text: string;
}

let current: Toast | null = null;
let nextId = 1;
const listeners = new Set<() => void>();
const emit = () => listeners.forEach((l) => l());

export function showToast(text: string): void {
  current = { id: nextId++, text };
  emit();
}

export const getToast = (): Toast | null => current;

export function dismissToast(id: number): void {
  if (current?.id !== id) return;
  current = null;
  emit();
}

export function subscribeToast(cb: () => void): () => void {
  listeners.add(cb);
  return () => listeners.delete(cb);
}
