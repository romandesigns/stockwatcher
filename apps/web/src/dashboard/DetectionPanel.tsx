import { formatEtMillis } from "@stockwatcher/chart-core";
import { cn } from "@stockwatcher/chart-web/ui";
import { DETECTION_FILTERS, detectionBlocked, detectionDistance, detectionEvidenceValue, detectionFamilyLabel, detectionPrice, detectionStateLabel, groupDetections, type DetectionChannel, type DetectionEpisode, type DetectionEvidence, type DetectionFilter, type DetectionGroup } from "@stockwatcher/shared";
import { memo, useMemo, useState } from "react";
import { useDetection } from "../feed/useDetection.ts";
import { useTickerPress } from "../lib/useTickerPress.ts";
import { TickerLabel } from "../lib/TickerLabel.tsx";

const at = (ns: number | null) => ns === null || ns === 0 ? "—" : `${formatEtMillis(ns / 1_000_000)} ET`;
const words = (s: string) => s.replace(/[_.]/g, " ");
const CONTROL = "min-h-8 rounded-md bg-bg-row px-2 text-[11px] text-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring";
const tone = (e: DetectionEpisode) => ["invalidated", "rejected", "interrupted"].includes(e.state) ? "text-critical" : ["trigger_observed", "confirmed", "recognized", "retained"].includes(e.state) ? "text-good" : ["armed", "crossing_provisional", "crossing_wick"].includes(e.state) ? "text-warning" : "text-primary";
const measured = (c: DetectionEvidence): string => {
  if (typeof c.value !== "number") return detectionEvidenceValue(c);
  if (c.units === "UTC ns") return at(c.value);
  if (/(?:depth_milli|coverage_milli|close_location_milli|a_milli)$/.test(c.name)) return `${(c.value / 10).toFixed(2)}%`;
  if (c.units === "thousandths" || c.units === "milli") return `${(c.value / 1000).toFixed(3)}${/z_milli$/.test(c.name) ? " Z" : "×"}`;
  if (/recognition\.(?:range|body)\.(?:r|b|median|floor)$/.test(c.name) || /upper_wick$/.test(c.name)) return detectionPrice(c.value);
  return detectionEvidenceValue(c);
};

/** One event-discovery surface. All state/checks come from Rust; filters, pagination and row
 * selection never change what is monitored or recompute a detector from chart candles. */
