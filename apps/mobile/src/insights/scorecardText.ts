// The phone scorecard's words (pure, so they are tested without React Native). The phone has no
// hover, so what the web shows in tooltips is the card's expanded detail here.
import { HOLD_LABELS, pct, SCORECARD_SEGMENTS, type ScorecardKind } from "@stockwatcher/shared";

const signed = (v: number | null) => (v === null ? "—" : `${v >= 0 ? "+" : "−"}${Math.abs(v).toFixed(1)}%`);

/** The one-line summary under a card's rate: fired, sent / held, gain, drop. */
export function summaryLine(k: ScorecardKind): string {
  const sent = k.phone.sent ?? 0;
  const held = Object.entries(k.phone).filter(([c]) => c !== "sent" && c !== "unknown").reduce((n, [, c]) => n + c, 0);
  return [`${k.fired} fired`, `${sent} sent${held > 0 ? ` · ${held} held` : ""}`, `gain ${signed(k.median_gain_pct)}`, `drop ${signed(k.median_drawdown_pct)}`].join(" · ");
}

/** The expanded detail, one fact per line. */
export function detailLines(k: ScorecardKind): string[] {
  const lines = [
    `${k.follow_through} of ${k.scored} scored alerts followed through, ${k.stopped} stopped out (${k.fired - k.scored} could not be scored)`,
    `To your phone: ${Object.entries(k.phone).map(([c, n]) => `${n} ${HOLD_LABELS[c] ?? c}`).join(" · ") || "—"}`,
    `Push time (print to push service): ${k.median_latency_ms === null ? "—" : `${(k.median_latency_ms / 1000).toFixed(1)} s`} median`,
    `How late: ${pct(k.median_share_before)} of the hour's move already behind the alert (median)`,
  ];
  if (k.batch.fired > 0) lines.push(`${k.batch.fired} fired on the 04:00 trade-report batch, ${k.batch.follow_through} of ${k.batch.scored} followed through`);
  for (const s of SCORECARD_SEGMENTS) {
    const t = k.segments[s.id];
    if (t && t.fired > 0) lines.push(`${s.label}: ${t.scored > 0 ? pct(t.follow_through / t.scored) : "—"} (${t.follow_through} of ${t.scored}, ${t.fired} fired)`);
  }
  return lines;
}

/** Points for the trend line in a `w` × `h` box: runs of days that fired, split at gaps. */
export function trendRuns(values: readonly (number | null)[], w: number, h: number): string[] {
  const n = values.length;
  const x = (i: number) => (n <= 1 ? w / 2 : (i / (n - 1)) * (w - 4) + 2);
  const y = (v: number) => h - 3 - v * (h - 6);
  const runs: string[] = [];
  let run: string[] = [];
  values.forEach((v, i) => {
    if (v === null) {
      if (run.length) runs.push(run.join(" "));
      run = [];
    } else run.push(`${x(i).toFixed(1)},${y(v).toFixed(1)}`);
  });
  if (run.length) runs.push(run.join(" "));
  return runs;
}
