import { Slider as SliderPrimitive } from "@base-ui/react/slider"
import type { CSSProperties } from "react"
import { cn } from "./utils.ts"

// shadcn/ui slider, Base UI variant, cut down to what the indicator colour picker's hue strip
// needs (2026-11-07): one thumb, a track the caller paints (`trackStyle`, e.g. a gradient), no
// filled range. Base UI gives the keyboard (arrows step, Page Up/Down and Shift+arrows take
// `largeStep`, Home/End), pointer dragging anywhere on the control and the slider semantics; no
// browser range input is rendered by our code (owner rule: no browser-native controls).
// The control is 44 px tall so the strip is a full touch target on a phone, and inset by the
// thumb's radius so the thumb never leaves the strip at 0 or 359; the thumb is a fill
// with a two-tone ring of box-shadows, never a border (owner rule).

function Slider({
  className,
  trackStyle,
  thumbStyle,
  "aria-label": ariaLabel,
  getAriaValueText,
  ...props
}: SliderPrimitive.Root.Props<number> & {
  trackStyle?: CSSProperties
  thumbStyle?: CSSProperties
  "aria-label"?: string
  getAriaValueText?: SliderPrimitive.Thumb.Props["getAriaValueText"]
}) {
  return (
    <SliderPrimitive.Root data-slot="slider" className={cn("relative w-full touch-none select-none", className)} {...props}>
      <SliderPrimitive.Control className="flex h-11 w-full items-center px-3.5">
        <SliderPrimitive.Track className="relative h-6 w-full rounded-full" style={trackStyle}>
          <SliderPrimitive.Thumb
            aria-label={ariaLabel}
            getAriaValueText={getAriaValueText}
            className="size-7 rounded-full shadow-[0_0_0_2px_var(--color-card),0_0_0_4px_var(--color-foreground)] outline-none transition-[box-shadow] duration-100 has-[:focus-visible]:shadow-[0_0_0_2px_var(--color-card),0_0_0_5px_var(--color-ring)]"
            style={thumbStyle}
          />
        </SliderPrimitive.Track>
      </SliderPrimitive.Control>
    </SliderPrimitive.Root>
  )
}

export { Slider }
