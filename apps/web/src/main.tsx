import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
// Bundled into dist by Vite (self-hosted, OFL): no runtime font CDN.
import "@fontsource-variable/geist";
import "@fontsource-variable/geist-mono";
import "./index.css";
import { TooltipProvider } from "@stockwatcher/chart-web/ui";
import { ErrorBoundary } from "./ErrorBoundary";
import { Shell } from "./Shell";
import { installOpenChartHook } from "./notify/runnersNotify";
import { setServerOrigin } from "@stockwatcher/shared";

// The desktop shell's notification click opens a chart through this hook: present from the first
// paint, not only once a connection created the notifier (a click before that did nothing).
installOpenChartHook();

// The installed desktop app serves this page from its own origin (http://tauri.localhost; tauri://
// on macOS/Linux), not from the server, so it names the server (shared/http.ts `serverUrl`). `tauri
// dev` loads Vite on localhost instead, whose proxy reaches the local server: left same-origin.
if (window.location.hostname === "tauri.localhost" || window.location.protocol === "tauri:") {
  setServerOrigin("https://stockwatcher.wavystyle.io");
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <ErrorBoundary>
      <TooltipProvider>
        <Shell />
      </TooltipProvider>
    </ErrorBoundary>
  </StrictMode>,
);
