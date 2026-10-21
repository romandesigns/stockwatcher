import { formatEtMillis } from "@stockwatcher/chart-core";
import { Sheet, UI } from "@stockwatcher/chart-native";
import {
  DETECTION_FILTERS,
  detectionBlocked,
  detectionDistance,
  detectionEvidenceValue,
  detectionFamilyLabel,
  detectionPrice,
  detectionStateLabel,
  groupDetections,
  type DetectionChannel,
  type DetectionEpisode,
  type DetectionFilter,
  type DetectionGroup,
  type DetectionSnapshot,
} from "@stockwatcher/shared";
import { memo, useState } from "react";
import { Pressable, ScrollView, StyleSheet, Text, View } from "react-native";
import Animated, { FadeIn, FadeOut, LinearTransition, ReduceMotion } from "react-native-reanimated";

import { useDetection } from "../feed/hooks";
import { fonts } from "../theme";
import { useTickerPress } from "../ui/useTickerPress";

// Bound native view work without limiting discovery: the count and paging retain every group.
const PAGE = 12;
const at = (ns: number) => Number.isFinite(ns) && ns > 0 ? formatEtMillis(Math.floor(ns / 1_000_000)) : "unavailable";
const words = (value: string) => value.replace(/_/g, " ");

function stateColor(state: string): string {
  if (/invalidated|rejected|retracted/.test(state)) return UI.critical;
  if (/trigger_observed|triggered|confirmed|retained/.test(state)) return UI.good;
  if (/crossing|armed|extended|interrupted/.test(state)) return UI.warning;
  if (/building|forming|recognized|ignited/.test(state)) return UI.accent;
  return UI.textDim;
}

/** The phone's event-discovery surface. The shared projection formats authoritative Rust
 * episodes; this layout does not derive detections from chart candles or Ross membership. */
