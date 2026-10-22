import { formatEtMillis, formatEtTime } from "@stockwatcher/chart-core";
import { Icon, Segmented, Sheet, UI } from "@stockwatcher/chart-native";
import {
  DETECTION_FILTERS,
  detectionAttention,
  detectionDistance,
  detectionEvidenceValue,
  detectionFamilyLabel,
  detectionPrice,
  detectionRowView,
  detectionStateLabel,
  detectionView,
  detectionViewCounts,
  groupDetections,
  type DetectionChannel,
  type DetectionEpisode,
  type DetectionFilter,
  type DetectionGroup,
  type DetectionRowView,
  type DetectionSnapshot,
  type DetectionTone,
} from "@stockwatcher/shared";
import { memo, useEffect, useRef, useState } from "react";
import { Animated, Pressable, ScrollView, StyleSheet, Text, View } from "react-native";
import Reanimated, { FadeIn, FadeOut, ReduceMotion } from "react-native-reanimated";
import Svg, { Circle, Rect } from "react-native-svg";

import { useDetection } from "../feed/hooks";
import { Collapse } from "../motion/Collapse";
import { fonts } from "../theme";
import { useTickerPress } from "../ui/useTickerPress";

// Bound native view work without limiting discovery: the count and paging retain every group.
const PAGE = 12;
const at = (ns: number) => Number.isFinite(ns) && ns > 0 ? formatEtMillis(Math.floor(ns / 1_000_000)) : "unavailable";
const hhmm = (ns: number) => formatEtTime(Math.round(ns / 1_000_000));
const words = (value: string) => value.replace(/_/g, " ");
/** The price band as the Ross Panel prints it: "$0.10–$25". */
const band = (micros: number) => {
  const p = micros / 1_000_000;
  return `$${Number.isInteger(p) ? p : p.toFixed(2)}`;
};

const TONE: Record<DetectionTone, string> = {
  critical: UI.critical,
  warning: UI.warning,
  accent: UI.accent,
  text: UI.text,
  muted: UI.textDim,
  dim: UI.textFaint,
};
/** Only a crossing in progress (accent) and a price at the trigger (amber) earn an edge: a broke-trigger is a past stage, never a green "go". */
const EDGE: Partial<Record<DetectionRowView["stage"], string>> = { crossing: UI.accent, near: UI.warning };

function stateColor(state: string): string {
  if (/invalidated|rejected|retracted/.test(state)) return UI.critical;
  // A stage, not a "go" (see the web panel): normal text, never success green.
  if (/trigger_observed|triggered|confirmed|retained/.test(state)) return UI.text;
  if (/crossing|armed|extended|interrupted/.test(state)) return UI.warning;
  return UI.textDim;
}

/**
 * Signals on the phone (design canvas "Detection Panel", phone; approved 2026-10-21/22): its own
 * card under the Ross card, the web tab's rows stacked for the narrow screen (the Fold's cover
 * screen and unfolded alike: rows take the card's width, notes truncate). One row per
 * instrument from the shared `detectionRowView`, so the stage labels are the Ross cue's: an
 * edge only at the trigger (amber) or crossing it (accent, pulsing). Tapping a row opens its
 * chart with the episode drawn (the chart modal holds its evidence and timeline); tap, then
 * hold copies the ticker. Filters live in a sheet behind the header's filter button. A stage
 * change crossfades the edge and wash (opacity, native driver); with reduced motion it simply
 * changes. No layout animation on the rows: a moving Pressable takes stale taps on Android.
 *
 * Nothing at all while the detector is off or not yet known (owner review 2026-10-21; the
 * canvas's "off" state), and nothing while neither view has rows (owner, 2026-10-22: a card
 * with 0 items is not shown), so there is no quiet "Nothing developing" state. The Active /
 * Recent toggle shows only while both views have rows; with one, that one is shown. The card
 * stays mounted, collapsed (Collapse), so it opens by itself the moment rows arrive.
 */
