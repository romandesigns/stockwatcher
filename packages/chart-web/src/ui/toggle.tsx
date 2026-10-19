import { Toggle as TogglePrimitive } from "@base-ui/react/toggle"
import { cva, type VariantProps } from "class-variance-authority"
import { cn, TOUCH_HIT_Y } from "./utils.ts"

// Touch (owner, 2026-10-19): on a coarse pointer an option is 44 px wide and 40 px tall, and
// TOUCH_HIT_Y pads it to 44 vertically only, since segments sit 2 px apart and must not share a
// strip. Consumer widths (w-7, min-w-9) keep their mouse size: min-w-11 overrides them on touch.
const toggleVariants = cva(
  ["relative group/toggle inline-flex items-center justify-center gap-1 rounded-lg text-sm font-medium whitespace-nowrap transition-all outline-none hover:bg-muted hover:text-foreground focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:pointer-events-none disabled:opacity-50 aria-invalid:ring-destructive/20 aria-pressed:bg-muted data-[state=on]:bg-muted dark:aria-invalid:ring-destructive/40 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4", TOUCH_HIT_Y],
  {
    variants: {
      variant: {
        default: "bg-transparent",
        // "outline" keeps its name for API compatibility but is a filled
        // segment: the owner's rule is no borders, boundaries by contrast.
        outline: "bg-card hover:bg-bg-row",
        // One option of a segmented control: transparent on the group's
        // panel track, the selected one raised a surface step. Pair with
        // size="sm" and spacing={0.5} on the ToggleGroup. A selected
        // option's icon turns accent (both tones): the owner's rule that an
        // active item's icon shows its accent duotone (2026-10-18). The label
        // stays foreground.
        segmented: "bg-transparent text-muted-foreground hover:bg-transparent hover:text-foreground aria-pressed:bg-bg-raised aria-pressed:text-foreground aria-pressed:[&_svg]:text-primary",
      },
      size: {
        default:
          "h-8 min-w-8 px-2.5 has-data-[icon=inline-end]:pr-2 has-data-[icon=inline-start]:pl-2 pointer-coarse:h-10 pointer-coarse:min-w-11",
        sm: "h-7 min-w-7 rounded-[min(var(--radius-md),12px)] px-2.5 text-[0.8rem] has-data-[icon=inline-end]:pr-1.5 has-data-[icon=inline-start]:pl-1.5 [&_svg:not([class*='size-'])]:size-3.5 pointer-coarse:h-10 pointer-coarse:min-w-11",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  }
)

function Toggle({
  className,
  variant = "default",
  size = "default",
  ...props
}: TogglePrimitive.Props & VariantProps<typeof toggleVariants>) {
  return (
    <TogglePrimitive
      data-slot="toggle"
      className={cn(toggleVariants({ variant, size, className }))}
      {...props}
    />
  )
}

export { Toggle, toggleVariants }
