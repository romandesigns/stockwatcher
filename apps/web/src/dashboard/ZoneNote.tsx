import { cn } from "@stockwatcher/chart-web/ui";
import type { ZoneNoteView } from "@stockwatcher/shared";
import { Reveal } from "./Reveal.tsx";

/**
 * The volume-shelf note (shared zones.ts) on a Runners row and above a chart: "In volume shelf
 * $2.41–$2.55" in plain text with the research base rate muted beside it ("about 1 in 3 hold,
 * 1 in 2 fail"), then "Shelf held" / "Shelf lost" muted once the server resolves it. Information,
 * never a call: no success green, no accent, no icon. It opens and closes by its real height
 * (Reveal, which keeps the last words drawn while it closes); reduced motion makes that instant.
 * `closedClassName` applies while closed (e.g. a negative margin cancelling the parent's gap).
 */
export function ZoneNote({ note, className, closedClassName }: { note: ZoneNoteView | null; className?: string; closedClassName?: string }) {
  return (
    <Reveal as="span" show={note !== null} className={note === null ? closedClassName : undefined}>
      {note && (
        <span data-zone={note.state} aria-label={note.aria} className={cn("flex min-w-0 items-baseline gap-1.5 overflow-hidden font-sans text-[11px] whitespace-nowrap", className)}>
          <span className={cn("shrink-0 transition-colors duration-300 motion-reduce:transition-none", note.tone === "text" ? "text-foreground" : "text-muted-foreground")}>{note.label}</span>
          {note.baseRate && <span className="truncate text-text-faint">{note.baseRate}</span>}
        </span>
      )}
    </Reveal>
  );
}
