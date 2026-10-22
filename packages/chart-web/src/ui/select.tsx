import { useId, type ReactNode } from "react"
import { Select as SelectPrimitive } from "@base-ui/react/select"
import { cva, type VariantProps } from "class-variance-authority"
import { ScrollArea } from "./scroll-area.tsx"
import { cn } from "./utils.ts"
import { Icon } from "../Icon.tsx"

// shadcn/ui select, Base UI variant ("base-nova"), added 2026-10-22 when the
// owner asked for every browser-native control (the Detection Panel's four
// <select>s, the alert form's) to become shadcn. Base UI's Select is a
// listbox: arrow keys, Home/End, typeahead, Enter/Space to pick, Escape to
// close, and a hidden input for forms. Changes from the generated file:
// - No border (owner rule): the trigger is a filled control on bg-bg-row, the
//   popup bg-card with a shadow, the same surfaces as the dropdown menu, and
//   a highlighted row is bg-bg-row (this app maps shadcn's accent to
//   --bg-row, so the stock focus:bg-accent would vanish on the popup).
// - The selected mark is chart-core's duotone check in text-primary (it is
//   the active state, the one place the select spends accent); the trigger's
//   chevron is the shared duotone chevronDown, not lucide.
// - It opens below the trigger (alignItemWithTrigger off) rather than over
//   it, like the menus; the list scrolls with the shadcn ScrollArea.
// - Sizes cut to two: default (forms) and sm (dense panel filters). Both grow
//   to 44 px on a coarse pointer, like every menu row (owner, 2026-10-19).
// - tw-animate-css classes became Base UI's starting/ending-style
//   transitions; reduced motion is handled globally in the app's index.css.
// - Scroll arrows, separator and group parts were dropped (nothing uses them).
//
// OptionSelect at the bottom is the one-liner the app uses: options as data,
// so a labelled select is one element and the label is wired by Base UI.

const selectTriggerVariants = cva(
  "group/select-trigger relative flex min-w-0 cursor-default items-center justify-between gap-1.5 rounded-lg bg-bg-row text-left text-foreground outline-none select-none transition-colors hover:bg-bg-raised focus-visible:ring-2 focus-visible:ring-ring data-disabled:pointer-events-none data-disabled:opacity-50 data-placeholder:text-text-faint data-popup-open:bg-bg-raised [&_svg]:pointer-events-none [&_svg]:shrink-0",
  {
    variants: {
      size: {
        default: "h-[34px] px-2.5 text-[13px] pointer-coarse:h-11 [&_svg]:size-3.5",
        sm: "min-h-8 px-2 text-[11px] pointer-coarse:min-h-11 [&_svg]:size-3",
      },
    },
    defaultVariants: { size: "default" },
  }
)

function Select<Value>({ ...props }: SelectPrimitive.Root.Props<Value>) {
  return <SelectPrimitive.Root data-slot="select" {...props} />
}

function SelectLabel({ className, ...props }: SelectPrimitive.Label.Props) {
  return <SelectPrimitive.Label data-slot="select-label" className={className} {...props} />
}

function SelectTrigger({
  className,
  size,
  children,
  ...props
}: SelectPrimitive.Trigger.Props & VariantProps<typeof selectTriggerVariants>) {
  return (
    <SelectPrimitive.Trigger
      data-slot="select-trigger"
      className={cn(selectTriggerVariants({ size }), className)}
      {...props}
    >
      {children}
      <SelectPrimitive.Icon className="flex text-muted-foreground transition-transform duration-150 group-data-popup-open/select-trigger:rotate-180">
        <Icon name="chevronDown" />
      </SelectPrimitive.Icon>
    </SelectPrimitive.Trigger>
  )
}

function SelectValue({ className, ...props }: SelectPrimitive.Value.Props) {
  return <SelectPrimitive.Value data-slot="select-value" className={cn("min-w-0 flex-1 truncate", className)} {...props} />
}