export function DetectionPanel({ channel, onPick, reducedMotion }: { channel: DetectionChannel; onPick: (symbol: string, episodeId?: string, runEpoch?: string) => void; reducedMotion: boolean }) {
  const state = useDetection(channel);
  const [filter, setFilter] = useState<DetectionFilter>("all");
  const [history, setHistory] = useState(false);
  const [segment, setSegment] = useState("all");
  const [source, setSource] = useState("all");
  const [catalyst, setCatalyst] = useState("all");
  const [page, setPage] = useState(0);
  const [filtersOpen, setFiltersOpen] = useState(false);
  const [detail, setDetail] = useState<{ id: string; epoch: string } | null>(null);
  const snapshot = state.snapshot;
  const epoch = state.frame?.runEpoch ?? "";
  const groups = groupDetections(snapshot, filter, history, segment)
    .filter((g) => catalyst === "all" || g.selected.catalyst === catalyst)
    .filter(() => source === "all" || state.frame?.source === source);
  const pages = Math.max(1, Math.ceil(groups.length / PAGE));
  const actualPage = Math.min(page, pages - 1);
  const shown = groups.slice(actualPage * PAGE, (actualPage + 1) * PAGE);
  const selected = detail && detail.epoch === state.frame?.runEpoch ? snapshot?.episodes.find((e) => e.episode_id === detail.id) : undefined;
  const segments = [...new Set(snapshot?.episodes.map((e) => e.segment) ?? [])].sort();
  const terminalCount = snapshot?.episodes.filter((e) => e.terminal).length ?? 0;
  const latestResolution = groupDetections(snapshot, "all", true)[0]?.selected;
  const stale = !state.connected || state.resyncing;
  const pick = (episode: DetectionEpisode, capturedEpoch: string) => onPick(episode.symbol, episode.episode_id, capturedEpoch);
  return (
    <View style={styles.card} accessibilityLabel="Detection Panel">
      <View style={styles.line}>
        <Text accessibilityRole="header" style={styles.title}>Detection Panel</Text>
        <Text style={styles.experimental}>Experimental · alerts off</Text>
      </View>
      <Text style={styles.meta}>{state.frame?.source ?? "Source pending"} · {snapshot?.fidelity ? words(snapshot.fidelity) : "Fidelity pending"} · canonical 15s · times Eastern</Text>
      {snapshot && <Text style={styles.meta}>Active {detectionPrice(snapshot.price_min_micros)}–{detectionPrice(snapshot.price_max_micros)} · research {detectionPrice(snapshot.research_min_micros)}–{detectionPrice(snapshot.research_max_micros)}</Text>}
      {state.enabled === false ? <Text style={styles.empty}>Off on this server</Text> : state.enabled === null ? <Text style={styles.empty}>Connecting to detection state…</Text> : (
        <>
          {stale && <Text accessibilityRole="alert" style={styles.warning}>{state.resyncing ? "Resyncing · current assessment unavailable" : "Reconnecting · last received state"}{snapshot ? ` · ${at(snapshot.cutoff)}` : ""}</Text>}
          {state.coverageLost && <Text accessibilityRole="alert" style={styles.warning}>Transition history is incomplete. Current rows remain available; this is not a complete research record.</Text>}
          <FilterStrip label="Detection family" options={DETECTION_FILTERS} value={filter} onChange={(id) => { setFilter(id as DetectionFilter); setPage(0); }} />
          <View style={styles.line}>
            <View style={{ flex: 1 }}><FilterStrip label="Episode history" options={[{ id: "active", label: "Active" }, { id: "recent", label: `Recent history (${terminalCount})` }]} value={history ? "recent" : "active"} onChange={(id) => { setHistory(id === "recent"); setPage(0); }} /></View>
            <Action label={`Filters${segment !== "all" || source !== "all" || catalyst !== "all" ? " · applied" : ""}`} onPress={() => setFiltersOpen(true)} />
          </View>
          {groups.length > 0 && <Text style={styles.meta}>{groups.length} instruments · showing {actualPage * PAGE + 1}–{Math.min(groups.length, (actualPage + 1) * PAGE)}</Text>}
          {!history && latestResolution && <RecentResolution key={`${epoch}/${latestResolution.episode_id}`} episode={latestResolution} onPick={() => pick(latestResolution, epoch)} onDetail={() => setDetail({ id: latestResolution.episode_id, epoch })} />}
          {groups.length === 0 ? (
            <View style={styles.emptyBox}>
              <Text style={styles.empty}>{!snapshot ? "Waiting for the server's first snapshot…" : history ? "No retained episodes match these filters" : snapshot.monitored_symbols === 0 ? "Warming · no admissible symbol history yet" : "No active detections match these filters"}</Text>
              {snapshot?.unavailable.map((r) => <Text key={r.reason} style={styles.meta}>{r.symbols} · {words(r.reason)}</Text>)}
              <Text style={styles.meta}>Quiet-only stocks and the 1.15× diagnostics do not create discovery rows.</Text>
            </View>
          ) : (
            <View style={stale ? styles.stale : undefined}>
              {/* A reset remounts the recognizer and cancels its delayed tap/hold. Price updates
                  keep this key so ordinary ticks never abandon a deliberate press. */}
              {shown.map((group) => <DetectionRow key={`${epoch}/${group.selected.instrument_id}/${group.symbol}`} group={group} reducedMotion={reducedMotion} currentAvailable={!stale} onPick={() => pick(group.selected, epoch)} onDetail={() => setDetail({ id: group.selected.episode_id, epoch })} />)}
            </View>
          )}
          {groups.length > PAGE && (
            <View style={styles.line}>
              <Action label="Previous" disabled={actualPage === 0} onPress={() => setPage(actualPage - 1)} />
              <Text style={styles.meta}>{actualPage * PAGE + 1}–{Math.min(groups.length, (actualPage + 1) * PAGE)} of {groups.length}</Text>
              <Action label="Next" disabled={actualPage + 1 === pages} onPress={() => setPage(actualPage + 1)} />
            </View>
          )}
        </>
      )}
      <Sheet open={filtersOpen} onClose={() => setFiltersOpen(false)} title="Detection filters" reducedMotion={reducedMotion}>
        <View style={styles.details}>
          <Text style={styles.section}>Session</Text>
          <FilterStrip label="Session" options={[{ id: "all", label: "All sessions" }, ...segments.map((id) => ({ id, label: words(id) }))]} value={segment} onChange={(id) => { setSegment(id); setPage(0); }} />
          <Text style={styles.section}>Source · this connection reads {state.frame?.source ?? "the selected market"}</Text>
          <FilterStrip label="Source" options={[{ id: "all", label: "Current source" }, { id: "live", label: "Live" }, { id: "replay", label: "Replay" }, { id: "fixture", label: "Fixture" }]} value={source} onChange={(id) => { setSource(id); setPage(0); }} />
          <Text style={styles.section}>Catalyst evidence</Text>
          <FilterStrip label="Catalyst status" options={[{ id: "all", label: "Any status" }, { id: "qualifying", label: "Qualifying evidence" }, { id: "none", label: "No qualifying evidence" }, { id: "unknown", label: "Unknown coverage" }]} value={catalyst} onChange={(id) => { setCatalyst(id); setPage(0); }} />
          <Action label="Done" onPress={() => setFiltersOpen(false)} />
        </View>
      </Sheet>
      <Sheet open={detail !== null} onClose={() => setDetail(null)} title={selected ? "Detection details" : "Detection unavailable"} badge={selected && detail ? <DetailTicker key={`${detail.epoch}/${selected.episode_id}`} episode={selected} onPick={() => { pick(selected, detail.epoch); setDetail(null); }} /> : undefined} reducedMotion={reducedMotion} tall>
        <ScrollView showsVerticalScrollIndicator={false} showsHorizontalScrollIndicator={false} contentContainerStyle={styles.details}>
          {selected && snapshot && detail ? <>
            {stale && <Text accessibilityRole="alert" style={styles.warning}>Last received state · current assessment unavailable</Text>}
            <Action label="Open chart with this episode" onPress={() => { pick(selected, detail.epoch); setDetail(null); }} />
            <DetectionEpisodeDetails episode={selected} snapshot={snapshot} currentAvailable={!stale} />
            {snapshot.episodes.filter((e) => e.symbol === selected.symbol && e.episode_id !== selected.episode_id).map((e) => <Action key={e.episode_id} label={`${detectionFamilyLabel(e.family)} · ${detectionStateLabel(e.state)} · ${at(e.first_detected_at)}`} onPress={() => setDetail({ id: e.episode_id, epoch: detail.epoch })} />)}
          </> : <Text style={styles.empty}>The source/run changed or this episode left bounded history. Select a current row to inspect its authoritative references.</Text>}
        </ScrollView>
      </Sheet>
    </View>
  );
}