export function DetectionPanel({ channel, onPick, reducedMotion, gap = 0 }: { channel: DetectionChannel; onPick: (symbol: string, episodeId?: string, runEpoch?: string) => void; reducedMotion: boolean; gap?: number }) {
  const state = useDetection(channel);
  const [filter, setFilter] = useState<DetectionFilter>("all");
  const [pickedRecent, setHistory] = useState(false);
  const [segment, setSegment] = useState("all");
  const [source, setSource] = useState("all");
  const [catalyst, setCatalyst] = useState("all");
  const [page, setPage] = useState(0);
  const [filtersOpen, setFiltersOpen] = useState(false);
  const snapshot = state.snapshot;
  const epoch = state.frame?.runEpoch ?? "";
  const counts = detectionViewCounts(state);
  const { recent: history, showToggle } = detectionView(pickedRecent, counts);
  const groups = groupDetections(snapshot, filter, history, segment)
    .filter((g) => catalyst === "all" || g.selected.catalyst === catalyst)
    .filter(() => source === "all" || state.frame?.source === source);
  const pages = Math.max(1, Math.ceil(groups.length / PAGE));
  const actualPage = Math.min(page, pages - 1);
  const shown = groups.slice(actualPage * PAGE, (actualPage + 1) * PAGE);
  const segments = [...new Set(snapshot?.episodes.map((e) => e.segment) ?? [])].sort();
  const current = state.connected && !state.resyncing;
  const now = snapshot ? Math.max(snapshot.cutoff, snapshot.as_of) : 0;
  // Only from current data: while reconnecting the rows are last known, so nothing claims attention.
  const attention = current ? detectionAttention(snapshot) : 0;
  const applied = [filter !== "all", segment !== "all", source !== "all", catalyst !== "all"].filter(Boolean).length;
  // No rows in either view, no card (owner, 2026-10-22); it opens and closes by its real height.
  return (
    <Collapse open={counts.active + counts.recent > 0} reducedMotion={reducedMotion} gap={gap}>
    <View style={styles.card} accessibilityLabel="Signals">
      <View style={styles.head}>
        <Text accessibilityRole="header" style={styles.title}>
          Signals
        </Text>
        {attention > 0 && (
          <View style={styles.count} accessibilityLabel={`${attention} at the trigger or crossing it`}>
            <Pulse color={UI.accent} size={6} reducedMotion={reducedMotion} />
            <Text style={styles.countText}>{attention}</Text>
          </View>
        )}
        {/* The view toggle only while both views have rows; it fades (opacity only, no layout
            animation on its buttons), and its slot keeps the filter button where it is. */}
        <View style={styles.show}>
          {showToggle && (
            <Reanimated.View entering={FadeIn.duration(180).reduceMotion(reducedMotion ? ReduceMotion.Always : ReduceMotion.System)} exiting={FadeOut.duration(150).reduceMotion(reducedMotion ? ReduceMotion.Always : ReduceMotion.System)}>
              <Segmented
                options={[
                  { value: "active", label: "Active", a11y: "Show active signals" },
                  { value: "recent", label: "Recent", a11y: "Show recent signals" },
                ]}
                value={history ? "recent" : "active"}
                onChange={(v) => {
                  setHistory(v === "recent");
                  setPage(0);
                }}
              />
            </Reanimated.View>
          )}
        </View>
        <Pressable accessibilityRole="button" accessibilityLabel={`Filters${applied ? `, ${applied} applied` : ""}`} onPress={() => setFiltersOpen(true)} hitSlop={6} style={({ pressed }) => [styles.round, pressed && { backgroundColor: UI.raised }]}>
          <Icon name="levels" color={applied ? UI.text : UI.textDim} size={14} />
          {applied > 0 && <View style={styles.appliedDot} />}
        </Pressable>
      </View>
      {snapshot && snapshot.source !== "live" && (
        <Text style={[styles.sub, snapshot.source === "fixture" && { color: UI.warning }]}>{snapshot.source === "fixture" ? "Fixture / demo data" : `${words(snapshot.source)} data`} · experimental</Text>
      )}
      {/* The canvas's "connection lost" state: the rows stay, dimmed and still, as last known. */}
      {!state.connected && (
        <View style={styles.status} accessibilityRole="alert">
          <View style={styles.statusDot} />
          <Text style={styles.statusText}>
            {state.resyncing ? "Resyncing" : "Reconnecting"}
            {snapshot ? ` · last ${hhmm(snapshot.cutoff)}` : ""}
          </Text>
        </View>
      )}
      {state.coverageLost && <Text accessibilityRole="alert" style={styles.warning}>Transition history is incomplete. Current rows remain available; this is not a complete research record.</Text>}
      {state.error && state.error !== "transition_history_incomplete" && <Text accessibilityRole="alert" style={styles.warning}>{words(state.error)}</Text>}
      {snapshot?.unavailable.map((r) => <Text key={r.reason} accessibilityRole="alert" style={styles.warning}>Assessment unavailable: {words(r.reason)}{r.symbols ? ` · ${r.symbols} symbols` : ""}</Text>)}
      {groups.length === 0 ? (
        // Only the user's own filters can empty a shown view (an empty view is never offered),
        // so this says so; the filter button clears them.
        applied > 0 && snapshot ? (
          <View style={styles.quiet}>
            <Text style={styles.quietText}>Nothing {history ? "resolved" : "developing"} in these filters</Text>
            <Text style={styles.quietMeta}>watching {snapshot.monitored_symbols.toLocaleString()} symbols · {band(snapshot.price_min_micros)}–{band(snapshot.price_max_micros)}</Text>
          </View>
        ) : null
      ) : (
        <View style={[styles.rows, !state.connected && styles.dimmed]}>
          {/* A reset remounts the recognizer and cancels its delayed tap/hold. Price updates
              keep this key so ordinary ticks never abandon a deliberate press. */}
          {shown.map((group) => (
            <SignalRow key={`${epoch}/${group.selected.instrument_id}/${group.selected.episode_id}`} group={group} row={detectionRowView(group, now, current, hhmm)} reducedMotion={reducedMotion} onPick={() => onPick(group.selected.symbol, group.selected.episode_id, epoch)} />
          ))}
        </View>
      )}
      {groups.length > PAGE && (
        <View style={styles.pager}>
          <Action label="Previous" disabled={actualPage === 0} onPress={() => setPage(actualPage - 1)} />
          <Text style={styles.meta}>{actualPage * PAGE + 1}–{Math.min(groups.length, (actualPage + 1) * PAGE)} of {groups.length}</Text>
          <Action label="Next" disabled={actualPage + 1 === pages} onPress={() => setPage(actualPage + 1)} />
        </View>
      )}
      <Sheet open={filtersOpen} onClose={() => setFiltersOpen(false)} title="Signal filters" reducedMotion={reducedMotion}>
        <View style={styles.details}>
          <Text style={styles.section}>Family</Text>
          <FilterStrip label="Detection family" options={DETECTION_FILTERS} value={filter} onChange={(id) => { setFilter(id as DetectionFilter); setPage(0); }} />
          <Text style={styles.section}>Session</Text>
          <FilterStrip label="Session" options={[{ id: "all", label: "All sessions" }, ...segments.map((id) => ({ id, label: words(id) }))]} value={segment} onChange={(id) => { setSegment(id); setPage(0); }} />
          <Text style={styles.section}>Source · this connection reads {state.frame?.source ?? "the selected market"}</Text>
          <FilterStrip label="Source" options={[{ id: "all", label: "Current source" }, { id: "live", label: "Live" }, { id: "replay", label: "Replay" }, { id: "fixture", label: "Fixture" }]} value={source} onChange={(id) => { setSource(id); setPage(0); }} />
          <Text style={styles.section}>Catalyst evidence</Text>
          <FilterStrip label="Catalyst status" options={[{ id: "all", label: "Any status" }, { id: "qualifying", label: "Qualifying evidence" }, { id: "none", label: "No qualifying evidence" }, { id: "unknown", label: "Unknown coverage" }]} value={catalyst} onChange={(id) => { setCatalyst(id); setPage(0); }} />
          {snapshot && <Text style={styles.meta}>Experimental · alerts off · {words(snapshot.fidelity)} · canonical {snapshot.base_timeframe_secs}s · research {detectionPrice(snapshot.research_min_micros)}–{detectionPrice(snapshot.research_max_micros)} · {snapshot.policy_id}</Text>}
          <Action label="Done" onPress={() => setFiltersOpen(false)} />
        </View>
      </Sheet>
    </View>
    </Collapse>
  );
}

