import { Button, cn, Tip } from "@stockwatcher/chart-web/ui";
import { HOLD_LABELS, parseScorecard, pct, SCORECARD_SEGMENTS, scorecardRows, serverUrl, type ScorecardDay, type ScorecardKind, type ScorecardRow } from "@stockwatcher/shared";
import { useCallback, useEffect, useMemo, useState } from "react";
import { getToken } from "../feed/token.ts";

// Insights › Scorecard (owner, 2026-10-26): every alert kind scored each night by the maintainer
// (crates/market-providers/src/import/scorecard.rs, GET /api/scorecard): how many fired and
// reached you, how many followed through (+10% before −5% within 30 minutes), how big, how fast
// and how late in the move. Self-contained: it fetches its own data; the Insights page mounts it.

type Load = { state: "loading" } | { state: "error" } | { state: "ready"; days: ScorecardDay[] };

function useScorecard(days: number): { load: Load; retry: () => void } {
  const [attempt, setAttempt] = useState(0);
  // The answer for the request it belongs to: a new request (another `days`, a retry) reads as
  // loading until its own answer lands, without setting state before the fetch.
  const key = `${days}:${attempt}`;
  const [got, setGot] = useState<{ key: string; load: Load } | null>(null);
  useEffect(() => {
    let live = true;
    const token = getToken();
    fetch(serverUrl(`/api/scorecard?days=${days}`), { headers: token ? { Authorization: `Bearer ${token}` } : {} })
      .then((res) => (res.ok ? res.json() : Promise.reject(new Error(String(res.status)))))
      .then((body: unknown) => {
        if (live) setGot({ key, load: { state: "ready", days: parseScorecard(body) } });
      })
      .catch(() => {
        if (live) setGot({ key, load: { state: "error" } });
      });
    return () => {
      live = false;
    };
  }, [days, key]);
  const load: Load = got && got.key === key ? got.load : { state: "loading" };
  return { load, retry: useCallback(() => setAttempt((n) => n + 1), []) };
}

const dayLabel = (iso: string) => new Date(`${iso}T12:00:00Z`).toLocaleDateString("en-US", { weekday: "short", month: "short", day: "numeric", timeZone: "UTC" });
const signed = (v: number | null) => (v === null ? "—" : `${v >= 0 ? "+" : "−"}${Math.abs(v).toFixed(1)}%`);
const seconds = (ms: number | null) => (ms === null ? "—" : `${(ms / 1000).toFixed(1)} s`);

/** The view (no page chrome: the Insights page owns the heading and tabs). */
export function ScorecardView({ days = 30 }: { days?: number }) {
  const { load, retry } = useScorecard(days);
  if (load.state === "loading") {
    return (
      <p className="m-0 px-1 py-6 text-sm text-text-faint" role="status">
        Loading the scorecard…
      </p>
    );
  }
  if (load.state === "error") {
    return (
      <div className="flex flex-col items-start gap-2 px-1 py-6" role="alert">
        <p className="m-0 text-sm">The scorecard could not be loaded.</p>
        <Button variant="secondary" size="sm" onClick={retry}>
          Try again
        </Button>
      </div>
    );
  }
  return <ScorecardBody days={load.days} />;
}

export function ScorecardBody({ days }: { days: readonly ScorecardDay[] }) {
  const rows = useMemo(() => scorecardRows(days), [days]);
  const latest = days[0];
  if (!latest || rows.length === 0) {
    return (
      <div className="flex flex-col gap-1 px-1 py-6">
        <p className="m-0 text-sm font-medium">Nothing scored yet</p>
        <p className="m-0 max-w-md text-xs text-text-faint">Each trading day is scored after the nightly import (around 9:30 PM ET), once the full tape for the day is in.</p>
      </div>
    );
  }
  const rule = latest.rule;
  return (
    <section aria-label="Scorecard" className="@container/score flex min-w-0 flex-col gap-3">
      <p className="m-0 px-1 text-xs text-text-faint">
        Latest scored day <span className="text-muted-foreground">{dayLabel(latest.date)}</span> · follow-through = +{rule.target_pct}% before −{rule.stop_pct}% within {rule.window_min} min of the alert price · trend over the last {days.length} scored {days.length === 1 ? "day" : "days"}
      </p>
      <ul className="m-0 grid list-none grid-cols-1 gap-2 p-0 @2xl/score:grid-cols-2 @6xl/score:grid-cols-3">
        {rows.map((r) => (
          <li key={r.kind}>
            <KindCard row={r} />
          </li>
        ))}
      </ul>
    </section>
  );
}

