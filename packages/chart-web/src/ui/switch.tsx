import { Switch as SwitchPrimitive } from "@base-ui/react/switch"
import { cn } from "./utils.ts"

// shadcn/ui switch, Base UI variant, recoloured for the 60/30/10 rule: a
// settings list is mostly switches that are ON, and an accent track on
// every one of them would make the accent the page's dominant colour. So on
// is a light neutral track (--text) with a dark thumb, off a raised
// surface with a dim thumb: the state reads by contrast and position, and
// the accent stays on the one thing that is "selected" in the dialog, the
// active tab. No border (owner rule); the thumb slides (reduced motion is
// handled globally in index.css).

function Switch({ className, ...props }: SwitchPrimitive.Root.Props) {
  return (
    <SwitchPrimitive.Root
      data-slot="switch"
      className={cn(
        "peer relative inline-flex h-[18px] w-8 shrink-0 cursor-pointer items-center rounded-full bg-bg-raised p-0.5 outline-none transition-colors duration-150 focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-card data-checked:bg-foreground data-disabled:cursor-not-allowed data-disabled:opacity-50",
        className
      )}
      {...props}
    >
      <SwitchPrimitive.Thumb
        data-slot="switch-thumb"
        className="pointer-events-none block size-3.5 rounded-full bg-muted-foreground shadow-sm transition-[translate,background-color] duration-150 ease-out data-checked:translate-x-3.5 data-checked:bg-background"
      />
    </SwitchPrimitive.Root>
  )
}

export { Switch }