function FilterStrip({ label, options, value, onChange }: { label: string; options: readonly { id: string; label: string }[]; value: string; onChange: (id: string) => void }) {
  return <ScrollView horizontal showsVerticalScrollIndicator={false} showsHorizontalScrollIndicator={false} accessibilityLabel={label} contentContainerStyle={styles.filters}>
    {options.map((item) => <Pressable key={item.id} accessibilityRole="button" accessibilityState={{ selected: value === item.id }} accessibilityLabel={`${label}: ${item.label}`} onPress={() => onChange(item.id)} style={[styles.filter, value === item.id && styles.selectedFilter]}><Text style={[styles.filterText, value === item.id && { color: UI.text }]}>{item.label}</Text></Pressable>)}
  </ScrollView>;
}

/** Fades to 1 whenever `key` changes (0 → 1 over 300 ms); with reduced motion it is simply 1 (Runners' crossfade). */
function useCrossfade(key: string, reducedMotion: boolean): Animated.Value {
  const [o] = useState(() => new Animated.Value(1));
  const first = useRef(true);
  useEffect(() => {
    if (first.current || reducedMotion) {
      first.current = false;
      o.setValue(1);
      return;
    }
    o.setValue(0);
    const a = Animated.timing(o, { toValue: 1, duration: 300, useNativeDriver: true });
    a.start();
    return () => a.stop();
  }, [o, key, reducedMotion]);
  return o;
}

