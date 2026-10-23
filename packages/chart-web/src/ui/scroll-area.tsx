import { ScrollArea as ScrollAreaPrimitive } from "@base-ui/react/scroll-area"
import type { Ref } from "react"
import { cn } from "./utils.ts"

// shadcn/ui scroll-area, Base UI variant. The ONLY scroll container in the web and desktop UI
// (owner, 2026-10-23: the browser's own scrollbars were the last native UI left). A raw
// overflow-auto/scroll anywhere else fails the guard in apps/web/src/lib/controls.test.tsx.
//
// - Base UI's Viewport is the element that scrolls: it hides the native bar itself
//   (scrollbar-width: none + ::-webkit-scrollbar), is focusable while it overflows so the
//   keyboard can scroll it, and the scrollbars are aria-hidden (the viewport is the semantics).
// - Overlay bars: a slim thumb on a transparent track, no border, laid over the content. They
//   fade in while the pointer is over the area, while it scrolls and while the viewport has
//   keyboard focus, then fade out (a slower fade out than in, so a pause in scrolling does not
//   blink). prefers-reduced-motion drops the fade: the bar just shows and hides.
// - The thumb is a 30% structural tone (--text-faint), never the accent: a scrollbar is
//   furniture, not something to look at.
// - Both axes: a horizontal bar appears only when the content is actually wider.
// - Layout: the root is a flex column and the viewport its flex-1, so the area works both as a
//   flex child that shrinks into a bounded column (`className="min-h-0 flex-1"`, the side
//   panels) and as a list that grows to a cap (`viewportClassName="max-h-…"`, menus, search):
//   the root's own height comes from the capped viewport. A child with h-full still resolves
//   when the root has a definite height (the Shell's page area).
// - `viewportRef` reaches the scrolling element (scrollIntoView, keyboard lists, a virtualiser's
//   scroll element, an auto-scroll that pins to the bottom); `viewportClassName` styles it
//   (max-h, padding).

const BAR =
  "flex touch-none p-0.5 opacity-0 select-none transition-opacity duration-300 ease-out motion-reduce:transition-none " +
  "data-hovering:opacity-100 data-hovering:duration-100 data-scrolling:opacity-100 data-scrolling:duration-100 " +
  "peer-focus-visible/viewport:opacity-100"

function ScrollArea({
  className,
  viewportClassName,
  viewportRef,
  children,
  ...props
}: ScrollAreaPrimitive.Root.Props & { viewportClassName?: string; viewportRef?: Ref<HTMLDivElement> }) {
  return (
    <ScrollAreaPrimitive.Root data-slot="scroll-area" className={cn("relative flex min-h-0 min-w-0 flex-col", className)} {...props}>
      <ScrollAreaPrimitive.Viewport
        ref={viewportRef}
        data-slot="scroll-area-viewport"
        className={cn(
          // The focus mark is an outline drawn inside the edge: it paints over the content
          // (a ring would be covered by the first row) and is never clipped by a parent.
          // outline-solid is needed: outline-none sets the style to none for every state.
          "peer/viewport min-h-0 w-full flex-1 overscroll-contain rounded-[inherit] outline-none focus-visible:outline-2 focus-visible:outline-solid focus-visible:-outline-offset-2 focus-visible:outline-ring/60",
          viewportClassName
        )}
      >
        {children}
      </ScrollAreaPrimitive.Viewport>
      <ScrollBar orientation="vertical" />
      <ScrollBar orientation="horizontal" />
      <ScrollAreaPrimitive.Corner />
    </ScrollAreaPrimitive.Root>
  )
}

function ScrollBar({ className, orientation = "vertical", ...props }: ScrollAreaPrimitive.Scrollbar.Props) {
  return (
    <ScrollAreaPrimitive.Scrollbar
      data-slot="scroll-area-scrollbar"
      orientation={orientation}
      className={cn(BAR, orientation === "vertical" ? "w-2" : "h-2 flex-col", className)}
      {...props}
    >
      <ScrollAreaPrimitive.Thumb
        data-slot="scroll-area-thumb"
        className="relative flex-1 rounded-full bg-text-faint/55 transition-colors duration-150 hover:bg-text-faint active:bg-muted-foreground/70 motion-reduce:transition-none"
      />
    </ScrollAreaPrimitive.Scrollbar>
  )
}

export { ScrollArea, ScrollBar }
