// The daily scorecard (GET /api/scorecard?days=N; written nightly by the maintainer,
// crates/market-providers/src/import/scorecard.rs): per alert kind, how many fired, how many
// reached a phone or a desk and why the rest were held, how many followed through (+10% before
// −5% within 30 minutes of the alert price), how big the move was, how fast the push was and how
// late in the hour's move the alert came. Shared by the web and phone views (Insights › Scorecard).

export interface ScorecardTally {
  fired: number;
  /** Fired alerts with prints after them; the rest (a halt, a gap in the tape) cannot be scored. */
  scored: number;
  follow_through: number;
  stopped: number;
}

export interface ScorecardKind extends ScorecardTally {
  kind: string;
  /** follow_through / scored; null when nothing was scored. */
  rate: number | null;
  /** Decisions per path: "sent", a hold code, or "unknown" (no ledger line for it). */
  phone: Record<string, number>;
  desk: Record<string, number>;
  median_gain_pct: number | null;
  median_drawdown_pct: number | null;
  median_latency_ms: number | null;
  /** Share (0–1) of the hour's move already behind the alert. */
  median_share_before: number | null;
  segments: Record<string, ScorecardTally>;
  /** Alerts that fired on a trade-report batch (FINRA TRF opening reports). */
  batch: ScorecardTally;
}

export interface ScorecardDay {
  date: string;
  computed_at: string;
  rule: { target_pct: number; stop_pct: number; window_min: number };
  kinds: ScorecardKind[];
}

const num = (v: unknown): number => (typeof v === "number" && Number.isFinite(v) ? v : 0);
const numOrNull = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);
const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);
const counts = (v: unknown): Record<string, number> => {
  const out: Record<string, number> = {};
  if (isObj(v)) for (const [k, n] of Object.entries(v)) if (typeof n === "number" && Number.isFinite(n)) out[k] = n;
  return out;
};
const tally = (v: unknown): ScorecardTally => {
  const o = isObj(v) ? v : {};
  return { fired: num(o.fired), scored: num(o.scored), follow_through: num(o.follow_through), stopped: num(o.stopped) };
};

/** The API body → its days, newest first. Malformed days and kinds are dropped, never guessed. */
export function parseScorecard(body: unknown): ScorecardDay[] {
  if (!isObj(body) || !Array.isArray(body.days)) return [];
  const out: ScorecardDay[] = [];
  for (const d of body.days) {
    if (!isObj(d) || typeof d.date !== "string" || !/^\d{4}-\d{2}-\d{2}$/.test(d.date) || !Array.isArray(d.kinds)) continue;
    const r = isObj(d.rule) ? d.rule : {};
    const kinds: ScorecardKind[] = [];
    for (const k of d.kinds) {
      if (!isObj(k) || typeof k.kind !== "string") continue;
      const segments: Record<string, ScorecardTally> = {};
      if (isObj(k.segments)) for (const [s, t] of Object.entries(k.segments)) segments[s] = tally(t);
      kinds.push({
        kind: k.kind,
        ...tally(k),
        rate: numOrNull(k.rate),
        phone: counts(k.phone),
        desk: counts(k.desk),
        median_gain_pct: numOrNull(k.median_gain_pct),
        median_drawdown_pct: numOrNull(k.median_drawdown_pct),
        median_latency_ms: numOrNull(k.median_latency_ms),
        median_share_before: numOrNull(k.median_share_before),
        segments,
        batch: tally(k.batch),
      });
    }
    out.push({ date: d.date, computed_at: typeof d.computed_at === "string" ? d.computed_at : "", rule: { target_pct: num(r.target_pct) || 10, stop_pct: num(r.stop_pct) || 5, window_min: num(r.window_min) || 30 }, kinds });
  }
  return out.sort((a, b) => (a.date < b.date ? 1 : a.date > b.date ? -1 : 0));
}

/** Display names and order: the Runners paths first (best evidence first), then the follow-ups. */
export const SCORECARD_KINDS: readonly { kind: string; label: string }[] = [
  { kind: "runners.burst", label: "Buying burst" },
  { kind: "runners.run", label: "Run +20%" },
  { kind: "runners.fast", label: "Fast move" },
  { kind: "runners.running", label: "Running" },
  { kind: "runners.new", label: "R2 flag" },
  { kind: "runners.pullback.setup", label: "Pullback setting up" },
  { kind: "runners.pullback.crossing", label: "Pullback crossing" },
  { kind: "ross.crossing", label: "Ross crossing" },
  { kind: "signals.crossing", label: "Signals crossing" },
  { kind: "ross.trigger", label: "Ross at trigger" },
  { kind: "signals.trigger", label: "Signals at trigger" },
  { kind: "runners.shelf", label: "Volume shelf" },
  { kind: "runners.followup", label: "Follow-up" },
  { kind: "runners.breakout", label: "Breakout" },
  { kind: "halt.watch", label: "Halt watcher" },
];

export const SCORECARD_SEGMENTS: readonly { id: string; label: string }[] = [
  { id: "open_print", label: "04:00–04:02" },
  { id: "pre", label: "Pre-market" },
  { id: "mkt_open", label: "9:30–10:00" },
  { id: "rth", label: "Regular" },
  { id: "after", label: "After hours" },
];

/** Readable hold codes (the ledger's limit names). */
export const HOLD_LABELS: Readonly<Record<string, string>> = {
  sent: "sent",
  symbol_limit: "held: same stock just notified",
  quiet_hours: "held: quiet hours",
  stale: "held: too old",
  duplicate: "held: already sent",
  no_device: "no device wanted it",
  unknown: "not in the ledger",
};

export interface ScorecardRow {
  kind: string;
  label: string;
  /** The latest scored day's line for this kind (null: it did not fire that day). */
  latest: ScorecardKind | null;
  /** Follow-through rate per day, oldest → newest, null on days it did not fire or was unscored. */
  trend: (number | null)[];
  /** Pooled over every day shown: follow-through / scored. */
  pooled: { scored: number; follow_through: number; rate: number | null };
}

/** One row per kind that fired on any day shown, in [`SCORECARD_KINDS`] order (unknown kinds last). */
export function scorecardRows(days: readonly ScorecardDay[]): ScorecardRow[] {
  const oldestFirst = [...days].reverse();
  const latest = days[0];
  const seen = new Set<string>();
  for (const d of days) for (const k of d.kinds) seen.add(k.kind);
  const order = [...SCORECARD_KINDS.map((k) => k.kind), ...[...seen].filter((k) => !SCORECARD_KINDS.some((x) => x.kind === k)).sort()];
  return order
    .filter((kind) => seen.has(kind))
    .map((kind) => {
      let scored = 0;
      let follow = 0;
      const trend = oldestFirst.map((d) => {
        const k = d.kinds.find((x) => x.kind === kind);
        if (!k) return null;
        scored += k.scored;
        follow += k.follow_through;
        return k.rate;
      });
      return {
        kind,
        label: SCORECARD_KINDS.find((x) => x.kind === kind)?.label ?? kind,
        latest: latest?.kinds.find((x) => x.kind === kind) ?? null,
        trend,
        pooled: { scored, follow_through: follow, rate: scored > 0 ? follow / scored : null },
      };
    });
}

/** "42%" (whole percent), or "—". */
export const pct = (v: number | null | undefined): string => (v === null || v === undefined ? "—" : `${Math.round(v * 100)}%`);