export function DetectionPanel({ channel, onPick }: { channel: DetectionChannel | undefined; onPick: (symbol: string, episodeId: string, runEpoch: string) => void }) {
  const state = useDetection(channel);
  const [filter, setFilter] = useState<DetectionFilter>("all");
  const [history, setHistory] = useState(false);
  const [segment, setSegment] = useState("all");
  const [source, setSource] = useState("all");
  const [catalyst, setCatalyst] = useState("all");
  const [page, setPage] = useState(0);
  const [selection, setSelection] = useState<{ epoch: string; id: string } | null>(null);
  const snapshot = state.snapshot;
  const epoch = state.frame?.runEpoch ?? "";
  const groups = useMemo(() => source === "all" || source === snapshot?.source ? groupDetections(snapshot, filter, history, segment).filter((g) => catalyst === "all" || g.episodes.some((e) => e.catalyst === catalyst)) : [], [snapshot, filter, history, segment, source, catalyst]);
  const currentPage = Math.min(page, Math.max(0, Math.ceil(groups.length / 40) - 1));
  const shown = groups.slice(currentPage * 40, (currentPage + 1) * 40);
  const detail = selection?.epoch === epoch ? snapshot?.episodes.find((e) => e.episode_id === selection.id) : undefined;
  const newestResolution = snapshot?.episodes.filter((e) => e.terminal && e.default_visible).sort((a, b) => (b.resolved_at ?? 0) - (a.resolved_at ?? 0))[0];
  const select = (e: DetectionEpisode, chart: boolean) => { setSelection({ epoch, id: e.episode_id }); if (chart) onPick(e.symbol, e.episode_id, epoch); };
  return (
    <aside aria-labelledby="detection-title" className="flex min-h-0 min-w-0 flex-col gap-2 rounded-xl bg-card p-3">
      <div className="flex flex-wrap items-center gap-2">
        <h2 id="detection-title" className="m-0 text-sm font-semibold">Detection Panel</h2>
        <span className="rounded bg-primary/10 px-1.5 py-0.5 text-[9px] font-semibold text-primary">EXPERIMENTAL</span>
        <span className={cn("ml-auto rounded px-1.5 py-0.5 text-[10px] font-semibold", snapshot?.source === "fixture" ? "bg-warning/15 text-warning" : "bg-bg-row text-muted-foreground")}>{snapshot?.source === "fixture" ? "FIXTURE / DEMO" : (snapshot?.source ?? state.frame?.source ?? "source pending").toUpperCase()}</span>
      </div>
      <p className="m-0 text-[11px] text-text-faint">{snapshot ? `${detectionPrice(snapshot.price_min_micros)}–${detectionPrice(snapshot.price_max_micros)} · ${snapshot.base_timeframe_secs}s structural bars · ${snapshot.episodes[0]?.trading_date ?? "session pending"}` : "Independent event discovery · no audio or push alerts"}</p>
      {snapshot && <p className="m-0 truncate text-[10px] text-text-faint" title={`${snapshot.policy_id}@${snapshot.policy_hash}`}>{snapshot.fidelity} · {snapshot.policy_id} · research {detectionPrice(snapshot.research_min_micros)}–{detectionPrice(snapshot.research_max_micros)}</p>}
      {!state.connected && state.enabled !== null && <p role="status" className="m-0 text-[11px] text-warning">{state.resyncing ? "Resynchronizing — last known observations" : "Reconnecting — last known observations"}</p>}
      {state.coverageLost && <p role="alert" className="m-0 text-[11px] text-warning">Detection records were lost. Current state is recoverable; this history cannot establish complete research coverage.</p>}
      {(state.frame?.history.dropped ?? 0) > 0 && <p className="m-0 text-[10px] text-text-faint">Recent transport history is bounded; {state.frame!.history.dropped.toLocaleString()} older transitions are outside this view.</p>}
      {state.error && state.error !== "transition_history_incomplete" && <p role="alert" className="m-0 text-[11px] text-critical">{words(state.error)}</p>}
      {snapshot?.unavailable.map((r) => <p key={r.reason} role="status" className="m-0 text-[11px] text-warning">Assessment unavailable: {words(r.reason)}{r.symbols ? ` · ${r.symbols} symbols` : ""}</p>)}
      {newestResolution && !history && <RecentDetection key={`${epoch}/${newestResolution.episode_id}`} episode={newestResolution} onSelect={() => { setHistory(true); setPage(0); select(newestResolution, false); }} />}
      <div className="flex flex-wrap gap-1.5">
        <select aria-label="Detection family" value={filter} onChange={(e) => { setFilter(e.target.value as DetectionFilter); setPage(0); }} className={cn(CONTROL, "w-full")}>{DETECTION_FILTERS.map((f) => <option key={f.id} value={f.id}>{f.label}</option>)}</select>
        <select aria-label="Detection session" value={segment} onChange={(e) => { setSegment(e.target.value); setPage(0); }} className={CONTROL}><option value="all">All sessions</option><option value="pre">Premarket</option><option value="regular">Regular</option><option value="after">After-hours</option></select>
        <select aria-label="Detection source filter" value={source} onChange={(e) => { setSource(e.target.value); setPage(0); }} className={CONTROL}><option value="all">All sources</option><option value="live">Live</option><option value="replay">Replay</option><option value="fixture">Fixture / demo</option></select>
        <select aria-label="Detection catalyst status" value={catalyst} onChange={(e) => { setCatalyst(e.target.value); setPage(0); }} className={CONTROL}><option value="all">All catalyst statuses</option><option value="qualifying">Qualifying evidence</option><option value="none">No qualifying evidence</option><option value="unknown">Coverage unknown</option></select>
        <button type="button" aria-pressed={!history} onClick={() => { setHistory(false); setPage(0); }} className={cn(CONTROL, !history && "bg-primary/10 text-primary")}>Active</button>
        <button type="button" aria-pressed={history} onClick={() => { setHistory(true); setPage(0); }} className={cn(CONTROL, history && "bg-primary/10 text-primary")}>Recent history</button>
      </div>
      {!channel ? <p className="m-0 py-2 text-xs text-text-faint">This browser-only chart fixture has no detector engine. Connect to the StockWatcher server.</p> : state.enabled === false ? <p className="m-0 py-2 text-xs text-text-faint">Detection is off on this server.</p> : state.enabled === null ? <p className="m-0 py-2 text-xs text-text-faint">Connecting to detection…</p> : groups.length === 0 ? <p className="m-0 py-2 text-xs text-text-faint">{snapshot ? `${snapshot.monitored_symbols.toLocaleString()} reported symbols · no ${history ? "recent" : "active"} event in these filters. Baselines may still be warming.` : "Waiting for the detector's first clock and observations…"}</p> : (
        <ul className={cn("m-0 flex max-h-[55vh] list-none flex-col gap-1.5 overflow-y-auto p-0", !state.connected && "opacity-60")}>
          {shown.map((g) => <li key={`${epoch}/${g.selected.instrument_id}/${g.selected.episode_id}`}><DetectionRow group={g} fresh={state.connected && !state.resyncing} selected={detail?.episode_id === g.selected.episode_id} onSelect={select} /></li>)}
        </ul>
      )}
      {groups.length > 40 && <div className="flex items-center gap-2 text-[10px] text-text-faint"><button type="button" className={CONTROL} disabled={!currentPage} onClick={() => setPage(currentPage - 1)}>Previous</button><span>{currentPage * 40 + 1}–{Math.min((currentPage + 1) * 40, groups.length)} of {groups.length} instruments</span><button type="button" className={CONTROL} disabled={(currentPage + 1) * 40 >= groups.length} onClick={() => setPage(currentPage + 1)}>Next</button></div>}
      {detail && <DetectionDetails key={`${epoch}/${detail.episode_id}`} episode={detail} onClose={() => setSelection(null)} onPick={() => onPick(detail.symbol, detail.episode_id, epoch)} onRelated={(id) => setSelection({ epoch, id })} availableIds={new Set(snapshot?.episodes.map((e) => e.episode_id))} />}
      {selection && !detail && <p className="m-0 text-[10px] text-text-faint">The selected episode is outside this run or bounded history. No substitute is shown.</p>}
    </aside>
  );
}

