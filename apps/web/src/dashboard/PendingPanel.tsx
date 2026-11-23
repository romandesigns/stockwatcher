import { Icon } from "@stockwatcher/chart-web";
import { cn, ScrollArea } from "@stockwatcher/chart-web/ui";
import {
  fmtPendingClock,
  fmtPendingHm,
  fmtPendingPct,
  fmtPendingPrice,
  fmtPendingTime,
  pendingReopenPct,
  pendingSentence,
  pendingStatus,
  visiblePendingCards,
  type PendingCardWire,
  type PendingState,
} from "@stockwatcher/shared";
import { memo, useEffect, useMemo, useState } from "react";
import { useSecondClock } from "../feed/usePending.ts";
import { useTickerPress } from "../lib/useTickerPress.ts";
import { Reveal } from "./Reveal.tsx";

/** How long a removed card takes to fold away (Reveal's 300 ms, and a frame of margin). */
const LEAVE_MS = 320;

const signed = (x: number | null) => (x === null ? "text-text-faint" : x > 0 ? "text-good" : x < 0 ? "text-critical" : "text-muted-foreground");

/**
 * The cards to draw: the visible ones in order, plus the ones that just left for as long as they
 * take to fold away (drawn with their last state), so a removal never makes the list jump.
 */
function useLeaving(cards: PendingCardWire[]): { card: PendingCardWire; show: boolean }[] {
  const [prev, setPrev] = useState(cards);
  const [leaving, setLeaving] = useState<PendingCardWire[]>([]);
  if (prev !== cards) {
    const ids = new Set(cards.map((c) => c.id));
    const gone = prev.filter((c) => !ids.has(c.id));
    if (gone.length || leaving.some((c) => ids.has(c.id))) setLeaving((l) => [...l.filter((c) => !ids.has(c.id)), ...gone]);
    setPrev(cards);
  }
  useEffect(() => {
    if (leaving.length === 0) return;
    const id = setTimeout(() => setLeaving([]), LEAVE_MS);
    return () => clearTimeout(id);
  }, [leaving]);
  return [...cards.map((card) => ({ card, show: true })), ...leaving.map((card) => ({ card, show: false }))];
}

/**
 * The News pending card (owner, 2026-11-22): one card per stock the exchange halted because news
 * is pending (a T1 halt, $0.10–$25), walking through its life: **halted** (halt time, halt price,
 * the gain into the halt, a count-up timer, in the warning colour: something is coming, not "act"),
 * **news found** (the matched headline or filing slides in with its source; an offering is flagged
 * in the danger colour at once, it usually kills the move), **resumption set** (a countdown to the
 * reopen, the one place this card spends the accent) and **resumed** (it dims and folds away after
 * the server's grace period: the stock is the normal panels' again).
 *
 * The server (crates/server/src/pending) sends one frame per state change; every timer is computed
 * here from the card's timestamps, re-read once a second only while a card is on screen. Laid out
 * by the panel's own width (`@container/pend`): one column, two, then three. Pressing a card's head
 * loads its ticker into the selected chart (tap, then hold copies it); a headline with a link
 * opens it in a new tab (the desktop's opener plugin sends it to the browser).
 */
export function PendingPanelView({ state, onPick }: { state: PendingState; onPick: (symbol: string) => void }) {
  const now = useSecondClock(state.cards.length > 0);
  const cards = useMemo(() => visiblePendingCards(state, now), [state, now]);
  const slots = useLeaving(cards);
  const reconnecting = !state.connected && cards.length > 0;
  const live = cards.filter((c) => c.phase !== "resumed").length;
  const band = state.band ? `$${state.band[0].toFixed(2)}–$${state.band[1]}` : "in band";
  return (
    <aside aria-labelledby="pending-title" data-panel="pending" className="@container/pend flex h-full min-h-0 min-w-0 flex-col gap-2 rounded-xl bg-card p-2.5">
      <div className="flex min-w-0 items-center gap-2 px-1">
        <Icon name="news" className="size-4 shrink-0 text-muted-foreground" />
        <h2 id="pending-title" className="panel-title shrink-0">
          News pending
        </h2>
        <span className="truncate font-mono text-[11px] text-text-faint">T1 halts · {band}</span>
        <span className="ml-auto font-mono text-[11px] text-text-faint tabular-nums" aria-label={`${live} halted`}>
          {live}
        </span>
      </div>
      {reconnecting && (
        <p className="m-0 flex items-center gap-1.5 px-1 text-[11px] text-warning" role="status">
          <span className="size-1.5 rounded-full bg-warning" aria-hidden />
          Reconnecting · last known state
        </p>
      )}
      <ScrollArea className={cn("min-h-0 flex-1 transition-opacity duration-200", reconnecting && "opacity-60")} viewportClassName="h-full">
        <ul
          aria-label="News pending halts"
          // One column, two from 34rem, three from 52rem; a short last row stretches to fill it
          // (no card left alone beside an empty slot).
          className={cn(
            "m-0 grid list-none grid-cols-1 content-start gap-1.5 p-0",
            "@[34rem]/pend:grid-cols-2 @[34rem]/pend:@max-[52rem]/pend:[&>li:nth-child(odd):last-child]:col-span-2",
            "@[52rem]/pend:grid-cols-3 @[52rem]/pend:[&>li:nth-child(3n+1):last-child]:col-span-3 @[52rem]/pend:[&>li:nth-child(3n+2):last-child]:col-span-2",
          )}
        >
          {slots.map(({ card, show }) => (
            <li key={card.id} className="min-w-0">
              <Reveal show={show} className="h-full" innerClassName="h-full">
                <PendingCard card={card} now={now} onPick={onPick} />
              </Reveal>
            </li>
          ))}
        </ul>
      </ScrollArea>
    </aside>
  );
}

