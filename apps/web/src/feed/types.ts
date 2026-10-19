import type { ChartMessage, OrderCancel, OrderModify, OrderPlace, PaperMessage, PaperState, ReplayControl, Timeframe } from "@stockwatcher/chart-core";
import type { PressureChannel, QuoteChannel } from "@stockwatcher/shared";

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
