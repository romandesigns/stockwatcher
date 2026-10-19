import { Icon } from "@stockwatcher/chart-web";
import { cn } from "@stockwatcher/chart-web/ui";
import { useEffect, useState, useSyncExternalStore } from "react";
import { dismissToast, getToast, subscribeToast } from "./toastStore.ts";

// A one-line confirmation that fades out ("Copied AAL"), announced politely to screen readers.
// Shown with showToast (toastStore.ts).

/** How long a toast is fully visible, then how long it fades (ms). */
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
    const fade = setTimeout(() => setFading(true), SHOW_MS);
    const gone = setTimeout(() => dismissToast(toast.id), SHOW_MS + FADE_MS);
    return () => {
      clearTimeout(fade);
      clearTimeout(gone);
    };
  }, [toast]);
  return (
    <div aria-live="polite" className="pointer-events-none fixed inset-x-0 z-50 flex justify-center px-4" style={{ bottom }}>
      {toast && (
        <div
          role="status"
          className={cn(
            "flex items-center gap-2 rounded-full bg-bg-raised px-3.5 py-2 text-sm text-foreground shadow-lg transition-opacity ease-out motion-reduce:transition-none [&_svg]:size-3.5 [&_svg]:text-primary",
            fading ? "opacity-0" : "opacity-100",
          )}
          style={{ transitionDuration: `${FADE_MS}ms` }}
        >
          <Icon name="check" />
          {toast.text}
        </div>
      )}
    </div>
  );
}