function FilterStrip({ label, options, value, onChange }: { label: string; options: readonly { id: string; label: string }[]; value: string; onChange: (id: string) => void }) {
  return <ScrollView horizontal showsVerticalScrollIndicator={false} showsHorizontalScrollIndicator={false} accessibilityLabel={label} contentContainerStyle={styles.filters}>
    {options.map((item) => <Pressable key={item.id} accessibilityRole="button" accessibilityState={{ selected: value === item.id }} accessibilityLabel={`${label}: ${item.label}`} onPress={() => onChange(item.id)} style={[styles.filter, value === item.id && styles.selectedFilter]}><Text style={[styles.filterText, value === item.id && { color: UI.accent }]}>{item.label}</Text></Pressable>)}
  </ScrollView>;
}

/** Kept inside the epoch-keyed element so a removed detail cannot finish an old copy/tap. */
function DetailTicker({ episode, onPick }: { episode: DetectionEpisode; onPick: () => void }) {
  const press = useTickerPress(episode.symbol, onPick);
  return <Pressable accessibilityRole="button" accessibilityLabel={episode.symbol} accessibilityHint="Opens its chart. Tap, then press and hold to copy" {...press} style={styles.detailTicker}><Text style={styles.symbol}>{episode.symbol}</Text></Pressable>;
}