function SelectContent({
  className,
  children,
  side = "bottom",
  sideOffset = 4,
  align = "start",
  ...props
}: SelectPrimitive.Popup.Props &
  Pick<SelectPrimitive.Positioner.Props, "align" | "side" | "sideOffset">) {
  return (
    <SelectPrimitive.Portal>
      <SelectPrimitive.Positioner
        className="isolate z-50 outline-none"
        side={side}
        sideOffset={sideOffset}
        align={align}
        alignItemWithTrigger={false}
        collisionPadding={8}
      >
        <SelectPrimitive.Popup
          data-slot="select-content"
          className={cn("z-50 flex max-h-(--available-height) min-w-(--anchor-width) max-w-[min(24rem,calc(100vw-1rem))] origin-(--transform-origin) flex-col overflow-hidden rounded-lg bg-card p-1 text-card-foreground shadow-lg outline-none transition-[opacity,scale] duration-100 data-ending-style:scale-95 data-ending-style:opacity-0 data-starting-style:scale-95 data-starting-style:opacity-0", className)}
          {...props}
        >
          <ScrollArea className="flex min-h-0 flex-col" viewportClassName="max-h-[calc(var(--available-height)-8px)]">
            <SelectPrimitive.List className="flex flex-col outline-none">{children}</SelectPrimitive.List>
          </ScrollArea>
        </SelectPrimitive.Popup>
      </SelectPrimitive.Positioner>
    </SelectPrimitive.Portal>
  )
}

function SelectItem({ className, children, ...props }: SelectPrimitive.Item.Props) {
  return (
    <SelectPrimitive.Item
      data-slot="select-item"
      className={cn(
        "relative flex cursor-default items-center gap-1.5 rounded-md py-1 pr-8 pl-1.5 text-sm outline-hidden select-none pointer-coarse:min-h-11 data-highlighted:bg-bg-row data-highlighted:text-foreground data-disabled:pointer-events-none data-disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
        className
      )}
      {...props}
    >
      <SelectPrimitive.ItemText className="min-w-0 truncate">{children}</SelectPrimitive.ItemText>
      <span className="pointer-events-none absolute right-2 flex items-center justify-center text-primary">
        <SelectPrimitive.ItemIndicator>
          <Icon name="check" />
        </SelectPrimitive.ItemIndicator>
      </span>
    </SelectPrimitive.Item>
  )
}

interface SelectOption<T> {
  value: T
  label: string
}

interface OptionSelectProps<T> {
  options: readonly SelectOption<T>[]
  value: T
  onValueChange: (value: T) => void
  /** A visible label, wired to the trigger by Base UI (click focuses it). */
  label?: ReactNode
  /** Classes for the label + trigger wrapper; only used with `label`. */
  labelClassName?: string
  /** The accessible name when there is no visible label. */
  "aria-label"?: string
  size?: VariantProps<typeof selectTriggerVariants>["size"]
  disabled?: boolean
  className?: string
  contentClassName?: string
}

/**
 * One value out of a short list, options as data:
 * `<OptionSelect aria-label="Session" options={[{ value: "all", label: "All" }]} value={v} onValueChange={setV} />`.
 * `items` lets the trigger show the picked option's label (SSR included) and
 * typeahead match on labels.
 */
function OptionSelect<T>({ options, value, onValueChange, label, labelClassName, "aria-label": ariaLabel, size, disabled, className, contentClassName }: OptionSelectProps<T>) {
  // Base UI names the label `${rootId}-label` and wires it after mount; passing the root id and
  // aria-labelledby up front makes the name present on the first render too.
  const id = useId()
  const trigger = (
    <SelectTrigger size={size} aria-label={label ? undefined : ariaLabel} aria-labelledby={label ? `${id}-label` : undefined} className={className}>
      <SelectValue />
    </SelectTrigger>
  )
  return (
    <Select<T>
      id={id}
      items={options}
      value={value}
      onValueChange={(v) => {
        if (v !== null) onValueChange(v as T)
      }}
      disabled={disabled}
    >
      {label ? (
        <div className={labelClassName}>
          <SelectLabel>{label}</SelectLabel>
          {trigger}
        </div>
      ) : (
        trigger
      )}
      <SelectContent className={contentClassName}>
        {options.map((o) => (
          <SelectItem key={String(o.value)} value={o.value}>
            {o.label}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  )
}

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
}
