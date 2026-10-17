import { Tooltip as TooltipPrimitive } from "@base-ui/react/tooltip"
import type { ReactElement, ReactNode } from "react"
import { cn } from "./utils.ts"

// shadcn/ui tooltip, Base UI variant, cut down to what the chart uses. Same
// surface as the menus (bg-card with a shadow, no border: owner rule), a
// short delay so passing over a toolbar does not flash a row of tips, and
// Base UI's starting/ending-style transitions (reduced motion is global).
// Tip is the one-liner the chart uses everywhere a native `title` used to be.

function TooltipProvider({ delay = 350, ...props }: TooltipPrimitive.Provider.Props) {
  return <TooltipPrimitive.Provider data-slot="tooltip-provider" delay={delay} {...props} />
}

function Tooltip({ ...props }: TooltipPrimitive.Root.Props) {
  return <TooltipPrimitive.Root data-slot="tooltip" {...props} />
}

function TooltipTrigger({ ...props }: TooltipPrimitive.Trigger.Props) {
  return <TooltipPrimitive.Trigger data-slot="tooltip-trigger" {...props} />
}

function TooltipContent({
  className,
  side = "bottom",
  sideOffset = 6,
  align = "center",
  children,
  ...props
}: TooltipPrimitive.Popup.Props &
  Pick<TooltipPrimitive.Positioner.Props, "align" | "side" | "sideOffset">) {
  return (
    <TooltipPrimitive.Portal>
      <TooltipPrimitive.Positioner className="isolate z-50" side={side} sideOffset={sideOffset} align={align}>
        <TooltipPrimitive.Popup
          data-slot="tooltip-content"
          className={cn(
            "z-50 max-w-64 origin-(--transform-origin) rounded-md bg-bg-raised px-2 py-1 text-[11px] leading-snug text-foreground shadow-lg transition-[opacity,scale] duration-100 data-ending-style:scale-95 data-ending-style:opacity-0 data-starting-style:scale-95 data-starting-style:opacity-0",
            className
          )}
          {...props}
        >
          {children}
        </TooltipPrimitive.Popup>
      </TooltipPrimitive.Positioner>
    </TooltipPrimitive.Portal>
  )
}

/**
 * A tooltip around one element: `<Tip label="Undo (Ctrl+Z)"><Button …/></Tip>`.
 * The child becomes the trigger (its own props, ref and render props are
 * kept), so it works around Base UI buttons, toggles and menu triggers alike.
 */
function Tip({ label, side, children }: { label: ReactNode; side?: TooltipPrimitive.Positioner.Props["side"]; children: ReactElement }) {
  return (
    <Tooltip>
      <TooltipTrigger render={children} />
      <TooltipContent side={side}>{label}</TooltipContent>
    </Tooltip>
  )
}

export { Tip, Tooltip, TooltipContent, TooltipProvider, TooltipTrigger }
