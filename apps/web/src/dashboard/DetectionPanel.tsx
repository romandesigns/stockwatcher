import { formatEtMillis, formatEtTime } from "@stockwatcher/chart-core";
import { Collapsible, CollapsibleContent, CollapsibleTrigger, OptionSelect, ScrollArea, Tip, cn } from "@stockwatcher/chart-web/ui";
import { DETECTION_FILTERS, detectionEvidenceValue, detectionFamilyLabel, detectionPrice, detectionRowView, detectionStateLabel, detectionView, detectionViewCounts, groupDetections, type DetectionEpisode, type DetectionEvidence, type DetectionFilter, type DetectionGroup, type DetectionRowView, type DetectionState, type DetectionTone } from "@stockwatcher/shared";
import { memo, useMemo, useState } from "react";
import { useTickerPress } from "../lib/useTickerPress.ts";
import { Reveal } from "./Reveal.tsx";
import { TriggerMeter } from "./RossPanel.tsx";

const at = (ns: number | null) => ns === null || ns === 0 ? "—" : `${formatEtMillis(Math.floor(ns / 1_000_000))} ET`;
const hhmm = (ns: number) => formatEtTime(Math.round(ns / 1_000_000));
const words = (s: string) => s.replace(/[_.]/g, " ");
/** The price band as the Ross Panel prints it: "$0.10–$25". */
const band = (micros: number) => {
  const p = micros / 1_000_000;
  return `$${Number.isInteger(p) ? p : p.toFixed(2)}`;
};
const CONTROL = "min-h-8 rounded-md bg-bg-row px-2 text-[11px] text-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring";
// Segmented choices (Active / Recent, the detail tabs): the picked one is raised, never accent
// (owner's segmented rule).
const SELECTED = "bg-bg-raised text-foreground";
const UNSELECTED = "text-muted-foreground hover:text-foreground";
const DISCLOSURE = "text-[11px] text-muted-foreground hover:text-foreground";
const PAGE = 40;
// The four filters (owner, 2026-10-22: shadcn Selects, not the browser's <select>).
const FAMILY_OPTIONS = DETECTION_FILTERS.map((f) => ({ value: f.id, label: f.label }));
const SESSION_OPTIONS = [{ value: "all", label: "All sessions" }, { value: "pre", label: "Premarket" }, { value: "regular", label: "Regular" }, { value: "after", label: "After-hours" }];
const SOURCE_OPTIONS = [{ value: "all", label: "All sources" }, { value: "live", label: "Live" }, { value: "replay", label: "Replay" }, { value: "fixture", label: "Fixture / demo" }];
const CATALYST_OPTIONS = [{ value: "all", label: "All catalyst statuses" }, { value: "qualifying", label: "Qualifying evidence" }, { value: "none", label: "No qualifying evidence" }, { value: "unknown", label: "Coverage unknown" }];

const TONE: Record<DetectionTone, string> = {
  critical: "text-critical",
  warning: "text-warning",
  accent: "text-primary",
  text: "text-foreground",
  muted: "text-muted-foreground",
  dim: "text-text-faint",
};
/** The day's gain, coloured like the Ross row's (text-good), red only when down. */
const GAIN: Record<DetectionRowView["gainTone"], string> = { good: "text-good", critical: "text-critical", muted: "text-muted-foreground" };
/**
 * The edge a stage earns (the Ross cue edge: a filled inset shadow, never a border). Only a
 * crossing in progress (accent, with a faint wash) and a price at the trigger (amber): a
 * broke-trigger is a past stage that failed about three times in four, never a green "go", so
 * it earns none, whatever the canvas mock-up drew.
 */
const EDGE: Partial<Record<DetectionRowView["stage"], string>> = {
  crossing: "shadow-[inset_3px_0_0_var(--accent)] bg-primary/10",
  near: "shadow-[inset_3px_0_0_var(--warning)]",
};
const tone = (e: DetectionEpisode) => ["invalidated", "rejected", "interrupted"].includes(e.state) ? "text-critical" : ["armed", "crossing_provisional", "crossing_wick"].includes(e.state) ? "text-warning" : "text-foreground";
const measured = (c: DetectionEvidence): string => {
  if (typeof c.value !== "number") return detectionEvidenceValue(c);
  if (c.units === "UTC ns") return at(c.value);
  if (/(?:depth_milli|coverage_milli|close_location_milli|a_milli)$/.test(c.name)) return `${(c.value / 10).toFixed(2)}%`;
  if (c.units === "thousandths" || c.units === "milli") return `${(c.value / 1000).toFixed(3)}${/z_milli$/.test(c.name) ? " Z" : "×"}`;
  if (/recognition\.(?:range|body)\.(?:r|b|median|floor)$/.test(c.name) || /upper_wick$/.test(c.name)) return detectionPrice(c.value);
  return detectionEvidenceValue(c);
};

