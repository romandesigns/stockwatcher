import { Icon } from "@stockwatcher/chart-web";
import { cn, Tip } from "@stockwatcher/chart-web/ui";
import type { ReactNode } from "react";
import { rossLinePaths, rowLineOverlay, type FlameMark, type PathKind, type RossLineView, type RossTone, type RowMarks, type StageMark, type TailMark } from "@stockwatcher/shared";

// The lighter rows' marks on the web (owner, 2026-10-26, approved design canvas "StockWatcher
// lighter rows"; shared derivation in packages/shared rowMarks.ts). Each mark carries its name
// and exact value in a tooltip on hover or keyboard focus (owner: "Add their respective label name
// on a tooltip when hovered"), so nothing the row stopped spelling out is lost.

const TONE: Record<RossTone, string> = {
  good: "text-good",
  critical: "text-critical",
  warning: "text-warning",
  accent: "text-primary",
  text: "text-foreground",
  muted: "text-muted-foreground",
  dim: "text-text-faint",
};

const PATH_ICON: Record<PathKind, "pathRunner" | "pathBurst" | "pathFast" | "pathRun" | "pathRunning"> = {
  runner: "pathRunner",
  burst: "pathBurst",
  fast: "pathFast",
  run: "pathRun",
  running: "pathRunning",
};

/** "Look now": the flame before the ticker, with why it is lit on hover. */
export function Flame({ flame }: { flame: FlameMark | null }) {
  if (!flame?.lit) return null;
  return (
    <Tip label={flame.label}>
      <span className="inline-flex self-center text-flame" data-testid="row-flame" role="img" aria-label={flame.label}>
        <Icon name="flame" width={15} height={15} />
      </span>
    </Tip>
  );
}

/**
 * The row's second line: pillar dots, relative-volume bars, news, the Runners path with its age,
 * then the setup's stage, meter and trigger on the right. Only the marks a row has are drawn.
 */
export function MarksLine({ marks: m, className, extra }: { marks: RowMarks; className?: string; extra?: ReactNode }) {
  const { pillars, rvol, news, path, stage } = m;
  if (!pillars && !rvol && !news && !path && !stage && !extra) return null;
  return (
    <span className={cn("flex min-w-0 items-center gap-3", className)} data-testid="row-marks">
      {pillars && (
        <Tip label={pillars.label}>
          <span className="inline-flex shrink-0 items-center gap-0.75" role="img" aria-label={pillars.label} data-testid="mark-pillars">
            {pillars.dots.map((d, i) => (
              <span key={i} className={cn("size-1.75 rounded-full", d.state === "pass" ? "bg-muted-foreground" : "shadow-[inset_0_0_0_1.4px_var(--text-faint)]")} />
            ))}
          </span>
        </Tip>
      )}
      {rvol && (
        <Tip label={rvol.label}>
          <svg viewBox="0 0 13 12" className="h-3 w-3.25 shrink-0" role="img" aria-label={rvol.label} data-testid="mark-rvol" data-bars={rvol.bars}>
            <rect x="0" y="7" width="3" height="5" rx="1" className="fill-muted-foreground" />
            <rect x="5" y="4" width="3" height="8" rx="1" className={rvol.bars >= 2 ? "fill-muted-foreground" : "fill-bg-raised"} />
            <rect x="10" y="0" width="3" height="12" rx="1" className={rvol.bars >= 3 ? "fill-muted-foreground" : "fill-bg-raised"} />
          </svg>
        </Tip>
      )}
      {news && (
        <Tip label={news.label}>
          <span className={cn("inline-flex shrink-0", news.dilution ? "text-warning" : "text-muted-foreground")} role="img" aria-label={news.label} data-testid="mark-news" data-dilution={news.dilution || undefined}>
            <Icon name="news" width={14} height={14} />
          </span>
        </Tip>
      )}
      {path && (
        <Tip label={path.label}>
          <span className="inline-flex shrink-0 items-center gap-1 text-muted-foreground" role="img" aria-label={path.label} data-testid="mark-path" data-kind={path.kind}>
            <Icon name={PATH_ICON[path.kind]} width={13} height={13} />
            <span className="font-mono text-[11px] text-text-faint tabular-nums">{path.age}</span>
          </span>
        </Tip>
      )}
      {extra}
      {stage && <Stage stage={stage} />}
    </span>
  );
}

