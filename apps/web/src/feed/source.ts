// Which chart source is active, decided once at startup.
//
// The server is the default (VITE_CHART_SOURCE unset or "server"). The app
// asks GET /api/symbols first; it falls back to the in-browser fixture ONLY if
// the server is unreachable, refuses, or reports no symbols, and it always
// says which source is active and why. VITE_CHART_SOURCE=fixture forces the
// in-browser fixture. A live page (the dashboard) never falls back for an empty list: an empty
// live market is real (see chooseSource).

import { withMarketSource, type MarketSource } from "@stockwatcher/shared";

export interface ServerSymbols {
  mode: "replay" | "live" | (string & {});
  /** Provider status: starting, running, reconnecting (live link down, retrying), no_data, not_configured, error, stopped, idle. */
  status: string;
  /** e.g. "archive 2026-10-15 (fixture)" */
  source: string | null;
  day: string | null;
  /** Every event the server replays is fixture data (never market evidence). */
  fixture: boolean;
  /** Candidates only: symbols whose latest price is inside the server's price band. */
  symbols: string[];
  /** The band the server screens with, as decimal strings (e.g. "0.10" / "25.00"). */
  priceBand?: { min: string; max: string };
  /** How many known symbols were screened and how many passed. */
  screened?: { total: number; candidates: number };
  /** Live: the trading day shown (`LiveSession` in @stockwatcher/shared; read with parseLiveSession). */
  session?: unknown;
}

/** unauthorized: the server wants an access token and has none, or not this one (HTTP 401). */
export type Probe = { ok: true; data: ServerSymbols } | { ok: false; reason: string; unauthorized?: boolean };

export interface SourceChoice {
  kind: "server" | "fixture";
  /** Why this source; shown in the UI. */
  reason: string;
  symbols: string[];
  /** The data itself is fixture data (in-browser fixture, or a server fixture archive). */
  fixtureData: boolean;
  mode: string;
  label: string;
  /** The market the page asked for (ConnectionGate `source`): its REST calls and socket name it. */
  market: MarketSource;
}

/** GET /api/symbols with a timeout. Never throws. */
export async function probeServer(fetchImpl: typeof fetch = fetch, timeoutMs = 2500, token?: string | null, market?: MarketSource): Promise<Probe> {
  const ctl = new AbortController();
  const timer = setTimeout(() => ctl.abort(), timeoutMs);
  try {
    const res = await fetchImpl(withMarketSource("/api/symbols", market), { signal: ctl.signal, headers: token ? { Authorization: `Bearer ${token}` } : {} });
    if (res.status === 401) return { ok: false, reason: "the server needs an access token", unauthorized: true };
    if (!res.ok) return { ok: false, reason: `server answered HTTP ${res.status}` };
    const type = res.headers.get("content-type") ?? "";
    if (!type.includes("json")) return { ok: false, reason: "server did not answer with JSON (no Rust server behind /api?)" };
    return { ok: true, data: (await res.json()) as ServerSymbols };
  } catch (e) {
    return { ok: false, reason: ctl.signal.aborted ? "server did not answer in time" : `server unreachable (${e instanceof Error ? e.message : String(e)})` };
  } finally {
    clearTimeout(timer);
  }
}

/** Pure decision, so it is unit-tested without a network. */
export function chooseSource(env: string | undefined, probe: Probe | null, fixtureSymbol: string, market: MarketSource = "replay"): SourceChoice {
  const fixture = (reason: string): SourceChoice => ({ kind: "fixture", reason, symbols: [fixtureSymbol], fixtureData: true, mode: "replay", label: "In-browser fixture", market });
  if (env === "fixture") return fixture("VITE_CHART_SOURCE=fixture");
  if (!probe) return fixture("server not checked");
  if (!probe.ok) return fixture(probe.reason);
  const d = probe.data;
  const symbols = Array.isArray(d.symbols) ? d.symbols : [];
  // An empty live market is the market, not a missing server: before a session's first trade (and
  // all weekend) nothing has traded. The live page shows that, never made-up fixture prices.
  if (symbols.length === 0 && market !== "live") return fixture(`server has no data (${d.status}${d.mode ? `, ${d.mode} mode` : ""})`);
  return {
    kind: "server",
    reason: d.source ?? "server",
    symbols,
    fixtureData: !!d.fixture,
    mode: d.mode,
    label: serverLabel(d),
    market,
  };
}

/** The label for a server source: "Server replay · 2026-10-15". */
const serverLabel = (d: ServerSymbols) => `Server ${d.mode}${d.day ? ` · ${d.day}` : ""}`;

/**
 * The startup choice brought up to date with a later GET /api/symbols. The replay day can change
 * under a running page (replay.control `day`, from this client or another), and with it the
 * source label, the day and whether the data is fixture data; the source chip and the tape note
 * must say what is playing now, not what played when the page loaded. The in-browser fixture
 * never changes, and a failed refresh (no `latest`) keeps the last known description.
 */
export function followServer(choice: SourceChoice, latest: ServerSymbols | undefined): SourceChoice {
  if (choice.kind !== "server" || !latest) return choice;
  return { ...choice, reason: latest.source ?? choice.reason, fixtureData: !!latest.fixture, mode: latest.mode, label: serverLabel(latest) };
}

/** The symbol to show first: the fixture's symbol if the server has it, else the first one. */
export function initialSymbol(choice: SourceChoice, preferred: string): string {
  return choice.symbols.includes(preferred) ? preferred : (choice.symbols[0] ?? preferred);
}

/**
 * The symbol to keep showing after the candidate list changed. The server lets through only
 * stocks inside its price band, so a stock that trades out of the band disappears from the list;
 * the view then moves to the first remaining candidate rather than showing a stale chart.
 * Returns the current symbol when it is still a candidate (or no candidates are known yet).
 */
export function followCandidates(current: string, candidates: readonly string[]): string {
  if (candidates.length === 0 || candidates.includes(current)) return current;
  return candidates[0]!;
}