/**
 * Signals (design canvas "Detection Panel", approved 2026-10-21/22): the Detection Panel as the
 * side column's third tab, beside Ross and Runners. One compact row per instrument: ticker,
 * price, age, Ross/news marks, then family · stage · level note, and the trigger meter while a
 * pullback is live. The stage labels are the Ross cue's (shared `detectionRowView`); a row's
 * colour is spent only at the trigger (amber) or crossing it (accent, pulsing). The day's gain
 * follows the price (server `context.gain_bp`), and an ignition or momentum row's note is the
 * engine's recognition summary. Pressing a row draws its episode on the selected chart (faint
 * event-start, trigger and invalidation marks: chart-core `layoutDetections`) and opens its
 * evidence below it; pressing it again clears both. Tap, then hold copies the ticker. Filters fold away under one disclosure. All state and checks come from Rust:
 * filters, pages and the selection never change what is monitored.
 *
 * Its tab is hidden while the detector is off or not yet known (owner review 2026-10-21; the
 * canvas's "off" state) and while neither view has rows (owner, 2026-10-22: a panel with 0 items
 * is hidden), so there is no quiet "Nothing developing" state. The Active / Recent toggle shows
 * only while both views have rows; with one, that one is shown. It takes its state: the side
 * column subscribes (useDetection) to decide the tab and passes it down; tests need no socket.
 */
