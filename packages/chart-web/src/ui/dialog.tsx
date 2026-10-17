import { Dialog as DialogPrimitive } from "@base-ui/react/dialog"
import { cn } from "./utils.ts"

// shadcn/ui dialog, Base UI variant, cut down to what the ticker quick-open
// uses: a controlled root, a dim backdrop and a popup the caller places.
// Same surface as the menus (bg-card, shadow, no border: owner rule); Base
// UI traps focus, closes on Escape and outside press, and restores focus.

function Dialog({ ...props }: DialogPrimitive.Root.Props) {
  return <DialogPrimitive.Root data-slot="dialog" {...props} />
}

function DialogContent({ className, children, ...props }: DialogPrimitive.Popup.Props) {
  return (
    <DialogPrimitive.Portal>
      <DialogPrimitive.Backdrop className="fixed inset-0 z-50 bg-black/40 transition-opacity duration-150 data-ending-style:opacity-0 data-starting-style:opacity-0" />
      <DialogPrimitive.Popup
        data-slot="dialog-content"
        className={cn(
          "fixed z-50 rounded-xl bg-card text-card-foreground shadow-2xl outline-none transition-[opacity,scale,translate] duration-150 data-ending-style:-translate-y-1 data-ending-style:opacity-0 data-starting-style:-translate-y-1 data-starting-style:opacity-0",
          className
        )}
        {...props}
      >
        {children}
      </DialogPrimitive.Popup>
    </DialogPrimitive.Portal>
  )
}

function DialogTitle({ className, ...props }: DialogPrimitive.Title.Props) {
  return <DialogPrimitive.Title data-slot="dialog-title" className={cn("sr-only", className)} {...props} />
}

export { Dialog, DialogContent, DialogTitle }
