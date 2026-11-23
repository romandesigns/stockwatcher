import { Icon } from "@stockwatcher/chart-web";
import { cn } from "@stockwatcher/chart-web/ui";
import { useEffect, useState, useSyncExternalStore } from "react";
import { dismissToast, getToast, runToastAction, subscribeToast, UNDO_MS } from "./toastStore.ts";

// A one-line toast at the bottom centre ("Copied AAL", "Alert moved to 2.55 · Undo"), announced
// politely to screen readers. Shown with showToast (toastStore.ts).

/** How long a plain toast is fully visible, then how long it fades (ms). */
const SHOW_MS = 1300;
const FADE_MS = 300;

/** Renders the current toast at the bottom centre, above the phone's tab bar when there is one. */
export function Toaster({ bottom = "1.25rem" }: { bottom?: string }) {
  const toast = useSyncExternalStore(subscribeToast, getToast);
  const [fading, setFading] = useState(false);
  // A new toast starts fully visible (adjusted while rendering, not in an effect).
  const [shownId, setShownId] = useState<number | null>(null);
  if (toast && toast.id !== shownId) {
    setShownId(toast.id);
    setFading(false);
  }
  useEffect(() => {
    if (!toast) return;
    const show = toast.ms ?? (toast.action ? UNDO_MS : SHOW_MS);
    const fade = setTimeout(() => setFading(true), show);
    const gone = setTimeout(() => dismissToast(toast.id), show + FADE_MS);
    return () => {
      clearTimeout(fade);
      clearTimeout(gone);
    };
  }, [toast]);
  // Ctrl+Z (⌘Z) takes the Undo while one shows, unless something focused already handled it
  // (the chart's own drawing undo, a text field).
  useEffect(() => {
    if (toast?.action?.label !== "Undo") return;
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || !(e.ctrlKey || e.metaKey) || e.shiftKey || e.altKey || (e.key !== "z" && e.key !== "Z")) return;
      const t = e.target as HTMLElement | null;
      if (t && (t.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(t.tagName))) return;
      if (runToastAction("Undo")) e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [toast]);
  return (
    <div aria-live="polite" className="pointer-events-none fixed inset-x-0 z-50 flex justify-center px-4" style={{ bottom }}>
      {toast && (
        <div
          role="status"
          className={cn(
            "flex max-w-full items-center gap-2 rounded-full bg-bg-raised px-3.5 py-2 text-sm text-foreground shadow-lg transition-opacity ease-out motion-reduce:transition-none [&>svg]:size-3.5 [&>svg]:shrink-0 [&>svg]:text-primary",
            toast.action && "pointer-events-auto py-1.5 pr-1.5",
            fading ? "opacity-0" : "opacity-100",
          )}
          style={{ transitionDuration: `${FADE_MS}ms` }}
        >
          <Icon name={toast.icon ?? "check"} />
          {toast.tag && (
            // Quiet neutral for practice, the live market's own green for a real alert (the
            // pages' source chips use the same two tones): never the accent.
            <span className={cn("shrink-0 rounded-full px-2 py-0.5 text-[11px] font-semibold", toast.tag === "live" ? "bg-good-bg text-good" : "bg-bg-row text-muted-foreground")}>
              {toast.tag === "live" ? "Live" : "Practice"}
            </span>
          )}
          <span className="min-w-0 truncate">{toast.text}</span>
          {toast.action && (
            <button
              type="button"
              onClick={() => runToastAction()}
              className="h-7 shrink-0 rounded-full px-2.5 text-[13px] font-semibold text-primary outline-none hover:bg-bg-row focus-visible:ring-2 focus-visible:ring-ring"
            >
              {toast.action.label}
            </button>
          )}
        </div>
      )}
    </div>
  );
}