export function DetectionPanelView({ state, onPick, onClear }: { state: DetectionState; onPick: (symbol: string, episodeId: string, runEpoch: string) => void; onClear?: () => void }) {
  const [filter, setFilter] = useState<DetectionFilter>("all");
  const [pickedRecent, setHistory] = useState(false);
  const [segment, setSegment] = useState("all");
  const [source, setSource] = useState("all");
  const [catalyst, setCatalyst] = useState("all");
  const [page, setPage] = useState(0);
  const [selection, setSelection] = useState<{ epoch: string; id: string } | null>(null);
  const snapshot = state.snapshot;
  const epoch = state.frame?.runEpoch ?? "";
  // A view with no rows is not offered (owner, 2026-10-22): the toggle only while both have rows,
  // and the panel itself only while either has (its tab is hidden otherwise, SideTabs).
  const counts = useMemo(() => detectionViewCounts(state), [state]);
  const { recent: history, showToggle } = detectionView(pickedRecent, counts);
  const groups = useMemo(() => source === "all" || source === snapshot?.source ? groupDetections(snapshot, filter, history, segment).filter((g) => catalyst === "all" || g.episodes.some((e) => e.catalyst === catalyst)) : [], [snapshot, filter, history, segment, source, catalyst]);
  const currentPage = Math.min(page, Math.max(0, Math.ceil(groups.length / PAGE) - 1));
  const shown = groups.slice(currentPage * PAGE, (currentPage + 1) * PAGE);
  const detail = selection?.epoch === epoch ? snapshot?.episodes.find((e) => e.episode_id === selection.id) : undefined;
  const current = state.connected && !state.resyncing;
  const now = snapshot ? Math.max(snapshot.cutoff, snapshot.as_of) : 0;
  const applied = [filter !== "all", segment !== "all", source !== "all", catalyst !== "all"].filter(Boolean).length;
  // Pressing the selected row again deselects it: its details fold away and its marks leave the
  // chart (the chart keeps the ticker).
  const select = (e: DetectionEpisode, wasSelected: boolean) => {
    if (wasSelected) {
      setSelection(null);
      onClear?.();
      return;
    }
    setSelection({ epoch, id: e.episode_id });
    onPick(e.symbol, e.episode_id, epoch);
  };
  const view = (v: boolean) => {
    setHistory(v);
    setPage(0);
  };
  return (
    <aside aria-label="Signals" className="flex min-h-0 min-w-0 flex-col gap-2 rounded-xl bg-card p-2.5">
      <div className="flex items-center gap-2 px-1">
        <span className="rounded bg-bg-raised px-1.5 py-px text-[10px] font-medium text-muted-foreground">Experimental</span>
        {snapshot && snapshot.source !== "live" && <span className={cn("rounded px-1.5 py-px text-[10px] font-semibold", snapshot.source === "fixture" ? "bg-warning/15 text-warning" : "bg-bg-raised text-muted-foreground")}>{snapshot.source === "fixture" ? "FIXTURE / DEMO" : snapshot.source.toUpperCase()}</span>}
        <Reveal show={showToggle} axis="x" className="ml-auto">
          <div role="group" aria-label="Show" className="inline-flex gap-0.5 rounded-[9px] bg-background p-0.5">
            <button type="button" aria-pressed={!history} onClick={() => view(false)} className={cn("h-6 rounded-[7px] px-2 text-[11px] whitespace-nowrap outline-none transition-colors duration-150 focus-visible:ring-2 focus-visible:ring-ring motion-reduce:transition-none", !history ? SELECTED : UNSELECTED)}>Active</button>
            <button type="button" aria-pressed={history} onClick={() => view(true)} className={cn("h-6 rounded-[7px] px-2 text-[11px] whitespace-nowrap outline-none transition-colors duration-150 focus-visible:ring-2 focus-visible:ring-ring motion-reduce:transition-none", history ? SELECTED : UNSELECTED)}>Recent</button>
          </div>
        </Reveal>
      </div>
      {/* The canvas's "connection lost" state: the rows stay, dimmed and still, as last known. */}
      {!state.connected && (
        <p role="status" className="m-0 flex items-center gap-1.5 px-1 text-[11px] text-warning">
          <span className="size-1.5 rounded-full bg-warning" aria-hidden />
          {state.resyncing ? "Resynchronizing" : "Reconnecting"}{snapshot ? ` · last ${hhmm(snapshot.cutoff)}` : ""}
        </p>
      )}
      {state.coverageLost && <p role="alert" className="m-0 px-1 text-[11px] text-warning">Detection records were lost. Current state is recoverable; this history cannot establish complete research coverage.</p>}
      {state.error && state.error !== "transition_history_incomplete" && <p role="alert" className="m-0 px-1 text-[11px] text-critical">{words(state.error)}</p>}
      {snapshot?.unavailable.map((r) => <p key={r.reason} role="status" className="m-0 px-1 text-[11px] text-warning">Assessment unavailable: {words(r.reason)}{r.symbols ? ` · ${r.symbols} symbols` : ""}</p>)}
      <Collapsible>
        <CollapsibleTrigger className={cn(DISCLOSURE, "px-1")}>Filters{applied > 0 ? ` · ${applied} applied` : ""}</CollapsibleTrigger>
        <CollapsibleContent>
          <div className="flex flex-wrap gap-1.5 pt-1.5">
            <OptionSelect size="sm" aria-label="Detection family" options={FAMILY_OPTIONS} value={filter} onValueChange={(v) => { setFilter(v); setPage(0); }} className="w-full" />
            <OptionSelect size="sm" aria-label="Detection session" options={SESSION_OPTIONS} value={segment} onValueChange={(v) => { setSegment(v); setPage(0); }} />
            <OptionSelect size="sm" aria-label="Detection source filter" options={SOURCE_OPTIONS} value={source} onValueChange={(v) => { setSource(v); setPage(0); }} />
            <OptionSelect size="sm" aria-label="Detection catalyst status" options={CATALYST_OPTIONS} value={catalyst} onValueChange={(v) => { setCatalyst(v); setPage(0); }} />
          </div>
          {snapshot && <Tip label={`${snapshot.policy_id}@${snapshot.policy_hash}`}><p className="m-0 truncate px-1 pt-1.5 text-[10px] text-text-faint">{snapshot.fidelity} · {snapshot.base_timeframe_secs}s bars · {snapshot.policy_id} · research {detectionPrice(snapshot.research_min_micros)}–{detectionPrice(snapshot.research_max_micros)} · no audio or push alerts</p></Tip>}
          {(state.frame?.history.dropped ?? 0) > 0 && <p className="m-0 px-1 pt-1 text-[10px] text-text-faint">Recent transport history is bounded; {state.frame!.history.dropped.toLocaleString()} older transitions are outside this view.</p>}
        </CollapsibleContent>
      </Collapsible>
      {groups.length === 0 ? (
        // Only the user's own filters can empty a shown view (an empty view is never offered),
        // so this says so; clearing them is right above.
        applied > 0 && snapshot ? (
          <div className="flex flex-col items-center justify-center gap-1 px-1 py-4 text-center">
            <p className="m-0 text-[13px]">Nothing {history ? "resolved" : "developing"} in these filters</p>
            <p className="m-0 font-mono text-[11px] text-text-faint">watching {snapshot.monitored_symbols.toLocaleString()} symbols · {band(snapshot.price_min_micros)}–{band(snapshot.price_max_micros)}</p>
          </div>
        ) : null
      ) : (
        <ScrollArea className={cn("transition-opacity duration-200 motion-reduce:transition-none", !state.connected && "opacity-60")}>
        <ul className="m-0 flex list-none flex-col gap-1 p-0">
          {shown.map((g) => {
            // A related episode picked from the details stays under its instrument's row.
            const selected = detail !== undefined && g.episodes.some((x) => x.episode_id === detail.episode_id);
            return (
              <li key={`${epoch}/${g.selected.instrument_id}/${g.selected.episode_id}`} className="flex flex-col gap-1">
                <SignalRow group={g} row={detectionRowView(g, now, current, hhmm)} selected={selected} onSelect={select} />
                {selected && detail && <DetectionDetails key={`${epoch}/${detail.episode_id}`} episode={detail} developments={g.episodes.length} onRelated={(id) => setSelection({ epoch, id })} availableIds={new Set(snapshot?.episodes.map((e) => e.episode_id))} />}
              </li>
            );
          })}
        </ul>
        </ScrollArea>
      )}
      {groups.length > PAGE && (
        <div className="flex items-center gap-2 px-1 text-[10px] text-text-faint">
          <button type="button" className={CONTROL} disabled={!currentPage} onClick={() => setPage(currentPage - 1)}>Previous</button>
          <span>{currentPage * PAGE + 1}–{Math.min((currentPage + 1) * PAGE, groups.length)} of {groups.length}</span>
          <button type="button" className={CONTROL} disabled={(currentPage + 1) * PAGE >= groups.length} onClick={() => setPage(currentPage + 1)}>Next</button>
        </div>
      )}
      {selection && !detail && <p className="m-0 px-1 text-[10px] text-text-faint">The selected episode is outside this run or bounded history. No substitute is shown.</p>}
    </aside>
  );
}

