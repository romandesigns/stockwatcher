import { ICON_SECONDARY_OPACITY } from "@stockwatcher/chart-core";
import { Platform } from "react-native";

// Design tokens, mirroring apps/web/src/index.css `:root`. Change both
// together so web, desktop and mobile read as one product.
//
// 60/30/10 (workspace rule):
//   60  bg / bgPanel / bgRow         quiet dominant neutrals
//   30  text* / border               structure and body text
//   10  accent                       primary action, active state, focus.
//       Never a background or body text, so it keeps meaning "look here".
// good/warning/critical are status colours, always paired with a word.
// Dark-first; a light theme is not built yet.

export const colors = {
  bg: "#0f1216",
  bgPanel: "#151a20",
  bgRow: "#1c222a",

  border: "#262d36",
  text: "#e4e8ed",
  textDim: "#98a2ae",
  textFaint: "#66707c",

  accent: "#8b9cff",
  accentFg: "#0f1216",

  good: "#3fb97a",
  goodBg: "rgba(63, 185, 122, 0.14)",
  warning: "#e0a63a",
  warningBg: "rgba(224, 166, 58, 0.14)",
  critical: "#f0645c",
  criticalBg: "rgba(240, 100, 92, 0.14)",
} as const;

// Duotone icons (web: --icon-primary / --icon-secondary). Every icon is
// chart-native's <Icon>, drawn from chart-core's shared definitions: primary
// is the colour passed in (the state's text colour; accent only where a state
// spends it), secondary the same colour at this opacity.
export const icon = {
  secondaryOpacity: ICON_SECONDARY_OPACITY,
} as const;

// Compact spacing (owner preference): enough room to breathe, no more.
export const space = {
  xs: 4,
  sm: 8,
  md: 12,
  lg: 16,
  xl: 24,
} as const;

export const radius = {
  sm: 4,
  md: 6,
  lg: 8,
} as const;

// Font family names as embedded by the expo-font config plugin
// (app.config.ts): Android uses the file name without extension, iOS the
// font's PostScript name.
export const fonts = {
  regular: Platform.select({ ios: "Geist-Regular", default: "Geist_400Regular" }),
  semibold: Platform.select({ ios: "Geist-SemiBold", default: "Geist_600SemiBold" }),
} as const;

export const type = {
  body: { fontFamily: fonts.regular, fontSize: 15, lineHeight: 21 },
  title: { fontFamily: fonts.semibold, fontSize: 20, lineHeight: 26 },
} as const;