function RecentResolution({ episode: e, onPick, onDetail }: { episode: DetectionEpisode; onPick: () => void; onDetail: () => void }) {
  const press = useTickerPress(e.symbol, onPick);
  return <View style={styles.recent}>
    <Text style={styles.meta}>Latest resolution · retained in recent history</Text>
    <View style={styles.line}>
      <Pressable accessibilityRole="button" accessibilityLabel={`${e.symbol}. ${detectionStateLabel(e.state)}. ${words(e.reason ?? e.state)}`} accessibilityHint="Opens its episode on the chart. Tap, then press and hold to copy" {...press} style={styles.tickerPress}>
        <Text style={styles.symbol}>{e.symbol}</Text><Text style={[styles.state, { color: stateColor(e.state), flexShrink: 1 }]}>{detectionStateLabel(e.state)}</Text>
      </Pressable>
      <Action label="Details" onPress={onDetail} />
    </View>
    <Text style={styles.meta}>{e.trading_date} · {e.resolved_at === null ? at(e.last_material_transition_at) : at(e.resolved_at)} · {words(e.reason ?? e.state)}</Text>
  </View>;
}

const DetectionRow = memo(function DetectionRow({ group, reducedMotion, currentAvailable, onPick, onDetail }: { group: DetectionGroup; reducedMotion: boolean; currentAvailable: boolean; onPick: () => void; onDetail: () => void }) {
  const e = group.selected;
  const press = useTickerPress(e.symbol, onPick);
  const blocked = detectionBlocked(e);
  const motion = reducedMotion ? ReduceMotion.Always : ReduceMotion.System;
  return <Animated.View layout={LinearTransition.duration(160).reduceMotion(motion)} entering={FadeIn.duration(140).reduceMotion(motion)} exiting={FadeOut.duration(100).reduceMotion(motion)} style={styles.row}>
    <View style={styles.line}>
      <Pressable accessibilityRole="button" accessibilityLabel={`${e.symbol} ${detectionPrice(e.price)}. ${detectionFamilyLabel(e.family)}. ${detectionStateLabel(e.state)}.`} accessibilityHint="Opens this episode on the chart. Tap, then press and hold to copy the ticker" {...press} style={styles.tickerPress}>
        <Text style={styles.symbol}>{e.symbol}</Text><Text style={styles.price}>{detectionPrice(e.price)}</Text>
      </Pressable>
      <Action label="Details" onPress={onDetail} />
    </View>
    <Text style={[styles.state, { color: stateColor(e.state) }]}>{detectionFamilyLabel(e.family)} · {e.state === "near" && blocked.length > 0 ? "Near trigger — blocked" : detectionStateLabel(e.state)}{e.status === "provisional" ? " · provisional" : ""}</Text>
    <Text style={e.price_stale ? styles.warning : styles.meta}>{e.price_age_ms === null ? "Price age unavailable" : `Price age ${(e.price_age_ms / 1000).toFixed(1)}s`}{e.price_stale ? " · current assessment unavailable" : ""}{e.raw.warmup ? " · restored warmup" : ""}</Text>
    {e.raw.recognition?.in_research_band === false && <Text style={styles.meta}>Outside research band at recognition · active profile preserved</Text>}
    <Text style={styles.meta}>{e.trading_date} · first {at(e.first_detected_at)} · changed {at(e.last_material_transition_at)}{group.episodes.length > 1 ? ` · ${group.episodes.length} episodes` : ""}</Text>
    <Geometry episode={e} currentAvailable={currentAvailable} />
    {currentAvailable && !e.price_stale && e.near_trigger === true && <Text style={styles.warning}>Near trigger{blocked.length ? " — blocked; see required checks" : " · geometry only"}</Text>}
    {blocked.length > 0 && <Text style={styles.warning}>{blocked.slice(0, 3).map((c) => `${c.state} ${words(c.name)}${c.reason ? `: ${words(c.reason)}` : ""}`).join(" · ")}</Text>}
    {e.terminal && <Text style={[styles.meta, { color: stateColor(e.state) }]}>{e.resolved_at !== null ? `${at(e.resolved_at)} · ` : ""}{words(e.reason ?? e.state)}</Text>}
    <Text style={styles.meta}>Ross: {e.ross_member === null ? "unknown" : e.ross_member ? "member" : "not admitted"} · Catalyst: {words(e.catalyst)} · {words(e.coverage)}</Text>
  </Animated.View>;
});