const SignalRow = memo(function SignalRow({ group, row: r, reducedMotion, onPick }: { group: DetectionGroup; row: DetectionRowView; reducedMotion: boolean; onPick: () => void }) {
  const press = useTickerPress(group.selected.symbol, onPick);
  const fade = useCrossfade(r.stage, reducedMotion);
  const edge = EDGE[r.stage];
  return (
    <Pressable accessibilityRole="button" accessibilityLabel={r.aria} accessibilityHint="Opens the chart with this signal. Tap, then press and hold to copy" {...press} style={({ pressed }) => [styles.row, r.dimmed && { opacity: 0.6 }, pressed && { backgroundColor: UI.raised }]}>
      {/* The stage's wash and inset edge: filled views, never borders, crossfading in on a change. */}
      {r.stage === "crossing" && <Animated.View pointerEvents="none" style={[StyleSheet.absoluteFill, styles.wash, { opacity: fade }]} />}
      {edge && <Animated.View pointerEvents="none" style={[styles.edge, { backgroundColor: edge, opacity: fade }]} />}
      <View style={styles.line}>
        <Text style={styles.symbol}>{r.symbol}</Text>
        <Text style={styles.price}>{r.price}</Text>
        {r.ross && <View style={styles.rossDot} />}
        {r.news && (
          <View style={styles.badge}>
            <Text style={styles.badgeText}>NEWS</Text>
          </View>
        )}
        <Text style={styles.ago}>{r.ago}</Text>
      </View>
      <Animated.View style={[styles.line, { opacity: fade }]}>
        <Text style={styles.family} numberOfLines={1}>
          {r.family}
        </Text>
        <Text style={styles.faint}>·</Text>
        {r.pulse && <Pulse color={TONE[r.tone]} size={6} reducedMotion={reducedMotion} />}
        <Text numberOfLines={1} style={[styles.stage, { color: TONE[r.tone], fontFamily: r.attention ? fonts.semibold : fonts.regular }]}>
          {r.label}
          {r.note ? <Text style={styles.note}>{` ${r.note}`}</Text> : null}
        </Text>
      </Animated.View>
      {r.meter !== null && <Meter value={r.meter} color={TONE[r.tone]} />}
    </Pressable>
  );
});

/** Pullback low (left, red: the failure level) to trigger (right, neutral: a level, not a "go"), the price as a dot: the Ross row's meter. */
function Meter({ value, color }: { value: number; color: string }) {
  const v = Math.max(0, Math.min(1, value));
  return (
    <Svg width="100%" height={8} viewBox="0 0 160 8" preserveAspectRatio="none" accessibilityLabel={`price ${Math.round(v * 100)}% of the way from the pullback low to the trigger`}>
      <Rect x={0} y={3} width={160} height={2} rx={1} fill={UI.raised} />
      <Rect x={0} y={0} width={2} height={8} fill={UI.critical} />
      <Rect x={158} y={0} width={2} height={8} fill={UI.textDim} />
      <Rect x={0} y={3} width={v * 156} height={2} rx={1} fill={color} />
      <Circle cx={2 + v * 154} cy={4} r={3} fill={color} />
    </Svg>
  );
}

/** A dot that breathes while a crossing is in progress; still with reduced motion or stale data. */
function Pulse({ color, size, reducedMotion }: { color: string; size: number; reducedMotion: boolean }) {
  const [o] = useState(() => new Animated.Value(1));
  useEffect(() => {
    if (reducedMotion) {
      o.setValue(1);
      return;
    }
    const loop = Animated.loop(Animated.sequence([Animated.timing(o, { toValue: 0.35, duration: 500, useNativeDriver: true }), Animated.timing(o, { toValue: 1, duration: 500, useNativeDriver: true })]));
    loop.start();
    return () => loop.stop();
  }, [o, reducedMotion]);
  return <Animated.View style={{ width: size, height: size, borderRadius: size / 2, backgroundColor: color, opacity: o }} />;
}