function KindCard({ row }: { row: ScorecardRow }) {
  const k = row.latest;
  return (
    <article aria-label={row.label} className="flex h-full flex-col gap-2 rounded-xl bg-card p-3">
      <div className="flex items-start gap-3">
        <div className="flex min-w-0 flex-1 flex-col gap-0.5">
          <h3 className="m-0 truncate text-sm font-semibold">{row.label}</h3>
          <Tip label={k ? `${k.follow_through} of ${k.scored} scored alerts followed through on the latest day (${k.stopped} stopped out, the rest neither within the window)` : "Did not fire on the latest day"}>
            <span className="w-fit font-mono text-2xl font-semibold tabular-nums">{pct(k?.rate ?? null)}</span>
          </Tip>
          <span className="text-[11px] text-text-faint">follow-through, latest day</span>
        </div>
        <Tip label={`Follow-through per day, oldest to newest. Pooled over the period: ${row.pooled.follow_through} of ${row.pooled.scored} (${pct(row.pooled.rate)}).`}>
          <span className="flex flex-col items-end gap-0.5">
            <Trend values={row.trend} />
            <span className="font-mono text-[11px] text-text-faint">{pct(row.pooled.rate)} pooled</span>
          </span>
        </Tip>
      </div>
      {k ? <Facts k={k} /> : <p className="m-0 text-xs text-text-faint">Did not fire on the latest day.</p>}
    </article>
  );
}

function Facts({ k }: { k: ScorecardKind }) {
  const sent = k.phone.sent ?? 0;
  const held = Object.entries(k.phone).filter(([code]) => code !== "sent");
  const heldTotal = held.reduce((n, [, c]) => n + c, 0);
  const heldLabel = held.length ? held.map(([code, c]) => `${c} ${HOLD_LABELS[code] ?? code}`).join(" · ") : "none held";
  const segments = SCORECARD_SEGMENTS.map((s) => ({ ...s, t: k.segments[s.id] })).filter((s) => s.t && s.t.fired > 0);
  return (
    <div className="flex flex-col gap-1.5 text-xs">
      <div className="flex flex-wrap gap-x-3 gap-y-1 text-muted-foreground">
        <Fact label={`${k.fired} fired, ${k.scored} could be scored (the rest had no prints after the alert, e.g. a halt)`}>{k.fired} fired</Fact>
        <Fact label={`To your phone: ${sent} sent · ${heldLabel}`}>
          {sent} sent{heldTotal > 0 ? ` · ${heldTotal} held` : ""}
        </Fact>
        <Fact label="Median largest gain within 30 minutes of the alert price">gain {signed(k.median_gain_pct)}</Fact>
        <Fact label="Median deepest drop within 30 minutes of the alert price">drop {signed(k.median_drawdown_pct)}</Fact>
        <Fact label="Median time from the triggering print to the push service">{seconds(k.median_latency_ms)}</Fact>
        <Fact label="Median share of the hour's move already behind the alert (lowest price in the hour before to highest in the hour after)">late {pct(k.median_share_before)}</Fact>
        {k.batch.fired > 0 && (
          <Fact label={`${k.batch.fired} fired on the 04:00 trade-report batch (off-exchange reports released at once), ${k.batch.follow_through} of ${k.batch.scored} followed through`} tone="warning">
            {k.batch.fired} on report batch
          </Fact>
        )}
      </div>
      {segments.length > 1 && (
        <div className="flex flex-wrap gap-x-3 gap-y-1 font-mono text-[11px] text-text-faint">
          {segments.map((s) => (
            <Fact key={s.id} label={`${s.label}: ${s.t!.follow_through} of ${s.t!.scored} followed through (${s.t!.fired} fired)`}>
              {s.label} {s.t!.scored > 0 ? pct(s.t!.follow_through / s.t!.scored) : "—"}
            </Fact>
          ))}
        </div>
      )}
    </div>
  );
}

function Fact({ label, tone, children }: { label: string; tone?: "warning"; children: React.ReactNode }) {
  return (
    <Tip label={label}>
      <span className={cn("cursor-default tabular-nums", tone === "warning" && "text-warning")} tabIndex={0} aria-label={label}>
        {children}
      </span>
    </Tip>
  );
}

/** Follow-through per day as a small line; days it did not fire break the line. */
function Trend({ values }: { values: readonly (number | null)[] }) {
  const w = 120;
  const h = 32;
  const n = values.length;
  const x = (i: number) => (n <= 1 ? w / 2 : (i / (n - 1)) * (w - 4) + 2);
  const y = (v: number) => h - 3 - v * (h - 6);
  const segments: string[] = [];
  let run: string[] = [];
  values.forEach((v, i) => {
    if (v === null) {
      if (run.length) segments.push(run.join(" "));
      run = [];
    } else run.push(`${x(i).toFixed(1)},${y(v).toFixed(1)}`);
  });
  if (run.length) segments.push(run.join(" "));
  const last = values.length ? values[values.length - 1] : null;
  return (
    <svg width={w} height={h} viewBox={`0 0 ${w} ${h}`} aria-hidden className="text-muted-foreground">
      <line x1={0} x2={w} y1={y(0.5)} y2={y(0.5)} stroke="currentColor" strokeOpacity={0.25} strokeDasharray="2 3" />
      {segments.map((pts, i) =>
        pts.includes(" ") ? <polyline key={i} points={pts} fill="none" stroke="currentColor" strokeWidth={1.5} strokeLinejoin="round" /> : <circle key={i} cx={pts.split(",")[0]} cy={pts.split(",")[1]} r={1.5} fill="currentColor" />,
      )}
      {last !== null && last !== undefined && <circle cx={x(n - 1)} cy={y(last)} r={2.5} className="fill-foreground" />}
    </svg>
  );
}
