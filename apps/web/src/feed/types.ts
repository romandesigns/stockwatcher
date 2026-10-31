import type { ChartMessage, IndicatorDemand, OrderCancel, ServerIndicatorsChannel, OrderModify, OrderPlace, PaperMessage, PaperState, ReplayControl, Timeframe } from "@stockwatcher/chart-core";
import type { DetectionChannel, FavoritesChannel, MoversChannel, PressureChannel, QuoteChannel, RocketChannel, RossChannel } from "@stockwatcher/shared";

export interface ChartSubscription {
  symbol: string;
  timeframe: Timeframe;
  /** Profile bin width in dollars; changing it re-subscribes (the server re-bins). */
  profileBin: number;
  /**
   * The chart slot, for several charts on one connection (the dashboard's panel; at most
   * MAX_CHARTS_PER_CONNECTION). Absent = the single default slot (the chart page).
   */
  id?: string;
  /**
   * What the chart wants the server to compute (`indicators.v1`; chart-core `indicatorDemand`).
   * Sent on `chart.subscribe` only when the server advertised it; ignored by the fixture feed.
   */
  indicators?: IndicatorDemand;
}

export type ReplayCommand = Omit<ReplayControl, "type">;

/**
 * Where chart data comes from. The fixture feed and the server feed both
 * implement it, so nothing above this interface knows which one is active.
 */
export interface ChartFeed {
  readonly kind: "fixture" | "server";
  /** Sends a snapshot first, then updates, until the returned function is called. */
  subscribe(sub: ChartSubscription, emit: (m: ChartMessage) => void): () => void;
  control(cmd: ReplayCommand): void;
  /** The simulated paper broker (server feed only; the fixture has none, so trading stays off). */
  readonly paper?: PaperChannel;
  /** The server's live quote board for the ticker search (server feed only; the fixture has
   *  none, so its rows show no quote). Subscribing opts the connection in. */
  readonly quotes?: QuoteChannel;
  /** The server's Halt watcher board (server feed only); subscribing opts the connection in. */
  readonly pressure?: PressureChannel;
  /** The server's Ross Panel (server feed only); subscribing opts the connection in. */
  readonly ross?: RossChannel;
  /** The Favorites panel's rows (server feed only); subscribing opts the connection in. */
  readonly favorites?: FavoritesChannel;
  /** Top Gainers and Hot stocks Bull / Bear, ranked on the server (server feed only); subscribing opts the connection in. */
  readonly movers?: MoversChannel;
  /** The server's Rocket watch (experimental; server feed only); subscribing opts the connection in. */
  readonly rocket?: RocketChannel;
  /** Independent Rust event discovery; subscribing never changes its monitored universe. */
  readonly detection?: DetectionChannel;
  /** Server-computed chart indicators (server feed only; the fixture computes none). */
  readonly indicators?: ServerIndicatorsChannel;
  close(): void;
}

export type PaperCommand = OrderPlace | OrderModify | OrderCancel | { type: "paper.reset" };

/** docs/architecture/chart-trading-sim.md: state is broadcast whole; fills are events. */
export interface PaperChannel {
  /** Latest paper.state (it arrives right after welcome, before anyone may listen). */
  latest(): PaperState | null;
  subscribe(fn: (m: PaperMessage) => void): () => void;
  send(cmd: PaperCommand): void;
}
