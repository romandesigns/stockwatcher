/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** "server" reads chart data from the Rust server's /ws; anything else uses the fixture. */
  readonly VITE_CHART_SOURCE?: string;
}
