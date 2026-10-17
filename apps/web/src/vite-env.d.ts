/// <reference types="vite/client" />

interface ImportMetaEnv {
  /**
   * Chart source. Unset or "server" (default): the Rust server's /ws, falling
   * back to the in-browser fixture only when the server is unreachable or has
   * no data (feed/source.ts). "fixture": always the in-browser fixture.
   */
  readonly VITE_CHART_SOURCE?: string;
}