function Geometry({ episode: e, currentAvailable = true }: { episode: DetectionEpisode; currentAvailable?: boolean }) {
  const trigger = detectionDistance(e.price_stale || !currentAvailable ? null : e.price, e.level?.trigger ?? null, "trigger");
  const invalidation = detectionDistance(e.price_stale || !currentAvailable ? null : e.price, e.level?.invalidation ?? null, "invalidation");
  return <View style={{ gap: 3 }}>
    <Text style={styles.geometry}>{e.level?.kind === "range_high" ? "Range high" : "Trigger"} {detectionPrice(e.level?.trigger ?? null)}{trigger ? trigger.atOrAbove ? " · at / above reference" : ` · ${detectionPrice(Math.round(trigger.dollars * 1_000_000))} / ${trigger.percent.toFixed(2)}% away` : " · distance unavailable"}</Text>
    <Text style={styles.geometry}>Invalidation {detectionPrice(e.level?.invalidation ?? null)}{invalidation ? invalidation.atOrAbove ? ` · ${detectionPrice(Math.round(invalidation.dollars * 1_000_000))} / ${invalidation.percent.toFixed(2)}% below price` : " · price below reference" : " · distance unavailable"}</Text>
  </View>;
}

/** Also rendered beside the selected chart. Identity and version references survive timeframe
 * changes; neither a chart candle nor a user drawing supplies these levels. */