/** The setup: its stage icon, where the price sits between stop and trigger, and the trigger. */
function Stage({ stage: s }: { stage: StageMark }) {
  return (
    <Tip label={s.label}>
      <span className={cn("ml-auto inline-flex shrink-0 items-center gap-1.5", TONE[s.tone])} role="img" aria-label={s.label} data-testid="mark-stage" data-stage={s.stage}>
        <StageIcon stage={s.stage} />
        {s.meter !== null && (
          <span className="relative inline-block h-1.5 w-14 rounded-full bg-bg-raised" aria-hidden>
            <span className="absolute inset-y-0 left-0 rounded-full bg-current opacity-55 transition-[width] duration-300 motion-reduce:transition-none" style={{ width: `${Math.round(s.meter * 100)}%` }} />
            <span className="absolute -top-0.5 right-0 h-2.5 w-0.5 rounded-full bg-foreground" />
          </span>
        )}
        {s.trigger && <span className="font-mono text-[11px] text-muted-foreground tabular-nums">{s.trigger}</span>}
      </span>
    </Tip>
  );
}

function StageIcon({ stage }: { stage: StageMark["stage"] }) {
  // Forming: a small open dot; armed / at the trigger: a ring (filled soft at the trigger);
  // crossing and broke-trigger: a rising triangle; failed: a small cross.
  return (
    <svg viewBox="0 0 12 12" className="size-3" aria-hidden>
      {stage === "forming" && <circle cx="6" cy="6" r="2.5" fill="none" stroke="currentColor" strokeWidth="1.5" />}
      {(stage === "armed" || stage === "near") && <circle cx="6" cy="6" r="4.2" fill={stage === "near" ? "currentColor" : "none"} fillOpacity={0.35} stroke="currentColor" strokeWidth="1.5" />}
      {(stage === "crossing" || stage === "triggered") && <path d="M6 1.5 10.5 10h-9z" fill="currentColor" fillOpacity={stage === "crossing" ? 1 : 0.4} stroke="currentColor" strokeWidth="1.5" strokeLinejoin="round" />}
      {stage === "invalidated" && <path d="M3 3l6 6M9 3 3 9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />}
    </svg>
  );
}

const TAIL_TONE: Record<TailMark["tone"], string> = { good: "text-good", critical: "text-critical", muted: "text-muted-foreground" };

/**
 * The row chart: the day line (and its faint area) in `tone`, the previous close dotted, the
 * setup's trigger as a dotted level, and the last minute as a thick tail coloured by its direction.
 * `stretch` fills its box's width (the Favorites column) with strokes that keep their weight.
 */
export function RowLine({ line, tail, trigger, w = 104, h = 28, tone, className, stretch = false, label }: { line: RossLineView | null; tail: TailMark | null; trigger?: number | null; w?: number; h?: number; tone: string; className?: string; stretch?: boolean; label?: string }) {
  if (!line) return <span className={cn("block", className)} style={{ height: h }} aria-hidden />;
  const p = rossLinePaths(line, w, h);
  const o = rowLineOverlay(line, w, h, trigger);
  const scale = stretch ? { vectorEffect: "non-scaling-stroke" as const } : {};
  // The tooltip: the last minute, plus whatever the row moved here from its old text (`label`).
  const tip = [tail?.label, label].filter(Boolean).join(" · ");
  const svg = (
    <svg viewBox={`0 0 ${w} ${h}`} preserveAspectRatio={stretch ? "none" : undefined} className={cn("block animate-line-in", tone, className)} style={stretch ? { height: h } : { width: w, height: h }} role={tip ? "img" : undefined} aria-label={tip || undefined} aria-hidden={tip ? undefined : true} data-testid="row-line">
      {p.baseY !== null && <line x1={0} x2={w} y1={p.baseY} y2={p.baseY} stroke="currentColor" strokeOpacity={0.35} strokeWidth={1} strokeDasharray="1.5 2.5" {...scale} />}
      {o.triggerY !== null && <line x1={0} x2={w} y1={o.triggerY} y2={o.triggerY} className="text-muted-foreground" stroke="currentColor" strokeOpacity={0.6} strokeWidth={1} strokeDasharray="2 3" data-testid="row-line-trigger" {...scale} />}
      <path d={p.area} fill="currentColor" fillOpacity={0.12} />
      <path d={p.line} fill="none" stroke="currentColor" strokeWidth={1.5} strokeLinejoin="round" {...scale} />
      {tail && o.tail && <path d={o.tail} fill="none" className={TAIL_TONE[tail.tone]} stroke="currentColor" strokeWidth={2.6} strokeLinecap="round" strokeLinejoin="round" data-testid="row-line-tail" {...scale} />}
    </svg>
  );
  return tip ? <Tip label={tip}>{svg}</Tip> : svg;
}
