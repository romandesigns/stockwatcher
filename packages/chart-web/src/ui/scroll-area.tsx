import { ScrollArea as ScrollAreaPrimitive } from "@base-ui/react/scroll-area"
import type { Ref } from "react"
import { cn } from "./utils.ts"

// shadcn/ui scroll-area, Base UI variant. A slim thumb in a raised surface
// tone that shows while scrolling or hovering, no track and no border
// (owner rule: boundaries by contrast). `viewportRef` reaches the element
// that actually scrolls (for scrollIntoView and keyboard lists), and
// `viewportClassName` sizes it (max-h-… for a list that grows to a cap).

function ScrollArea({
  className,
  viewportClassName,
  viewportRef,
  children,
  ...props
}: ScrollAreaPrimitive.Root.Props & { viewportClassName?: string; viewportRef?: Ref<HTMLDivElement> }) {
  return (
    <ScrollAreaPrimitive.Root data-slot="scroll-area" className={cn("relative min-h-0", className)} {...props}>
      <ScrollAreaPrimitive.Viewport
        ref={viewportRef}
        data-slot="scroll-area-viewport"
        className={cn("size-full overscroll-contain rounded-[inherit] outline-none focus-visible:ring-2 focus-visible:ring-ring/50", viewportClassName)}
      >
        {children}
      </ScrollAreaPrimitive.Viewport>
      <ScrollBar />
      <ScrollAreaPrimitive.Corner />
    </ScrollAreaPrimitive.Root>
  )
}

function ScrollBar({ className, orientation = "vertical", ...props }: ScrollAreaPrimitive.Scrollbar.Props) {
  return (
    <ScrollAreaPrimitive.Scrollbar
      data-slot="scroll-area-scrollbar"
      orientation={orientation}
      className={cn(
        "flex touch-none p-0.5 opacity-0 transition-opacity duration-150 select-none data-hovering:opacity-100 data-scrolling:opacity-100",
        orientation === "vertical" ? "h-full w-2" : "h-2 flex-col",
        className
      )}
      {...props}
    >
      <ScrollAreaPrimitive.Thumb data-slot="scroll-area-thumb" className="relative flex-1 rounded-full bg-bg-raised hover:bg-muted-foreground/40" />
    </ScrollAreaPrimitive.Scrollbar>
  )
}

export { ScrollArea, ScrollBar }
