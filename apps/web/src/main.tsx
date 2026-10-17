import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
// Bundled into dist by Vite (self-hosted, OFL): no runtime font CDN.
import "@fontsource-variable/geist";
import "@fontsource-variable/geist-mono";
import "./index.css";
import { TooltipProvider } from "@stockwatcher/chart-web/ui";
import App from "./App";
import { ErrorBoundary } from "./ErrorBoundary";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <ErrorBoundary>
      <TooltipProvider>
        <App />
      </TooltipProvider>
    </ErrorBoundary>
  </StrictMode>,
);
