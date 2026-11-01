import type { ChartMessage, IndicatorDemand, OrderCancel, ServerIndicatorsChannel, OrderModify, OrderPlace, PaperMessage, PaperState, ReplayControl, Timeframe } from "@stockwatcher/chart-core";
import type { DetectionChannel, FavoritesChannel, MoversChannel, PressureChannel, QuoteChannel, QuoteSearchChannel, RocketChannel, RossChannel, NoticesChannel, SymbolsChannel, TickerChannel } from "@stockwatcher/shared";

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
   * Sent on `chart.subscribe` only when the server advertised it.
   */
  indicators?: IndicatorDemand;
}

export type ReplayCommand = Omit<ReplayControl, "type">;

/**
 * Where chart data comes from: the server feed (feed/serverFeed.ts), the only implementation since
 * the in-browser fixture was deleted; tests implement it with fakes. The optional channels stay
 * optional because a fake need not provide them.
 */
export interface ChartFeed {
  readonly kind: "server";
  /** Sends a snapshot first, then updates, until the returned function is called. */
  subscribe(sub: ChartSubscription, emit: (m: ChartMessage) => void): () => void;
  control(cmd: ReplayCommand): void;
  /** The simulated paper broker; absent (a test fake), trading stays off. */
  readonly paper?: PaperChannel;
  /** The server's live quote board for the ticker search; absent, its rows show no quote.
   *  Subscribing opts the connection in. */
  readonly quotes?: QuoteChannel;
  /** Searches ranked on the server (`quotes.search.v1`; server feed only). While `meta().available`
   *  the ticker search uses it instead of opening the whole `quotes` board. */
  readonly quoteSearch?: QuoteSearchChannel;
  /** The server's Halt watcher board (server feed only); subscribing opts the connection in. */
  readonly pressure?: PressureChannel;
  /** The server's Ross Panel (server feed only); subscribing opts the connection in. */
  readonly ross?: RossChannel;
  /** The Favorites panel's rows (server feed only); subscribing opts the connection in. */
  readonly favorites?: FavoritesChannel;
  /** Top Gainers and Hot stocks Bull / Bear, ranked on the server (server feed only); subscribing opts the connection in. */
  readonly movers?: MoversChannel;
  /** The price-band candidate list, pushed (server feed with `symbols.v1` only; see useCandidates). */
  readonly symbols?: SymbolsChannel;
  /** Today's notification log, pushed (server feed with `notices.v1` only; see useNoticeLog). */
  readonly notices?: NoticesChannel;
  /** The Insights ticker page's package (server feed with `insights.ticker.v1` only). */
  readonly ticker?: TickerChannel;
  /** The server's Rocket watch (experimental; server feed only); subscribing opts the connection in. */
  readonly rocket?: RocketChannel;
  /** Independent Rust event discovery; subscribing never changes its monitored universe. */
  readonly detection?: DetectionChannel;
  /** Server-computed chart indicators (`indicators.v1`). */
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