const PendingCard = memo(function PendingCard({ card: c, now, onPick }: { card: PendingCardWire; now: number; onPick: (symbol: string) => void }) {
  const press = useTickerPress(c.sym, () => onPick(c.sym));
  const resumed = c.phase === "resumed";
  const counting = !resumed && c.resume_at !== null;
  const news = c.news[0];
  const reopenPct = pendingReopenPct(c);
  // The status colour carries the meaning: warning while pending, danger once an offering shows.
  const statusTone = resumed ? "text-muted-foreground" : c.offering ? "text-critical" : "text-warning";
  return (
    <div
      data-pending-card={c.sym}
      data-phase={c.phase}
      className={cn(
        "animate-tape-in flex h-full min-w-0 flex-col gap-1.5 rounded-lg px-2.5 py-2 transition-[opacity,background-color] duration-300 ease-out",
        c.offering && !resumed ? "bg-[color-mix(in_srgb,var(--critical)_6%,var(--bg-row))]" : "bg-bg-row",
        resumed && "opacity-60",
      )}
    >
      <button
        type="button"
        {...press}
        aria-label={`${pendingSentence(c, now)}. Load into the selected chart`}
        className="-mx-1 -my-0.5 flex min-w-0 cursor-pointer touch-manipulation items-center gap-2 rounded-md px-1 py-0.5 text-left outline-none select-none transition-colors duration-150 hover:bg-bg-raised focus-visible:ring-2 focus-visible:ring-ring pointer-coarse:min-h-11"
      >
        <span className="flex min-w-0 flex-1 flex-col gap-0.5">
          <span className="flex min-w-0 items-baseline gap-1.5 font-mono">
            <span className="shrink-0 text-sm font-semibold text-foreground">{c.sym}</span>
            <span className="shrink-0 text-[13px] text-foreground tabular-nums">{fmtPendingPrice(c.price)}</span>
            <span className={cn("shrink-0 text-xs tabular-nums", signed(c.chg_pct))}>{fmtPendingPct(c.chg_pct)}</span>
          </span>
          <span className={cn("flex min-w-0 items-center gap-1 text-[11px] transition-colors duration-300", statusTone)}>
            <Icon name={resumed ? "play" : "pause"} className="size-3 shrink-0" />
            <span className="truncate">
              {pendingStatus(c)}
              <span className="text-text-faint"> · {fmtPendingTime(c.halted_at)}</span>
            </span>
          </span>
        </span>
        {/* The one timer: up from the halt, down to the reopen (accent), or the resume time. */}
        <span className="flex shrink-0 flex-col items-end gap-0.5 font-mono tabular-nums">
          {resumed ? (
            <>
              <span className="text-[11px] text-text-faint">resumed</span>
              <span className="text-sm text-muted-foreground">{c.resumed_at !== null ? fmtPendingHm(c.resumed_at) : "—"}</span>
            </>
          ) : counting ? (
            <>
              <span className="text-[11px] text-text-faint">reopens in</span>
              <span className="text-sm font-semibold text-primary">{c.resume_est ? "~" : ""}{fmtPendingClock((c.resume_at ?? now) - now)}</span>
            </>
          ) : (
            <>
              <span className="text-[11px] text-text-faint">halted</span>
              <span className={cn("text-sm", statusTone)}>{fmtPendingClock(now - c.halted_at)}</span>
            </>
          )}
        </span>
      </button>

      <Reveal show={!!news || c.released_at !== null}>
        <div className="flex min-w-0 flex-col gap-1 rounded-md bg-bg-raised/50 px-2 py-1.5">
          <span className="flex min-w-0 items-center gap-1.5 text-[11px]">
            <Icon name="news" className="size-3 shrink-0 text-muted-foreground" />
            <span className="truncate text-muted-foreground">
              {news ? news.label : "Exchange"}
              <span className="text-text-faint"> · {fmtPendingTime(news ? news.at : (c.released_at ?? now))}</span>
            </span>
            {c.offering && (
              <span className="ml-auto inline-flex shrink-0 items-center rounded bg-critical-bg px-1.5 py-px text-[10px] font-semibold text-critical">Offering</span>
            )}
          </span>
          {news ? (
            news.url ? (
              <a
                href={news.url}
                target="_blank"
                rel="noopener noreferrer"
                className="line-clamp-2 rounded-sm text-xs leading-snug text-foreground outline-none hover:underline focus-visible:ring-2 focus-visible:ring-ring pointer-coarse:py-1"
              >
                {news.headline ?? `${news.label} item`}
              </a>
            ) : (
              <span className="line-clamp-2 text-xs leading-snug text-foreground">{news.headline ?? (news.kind === "offering" ? "Offering filed" : `${news.label} item, no headline`)}</span>
            )
          ) : (
            <span className="text-xs leading-snug text-muted-foreground">News released, no story matched yet</span>
          )}
        </div>
      </Reveal>

      <Reveal show={counting || (resumed && c.reopen_price !== null)}>
        <span className="flex min-w-0 items-baseline gap-1.5 px-0.5 font-mono text-[11px] text-muted-foreground tabular-nums">
          {resumed ? (
            <>
              Reopened <span className="text-foreground">{fmtPendingPrice(c.reopen_price)}</span>
              <span className={signed(reopenPct)}>{fmtPendingPct(reopenPct)}</span>
              <span className="text-text-faint">from the halt</span>
            </>
          ) : (
            <>
              Trading resumes {c.resume_est ? "~" : ""}
              <span className="text-foreground">{c.resume_at !== null ? fmtPendingHm(c.resume_at) : "—"}</span>
              {c.resume_est && <span className="text-text-faint">est.</span>}
            </>
          )}
        </span>
      </Reveal>
    </div>
  );
});