function Geometry({ episode: e, currentAvailable = true }: { episode: DetectionEpisode; currentAvailable?: boolean }) {
  const trigger = detectionDistance(e.price_stale || !currentAvailable ? null : e.price, e.level?.trigger ?? null, "trigger");
  const invalidation = detectionDistance(e.price_stale || !currentAvailable ? null : e.price, e.level?.invalidation ?? null, "invalidation");
  return <View style={{ gap: 3 }}>
    <Text style={styles.geometry}>Trigger {detectionPrice(e.level?.trigger ?? null)}{trigger ? trigger.atOrAbove ? " · at / above reference" : ` · ${detectionPrice(Math.round(trigger.dollars * 1_000_000))} / ${trigger.percent.toFixed(2)}% away` : " · distance unavailable"}</Text>
    <Text style={styles.geometry}>Invalidation {detectionPrice(e.level?.invalidation ?? null)}{invalidation ? invalidation.atOrAbove ? ` · ${detectionPrice(Math.round(invalidation.dollars * 1_000_000))} / ${invalidation.percent.toFixed(2)}% below price` : " · price below reference" : " · distance unavailable"}</Text>
  </View>;
}

/** Also rendered beside the selected chart. Identity and version references survive timeframe
 * changes; neither a chart candle nor a user drawing supplies these levels. */
export function DetectionEpisodeDetails({ episode: e, snapshot, currentAvailable = true }: { episode: DetectionEpisode; snapshot: DetectionSnapshot; currentAvailable?: boolean }) {
  return <View style={styles.details}>
    <Text style={[styles.state, { color: stateColor(e.state) }]}>{detectionFamilyLabel(e.family)} · {detectionStateLabel(e.state)}</Text>
    <Text style={styles.meta}>Experimental · {e.source} · {words(e.fidelity)} · {words(e.coverage)}</Text>
    <Text style={styles.meta}>{e.trading_date} · {words(e.segment)} · canonical {snapshot.base_timeframe_secs}s</Text>
    <Text style={e.price_stale ? styles.warning : styles.meta}>{e.price_age_ms === null ? "Price age unavailable" : `Price age ${(e.price_age_ms / 1000).toFixed(1)}s`}{e.price_stale ? " · current assessment unavailable" : ""}{e.raw.warmup ? " · restored warmup" : ""}</Text>
    <Text style={styles.meta}>Context: {words(e.raw.context ?? "unavailable")}</Text>
    <Text selectable style={styles.identifier}>Episode {e.episode_id}</Text>
    <Text selectable style={styles.identifier}>Policy {e.policy_id}@{e.policy_hash}</Text>
    <Text style={styles.meta}>First recognized {at(e.first_detected_at)} · input cutoff {at(e.input_cutoff)}{e.bar_end !== null ? ` · bar end ${at(e.bar_end)}` : ""}</Text>
    {e.parent_episode_id && <Text selectable style={styles.identifier}>Parent {e.parent_episode_id}</Text>}
    {e.related_episode_ids.map((id) => <Text key={id} selectable style={styles.identifier}>Related {id}</Text>)}
    <Geometry episode={e} currentAvailable={currentAvailable} />
    <Text style={styles.meta}>Quote/spread evidence unavailable. Trigger and invalidation are structural references.</Text>
    <AnchorDetails episode={e} />
    <ExpansionDetails episode={e} />
    <Text style={styles.section}>Requirements and measurements</Text>
    <Text style={styles.meta}>Distance is geometry. PASS / FAIL / UNKNOWN describe actual requirements.</Text>
    {e.evidence.map((item, index) => <View key={`${item.name}/${index}`} style={styles.measurement}>
      <Text style={styles.body}>{words(item.name)} · {detectionEvidenceValue(item)}</Text>
      <Text style={[styles.meta, item.state === "FAIL" ? { color: UI.critical } : item.state === "UNKNOWN" ? { color: UI.warning } : undefined]}>{item.state ?? "Observation"}{item.required ? " · required" : " · diagnostic"}{item.rule ? ` · ${item.rule}` : ""}</Text>
      <Text style={styles.meta}>{words(item.source)} · as of {at(item.as_of)} · observed {at(item.observed_at)}</Text>
    </View>)}
    <Text style={styles.meta}>Body and range expansion are separate observations; 1.15× alone is not ignition. Trade and dollar-volume participation do not identify buyer aggression.</Text>
    <Text style={styles.section}>Causal level versions</Text>
    {e.level_history.length === 0 ? <Text style={styles.meta}>No armed level is available.</Text> : e.level_history.map((level) => <View key={`${level.level_id}/${level.version}`} style={styles.measurement}>
      <Text selectable style={styles.identifier}>{level.level_id} · v{level.version}</Text>
      <Text style={styles.body}>T {detectionPrice(level.trigger)} · I {detectionPrice(level.invalidation)}</Text>
      <Text style={styles.meta}>Known {at(level.known_at)} · {level.armed_at === null ? "not armed" : `armed ${at(level.armed_at)}`}{level.valid_to === null ? "" : ` · valid through ${at(level.valid_to)}`}</Text>
      {level.formed_from.length > 0 && <Text selectable style={styles.identifier}>Formed from {level.formed_from.join(", ")}</Text>}
    </View>)}
    <Text style={styles.section}>Episode timeline</Text>
    {e.timeline.length === 0 ? <Text style={styles.meta}>No transition is retained for this episode.</Text> : e.timeline.map((t) => <View key={t.transition_id} style={styles.measurement}>
      <Text style={[styles.body, { color: stateColor(t.state) }]}>{at(t.observed_at)} · {detectionStateLabel(t.state)} · {t.status}</Text>
      <Text style={styles.meta}>{words(t.event)} · {words(t.reason)} · cutoff {at(t.input_cutoff)}</Text>
      <Text selectable style={styles.identifier}>{t.transition_id}{t.tested_level_id ? ` · ${t.tested_level_id} v${t.tested_level_version}` : ""}</Text>
      {t.supersedes && <Text selectable style={styles.identifier}>Supersedes {t.supersedes}</Text>}
    </View>)}
  </View>;
}