const SignalRow = memo(function SignalRow({ group, row: r, selected, onSelect }: { group: DetectionGroup; row: DetectionRowView; selected: boolean; onSelect: (e: DetectionEpisode, wasSelected: boolean) => void }) {
  const e = group.selected;
  const press = useTickerPress(e.symbol, () => onSelect(e, selected));
  return (
    <button
      type="button"
      {...press}
      aria-pressed={selected}
      aria-label={`${r.aria}. ${selected ? "Shown on the chart; press to clear" : "Shows it on the chart"}`}
      className={cn(
        "flex w-full cursor-pointer touch-manipulation flex-col gap-1 rounded-lg bg-bg-row px-2.5 py-2 text-left font-mono outline-none select-none transition-[background-color,box-shadow,opacity] duration-200 hover:bg-bg-raised focus-visible:ring-2 focus-visible:ring-ring motion-reduce:transition-none",
        EDGE[r.stage],
        selected && "bg-bg-raised",
        r.dimmed && "opacity-60",
      )}
    >
      <span className="flex items-baseline gap-1.5">
        <span className="text-sm font-semibold">{r.symbol}</span>
        <span className="text-[13px] tabular-nums">{r.price}</span>
        {r.gain && <span className={cn("text-xs tabular-nums", GAIN[r.gainTone])}>{r.gain}</span>}
        {r.ross && <span className="size-1.5 self-center rounded-full bg-primary/70" aria-hidden />}
        {r.news && <span className="rounded bg-bg-raised px-1 py-px font-sans text-[9px] font-semibold text-muted-foreground">NEWS</span>}
        <span className="ml-auto text-[11px] text-text-faint tabular-nums">{r.ago}</span>
      </span>
      <span className="flex items-center gap-1.5 overflow-hidden font-sans text-xs whitespace-nowrap">
        <span className="shrink-0 text-muted-foreground">{r.family}</span>
        <span className="text-text-faint" aria-hidden>·</span>
        {r.pulse && <span className="size-1.5 shrink-0 animate-pulse rounded-full bg-primary motion-reduce:animate-none" aria-hidden />}
        <span className={cn("shrink-0", TONE[r.tone], r.attention && "font-semibold")}>{r.label}</span>
        {r.note && <span className="truncate font-mono text-muted-foreground tabular-nums">{r.note}</span>}
      </span>
      {r.meter !== null && <TriggerMeter value={r.meter} tone={r.tone} inset={false} />}
    </button>
  );
});

