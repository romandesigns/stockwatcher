import { cn, Tip } from "@stockwatcher/chart-web/ui";

import { useNoticeCard } from "../notify/noticeCards.ts";
import { noticeCardLabel } from "@stockwatcher/shared";
import { Reveal } from "./Reveal.tsx";

/**
 * "Group repeat alerts" (owner, 2026-11-25): a stock's alerts today folded into its row, "12 alerts ·
 * 10:37", the latest notice's own text in the tooltip, instead of one notification per repeat
 * (notify/noticeCards.ts; server runners.rs "Limits" 4a). Neutral: a count is a fact, not a call to
 * act, so it spends no accent. Absent for a single alert (the row already is that alert).
 */
export function AlertCountChip({ symbol, className }: { symbol: string; className?: string }) {
  const card = useNoticeCard(symbol);
  const label = noticeCardLabel(card);
  const aria = card && label ? `${label}. Latest: ${card.title} · ${card.body}` : "";
  return (
    <Reveal show={label !== null} axis="x" as="span" className={cn("shrink-0", className)}>
      {card && label && (
        <Tip label={aria}>
          <span role="img" aria-label={aria} data-testid="alert-count-chip" className="inline-flex items-center rounded bg-bg-raised px-1.5 py-px font-mono text-[10px] whitespace-nowrap text-muted-foreground tabular-nums">
            <span className="@[18rem]/row:hidden">{card.count}×</span>
            <span className="hidden @[18rem]/row:inline">{label}</span>
          </span>
        </Tip>
      )}
    </Reveal>
  );
}
