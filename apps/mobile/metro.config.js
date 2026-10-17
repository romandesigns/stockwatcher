// Expo's default config already handles this monorepo: it watches the
// workspace root (so packages/shared hot-reloads) and follows Bun's
// isolated-install symlinks. Keep it the default unless something breaks;
// every override here is one more thing to re-check on an SDK upgrade.
const { getDefaultConfig } = require("expo/metro-config");

module.exports = getDefaultConfig(__dirname);
