import { Popover as PopoverPrimitive } from "@base-ui/react/popover"
import { ScrollArea } from "./scroll-area.tsx"
import { cn } from "./utils.ts"

// shadcn/ui popover, Base UI variant, cut down to what the order ticket uses:
// controlled open state and a virtual `anchor` (it opens at a price on the
// chart, not under a trigger). Same surface treatment as the menus: bg-card
// with a shadow, no border or ring (owner rule: boundaries by contrast).
// PopoverTrigger was added back (2026-10-18) for the chart settings panel,
// which opens under its toolbar button like the Indicators menu.

function Popover({ ...props }: PopoverPrimitive.Root.Props) {
  return <PopoverPrimitive.Root data-slot="popover" {...props} />
}

function PopoverTrigger({ ...props }: PopoverPrimitive.Trigger.Props) {
  return <PopoverPrimitive.Trigger data-slot="popover-trigger" {...props} />
}

function PopoverContent({
  align = "end",
  side = "left",
  sideOffset = 8,
  anchor,
  className,
  children,
  ...props
}: PopoverPrimitive.Popup.Props &
  Pick<PopoverPrimitive.Positioner.Props, "align" | "side" | "sideOffset" | "anchor">) {
  return (
    <PopoverPrimitive.Portal>
      <PopoverPrimitive.Positioner className="isolate z-50 outline-none" align={align} side={side} sideOffset={sideOffset} anchor={anchor}>
        <PopoverPrimitive.Popup
          data-slot="popover-content"
          className={cn("z-50 flex max-h-(--available-height) w-64 origin-(--transform-origin) flex-col overflow-hidden rounded-xl bg-card text-card-foreground shadow-lg outline-none transition-[opacity,scale] duration-100 data-ending-style:scale-95 data-ending-style:opacity-0 data-starting-style:scale-95 data-starting-style:opacity-0", className)}
          {...props}
        >
          {/* A tall popover (the order ticket with its history) scrolls with the shadcn scrollbar. */}
          <ScrollArea className="flex min-h-0 flex-col" viewportClassName="max-h-[var(--available-height)] p-3">
            {children}
          </ScrollArea>
        </PopoverPrimitive.Popup>
      </PopoverPrimitive.Positioner>
    </PopoverPrimitive.Portal>
  )
}

export { Popover, PopoverContent, PopoverTrigger }