/** The selected row's evidence, timeline and provenance, folded under the row. */
function DetectionDetails({ episode: e, developments, onRelated, availableIds }: { episode: DetectionEpisode; developments: number; onRelated: (id: string) => void; availableIds: Set<string> }) {
  const [tab, setTab] = useState("evidence");
  const requirements = e.evidence.filter((x) => x.required);
  const measures = e.evidence.filter((x) => !x.required);
  const context = measures.filter((x) => /quiet_context|window\.(context|a_milli|n_base|dv_base|q_ppm|d_ppm|quiet_reference)|baseline\.(returns_n|status|floor_binding)|range\.(e_|floor_binding)|body\.e_milli|log_return_ppm|z_milli|upper_wick|close_location_milli|prior_range_relation|anchors\./.test(x.name));
  const diagnostics = measures.filter((x) => /(?:range|body)_expansion_15pct$/.test(x.name));
  return (
    <Collapsible className="rounded-lg bg-bg-row px-2.5 py-1.5">
      <CollapsibleTrigger className={DISCLOSURE}>Evidence & timeline</CollapsibleTrigger>
      <CollapsibleContent>
        <ScrollArea viewportClassName="max-h-[55vh]">
        <section aria-label={`${e.symbol} detection details`} className="flex flex-col gap-2 pt-2">
          <p className="m-0 text-[11px]"><strong className="font-mono">{e.symbol}</strong> ·{detectionFamilyLabel(e.family)} · <span className={tone(e)}>{detectionStateLabel(e.state)}</span> · {e.status}{e.restored ? " · restored history" : ""}{e.raw.warmup ? " · warmup" : ""}</p>
          <p className="m-0 text-[10px] text-text-faint">First {at(e.first_detected_at)} · price {e.price_stale ? "stale" : e.price_age_ms === null ? "age unknown" : `${(e.price_age_ms / 1000).toFixed(1)}s old`} · Ross: {e.ross_member === null ? "context unavailable" : e.ross_member ? "member" : "not admitted"} · News: {e.catalyst === "qualifying" ? "qualifying evidence" : e.catalyst === "none" ? "no qualifying evidence" : "coverage unknown"}{developments > 1 ? ` · ${developments} developments` : ""}</p>
          {e.raw.recognition?.in_research_band === false && <p className="m-0 text-[10px] text-warning">Recognition outside the research price band</p>}
          {e.reason && <p className={cn("m-0 text-[10px]", tone(e))}>{words(e.reason)}</p>}
          <div className="flex flex-wrap gap-1">{["evidence", "timeline", "provenance"].map((t) => <button type="button" key={t} aria-pressed={tab === t} className={cn(CONTROL, "transition-colors duration-150 motion-reduce:transition-none", tab === t ? SELECTED : UNSELECTED)} onClick={() => setTab(t)}>{words(t)}</button>)}</div>
          {tab === "evidence" && <>
            <p className="m-0 text-[10px] text-text-faint">Geometry is separate from requirements. No probability or execution guarantee is implied. Archived NBBO / spread and announcement authenticity are not established by this detector.</p>
            {e.level && <p className="m-0 font-mono text-[11px]">{e.level.kind === "range_high" ? "Range high" : "T"} {detectionPrice(e.level.trigger)} · I {detectionPrice(e.level.invalidation)} · version {e.level.version}<br /><span className="text-text-faint">Known {at(e.level.known_at)} · armed {at(e.level.armed_at)}</span></p>}
            <ul className="m-0 list-none space-y-1 p-0 text-[11px]">{requirements.map((c, i) => <li key={`${c.name}/${i}`}><span className={c.state === "PASS" ? "text-good" : c.state === "FAIL" ? "text-critical" : "text-warning"}>{c.state}</span> · {words(c.name)} · {measured(c)}{c.rule && <span className="text-text-faint"> · rule {c.rule} {c.units}</span>}{c.reason && <span className="text-text-faint"> · {words(c.reason)}</span>}</li>)}</ul>
            {diagnostics.length > 0 && <div className="rounded-md bg-card p-2 text-[11px]"><p className="m-0 mb-1 text-text-faint">15% candle expansion · diagnostic only · not a 15% stock return</p>{diagnostics.map((c) => <p className="m-0 font-mono" key={c.name}>{words(c.name.replace("recognition.", ""))}: {measured(c)}</p>)}</div>}
            <Collapsible><CollapsibleTrigger className={DISCLOSURE}>Context, expansion, anchors & participation</CollapsibleTrigger><CollapsibleContent><dl className="m-0 mt-2 grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)] gap-x-2 gap-y-1 text-[10px]">{context.map((c, i) => <div key={`${c.name}/${i}`} className="contents"><dt className="break-words text-text-faint">{words(c.name)}</dt><dd className="m-0 break-words font-mono">{measured(c)}</dd></div>)}</dl></CollapsibleContent></Collapsible>
            <Collapsible><CollapsibleTrigger className={DISCLOSURE}>All {measures.length} engine measurements</CollapsibleTrigger><CollapsibleContent><dl className="m-0 mt-2 space-y-1 text-[10px]">{measures.map((c, i) => <div key={`${c.name}/${i}`}><dt className="text-text-faint">{words(c.name)}</dt><dd className="m-0 font-mono">{measured(c)} · as of {at(c.as_of)}</dd></div>)}</dl></CollapsibleContent></Collapsible>
          </>}
          {tab === "timeline" && <><p className="m-0 text-[10px] text-text-faint">Immutable observed transitions; this window is bounded. Restored records are history.</p>{e.timeline.length === 0 ? <p className="m-0 text-xs text-text-faint">Earlier transitions are not in this transport window.</p> : <ol className="m-0 space-y-2 pl-4 text-[11px]">{e.timeline.map((t) => <li key={t.transition_id}><span className="font-mono">{at(t.observed_at)}</span> · {detectionStateLabel(t.state)} · {t.status}{t.restored ? " · restored history" : ""}{t.warmup ? " · warmup" : ""}<br /><span className="text-text-faint">{words(t.reason)} · cutoff {at(t.input_cutoff)}{t.tested_level_version !== null ? ` · tested level v${t.tested_level_version}` : ""}{t.supersedes ? ` · retracts ${t.supersedes}` : ""}</span></li>)}</ol>}</>}
          {tab === "provenance" && <div className="space-y-1 break-all font-mono text-[10px] text-text-faint"><p className="m-0">{e.source} · {e.fidelity} · {e.coverage}</p><p className="m-0">{e.trading_date} · {e.segment}</p><p className="m-0">Episode {e.episode_id}</p><p className="m-0">{e.policy_id}@{e.policy_hash} · {e.variant}</p><p className="m-0">Bar end {at(e.bar_end)} · input cutoff {at(e.input_cutoff)}</p>{e.raw.levels.map((l) => <p className="m-0" key={`${l.level_id}/${l.version}`}>{l.kind} · {l.level_id} v{l.version} · known {at(l.known_at)} · {words(l.reason)}</p>)}</div>}
          {e.related_episode_ids.length > 0 && <div className="flex flex-wrap gap-1"><span className="text-[10px] text-text-faint">Related:</span>{e.related_episode_ids.map((id) => <Tip key={id} label={id}><button type="button" disabled={!availableIds.has(id)} className="max-w-full truncate rounded text-[10px] text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring disabled:text-text-faint" onClick={() => onRelated(id)}>{id}{!availableIds.has(id) ? " (outside this history)" : ""}</button></Tip>)}</div>}
        </section>
        </ScrollArea>
      </CollapsibleContent>
    </Collapsible>
  );
}