export function DetectionEpisodeDetails({ episode: e, snapshot, currentAvailable = true }: { episode: DetectionEpisode; snapshot: DetectionSnapshot; currentAvailable?: boolean }) {
  return <View style={styles.details}>
    <Text style={[styles.state, { color: stateColor(e.state) }]}>{detectionFamilyLabel(e.family)} · {detectionStateLabel(e.state)}</Text>
    <Text style={styles.meta}>Experimental · {e.source} · {words(e.fidelity)} · {words(e.coverage)}{e.restored ? " · restored history" : ""}</Text>
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
      <Text style={styles.body}>{level.kind === "range_high" ? "Range high" : "T"} {detectionPrice(level.trigger)} · I {detectionPrice(level.invalidation)}</Text>
      <Text style={styles.meta}>Known {at(level.known_at)} · {level.armed_at === null ? "not armed" : `armed ${at(level.armed_at)}`}{level.valid_to === null ? "" : ` · valid through ${at(level.valid_to)}`}</Text>
      {level.formed_from.length > 0 && <Text selectable style={styles.identifier}>Formed from {level.formed_from.join(", ")}</Text>}
    </View>)}
    {e.raw.levels.filter((level) => level.kind === "range_low").map((level) => <View key={`${level.level_id}/${level.version}`} style={styles.measurement}>
      <Text style={styles.body}>Range low {detectionPrice(level.price)} · v{level.version}</Text>
      <Text selectable style={styles.identifier}>{level.level_id}</Text>
      <Text style={styles.meta}>Known {at(level.known_at)} · range boundary</Text>
    </View>)}
    <Text style={styles.section}>Episode timeline</Text>
    {e.timeline.length === 0 ? <Text style={styles.meta}>No transition is retained for this episode.</Text> : e.timeline.map((t) => <View key={t.transition_id} style={styles.measurement}>
      <Text style={[styles.body, { color: stateColor(t.state) }]}>{at(t.observed_at)} · {detectionStateLabel(t.state)} · {t.status}{t.restored ? " · restored history" : ""}{t.warmup ? " · warmup" : ""}</Text>
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
  card: { borderRadius: 14, backgroundColor: UI.panel, paddingVertical: 10, paddingHorizontal: 8, gap: 6 },
  head: { flexDirection: "row", alignItems: "center", gap: 8, paddingHorizontal: 4 },
  title: { color: UI.text, fontSize: 14, fontFamily: fonts.semibold },
  count: { flexDirection: "row", alignItems: "center", gap: 5, height: 22, paddingHorizontal: 8, borderRadius: 11, backgroundColor: "rgba(139,156,255,0.16)" },
  countText: { color: "#c3cbff", fontSize: 11, fontVariant: ["tabular-nums"] },
  // Active / Recent at the right, then the filter button: fits the Fold's ~360 dp cover screen.
  show: { marginLeft: "auto" },
  round: { width: 32, height: 32, borderRadius: 16, alignItems: "center", justifyContent: "center", backgroundColor: UI.row },
  appliedDot: { position: "absolute", top: 6, right: 6, width: 5, height: 5, borderRadius: 2.5, backgroundColor: UI.text },
  sub: { color: UI.textFaint, fontSize: 10, paddingHorizontal: 4 },
  status: { flexDirection: "row", alignItems: "center", gap: 6, paddingHorizontal: 4 },
  statusDot: { width: 6, height: 6, borderRadius: 3, backgroundColor: UI.warning },
  statusText: { color: UI.warning, fontSize: 11, fontFamily: fonts.regular },
  quiet: { alignItems: "center", gap: 4, paddingVertical: 14, paddingHorizontal: 4 },
  quietText: { color: UI.text, fontSize: 13, fontFamily: fonts.regular, textAlign: "center" },
  quietMeta: { color: UI.textFaint, fontSize: 11, fontVariant: ["tabular-nums"], textAlign: "center" },
  pager: { flexDirection: "row", alignItems: "center", justifyContent: "space-between", gap: 8 },
  // The card's own row gap: the rows are separated by space, never borders.
  rows: { gap: 6 },
  dimmed: { opacity: 0.6 },
  row: { gap: 4, paddingVertical: 8, paddingHorizontal: 10, borderRadius: 10, backgroundColor: UI.row, overflow: "hidden" },
  wash: { backgroundColor: "rgba(139,156,255,0.10)" },
  edge: { position: "absolute", left: 0, top: 0, bottom: 0, width: 3 },
  line: { flexDirection: "row", alignItems: "center", gap: 6 },
  symbol: { color: UI.text, fontSize: 14, fontFamily: fonts.semibold },
  price: { color: UI.text, fontSize: 13, fontVariant: ["tabular-nums"] },
  rossDot: { width: 6, height: 6, borderRadius: 3, backgroundColor: "#c3cbff" },
  badge: { borderRadius: 4, paddingHorizontal: 5, paddingVertical: 1, backgroundColor: UI.raised },
  badgeText: { color: UI.textDim, fontSize: 9, fontFamily: fonts.semibold },
  ago: { marginLeft: "auto", color: UI.textFaint, fontSize: 11, fontVariant: ["tabular-nums"] },
  family: { flexShrink: 0, maxWidth: "45%", color: UI.textDim, fontSize: 12, fontFamily: fonts.regular },
  faint: { color: UI.textFaint, fontSize: 12 },
  stage: { flexShrink: 1, fontSize: 12 },
  note: { color: UI.textDim, fontFamily: fonts.regular, fontVariant: ["tabular-nums"] },
  meta: { color: UI.textFaint, fontFamily: fonts.regular, fontSize: 11, lineHeight: 16 },
  body: { color: UI.textDim, fontFamily: fonts.regular, fontSize: 12, lineHeight: 17 },
  warning: { color: UI.warning, fontFamily: fonts.regular, fontSize: 11, lineHeight: 16, paddingHorizontal: 4 },
  filters: { gap: 5, alignItems: "center" },
  filter: { backgroundColor: UI.row, borderRadius: 12, minHeight: 34, paddingHorizontal: 10, justifyContent: "center" },
  // A picked filter is raised one surface step, never accent (the segmented rule).
  selectedFilter: { backgroundColor: UI.raised },
  filterText: { color: UI.textDim, fontSize: 11, fontFamily: fonts.regular },
  state: { fontFamily: fonts.semibold, fontSize: 12, lineHeight: 17 },
  geometry: { color: UI.textDim, fontFamily: fonts.regular, fontSize: 11, fontVariant: ["tabular-nums"] },
  action: { minHeight: 44, paddingHorizontal: 10, paddingVertical: 8, justifyContent: "center", borderRadius: 10, backgroundColor: UI.row },
  actionText: { color: UI.accent, fontFamily: fonts.semibold, fontSize: 11 },
  details: { paddingVertical: 6, gap: 9 },
  section: { color: UI.text, fontFamily: fonts.semibold, fontSize: 12, marginTop: 4 },
  measurement: { gap: 3, padding: 8, backgroundColor: UI.row, borderRadius: 8 },
  identifier: { color: UI.textFaint, fontSize: 10, fontFamily: fonts.regular, lineHeight: 14 },
});