const DetectionRow = memo(function DetectionRow({ group, fresh, selected, onSelect }: { group: DetectionGroup; fresh: boolean; selected: boolean; onSelect: (e: DetectionEpisode, chart: boolean) => void }) {
  const e = group.selected;
  const press = useTickerPress(e.symbol, () => onSelect(e, true));
  const blocked = detectionBlocked(e);
  const distance = fresh && !e.price_stale && !e.terminal && e.level ? detectionDistance(e.price, e.level.trigger, "trigger") : null;
  const invalidation = fresh && !e.price_stale && !e.terminal && e.level ? detectionDistance(e.price, e.level.invalidation, "invalidation") : null;
  return <div className={cn("flex flex-col gap-1 rounded-lg bg-bg-row p-2.5", selected && "shadow-[inset_2px_0_0_var(--accent)]")}>
    <button type="button" {...press} aria-label={`${e.symbol}, ${detectionFamilyLabel(e.family)}, ${detectionStateLabel(e.state)}. Load chart and evidence`} className="flex cursor-pointer touch-manipulation flex-col gap-1 text-left outline-none select-none focus-visible:ring-2 focus-visible:ring-ring">
      <span className="flex items-baseline gap-2"><strong className="font-mono text-sm">{e.symbol}</strong><span className="font-mono text-xs">{detectionPrice(e.price)}</span><span className="ml-auto text-[10px] text-text-faint">{e.price_stale ? "stale price" : e.price_age_ms === null ? "price age unknown" : `${(e.price_age_ms / 1000).toFixed(1)}s age`}</span></span>
      <span className="text-[11px] text-muted-foreground">{detectionFamilyLabel(e.family)} <span className={tone(e)}>· {detectionStateLabel(e.state)}</span></span>
      {fresh && !e.price_stale && e.near_trigger === true && <span className="text-[11px] text-warning">Near trigger{blocked.length ? " — blocked; see required checks" : " · geometry only"}</span>}
      <span className="text-[10px] text-text-faint">First {at(e.first_detected_at)} · change {at(group.lastMaterialAt)}</span>
      {distance && <span className="font-mono text-[11px] text-warning">{e.level?.kind === "range_high" ? "Range high" : "Trigger"} {detectionPrice(e.level!.trigger)} · {distance.atOrAbove ? "at / above level; see confirmation state" : `${detectionPrice(Math.round(distance.dollars * 1_000_000))} / ${distance.percent.toFixed(2)}% below`}</span>}
      {invalidation && <span className="font-mono text-[10px] text-muted-foreground">Invalidation {detectionPrice(e.level!.invalidation)} · {detectionPrice(Math.round(invalidation.dollars * 1_000_000))} / {invalidation.percent.toFixed(2)}% {invalidation.atOrAbove ? "below price" : "above price"}</span>}
      {!distance && e.level && <span className="text-[10px] text-text-faint">Level {detectionPrice(e.level.trigger)} · current distance unavailable{e.terminal ? " for resolved setup" : ""}</span>}
      {blocked.length > 0 && <span className="text-[10px] text-muted-foreground">{blocked.slice(0, 3).map((c) => `${c.state}: ${words(c.name)}${c.reason ? ` (${words(c.reason)})` : ""}`).join(" · ")}{blocked.length > 3 ? ` · +${blocked.length - 3} checks` : ""}</span>}
      <span className="text-[10px] text-text-faint">Ross: {e.ross_member === null ? "context unavailable" : e.ross_member ? "member" : "not admitted"} · News: {e.catalyst === "qualifying" ? "qualifying evidence" : e.catalyst === "none" ? "no qualifying evidence" : "coverage unknown"}{group.episodes.length > 1 ? ` · ${group.episodes.length} developments` : ""}</span>
      {e.raw.recognition?.in_research_band === false && <span className="text-[10px] text-warning">Recognition outside the research price band</span>}
      {e.reason && <span className={cn("text-[10px]", tone(e))}>{words(e.reason)}</span>}
    </button>
    <button type="button" className="self-start rounded px-1 py-0.5 text-[10px] text-primary outline-none focus-visible:ring-2 focus-visible:ring-ring" onClick={() => onSelect(e, false)}>Evidence & timeline</button>
  </div>;
});

