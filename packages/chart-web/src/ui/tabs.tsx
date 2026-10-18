import { Tabs as TabsPrimitive } from "@base-ui/react/tabs"
import { cn } from "./utils.ts"

// shadcn/ui tabs, Base UI variant, cut down to what the Settings dialog
// uses: a root that lays out by orientation, the list, the tab, a sliding
// indicator and the panel. Base UI supplies the ARIA (tablist / tab /
// tabpanel, aria-selected, aria-controls), roving focus with the arrow
// keys for the orientation (↑/↓ in a vertical list), Home / End, and the
// panel's data-starting-style / data-ending-style for its transition.
// No borders (owner rule): the active tab is a raised surface, drawn by the
// indicator so it SLIDES between tabs rather than jumping.

function Tabs({ className, orientation = "horizontal", ...props }: TabsPrimitive.Root.Props) {
  return (
    <TabsPrimitive.Root
      data-slot="tabs"
      orientation={orientation}
      className={cn("flex gap-2 data-[orientation=horizontal]:flex-col", className)}
      {...props}
    />
  )
}

function TabsList({ className, ...props }: TabsPrimitive.List.Props) {
  return (
    <TabsPrimitive.List
      data-slot="tabs-list"
      className={cn("relative z-0 flex gap-0.5 data-[orientation=vertical]:flex-col", className)}
      {...props}
    />
  )
}

function TabsTrigger({ className, ...props }: TabsPrimitive.Tab.Props) {
  return (
    <TabsPrimitive.Tab
      data-slot="tabs-trigger"
      className={cn(
        "inline-flex items-center gap-2 rounded-md text-[13px] font-medium whitespace-nowrap text-muted-foreground outline-none transition-colors hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset disabled:pointer-events-none disabled:opacity-50 data-active:text-primary [&_svg]:pointer-events-none [&_svg]:shrink-0",
        className
      )}
      {...props}
    />
  )
}

/** The active tab's raised surface; it follows the active tab through Base UI's --active-tab-* variables. */
function TabsIndicator({ className, ...props }: TabsPrimitive.Indicator.Props) {
  return (
    <TabsPrimitive.Indicator
      data-slot="tabs-indicator"
      className={cn(
        "absolute top-(--active-tab-top) left-(--active-tab-left) -z-10 h-(--active-tab-height) w-(--active-tab-width) rounded-md bg-bg-raised transition-[top,left,width,height] duration-200 ease-out",
        className
      )}
      {...props}
    />
  )
}

function TabsContent({ className, ...props }: TabsPrimitive.Panel.Props) {
  return <TabsPrimitive.Panel data-slot="tabs-content" className={cn("outline-none", className)} {...props} />
}

export { Tabs, TabsContent, TabsIndicator, TabsList, TabsTrigger }
