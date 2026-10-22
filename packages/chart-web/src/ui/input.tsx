import { Input as InputPrimitive } from "@base-ui/react/input"
import { cn } from "./utils.ts"

// shadcn/ui input, Base UI variant ("base-nova"), added 2026-10-22 so every
// text field in the app is the shadcn one rather than a bare <input>. Changes
// from the generated file: no border or ring at rest (owner rule: a field is
// a filled surface, bg-bg-row, and focus is the focus-visible ring), no
// aria-invalid red edge (callers say what is wrong in text), and 16 px text
// on a coarse pointer, because iOS zooms the page into any focused field set
// smaller. Callers size it (h-8, h-[34px], font-mono): their classes merge
// last, so a field on a bg-bg-row dialog passes bg-background, and so on.

function Input({ className, type, ...props }: InputPrimitive.Props) {
  return (
    <InputPrimitive
      type={type}
      data-slot="input"
      className={cn(
        "h-8 w-full min-w-0 rounded-lg bg-bg-row px-2.5 text-sm text-foreground outline-none transition-colors placeholder:text-text-faint focus-visible:ring-2 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 pointer-coarse:text-base",
        className
      )}
      {...props}
    />
  )
}

export { Input }
