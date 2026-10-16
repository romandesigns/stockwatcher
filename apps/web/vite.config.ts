import path from "node:path";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// Dev: Vite on 5173 proxies the API and WebSocket to the Rust server on
// 8790, so the browser sees one origin exactly as in production.
const server = process.env.STOCKWATCHER_DEV_SERVER ?? "http://127.0.0.1:8790";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: { "@": path.resolve(import.meta.dirname, "./src") },
  },
  server: {
    port: 5173,
    strictPort: true,
    proxy: {
      "/api": server,
      "/updates": server,
      "/ws": { target: server.replace(/^http/, "ws"), ws: true },
    },
  },
});
