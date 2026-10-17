// Keeps the server's candidate list current. The Rust server screens every stock against its
// price band ($0.10–$25 by default) at the replay/live clock, so candidates come and go as
// prices move; the symbol picker must follow, not show the list from page load.
import { useEffect, useState } from "react";
import { probeServer, type ServerSymbols, type SourceChoice } from "./source.ts";

export const CANDIDATE_REFRESH_MS = 10_000;

export interface Candidates {
  symbols: string[];
  band: ServerSymbols["priceBand"];
  screened: ServerSymbols["screened"];
}

export function useCandidates(choice: SourceChoice): Candidates {
  const [state, setState] = useState<Candidates>({ symbols: choice.symbols, band: undefined, screened: undefined });
  useEffect(() => {
    if (choice.kind !== "server") return;
    let alive = true;
    const refresh = async () => {
      const probe = await probeServer();
      if (!alive || !probe.ok) return;
      const d = probe.data;
      setState({ symbols: Array.isArray(d.symbols) ? d.symbols : [], band: d.priceBand, screened: d.screened });
    };
    void refresh();
    const id = setInterval(() => void refresh(), CANDIDATE_REFRESH_MS);
    return () => {
      alive = false;
      clearInterval(id);
    };
  }, [choice.kind]);
  return state;
}
