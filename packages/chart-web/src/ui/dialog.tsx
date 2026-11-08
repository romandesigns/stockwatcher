import { Dialog as DialogPrimitive } from "@base-ui/react/dialog"
import { cn } from "./utils.ts"

// shadcn/ui dialog, Base UI variant. Same surface as the menus (bg-card, shadow, no border: owner
// rule); Base UI traps focus, closes on Escape and outside press, and restores focus.
//
// Every modal is CENTRED in the viewport, horizontally and vertically, at every size (owner,
// 2026-11-07: "modals/dialogs should always appear in the center of the screen"), never anchored
// to a trigger, a corner or the top. The centring is `inset-0 m-auto` with a fit-content height,
// not a translate: the open/close animation below animates `translate`, and a centring translate
// would fight it. A caller sets only the size (`w-…`/`max-w-…`, a fixed `h-…` for a dialog whose
// content changes height, so it does not jump). Tall content scrolls INSIDE the dialog: the popup
// never grows past the viewport (minus a 1 rem gutter and the safe-area insets), so the caller
// gives its body a ScrollArea with `min-h-0`. Popovers, menus and tooltips are not modals and stay
// anchored to their triggers.

/** The centring every dialog keeps (DialogContent applies it first; dialog.test.tsx holds every caller to it). */
export const DIALOG_CENTERED =
  "fixed inset-0 m-auto h-fit max-h-[calc(100dvh-2rem-env(safe-area-inset-top)-env(safe-area-inset-bottom))] w-[calc(100vw-2rem)] max-w-md"

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
          DIALOG_CENTERED,
          "z-50 flex flex-col rounded-xl bg-card text-card-foreground shadow-2xl outline-none transition-[opacity,scale,translate] duration-150 data-ending-style:translate-y-1 data-ending-style:opacity-0 data-starting-style:translate-y-1 data-starting-style:opacity-0",
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

/** A button that closes the dialog (the Settings dialog's ✕). */
function DialogClose({ ...props }: DialogPrimitive.Close.Props) {
  return <DialogPrimitive.Close data-slot="dialog-close" {...props} />
}

export { Dialog, DialogClose, DialogContent, DialogTitle }
