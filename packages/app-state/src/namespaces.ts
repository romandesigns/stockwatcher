import { parseIndicators, parseLayers, TIMEFRAMES, TRADE_SIZES, type ChartLayers, type IndicatorSpec, type Timeframe } from "@stockwatcher/chart-core";
import { MAX_TICKER_RECENTS } from "@stockwatcher/shared";

// The app-level state that syncs across devices on top of chart-core's SYNC_NS (owner,
// 2026-10-18: "sync everything possible"; docs/architecture/app-state.md). Same rules as the
// chart namespaces: the server stores them opaquely, so every value read back, local or remote,
// goes through a parser here and junk from another build falls back to defaults.
//
// - `view`          what the chart shows: ticker, timeframe, profile bin. Changing the ticker
//                   on the phone moves the desktop to it.
// - `tradeQty`      the quick trade menus' share size (TRADE_SIZES).
// - `searchRecents` recently opened tickers, newest first.
// - `dashboard`     the main page's multi-chart panel: how many charts (1–4) and each slot's ticker
//                   and timeframe (owner, 2026-10-18). Which chart is selected stays per device.

export const APP_NS = {
  view: "view",
  tradeQty: "tradeQty",
  searchRecents: "searchRecents",
  dashboard: "dashboard",
} as const;

/** The session volume profile's bin widths, the same on every client. */
export const PROFILE_BINS = [0.01, 0.02, 0.05] as const;

export interface ViewState {
  /** The ticker last opened on any device; null until one is (each client then shows its own first candidate). */
  symbol: string | null;
  timeframe: Timeframe;
  bin: number;
}

export const DEFAULT_VIEW: ViewState = { symbol: null, timeframe: "1m", bin: PROFILE_BINS[0] };

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

/** A ticker as the wire spells it (upper case; letters, digits, `.` and `-`, e.g. BRK.B), else null. */
export function parseSymbol(raw: unknown): string | null {
  if (typeof raw !== "string") return null;
  const s = raw.trim().toUpperCase();
  return /^[A-Z][A-Z0-9.-]{0,11}$/.test(s) ? s : null;
}

export function parseView(raw: unknown): ViewState {
  if (!isObj(raw)) return DEFAULT_VIEW;
  return {
    symbol: parseSymbol(raw.symbol),
    timeframe: (TIMEFRAMES as readonly unknown[]).includes(raw.timeframe) ? (raw.timeframe as Timeframe) : DEFAULT_VIEW.timeframe,
    bin: (PROFILE_BINS as readonly unknown[]).includes(raw.bin) ? (raw.bin as number) : DEFAULT_VIEW.bin,
  };
}

export function parseTradeQty(raw: unknown): number {
  return (TRADE_SIZES as readonly unknown[]).includes(raw) ? (raw as number) : TRADE_SIZES[0];
}

/** The most charts the dashboard panel holds: the server's limit per connection (MAX_CHARTS_PER_CONNECTION). */
export const MAX_DASHBOARD_CHARTS = 4;

export interface DashboardSlot {
  /** The ticker loaded into this chart; null until one is (the device then shows a candidate). */
  symbol: string | null;
  timeframe: Timeframe;
  /**
   * This chart's own indicators and levels, used only while Settings → Chart → "Same indicators
   * and levels on every chart" is off; null = it follows the shared set (and starts from it when
   * first changed). Kept while linked, so unlinking again brings each chart's own set back.
   */
  indicators: IndicatorSpec[] | null;
  layers: ChartLayers | null;
}

/**
 * Which of the server's two markets the dashboard reads (owner, 2026-10-18, Settings → Data): the
 * real live market (Alpaca: SIP by day, Blue Ocean overnight) or the replay market (recorded days,
 * the practice feature the chart page always uses). Synced: every device's dashboard follows it.
 */
export type DashboardSource = "live" | "replay";

export interface DashboardState {
  /** Charts shown, 1–MAX_DASHBOARD_CHARTS. Slots past it keep their ticker for when it grows again. */
  count: number;
  /** Always MAX_DASHBOARD_CHARTS slots. */
  slots: DashboardSlot[];
  source: DashboardSource;
}

const EMPTY_SLOT: DashboardSlot = { symbol: null, timeframe: "1m", indicators: null, layers: null };

export const DEFAULT_DASHBOARD: DashboardState = { count: 1, slots: Array.from({ length: MAX_DASHBOARD_CHARTS }, () => EMPTY_SLOT), source: "live" };

export function parseDashboard(raw: unknown): DashboardState {
  if (!isObj(raw)) return DEFAULT_DASHBOARD;
  const count = typeof raw.count === "number" && Number.isInteger(raw.count) && raw.count >= 1 && raw.count <= MAX_DASHBOARD_CHARTS ? raw.count : DEFAULT_DASHBOARD.count;
  const list = Array.isArray(raw.slots) ? raw.slots : [];
  const slots = Array.from({ length: MAX_DASHBOARD_CHARTS }, (_, i): DashboardSlot => {
    const s = list[i];
    if (!isObj(s)) return EMPTY_SLOT;
    return {
      symbol: parseSymbol(s.symbol),
      timeframe: (TIMEFRAMES as readonly unknown[]).includes(s.timeframe) ? (s.timeframe as Timeframe) : EMPTY_SLOT.timeframe,
      indicators: Array.isArray(s.indicators) ? parseIndicators(s.indicators) : null,
      layers: isObj(s.layers) ? parseLayers(s.layers) : null,
    };
  });
  // Anything but "replay" is live: the dashboard is the live page unless asked otherwise.
  const source: DashboardSource = raw.source === "replay" ? "replay" : "live";
  return { count, slots, source };
}

/** Valid, de-duplicated tickers, newest first, capped like pushRecentTicker. */
export function parseSearchRecents(raw: unknown): string[] {
  if (!Array.isArray(raw)) return [];
  const out: string[] = [];
  for (const r of raw) {
    const s = parseSymbol(r);
    if (s && !out.includes(s)) out.push(s);
    if (out.length >= MAX_TICKER_RECENTS) break;
  }
  return out;
}