const numeric = (value: unknown): number | null => typeof value === "number" && Number.isFinite(value) ? value : null;
const object = (value: unknown): Record<string, unknown> => value !== null && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : {};
const ratio = (value: unknown): string => numeric(value) === null ? "unavailable" : `${(numeric(value)! / 1000).toFixed(3)}×`;
const percent = (value: unknown, unitsPerPercent: number): string => numeric(value) === null ? "unavailable" : `${(numeric(value)! / unitsPerPercent).toFixed(2)}%`;

/** Formatting the recorded anchor units, not selecting or revising any anchor. */
function AnchorDetails({ episode: e }: { episode: DetectionEpisode }) {
  const anchors = e.raw.anchors;
  if (!anchors) return null;
  return <View style={styles.details}>
    <Text style={styles.section}>Impulse and pullback</Text>
    <Text style={styles.body}>L0 {detectionPrice(numeric(anchors.l0))} → H0 {detectionPrice(numeric(anchors.h0))} · impulse {percent(anchors.impulse_bp, 100)}</Text>
    <Text style={styles.meta}>Anchors frozen {numeric(anchors.frozen_at) === null ? "unavailable" : at(numeric(anchors.frozen_at)!)}</Text>
    <Text style={styles.body}>Pullback low {detectionPrice(numeric(anchors.pullback_low))} · {percent(anchors.depth_milli, 10)} of impulse · stock retreat {percent(anchors.retreat_bp, 100)}</Text>
    <Text style={styles.meta}>{numeric(anchors.pullback_bars) ?? "Unknown"} observed bars · {numeric(anchors.pullback_slots) ?? "unknown"} canonical 15s slots · dollar-volume rate ratio {ratio(anchors.dv_rate_ratio_milli)}</Text>
  </View>;
}

