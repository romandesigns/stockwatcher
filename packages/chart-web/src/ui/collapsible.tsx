import { Collapsible as CollapsiblePrimitive } from "@base-ui/react/collapsible"
import { cn } from "./utils.ts"
import { Icon } from "../Icon.tsx"

// shadcn/ui collapsible, Base UI variant, added 2026-10-22 to replace the
// native <details>/<summary> disclosures (owner: no browser-native UI). The
// trigger is a real button with aria-expanded/aria-controls; the shared
// duotone chevronRight turns down while open (a native summary's marker is
// the browser's own triangle). The panel animates its height through Base
// UI's --collapsible-panel-height (reduced motion is handled globally), and
// hiddenUntilFound keeps a closed panel's text reachable by find-in-page, as
// a closed <details> is.

function Collapsible({ ...props }: CollapsiblePrimitive.Root.Props) {
  return <CollapsiblePrimitive.Root data-slot="collapsible" {...props} />
}

function CollapsibleTrigger({ className, children, ...props }: CollapsiblePrimitive.Trigger.Props) {
  return (
    <CollapsiblePrimitive.Trigger
      data-slot="collapsible-trigger"
      className={cn(
        "group/collapsible-trigger flex w-full cursor-pointer items-center gap-1 rounded-md text-left outline-none focus-visible:ring-2 focus-visible:ring-ring [&_svg]:size-3 [&_svg]:shrink-0",
        className
      )}
      {...props}
    >
      <Icon name="chevronRight" className="transition-transform duration-150 group-data-panel-open/collapsible-trigger:rotate-90" />
      {children}
    </CollapsiblePrimitive.Trigger>
  )
}

function CollapsibleContent({ className, ...props }: CollapsiblePrimitive.Panel.Props) {
  return (
    <CollapsiblePrimitive.Panel
      data-slot="collapsible-content"
      hiddenUntilFound
      className={cn(
        "h-(--collapsible-panel-height) overflow-hidden transition-[height] duration-150 ease-out data-ending-style:h-0 data-starting-style:h-0",
        className
      )}
      {...props}
    />
  )
}

export { Collapsible, CollapsibleContent, CollapsibleTrigger }
