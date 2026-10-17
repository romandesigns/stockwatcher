import { Component, type ReactNode } from "react";

/**
 * Last line of defence: an error anywhere in the screen shows a short note
 * and a reload button instead of unmounting everything, which used to leave
 * a dark, empty page (2026-10-17: a menu threw on open and blanked the app).
 */
export class ErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state = { error: null as Error | null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error) {
    console.error("StockWatcher crashed:", error);
  }

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <div role="alert" className="flex min-h-dvh flex-col items-center justify-center gap-3 bg-background p-6 text-center text-foreground">
        <p className="m-0 text-sm font-semibold">Something went wrong on this screen.</p>
        <p className="m-0 max-w-md font-mono text-xs text-muted-foreground">{this.state.error.message}</p>
        <button type="button" onClick={() => location.reload()} className="h-8 rounded-lg bg-primary px-3 text-xs font-semibold text-primary-foreground">
          Reload
        </button>
      </div>
    );
  }
}
