// Vendored shadcn/ui components, Base UI variant ("base-nova"), added with
// the shadcn CLI and moved here from apps/web. They live in chart-web, not
// apps/web, because the chart controls need them and a package cannot import
// from an app; apps/web imports them from "@stockwatcher/chart-web/ui".
// Base UI only, never Radix. If a shared packages/ui appears, move them there.
// The classes use the app's shadcn token names (bg-primary, text-muted-
// foreground, …), so the host app must map those onto its palette and add
// this package to Tailwind's @source.
export { Button, buttonVariants } from "./button.tsx";
export { Toggle, toggleVariants } from "./toggle.tsx";
export { ToggleGroup, ToggleGroupItem } from "./toggle-group.tsx";
export { cn } from "./utils.ts";