function ExpansionDetails({ episode: e }: { episode: DetectionEpisode }) {
  const features = e.raw.recognition;
  if (!features) return null;
  const range = object(features.range);
  const body = object(features.body);
  return <View style={styles.details}>
    <Text style={styles.section}>Candle observations at recognition</Text>
    <Text style={styles.body}>Literal range / prior median: {ratio(range.e_milli)} · body / prior median: {ratio(body.e_milli)}</Text>
    <Text style={styles.meta}>Range ≥1.15×: {String(features.range_expansion_15pct ?? "UNKNOWN")} · Body ≥1.15×: {String(features.body_expansion_15pct ?? "UNKNOWN")}</Text>
    <Text style={styles.meta}>Floor-adjusted range: {ratio(range.e_adj_milli)} · floor binding: {range.floor_binding === true ? "yes" : range.floor_binding === false ? "no" : "unknown"}</Text>
    <Text style={styles.meta}>Upper wick {detectionPrice(numeric(features.upper_wick))} · close location {percent(features.close_location_milli, 10)} of range</Text>
  </View>;
}

function Action({ label, onPress, disabled = false }: { label: string; onPress: () => void; disabled?: boolean }) {
  return <Pressable accessibilityRole="button" accessibilityState={{ disabled }} disabled={disabled} onPress={onPress} style={({ pressed }) => [styles.action, disabled && { opacity: 0.35 }, pressed && { backgroundColor: UI.raised }]}><Text style={styles.actionText}>{label}</Text></Pressable>;
}

const styles = StyleSheet.create({
  card: { backgroundColor: UI.panel, borderRadius: 14, padding: 10, gap: 6 },
  line: { flexDirection: "row", alignItems: "center", flexWrap: "wrap", gap: 8 },
  title: { fontFamily: fonts.semibold, fontSize: 14, color: UI.text },
  experimental: { color: UI.warning, fontSize: 10, marginLeft: "auto" },
  meta: { color: UI.textFaint, fontFamily: fonts.regular, fontSize: 11, lineHeight: 16 },
  body: { color: UI.textDim, fontFamily: fonts.regular, fontSize: 12, lineHeight: 17 },
  empty: { color: UI.textDim, fontFamily: fonts.regular, fontSize: 12, paddingVertical: 6 },
  emptyBox: { gap: 3 },
  warning: { color: UI.warning, fontFamily: fonts.regular, fontSize: 11, lineHeight: 16 },
  filters: { gap: 5, alignItems: "center" },
  filter: { backgroundColor: UI.row, borderRadius: 12, minHeight: 34, paddingHorizontal: 10, justifyContent: "center" },
  selectedFilter: { backgroundColor: "rgba(139,156,255,0.13)" },
  filterText: { color: UI.textDim, fontSize: 11, fontFamily: fonts.regular },
  row: { backgroundColor: UI.row, borderRadius: 10, paddingHorizontal: 10, paddingVertical: 7, gap: 4, marginTop: 5 },
  recent: { backgroundColor: UI.row, borderRadius: 10, paddingHorizontal: 10, paddingVertical: 5, gap: 2 },
  tickerPress: { flexDirection: "row", alignItems: "center", gap: 8, minHeight: 44, flex: 1 },
  detailTicker: { minHeight: 44, paddingHorizontal: 8, justifyContent: "center" },
  symbol: { color: UI.text, fontFamily: fonts.semibold, fontSize: 14 },
  price: { color: UI.text, fontFamily: fonts.regular, fontSize: 13, fontVariant: ["tabular-nums"] },
  state: { fontFamily: fonts.semibold, fontSize: 12, lineHeight: 17 },
  geometry: { color: UI.textDim, fontFamily: fonts.regular, fontSize: 11, fontVariant: ["tabular-nums"] },
  stale: { opacity: 0.62 },
  action: { minHeight: 44, paddingHorizontal: 10, paddingVertical: 8, justifyContent: "center", borderRadius: 10, backgroundColor: UI.row },
  actionText: { color: UI.accent, fontFamily: fonts.semibold, fontSize: 11 },
  details: { paddingVertical: 6, gap: 9 },
  section: { color: UI.text, fontFamily: fonts.semibold, fontSize: 12, marginTop: 4 },
  measurement: { gap: 3, padding: 8, backgroundColor: UI.row, borderRadius: 8 },
  identifier: { color: UI.textFaint, fontSize: 10, fontFamily: fonts.regular, lineHeight: 14 },
});
