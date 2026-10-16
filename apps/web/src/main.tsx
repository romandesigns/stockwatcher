import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
// Bundled into dist by Vite (self-hosted, OFL): no runtime font CDN.
import "@fontsource-variable/geist";
import "./index.css";
import App from "./App";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