function RecentDetection({ episode: e, onSelect }: { episode: DetectionEpisode; onSelect: () => void }) {
  const press = useTickerPress(e.symbol, onSelect);
  return <button type="button" {...press} className="touch-manipulation rounded-lg bg-bg-row px-2 py-1.5 text-left text-[11px] text-muted-foreground select-none">Recent: {e.symbol} · {detectionStateLabel(e.state)} · {at(e.resolved_at)}</button>;
}

function DetectionDetails({ episode: e, onClose, onPick, onRelated, availableIds }: { episode: DetectionEpisode; onClose: () => void; onPick: () => void; onRelated: (id: string) => void; availableIds: Set<string> }) {
  const [tab, setTab] = useState("evidence");
  const requirements = e.evidence.filter((x) => x.required);
  const measures = e.evidence.filter((x) => !x.required);
  const context = measures.filter((x) => /quiet_context|window\.(context|a_milli|n_base|dv_base|q_ppm|d_ppm|quiet_reference)|baseline\.(returns_n|status|floor_binding)|range\.(e_|floor_binding)|body\.e_milli|log_return_ppm|z_milli|upper_wick|close_location_milli|prior_range_relation|anchors\./.test(x.name));
  const diagnostics = measures.filter((x) => /(?:range|body)_expansion_15pct$/.test(x.name));
  return <section aria-label={`${e.symbol} detection details`} className="flex max-h-[65vh] min-h-0 flex-col gap-2 overflow-y-auto rounded-lg bg-bg-row p-2.5">
    <div className="flex items-center gap-2"><h3 className="m-0 text-xs font-semibold"><TickerLabel symbol={e.symbol} onTap={onPick} /> · {detectionFamilyLabel(e.family)}</h3><button type="button" className={cn(CONTROL, "ml-auto")} aria-label="Close detection details" onClick={onClose}>Close</button></div>
    <p className={cn("m-0 text-xs", tone(e))}>{detectionStateLabel(e.state)} · {e.status}{e.restored ? " · restored history" : ""}{e.raw.warmup ? " · warmup" : ""} · experimental observation</p>
    <div className="flex flex-wrap gap-1">{["evidence", "timeline", "provenance"].map((t) => <button type="button" key={t} aria-pressed={tab === t} className={cn(CONTROL, tab === t && "text-primary")} onClick={() => setTab(t)}>{words(t)}</button>)}<button type="button" className={cn(CONTROL, "ml-auto text-primary")} onClick={onPick}>Open chart</button></div>
    {tab === "evidence" && <>
      <p className="m-0 text-[10px] text-text-faint">Geometry is separate from requirements. No probability or execution guarantee is implied.</p>
      <p className="m-0 text-[10px] text-text-faint">Trade participation is measured separately from buying pressure. Archived NBBO / spread and announcement authenticity or favorability are not established by this detector.</p>
      {e.level && <p className="m-0 font-mono text-[11px]">{e.level.kind === "range_high" ? "Range high" : "T"} {detectionPrice(e.level.trigger)} · I {detectionPrice(e.level.invalidation)} · version {e.level.version}<br /><span className="text-text-faint">Known {at(e.level.known_at)} · armed {at(e.level.armed_at)}</span></p>}
      <ul className="m-0 list-none space-y-1 p-0 text-[11px]">{requirements.map((c, i) => <li key={`${c.name}/${i}`}><span className={c.state === "PASS" ? "text-good" : c.state === "FAIL" ? "text-critical" : "text-warning"}>{c.state}</span> · {words(c.name)} · {measured(c)}{c.rule && <span className="text-text-faint"> · rule {c.rule} {c.units}</span>}{c.reason && <span className="text-text-faint"> · {words(c.reason)}</span>}</li>)}</ul>
      {diagnostics.length > 0 && <div className="rounded-md bg-card p-2 text-[11px]"><p className="m-0 mb-1 text-text-faint">15% candle expansion · diagnostic only · not a 15% stock return</p>{diagnostics.map((c) => <p className="m-0 font-mono" key={c.name}>{words(c.name.replace("recognition.", ""))}: {measured(c)}</p>)}</div>}
      <details open><summary className="cursor-pointer text-[11px] text-muted-foreground">Context, expansion, anchors & participation</summary><dl className="m-0 mt-2 grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)] gap-x-2 gap-y-1 text-[10px]">{context.map((c, i) => <div key={`${c.name}/${i}`} className="contents"><dt className="break-words text-text-faint">{words(c.name)}</dt><dd className="m-0 break-words font-mono">{measured(c)}</dd></div>)}</dl></details>
      <details><summary className="cursor-pointer text-[11px] text-muted-foreground">All {measures.length} engine measurements</summary><dl className="m-0 mt-2 space-y-1 text-[10px]">{measures.map((c, i) => <div key={`${c.name}/${i}`}><dt className="text-text-faint">{words(c.name)}</dt><dd className="m-0 font-mono">{measured(c)} · as of {at(c.as_of)}</dd></div>)}</dl></details>
    </>}
    {tab === "timeline" && <><p className="m-0 text-[10px] text-text-faint">Immutable observed transitions; this window is bounded. Restored records are history.</p>{e.timeline.length === 0 ? <p className="m-0 text-xs text-text-faint">Earlier transitions are not in this transport window.</p> : <ol className="m-0 space-y-2 pl-4 text-[11px]">{e.timeline.map((t) => <li key={t.transition_id}><span className="font-mono">{at(t.observed_at)}</span> · {detectionStateLabel(t.state)} · {t.status}{t.restored ? " · restored history" : ""}{t.warmup ? " · warmup" : ""}<br /><span className="text-text-faint">{words(t.reason)} · cutoff {at(t.input_cutoff)}{t.tested_level_version !== null ? ` · tested level v${t.tested_level_version}` : ""}{t.supersedes ? ` · retracts ${t.supersedes}` : ""}</span></li>)}</ol>}</>}
    {tab === "provenance" && <div className="space-y-1 break-all font-mono text-[10px] text-text-faint"><p className="m-0">{e.source} · {e.fidelity} · {e.coverage}</p><p className="m-0">{e.trading_date} · {e.segment}</p><p className="m-0">Episode {e.episode_id}</p><p className="m-0">{e.policy_id}@{e.policy_hash} · {e.variant}</p><p className="m-0">Bar end {at(e.bar_end)} · input cutoff {at(e.input_cutoff)}</p>{e.raw.levels.map((l) => <p className="m-0" key={`${l.level_id}/${l.version}`}>{l.kind} · {l.level_id} v{l.version} · known {at(l.known_at)} · {words(l.reason)}</p>)}</div>}
    {e.related_episode_ids.length > 0 && <div className="flex flex-wrap gap-1"><span className="text-[10px] text-text-faint">Related:</span>{e.related_episode_ids.map((id) => <button type="button" key={id} disabled={!availableIds.has(id)} className="max-w-full truncate text-[10px] text-primary disabled:text-text-faint" title={id} onClick={() => onRelated(id)}>{id}{!availableIds.has(id) ? " (outside this history)" : ""}</button>)}</div>}
  </section>;
}
