// Which server market a page reads, decided once at startup.
//
// The Rust server is the only source. The app asks GET /api/symbols first; when the server is
// unreachable or refuses, the page says so ("Server unreachable", ConnectionGate) and retries
// rather than inventing data. There is no in-browser stand-in any more (owner rule: every heavy
// computation runs in Rust and is pushed; the browser fixture aggregated its own candles).
// Development runs the server's replay over its fixture day instead (`SIM_AUTOFIXTURE=1 bun dev`,
// crates/market-archive/src/fixture.rs). A server with no candidates (a replay still building,
// a live market before its first trade) is still the server: the lists fill as it pushes.

import { shownTicker } from "@stockwatcher/app-state";
import { serverUrl, withMarketSource, type MarketSource } from "@stockwatcher/shared";

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
  /** What the server is playing (its `source`); shown in the UI. */
  reason: string;
  symbols: string[];
  /** The server replays a fixture archive (never market evidence). */
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
    const res = await fetchImpl(serverUrl(withMarketSource("/api/symbols", market)), { signal: ctl.signal, headers: token ? { Authorization: `Bearer ${token}` } : {} });
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

/**
 * The symbol a page prefers before the candidates say otherwise: the first symbol of the server's
 * fixture day (crates/market-archive/src/fixture.rs), so a dev replay opens on it. Only a
 * preference: `initialSymbol` takes it only when the server lists it.
 */
export const DEFAULT_SYMBOL = "SMPL";

/** The page's source from a successful probe. Pure, so it is unit-tested without a network. */
export function serverChoice(d: ServerSymbols, market: MarketSource = "replay"): SourceChoice {
  return {
    reason: d.source ?? "server",
    symbols: Array.isArray(d.symbols) ? d.symbols : [],
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
 * must say what is playing now, not what played when the page loaded. A failed refresh (no
 * `latest`) keeps the last known description.
 */
export function followServer(choice: SourceChoice, latest: ServerSymbols | undefined): SourceChoice {
  if (!latest) return choice;
  return { ...choice, reason: latest.source ?? choice.reason, fixtureData: !!latest.fixture, mode: latest.mode, label: serverLabel(latest) };
}

/** The symbol to show first: `preferred` (DEFAULT_SYMBOL) if the server lists it, else its first candidate. */
export function initialSymbol(choice: SourceChoice, preferred: string): string {
  return choice.symbols.includes(preferred) ? preferred : (choice.symbols[0] ?? preferred);
}

/**
 * The symbol to keep showing after the candidate list changed. The server lets through only
 * stocks inside its price band, so a stock that trades out of the band disappears from the list;
 * the view then moves to the first remaining candidate rather than showing a stale chart.
 * Returns the current symbol when it is still a candidate (or no candidates are known yet), and
 * always when this device explicitly opened it (`picked`, app-state `shownTicker`: owner,
 * 2026-10-23, a picked ticker is never swapped for another one).
 */
export function followCandidates(current: string, candidates: readonly string[], picked: readonly string[] = []): string {
  return shownTicker(current, candidates, picked);
}
