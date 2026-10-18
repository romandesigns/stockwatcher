import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
// Bundled into dist by Vite (self-hosted, OFL): no runtime font CDN.
import "@fontsource-variable/geist";
import "@fontsource-variable/geist-mono";
import "./index.css";
import { TooltipProvider } from "@stockwatcher/chart-web/ui";
import { ErrorBoundary } from "./ErrorBoundary";
import { Shell } from "./Shell";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <ErrorBoundary>
      <TooltipProvider>
        <Shell />
      </TooltipProvider>
    </ErrorBoundary>
  </StrictMode>,
);
