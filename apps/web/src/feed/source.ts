// Which chart source is active, decided once at startup.
//
// The server is the default (VITE_CHART_SOURCE unset or "server"). The app
// asks GET /api/symbols first; it falls back to the in-browser fixture ONLY if
// the server is unreachable, refuses, or reports no symbols, and it always
// says which source is active and why. VITE_CHART_SOURCE=fixture forces the
// in-browser fixture.

export interface ServerSymbols {
  mode: "replay" | "live" | (string & {});
  /** Provider status: running, no_data, not_configured, error, stopped, idle. */
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
}

export type Probe = { ok: true; data: ServerSymbols } | { ok: false; reason: string };

export interface SourceChoice {
  kind: "server" | "fixture";
  /** Why this source; shown in the UI. */
  reason: string;
  symbols: string[];
  /** The data itself is fixture data (in-browser fixture, or a server fixture archive). */
  fixtureData: boolean;
  mode: string;
  label: string;
}

/** GET /api/symbols with a timeout. Never throws. */
export async function probeServer(fetchImpl: typeof fetch = fetch, timeoutMs = 2500, token?: string | null): Promise<Probe> {
  const ctl = new AbortController();
  const timer = setTimeout(() => ctl.abort(), timeoutMs);
  try {
    const res = await fetchImpl("/api/symbols", { signal: ctl.signal, headers: token ? { Authorization: `Bearer ${token}` } : {} });
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
export function chooseSource(env: string | undefined, probe: Probe | null, fixtureSymbol: string): SourceChoice {
  const fixture = (reason: string): SourceChoice => ({ kind: "fixture", reason, symbols: [fixtureSymbol], fixtureData: true, mode: "replay", label: "In-browser fixture" });
  if (env === "fixture") return fixture("VITE_CHART_SOURCE=fixture");
  if (!probe) return fixture("server not checked");
  if (!probe.ok) return fixture(probe.reason);
  const d = probe.data;
  if (!Array.isArray(d.symbols) || d.symbols.length === 0) return fixture(`server has no data (${d.status}${d.mode ? `, ${d.mode} mode` : ""})`);
  return {
    kind: "server",
    reason: d.source ?? "server",
    symbols: d.symbols,
    fixtureData: !!d.fixture,
    mode: d.mode,
    label: `Server ${d.mode}${d.day ? ` · ${d.day}` : ""}`,
  };
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
