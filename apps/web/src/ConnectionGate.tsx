import type { MarketSource } from "@stockwatcher/shared";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { createServerFeed } from "./feed/serverFeed.ts";
import { chooseSource, probeServer, type SourceChoice } from "./feed/source.ts";
import { clearToken, getToken, setToken } from "./feed/token.ts";
import type { ChartFeed } from "./feed/types.ts";
import { createFixtureFeed, FIXTURE_SYMBOL } from "./fixtures/fixtureFeed.ts";
import { getSettingsSync } from "./settings/sync.ts";
import { getAppStore } from "./state/store.ts";

// Data source: the Rust server by default, the in-browser fixture only when the
// server is unreachable or has no data (feed/source.ts), or when forced with
// VITE_CHART_SOURCE=fixture. The active source is always labelled.
const ENV_SOURCE = import.meta.env.VITE_CHART_SOURCE;

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
  const [ready, setReady] = useState<Connection | null>(null);
  // The server wants an access token (production): show the sign-in card instead of falling back to the fixture.
  const [signIn, setSignIn] = useState<{ rejected: boolean } | null>(null);
  const [attempt, setAttempt] = useState(0);
  // One engine for the page, whatever the feed: the fixture never connects it (local only).
  const sync = useMemo(() => getSettingsSync(), []);
  useEffect(() => {
    let alive = true;
    let feed: ChartFeed | null = null;
    const token = getToken();
    void (ENV_SOURCE === "fixture" ? Promise.resolve(null) : probeServer(fetch, 2500, token, source)).then((probe) => {
      if (!alive) return;
      if (probe && !probe.ok && probe.unauthorized) {
        if (token) clearToken();
        setSignIn({ rejected: !!token });
        return;
      }
      setSignIn(null);
      const choice = chooseSource(ENV_SOURCE, probe, FIXTURE_SYMBOL, source);
      // Alert frames (Activity, fires) reach the app store whatever page is open (state/store.ts).
      feed = choice.kind === "server" ? createServerFeed({ token, settings: sync, source, onAlerts: (m) => getAppStore().getState().applyAlertsMessage(m) }) : createFixtureFeed();
      setReady({ choice, feed });
    });
    return () => {
      alive = false;
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
  if (!ready) {
    return <div className="flex min-h-dvh items-center justify-center bg-background text-sm text-muted-foreground">Connecting to the server…</div>;
  }
  return <>{children(ready)}</>;
}

/** The one-field sign-in for a server that requires its access token. */
function SignIn({ rejected, onSubmit }: { rejected: boolean; onSubmit: (token: string) => void }) {
  const [value, setValue] = useState("");
  return (
    <div className="flex min-h-dvh items-center justify-center bg-background p-4 text-foreground">
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
        <input
          id="access-token"
          type="password"
          autoComplete="current-password"
          autoFocus
          value={value}
          onChange={(e) => setValue(e.target.value)}
          placeholder="Access token"
          aria-invalid={rejected}
          className="h-10 rounded-lg bg-bg-row px-3 font-mono text-sm outline-none placeholder:text-text-faint focus-visible:ring-2 focus-visible:ring-ring"
        />
        {rejected && <p className="m-0 text-xs text-critical">That token wasn't accepted. Check it and try again.</p>}
        <button type="submit" disabled={!value.trim()} className="h-10 rounded-lg bg-primary text-sm font-semibold text-primary-foreground disabled:opacity-50">
          Sign in
        </button>
      </form>
    </div>
  );
}
