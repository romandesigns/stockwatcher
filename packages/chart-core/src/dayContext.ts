import type { SessionSchedule } from "./time.ts";
import { parseChartMessage, type ChartLevels, type ChartZones, type SessionRef } from "./types.ts";

/**
 * A finished day's chart context as the server sends it beside the bars (the Insights ticker
 * package's `chart`, crates/server/src/chart_context.rs `day_context`): raw frames, read here into
 * what CandleChart takes (`serverLevels`, `serverZones`, `series.sessions`).
 */
export interface DayChartContextWire {
  levels: unknown;
  zones: unknown;
  zonesRth: unknown;
  sessions: readonly SessionRef[];
}

export interface DayChartContext {
  /** null: none sent (an older server), so none drawn. */
  levels: ChartLevels | null;
  zones: ChartZones["zones"] | null;
  zonesRth: ChartZones["zones"] | null;
  sessions: SessionSchedule | undefined;
}

export function dayChartContext(wire: DayChartContextWire | null | undefined): DayChartContext {
  if (!wire) return { levels: null, zones: null, zonesRth: null, sessions: undefined };
  // parseChartMessage turns the exchange-ns twins into ms like any live frame (wireTime.ts).
  const levels = parseChartMessage(wire.levels);
  const zones = parseChartMessage(wire.zones);
  const zonesRth = parseChartMessage(wire.zonesRth);
  return {
    levels: levels?.type === "chart.levels" ? levels : null,
    zones: zones?.type === "chart.zones" ? zones.zones : null,
    zonesRth: zonesRth?.type === "chart.zones" ? zonesRth.zones : null,
    sessions: wire.sessions.length ? [...wire.sessions].sort((a, b) => a.start - b.start) : undefined,
  };
}
