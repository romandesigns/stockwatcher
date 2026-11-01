import { Input } from "@stockwatcher/chart-web/ui";
import type { MarketSource } from "@stockwatcher/shared";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { createServerFeed } from "./feed/serverFeed.ts";
import { probeServer, serverChoice, type SourceChoice } from "./feed/source.ts";
import { clearToken, getToken, setToken } from "./feed/token.ts";
import type { ChartFeed } from "./feed/types.ts";
import { getSettingsSync } from "./settings/sync.ts";
import { getAppStore } from "./state/store.ts";
import { getRunnersNotifier } from "./notify/runnersNotify.ts";

// Data source: the Rust server, always (feed/source.ts). A server that cannot be reached is shown
// as exactly that, and retried, never replaced by data made up in the browser.

/** How long an unreachable server waits before the next probe (a deploy restarts it in seconds). */
const RETRY_MS = 5000;

export interface Connection {
  choice: SourceChoice;
  feed: ChartFeed;
}

/**
 * Connects a page to its data: probes the server, asks for the access token when the server wants
 * one (production), and opens the feed. Every page that shows market data renders inside one
 * (the chart page and the dashboard); only one page is open at a time, so there is one socket.
 * `source` picks the server's market for the page: `live` for the dashboard, `replay` for the
 * chart page (the practice feature).
 * The feed is created and closed by the same effect, so React StrictMode's dev-only
 * mount → unmount → mount gets a fresh feed, never a closed one.
 */
export function ConnectionGate({ source, children }: { source: MarketSource; children: (c: Connection) => ReactNode }) {
  const [ready, setReady] = useState<(Connection & { marketSource: MarketSource; attempt: number }) | null>(null);
  // The server wants an access token (production): show the sign-in card.
  const [signIn, setSignIn] = useState<{ rejected: boolean } | null>(null);
  // The server did not answer (or refused): say so and probe again every RETRY_MS.
  const [unreachable, setUnreachable] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  // One engine for the page, connected by the server feed.
  const sync = useMemo(() => getSettingsSync(), []);
  useEffect(() => {
    let alive = true;
    let feed: ChartFeed | null = null;
    let retry: ReturnType<typeof setTimeout> | undefined;
    const token = getToken();
    void probeServer(fetch, 2500, token, source).then((probe) => {
      if (!alive) return;
      if (!probe.ok && probe.unauthorized) {
        if (token) clearToken();
        setUnreachable(null);
        setSignIn({ rejected: !!token });
        return;
      }
      if (!probe.ok) {
        setUnreachable(probe.reason);
        retry = setTimeout(() => setAttempt((n) => n + 1), RETRY_MS);
        return;
      }
      setSignIn(null);
      setUnreachable(null);
      const choice = serverChoice(probe.data, source);
      // Alert frames (Activity, fires) reach the app store whatever page is open (state/store.ts).
      // Runners notices too (notify/runnersNotify.ts): a system notification on any page.
      feed = createServerFeed({ token, settings: sync, source, onAlerts: (m) => getAppStore().getState().applyAlertsMessage(m), notifier: getRunnersNotifier() });
      setReady({ choice, feed, marketSource: source, attempt });
    });
    return () => {
      alive = false;
      if (retry) clearTimeout(retry);
      feed?.close();
    };
  }, [attempt, sync, source]);
  if (signIn) {
    return (
      <SignIn
        rejected={signIn.rejected}
        onSubmit={(t) => {
          setToken(t);
          setAttempt((n) => n + 1);
        }}
      />
    );
  }
  if (unreachable !== null) return <Unreachable reason={unreachable} onRetry={() => setAttempt((n) => n + 1)} />;
  if (!ready || ready.marketSource !== source || ready.attempt !== attempt) {
    return <div className="flex min-h-full items-center justify-center bg-background text-sm text-muted-foreground">Connecting to the server…</div>;
  }
  return <>{children(ready)}</>;
}

/**
 * The server could not be reached or refused (not an auth problem): what happened, and that it is
 * being retried. Nothing else is shown, because nothing else would be real. Polite live region, so
 * a screen reader hears the state without the retries repeating it.
 */
function Unreachable({ reason, onRetry }: { reason: string; onRetry: () => void }) {
  return (
    <div className="flex min-h-full items-center justify-center bg-background p-4 text-foreground">
      <div role="status" aria-live="polite" className="flex w-full max-w-sm flex-col gap-3 rounded-2xl bg-card p-5">
        <div className="flex flex-col gap-1">
          <h1 className="m-0 text-base font-semibold">Server unreachable</h1>
          <p className="m-0 text-xs text-muted-foreground">StockWatcher shows only what its server sends. Trying again every {RETRY_MS / 1000} seconds.</p>
          <p className="m-0 break-words font-mono text-xs text-text-faint">{reason}</p>
        </div>
        <button type="button" onClick={onRetry} className="h-10 rounded-lg bg-bg-row text-sm font-semibold text-foreground hover:bg-bg-raised focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring pointer-coarse:h-11">
          Retry now
        </button>
      </div>
    </div>
  );
}

/** The one-field sign-in for a server that requires its access token. */
function SignIn({ rejected, onSubmit }: { rejected: boolean; onSubmit: (token: string) => void }) {
  const [value, setValue] = useState("");
  return (
    <div className="flex min-h-full items-center justify-center bg-background p-4 text-foreground">
      <form
        className="flex w-full max-w-sm flex-col gap-3 rounded-2xl bg-card p-5"
        onSubmit={(e) => {
          e.preventDefault();
          if (value.trim()) onSubmit(value.trim());
        }}
      >
        <div className="flex flex-col gap-1">
          <h1 className="m-0 text-base font-semibold">StockWatcher</h1>
          <p className="m-0 text-xs text-muted-foreground">Enter your access token to connect to the server.</p>
        </div>
        <label htmlFor="access-token" className="sr-only">
          Access token
        </label>
        <Input
          id="access-token"
          type="password"
          autoComplete="current-password"
          autoFocus
          value={value}
          onChange={(e) => setValue(e.target.value)}
          placeholder="Access token"
          aria-invalid={rejected}
          // 16 px on touch: iOS zooms the page into any focused field set smaller.
          className="h-10 rounded-lg bg-bg-row px-3 font-mono text-sm outline-none placeholder:text-text-faint focus-visible:ring-2 focus-visible:ring-ring pointer-coarse:h-11 pointer-coarse:text-base"
        />
        {rejected && <p className="m-0 text-xs text-critical">That token wasn't accepted. Check it and try again.</p>}
        <button type="submit" disabled={!value.trim()} className="h-10 rounded-lg bg-primary text-sm font-semibold text-primary-foreground disabled:opacity-50">
          Sign in
        </button>
      </form>
    </div>
  );
}
