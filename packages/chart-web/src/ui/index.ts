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
export { cn, TOUCH_HIT, TOUCH_HIT_Y } from "./utils.ts";
export {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
} from "./dropdown-menu.tsx";
export { Popover, PopoverContent, PopoverTrigger } from "./popover.tsx";
export { Tip, Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "./tooltip.tsx";
export { Dialog, DialogClose, DialogContent, DialogTitle } from "./dialog.tsx";
export { Switch } from "./switch.tsx";
export { Slider } from "./slider.tsx";
export { Tabs, TabsContent, TabsIndicator, TabsList, TabsTrigger } from "./tabs.tsx";
export { ScrollArea, ScrollBar } from "./scroll-area.tsx";
export {
  OptionSelect,
  Select,
  SelectContent,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
  selectTriggerVariants,
  type OptionSelectProps,
  type SelectOption,
} from "./select.tsx";
export { Input } from "./input.tsx";
export { Collapsible, CollapsibleContent, CollapsibleTrigger } from "./collapsible.tsx";
