import { Button as ButtonPrimitive } from "@base-ui/react/button"
import { cva, type VariantProps } from "class-variance-authority"
import { cn, TOUCH_HIT } from "./utils.ts"

// Touch (owner, 2026-10-19: the web app on phones and tablets): on a coarse pointer every size
// grows to 40 px and TOUCH_HIT pads the tap area to 44, so a toolbar of 28 px icon chips stays
// usable by finger. Consumers that set their own size (size-7, h-7) keep it on a mouse, since
// their classes carry no pointer-coarse: variant and the coarse rules sit later in the CSS.
// `relative` hosts the hit area; a consumer's `absolute` still wins (tailwind-merge keeps the last).
const buttonVariants = cva(
  ["relative group/button inline-flex shrink-0 items-center justify-center rounded-lg bg-clip-padding text-sm font-medium whitespace-nowrap transition-all outline-none select-none focus-visible:ring-3 focus-visible:ring-ring/50 active:not-aria-[haspopup]:translate-y-px disabled:pointer-events-none disabled:opacity-50 aria-invalid:ring-3 aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4", TOUCH_HIT],
  {
    variants: {
      // Cut down to what the chart controls use. default spends the accent
      // (primary actions: play/pause, Trade); secondary is the neutral one;
      // ghost is a toolbar button.
      variant: {
        default: "bg-primary text-primary-foreground hover:bg-primary/80",
        secondary:
          "bg-secondary text-secondary-foreground hover:bg-[color-mix(in_oklch,var(--secondary),var(--foreground)_5%)] aria-expanded:bg-secondary aria-expanded:text-secondary-foreground",
        // Toolbar buttons: no surface until hovered or open; open, the icon
        // turns accent (an active item's icon shows its accent duotone).
        ghost: "bg-transparent hover:bg-bg-row hover:text-foreground aria-expanded:bg-bg-row aria-expanded:text-foreground aria-expanded:[&_svg]:text-primary",
      },
      size: {
        default:
          "h-8 gap-1.5 px-2.5 has-data-[icon=inline-end]:pr-2 has-data-[icon=inline-start]:pl-2 pointer-coarse:h-10",
        sm: "h-7 gap-1 px-2 text-xs pointer-coarse:h-10 pointer-coarse:min-w-10",
        icon: "size-8 pointer-coarse:size-10",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  }
)

function Button({
  className,
  variant = "default",
  size = "default",
  ...props
}: ButtonPrimitive.Props & VariantProps<typeof buttonVariants>) {
  return (
    <ButtonPrimitive
      data-slot="button"
      className={cn(buttonVariants({ variant, size, className }))}
      {...props}
    />
  )
}

export { Button, buttonVariants }
