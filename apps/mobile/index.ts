// First: defines the "Copy ticker" background task in module scope, which a headless start (the
// button pressed on a killed app) needs before anything else (src/notifications/copyTicker.ts).
import { registerCopyTicker } from "./src/notifications/copyTicker";

import { registerRootComponent } from "expo";

import App from "./src/App";

registerCopyTicker();

// Registers the root as "main" for both development and release builds.
registerRootComponent(App);
