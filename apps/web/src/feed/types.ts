import type { ChartMessage, ReplayControl, Timeframe } from "@stockwatcher/chart-core";

export interface ChartSubscription {
  symbol: string;
  timeframe: Timeframe;
  /** Profile bin width in dollars; changing it re-subscribes (the server re-bins). */
  profileBin: number;
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
  close(): void;
}
