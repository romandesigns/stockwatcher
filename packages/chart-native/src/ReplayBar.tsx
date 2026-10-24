import {
  alertDots,
  alertJumpScope,
  alertLabel,
  alertNear,
  alertsForSymbol,
  formatEtTime,
  nextAlert,
  nextReplaySpeed,
  prevAlert,
  REPLAY_ALERT_COLOR,
  REPLAY_SPEEDS,
  replayBarLayout,
  replayDetailsInline,
  replayScrubberInline,
  replaySpeedLabel,
  SCRUBBER_BARS,
  SCRUBBER_MICRO_BARS,
  SCRUBBER_SNAP_PX,
  SCRUBBER_TICK_HOURS,
  scrubberBars,
  scrubberFraction,
  scrubberKeySeek,
  scrubberReleaseTarget,
  scrubberSegments,
  scrubberSpan,
  scrubberTicks,
  scrubberTimeAt,
  scrubberTooltip,
  scrubberVolumeAt,
  sessionOf,
  sessionProgress,
  type AlertDot,
  type Candle,
  type Clock,
  type ReplayAlert,
  type ReplayBarLayout,
  type ScrubberBar,
  type ScrubberSegment,
  type ScrubberSpan,
  type ScrubberTick,
  type SessionBand,
} from "@stockwatcher/chart-core";
import { useEffect, useMemo, useState } from "react";
import { Platform, Pressable, StyleSheet, Text, View, type GestureResponderEvent, type LayoutChangeEvent, type StyleProp, type ViewStyle } from "react-native";
import Animated, { Easing, useAnimatedStyle, useSharedValue, withTiming } from "react-native-reanimated";
import { Sheet } from "./sheets.tsx";
import { Icon, playHaptic, UI } from "./ui.tsx";

// The phone's replay bar: chart-web's ReplayBar for React Native, from the same chart-core maths
// (replayBar.ts). The two-tier bar ("B", owner-approved 2026-10-24) took about half the cover
// screen ("not practical", owner, 2026-10-24), so the phone's bar now rests as ONE compact row:
// play, Prev/Next alert (the main practice action, one tap away), the clock with its session, a
// thin scrubber, a speed chip that cycles 1→2→5→10→30× and "more". On the cover screen (< 600 dp)
// the row's 44 dp targets would leave the scrubber 40–80 dp, too coarse to aim, so it runs the
// bar's width as a strip over the row; unfolded (≥ 600 dp) it sits in the row, and from 900 dp the
// day and the ticker join it. "More" opens the rest in the app's bottom sheet over the chart (the
// chart never relayouts): play and the clock, day, ticker, Jump to, the speed segments and the
// full two-tier scrubber. Every target is at least 44 dp; the scrubbers' tooltip shows while
// dragging (touch has no hover).

// The app embeds Geist (apps/mobile theme.ts `fonts`, the expo-font plugin's names); no mono face
// is embedded, so the clock uses the platform's monospace with tabular figures.
const MONO = Platform.select({ ios: "Menlo", default: "monospace" });
const SANS = Platform.select({ ios: "Geist-Regular", default: "Geist_400Regular" });
const SANS_SEMI = Platform.select({ ios: "Geist-SemiBold", default: "Geist_600SemiBold" });
const SESSION_NAME = { overnight: "overnight", pre: "pre-market", regular: "regular session", after: "after hours" } as const;
/** Shared with the web's ReplayScrubber: a seek's target stays drawn until the rebuilt clock reaches it. */
const HOLD_MS = 180_000;
const SECONDARY_TEXT = "#c3c9d1";
/** The thin scrubber's drawn band; the touch area around it is taller (44 dp inline, 28 dp as the cover strip). */
const MICRO_BAND = 16;

export interface ReplayBarProps {
  clock: Clock | null;
  onPlayPause: (play: boolean) => void;
  onSpeedChange: (speed: number) => void;
  onSeek: (t: number) => void;
  bands: readonly SessionBand[];
  candles: readonly Pick<Candle, "t" | "v">[];
  symbol: string;
  /** Formatted last price, "" when unknown. */
  price: string;
  /** Opens the ticker search sheet; its pick replaces the chart. */
  onTickerPress: () => void;
  /** The day button: "Thu, Oct 9", or null when there is no day yet. */
  dayLabel: string | null;
  /** A picked day still loading ("Oct 22"), shown as "Loading Oct 22…". */
  loadingDay?: string | null;
  /** Opens the day sheet; absent = the date is plain text (nothing to pick). */
  onDayPress?: () => void;
  alerts?: readonly ReplayAlert[];
  onOpenTicker?: (symbol: string) => void;
  /** The bar's width in dp; picks the artboard (replayBarLayout: < 600 cover, else fold). */
  width: number;
  /** The "more" sheet; the screen keeps it (and remembers it on this phone). */
  expanded: boolean;
  onExpandedChange: (expanded: boolean) => void;
  reducedMotion?: boolean;
  style?: StyleProp<ViewStyle>;
}

/** The phone has two artboards; a desktop-wide tablet still uses the unfolded one. */
export function nativeReplayLayout(width: number): Exclude<ReplayBarLayout, "desktop"> {
  return replayBarLayout(width) === "cover" ? "cover" : "fold";
}

/**
 * The scrubber. "full": the two-tier bar's (labels, histogram, ticks, tappable alert dots), in the
 * "more" sheet. `micro`: the compact bar's thin one, a 16 dp band (session shading, a
 * micro-histogram, the played tint, the alert dots drawn over it, the handle) centred in a taller
 * touch area; its dots are too small to tap, so a release near one snaps onto that alert.
 */
function Scrubber({
  span,
  segments,
  bars,
  ticks,
  dots,
  now,
  onSeek,
  disabled,
  short,
  reducedMotion,
  micro,
  height,
}: {
  span: ScrubberSpan;
  segments: readonly ScrubberSegment[];
  bars: readonly ScrubberBar[];
  ticks: readonly ScrubberTick[];
  dots: readonly AlertDot[];
  now: number | null;
  onSeek: (t: number) => void;
  disabled: boolean;
  short: boolean;
  reducedMotion?: boolean;
  micro?: boolean;
  height: number;
}) {
  const [width, setWidth] = useState(0);
  const [drag, setDrag] = useState<{ t: number; x: number } | null>(null);
  const [held, setHeld] = useState<{ t: number; from: number | null } | null>(null);
  if (held && now !== null && now !== held.from && Math.abs(now - held.t) < Math.max(5_000, Math.abs((held.from ?? now) - held.t) / 2)) setHeld(null);
  useEffect(() => {
    if (!held) return;
    const id = setTimeout(() => setHeld(null), HOLD_MS);
    return () => clearTimeout(id);
  }, [held]);

  const seek = (t: number) => {
    setHeld({ t, from: now });
    playHaptic("selection");
    onSeek(t);
  };
  const shown = drag?.t ?? held?.t ?? now;
  const frac = shown === null ? null : scrubberFraction(span, shown);

  // The playhead glides between clock ticks (a timing, not a spring: it is a clock), and follows
  // a finger exactly. Reduced motion: it jumps.
  const pos = useSharedValue(frac ?? 0);
  useEffect(() => {
    if (frac === null) return;
    pos.value = drag || reducedMotion ? frac : withTiming(frac, { duration: 200, easing: Easing.linear });
  }, [frac, drag, reducedMotion, pos]);
  const headStyle = useAnimatedStyle(() => ({ left: pos.value * width }));
  const tintStyle = useAnimatedStyle(() => ({ width: pos.value * width }));

  const at = (e: GestureResponderEvent) => {
    const x = Math.min(width, Math.max(0, e.nativeEvent.locationX));
    return { x, t: scrubberTimeAt(span, width, x) };
  };
  const tipAlert = drag ? alertNear(dots, width, drag.x, micro ? SCRUBBER_SNAP_PX : 10)?.alert : undefined;
  const tip = drag ? scrubberTooltip(drag.t, scrubberVolumeAt(bars, span, drag.t), tipAlert ?? null) : null;
  const valueText = shown === null ? "Clock not started" : `${formatEtTime(shown, true)} Eastern, ${SESSION_NAME[sessionOf(shown)]}`;
  const step = (key: string) => {
    if (disabled) return;
    const t = scrubberKeySeek(key, false, shown ?? span.start, span);
    if (t !== null) seek(t);
  };
  const bandTop = (height - MICRO_BAND) / 2;

  // The track takes the touches (its children ignore them, so locationX is the track's).
  const track = {
    accessible: true,
    accessibilityRole: "adjustable",
    accessibilityLabel: "Replay position",
    accessibilityHint: "Drag to scrub, release to jump the replay there",
    accessibilityValue: { text: valueText },
    accessibilityState: { disabled },
    accessibilityActions: [{ name: "increment" }, { name: "decrement" }],
    onAccessibilityAction: (e: { nativeEvent: { actionName: string } }) => step(e.nativeEvent.actionName === "increment" ? "ArrowRight" : "ArrowLeft"),
    onStartShouldSetResponder: () => !disabled && width > 0,
    onMoveShouldSetResponder: () => !disabled && width > 0,
    onResponderTerminationRequest: () => false,
    onResponderGrant: (e: GestureResponderEvent) => setDrag(at(e)),
    onResponderMove: (e: GestureResponderEvent) => setDrag(at(e)),
    // One seek per release: each one rebuilds the replay on the server.
    onResponderRelease: () => {
      if (drag) seek(micro ? scrubberReleaseTarget(dots, width, drag.x, drag.t) : drag.t);
      setDrag(null);
    },
    onResponderTerminate: () => setDrag(null),
  } as const;
  const tipBox = tip && drag && (
    <View pointerEvents="none" style={[s.tip, micro ? { bottom: height + 4 } : { top: 26 }, { left: Math.max(0, Math.min(width - 190, drag.x - 60)) }]} accessibilityLiveRegion="polite">
      <Text style={[s.tipText, { fontFamily: MONO }]}>{tip.time}</Text>
      {tip.volume && <Text style={[s.tipText, { color: UI.textDim }]}>{tip.volume}</Text>}
      {tip.alert && tipAlert && <Text style={[s.tipText, { color: REPLAY_ALERT_COLOR[tipAlert.kind] }]}>{tip.alert}</Text>}
    </View>
  );

  if (micro) {
    return (
      <View style={[s.scrubber, { height, flex: 1 }]} onLayout={(e: LayoutChangeEvent) => setWidth(e.nativeEvent.layout.width)}>
        <View style={StyleSheet.absoluteFill} {...track}>
          <View pointerEvents="none" style={[s.microBand, { top: bandTop }]}>
            {segments.map((seg) => (
              <View key={seg.kind} style={{ height: "100%", width: `${(seg.to - seg.from) * 100}%`, backgroundColor: seg.kind === "regular" ? UI.row : "#11151a" }} />
            ))}
          </View>
          <View pointerEvents="none" style={[s.microBand, s.microBars, { top: bandTop }]}>
            {bars.map((b) => {
              const played = shown !== null && b.t < shown;
              return <View key={b.t} style={[s.bar, { height: `${Math.round(b.h * 100)}%`, backgroundColor: played ? UI.accent : "#3a4350", opacity: played ? 0.7 : 0.8 }]} />;
            })}
          </View>
          {frac !== null && <Animated.View pointerEvents="none" style={[s.microTint, { top: bandTop }, tintStyle]} />}
          {dots.map((d) => (
            <View key={`${d.alert.symbol}-${d.alert.kind}-${d.alert.t}`} pointerEvents="none" style={[s.microDot, { top: bandTop - 8, left: d.frac * width - 3, backgroundColor: REPLAY_ALERT_COLOR[d.alert.kind] }]} />
          ))}
        </View>
        {frac !== null && (
          <Animated.View pointerEvents="none" style={[s.microHead, { top: bandTop - 2 }, headStyle]}>
            <View style={s.microLine} />
            <View style={s.microHandle} />
          </Animated.View>
        )}
        {tipBox}
      </View>
    );
  }

  return (
    <View style={[s.scrubber, { height }]} onLayout={(e: LayoutChangeEvent) => setWidth(e.nativeEvent.layout.width)}>
      <View style={StyleSheet.absoluteFill} {...track}>
        <View pointerEvents="none" style={s.bands}>
          {segments.map((seg) => (
            <View key={seg.kind} style={[s.band, { width: `${(seg.to - seg.from) * 100}%`, backgroundColor: seg.kind === "regular" ? "#182029" : "#12161c" }]}>
              <Text numberOfLines={1} style={[s.bandLabel, short && s.bandLabelShort, { color: seg.kind === "regular" ? UI.textDim : "#8a929c" }]}>
                {short ? seg.short : seg.label}
              </Text>
            </View>
          ))}
        </View>
        <View pointerEvents="none" style={s.bars}>
          {bars.map((b) => (
            <View key={b.t} style={[s.bar, { height: `${Math.round(b.h * 100)}%`, backgroundColor: shown !== null && b.t < shown ? UI.accent : "#3a4350", opacity: shown !== null && b.t < shown ? 0.8 : 1 }]} />
          ))}
        </View>
        {frac !== null && <Animated.View pointerEvents="none" style={[s.tint, tintStyle]} />}
        <View pointerEvents="none" style={s.ticks}>
          {ticks.map((t) => (
            <Text key={t.t} style={[s.tick, { left: t.frac * width, transform: [{ translateX: -t.frac * 34 }] }]}>
              {t.label}
            </Text>
          ))}
        </View>
      </View>
      {/* Alert dots: each a 44 dp target that jumps to its alert. */}
      {dots.map((d) => (
        <Pressable
          key={`${d.alert.symbol}-${d.alert.kind}-${d.alert.t}`}
          accessibilityRole="button"
          accessibilityLabel={`${d.label}, jump there`}
          disabled={disabled}
          onPress={() => seek(d.alert.t)}
          hitSlop={12}
          style={[s.dotHit, { left: d.frac * width - 10 }]}
        >
          <View style={[s.dot, { backgroundColor: REPLAY_ALERT_COLOR[d.alert.kind] }]} />
        </Pressable>
      ))}
      {frac !== null && (
        <Animated.View pointerEvents="none" style={[s.head, headStyle]}>
          <View style={s.headLine} />
          <View style={s.handle} />
        </Animated.View>
      )}
      {tipBox}
    </View>
  );
}

function Chip({ label, a11y, dot, disabled, onPress, height, round }: { label: string; a11y: string; dot: string; disabled: boolean; onPress: () => void; height: number; round?: boolean }) {
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={a11y}
      accessibilityState={{ disabled }}
      disabled={disabled}
      onPress={onPress}
      hitSlop={(44 - height) / 2}
      style={({ pressed }) => [s.chip, { height, borderRadius: round ? height / 2 : 12 }, pressed && { backgroundColor: UI.raised }, disabled && s.disabled]}
    >
      <View style={[s.chipDot, { backgroundColor: dot }]} />
      <Text style={s.chipText}>{label}</Text>
    </Pressable>
  );
}

/** A 44 dp square button of the compact row (Prev/Next alert, more). */
function IconButton({ icon, a11y, disabled, onPress, badge, quiet, expanded }: { icon: "alertPrev" | "alertNext" | "more"; a11y: string; disabled?: boolean; onPress: () => void; badge?: string; quiet?: boolean; expanded?: boolean }) {
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={a11y}
      accessibilityState={{ disabled: !!disabled, ...(expanded === undefined ? {} : { expanded }) }}
      disabled={disabled}
      onPress={onPress}
      style={({ pressed }) => [s.iconButton, quiet && s.quiet, pressed && { backgroundColor: UI.raised }, disabled && s.disabled]}
    >
      <Icon name={icon} color={quiet ? UI.textDim : UI.text} size={18} />
      {/* "any": the jump crosses to another ticker (no alerts on this one today). */}
      {badge && <Text style={s.badge}>{badge}</Text>}
    </Pressable>
  );
}

/** The phone's replay bar. See the file comment; the web twin is chart-web ReplayBar. */
export function ReplayBar(p: ReplayBarProps) {
  const { clock, bands, candles, symbol, alerts = [], width, expanded, onExpandedChange } = p;
  const layout = nativeReplayLayout(width);
  const cover = layout === "cover";
  const inline = replayScrubberInline(layout);
  const details = inline && replayDetailsInline(width);
  const live = clock?.mode === "live";
  const controllable = !!clock && !live;
  const playing = !!clock?.playing;
  const span = useMemo(() => scrubberSpan(bands), [bands]);
  const segments = useMemo(() => scrubberSegments(bands), [bands]);
  const bars = useMemo(() => (span ? scrubberBars(candles, span, SCRUBBER_BARS[layout]) : []), [candles, span, layout]);
  const microBars = useMemo(() => (span ? scrubberBars(candles, span, SCRUBBER_MICRO_BARS) : []), [candles, span]);
  const ticks = useMemo(() => (span ? scrubberTicks(span, SCRUBBER_TICK_HOURS[layout]) : []), [span, layout]);
  const dots = useMemo(() => (span ? alertDots(alertsForSymbol(alerts, symbol), span) : []), [alerts, symbol, span]);
  const scope = useMemo(() => alertJumpScope(alerts, symbol), [alerts, symbol]);
  const now = clock?.now ?? null;
  const prev = now === null ? null : prevAlert(scope.alerts, now);
  const next = now === null ? null : nextAlert(scope.alerts, now);
  const any = scope.any ? " · any" : "";
  const open = bands.find((b) => b.kind === "regular")?.start ?? null;
  const time = clock ? formatEtTime(clock.now, true) : "--:--:--";
  const progress = clock && bands.length ? sessionProgress(clock.now, bands).text : "";
  const jump = (target: number | null, sym?: string) => {
    if (target === null) return;
    if (sym) p.onOpenTicker?.(sym);
    p.onSeek(target);
  };
  const prevJump = { a11y: prev ? `Previous alert: ${alertLabel(prev, true)}` : "No earlier alert", target: prev?.t ?? null, sym: scope.any ? prev?.symbol : undefined };
  const nextJump = { a11y: next ? `Next alert: ${alertLabel(next, true)}` : "No later alert", target: next?.t ?? null, sym: scope.any ? next?.symbol : undefined };
  const chipH = cover ? 36 : 40;
  const jumps = [
    { key: "start", label: "04:00", a11y: "Jump to 04:00, the pre-market open", dot: "#6b7480", target: span?.start ?? null, sym: undefined },
    { key: "open", label: "09:30 open", a11y: "Jump to 09:30, the regular open", dot: UI.textDim, target: open, sym: undefined },
    { key: "prev", label: `‹ Prev alert${any}`, dot: REPLAY_ALERT_COLOR.fast, ...prevJump },
    { key: "next", label: `Next alert${any} ›`, dot: REPLAY_ALERT_COLOR.fast, ...nextJump },
  ].map((j) => <Chip key={j.key} label={j.label} a11y={j.a11y} dot={j.dot} height={chipH} round disabled={!controllable || j.target === null} onPress={() => jump(j.target, j.sym)} />);

  const speed = clock?.speed ?? 1;
  const speeds = (
    <View style={s.speeds} accessibilityRole="radiogroup" accessibilityLabel="Replay speed">
      {REPLAY_SPEEDS.map((v) => {
        const on = v === speed;
        return (
          <Pressable
            key={v}
            accessibilityRole="radio"
            accessibilityLabel={`Speed ${replaySpeedLabel(v)}`}
            accessibilityState={{ selected: on, disabled: !controllable }}
            disabled={!controllable}
            onPress={() => !on && p.onSpeedChange(v)}
            hitSlop={{ top: 4, bottom: 4 }}
            style={[s.speed, on && { backgroundColor: UI.raised }, !controllable && s.disabled]}
          >
            <Text style={[s.speedText, { color: on ? UI.text : UI.textDim }]}>{replaySpeedLabel(v)}</Text>
          </Pressable>
        );
      })}
    </View>
  );
  const play = (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={playing ? "Pause replay" : "Play replay"}
      accessibilityState={{ disabled: !controllable }}
      disabled={!controllable}
      onPress={() => p.onPlayPause(!playing)}
      style={({ pressed }) => [s.play, pressed && { opacity: 0.8 }, !controllable && s.disabled]}
    >
      <Icon name={playing ? "pause" : "play"} color={UI.accentFg} size={18} />
    </Pressable>
  );
  const clockBlock = (big: boolean) => (
    <View style={[s.clock, !inline && !big && s.grow]} accessible accessibilityLabel={clock ? `${time} Eastern, ${progress}` : "Clock not started"}>
      <Text style={[s.time, { fontSize: big ? 22 : 15 }]}>{time}</Text>
      <Text style={[s.progress, !big && s.progressSmall, inline && !big && { maxWidth: 140 }]} numberOfLines={1}>
        {progress}
      </Text>
    </View>
  );
  // From the sheet, the day and ticker buttons hand over to their own sheets (one modal at a time).
  const handOff = (go: () => void) => () => {
    if (expanded) onExpandedChange(false);
    go();
  };
  const day = (grow: boolean) => (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={p.loadingDay ? `Replay day ${p.dayLabel ?? ""}, loading ${p.loadingDay}` : p.onDayPress ? `Replay day ${p.dayLabel ?? ""}, change` : `Replay date ${p.dayLabel ?? "unknown"}`}
      disabled={!p.onDayPress}
      onPress={p.onDayPress ? handOff(p.onDayPress) : undefined}
      style={({ pressed }) => [s.control, grow && s.grow, pressed && { backgroundColor: UI.raised }]}
    >
      <Icon name="calendar" color={UI.text} size={16} />
      <Text style={[s.controlText, !p.onDayPress && { color: UI.textDim }]} numberOfLines={1}>
        {p.loadingDay ? `Loading ${p.loadingDay}…` : (p.dayLabel ?? "—")}
      </Text>
      {grow && <View style={s.grow} />}
      {p.onDayPress && <Icon name="chevronDown" color={UI.textDim} size={12} />}
    </Pressable>
  );
  const ticker = (grow: boolean) => (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={`${symbol || "No ticker"}${p.price ? `, ${p.price}` : ""}: search tickers`}
      onPress={handOff(p.onTickerPress)}
      style={({ pressed }) => [s.control, grow && s.grow, pressed && { backgroundColor: UI.raised }]}
    >
      <Icon name="search" color={UI.text} size={16} />
      <Text style={[s.controlText, { fontFamily: SANS_SEMI }]}>{symbol || "—"}</Text>
      {!!p.price && (
        <Text style={[s.controlText, { color: UI.textDim }]} numberOfLines={1}>
          ${p.price}
        </Text>
      )}
    </Pressable>
  );
  const microH = inline ? 44 : 28;
  const micro = span ? (
    <Scrubber span={span} segments={segments} bars={microBars} ticks={[]} dots={dots} now={now} onSeek={p.onSeek} disabled={!controllable} short reducedMotion={p.reducedMotion} micro height={microH} />
  ) : (
    <View style={[s.microEmpty, { height: microH }]}>
      <Text style={[s.progress, s.progressSmall]}>{clock ? "No session for this day" : "Waiting for the replay clock…"}</Text>
    </View>
  );
  const full = span ? (
    <Scrubber span={span} segments={segments} bars={bars} ticks={ticks} dots={dots} now={now} onSeek={p.onSeek} disabled={!controllable} short={cover} reducedMotion={p.reducedMotion} height={cover ? 92 : 96} />
  ) : (
    <View style={[s.empty, { height: cover ? 92 : 96 }]}>
      <Text style={s.progress}>{clock ? "No session for this day" : "Waiting for the replay clock…"}</Text>
    </View>
  );
  const speedChip = (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={`Speed ${replaySpeedLabel(speed)}, change to ${replaySpeedLabel(nextReplaySpeed(speed))}`}
      accessibilityState={{ disabled: !controllable }}
      disabled={!controllable}
      onPress={() => p.onSpeedChange(nextReplaySpeed(speed))}
      style={({ pressed }) => [s.speedChip, pressed && { backgroundColor: UI.raised }, !controllable && s.disabled]}
    >
      <Text style={s.speedChipText}>{replaySpeedLabel(speed)}</Text>
    </Pressable>
  );

  return (
    <>
      <View style={[s.card, inline ? s.cardInline : s.cardCover, p.style]}>
        {!inline && <View style={s.strip}>{micro}</View>}
        <View style={s.row}>
          {play}
          <IconButton icon="alertPrev" a11y={prevJump.a11y} disabled={!controllable || prevJump.target === null} onPress={() => jump(prevJump.target, prevJump.sym)} badge={scope.any ? "any" : undefined} />
          <IconButton icon="alertNext" a11y={nextJump.a11y} disabled={!controllable || nextJump.target === null} onPress={() => jump(nextJump.target, nextJump.sym)} badge={scope.any ? "any" : undefined} />
          <View style={s.spacer} />
          {clockBlock(false)}
          {details && (
            <>
              {day(false)}
              {ticker(false)}
            </>
          )}
          {inline && <View style={s.inlineScrubber}>{micro}</View>}
          {speedChip}
          <IconButton icon="more" a11y="More replay controls" quiet expanded={expanded} onPress={() => onExpandedChange(!expanded)} />
        </View>
      </View>
      {/* "More": the app's bottom sheet over the chart (slides, or fades under reduced motion). */}
      <Sheet open={expanded} onClose={() => onExpandedChange(false)} title="Replay" reducedMotion={p.reducedMotion}>
        <View style={s.sheetBody}>
          <View style={s.sheetRow}>
            {play}
            {clockBlock(true)}
          </View>
          <View style={[s.sheetRow, { gap: 8 }]}>
            {day(true)}
            {ticker(true)}
          </View>
          <View style={{ gap: 6 }}>
            <Text style={s.label}>Jump to</Text>
            <View style={s.wrap}>{jumps}</View>
          </View>
          {speeds}
          {full}
        </View>
      </Sheet>
    </>
  );
}

const s = StyleSheet.create({
  card: { backgroundColor: UI.panel },
  // The cover strip sits flush over the row (28 + 44 + 8 dp); the inline row is 44 dp with 10 above and below.
  cardCover: { paddingHorizontal: 8, paddingBottom: 8, borderRadius: 16 },
  cardInline: { paddingHorizontal: 10, paddingVertical: 10, borderRadius: 16 },
  strip: { flexDirection: "row", paddingHorizontal: 4 },
  row: { flexDirection: "row", alignItems: "center", gap: 6 },
  spacer: { width: 2 },
  inlineScrubber: { flex: 1, flexDirection: "row", marginHorizontal: 6, minWidth: 0 },
  sheetBody: { gap: 14 },
  sheetRow: { flexDirection: "row", alignItems: "center", gap: 12 },
  label: { color: UI.textFaint, fontFamily: SANS, fontSize: 12 },
  wrap: { flexDirection: "row", flexWrap: "wrap", gap: 6 },
  grow: { flex: 1 },
  disabled: { opacity: 0.4 },
  quiet: { backgroundColor: "transparent" },
  play: { width: 44, height: 44, borderRadius: 12, alignItems: "center", justifyContent: "center", backgroundColor: UI.accent },
  iconButton: { width: 44, height: 44, borderRadius: 12, alignItems: "center", justifyContent: "center", backgroundColor: "#1e252e" },
  badge: { position: "absolute", bottom: 3, color: UI.textDim, fontFamily: SANS, fontSize: 8 },
  speedChip: { minWidth: 44, height: 44, paddingHorizontal: 8, borderRadius: 12, alignItems: "center", justifyContent: "center", backgroundColor: "#1e252e" },
  speedChipText: { color: UI.text, fontFamily: SANS_SEMI, fontSize: 13, fontVariant: ["tabular-nums"] },
  clock: { gap: 4, minWidth: 0, flexShrink: 1 },
  time: { color: UI.text, fontFamily: MONO, fontVariant: ["tabular-nums"] },
  progress: { color: UI.textDim, fontFamily: SANS, fontSize: 12 },
  progressSmall: { fontSize: 10 },
  control: { height: 44, paddingHorizontal: 12, borderRadius: 12, backgroundColor: "#1e252e", flexDirection: "row", alignItems: "center", gap: 8, minWidth: 0 },
  controlText: { color: UI.text, fontFamily: SANS, fontSize: 13, flexShrink: 1 },
  chip: { paddingHorizontal: 12, backgroundColor: "#1e252e", flexDirection: "row", alignItems: "center", gap: 6 },
  chipDot: { width: 6, height: 6, borderRadius: 3 },
  chipText: { color: SECONDARY_TEXT, fontFamily: SANS, fontSize: 12 },
  speeds: { flexDirection: "row", padding: 3, gap: 2, backgroundColor: "#1e252e", borderRadius: 12, alignSelf: "stretch" },
  speed: { flex: 1, height: 38, alignItems: "center", justifyContent: "center", borderRadius: 9 },
  speedText: { fontFamily: SANS_SEMI, fontSize: 13, fontVariant: ["tabular-nums"] },
  scrubber: { position: "relative" },
  empty: { borderRadius: 8, backgroundColor: "#12161c", alignItems: "center", justifyContent: "center" },
  microEmpty: { flex: 1, alignItems: "center", justifyContent: "center" },
  microBand: { position: "absolute", left: 0, right: 0, height: MICRO_BAND, flexDirection: "row", borderRadius: 5, overflow: "hidden" },
  microBars: { alignItems: "flex-end", gap: 1, paddingHorizontal: 1 },
  microTint: { position: "absolute", left: 0, height: MICRO_BAND, backgroundColor: UI.accent, opacity: 0.14, borderTopLeftRadius: 5, borderBottomLeftRadius: 5 },
  microDot: { position: "absolute", width: 6, height: 6, borderRadius: 3 },
  microHead: { position: "absolute", width: 0, height: MICRO_BAND + 4 },
  microLine: { position: "absolute", top: 0, bottom: 0, left: -1, width: 2, borderRadius: 1, backgroundColor: UI.text },
  microHandle: { position: "absolute", top: (MICRO_BAND + 4) / 2 - 7, left: -7, width: 14, height: 14, borderRadius: 7, backgroundColor: UI.text, shadowColor: "#000", shadowOpacity: 0.45, shadowRadius: 6, shadowOffset: { width: 0, height: 1 }, elevation: 4 },
  bands: { position: "absolute", left: 0, right: 0, top: 0, bottom: 18, flexDirection: "row", borderRadius: 8, overflow: "hidden" },
  band: { height: "100%" },
  bandLabel: { position: "absolute", left: 8, top: 6, fontFamily: SANS, fontSize: 11 },
  bandLabelShort: { left: 6, top: 5, fontSize: 10 },
  bars: { position: "absolute", left: 0, right: 0, bottom: 18, height: 50, flexDirection: "row", alignItems: "flex-end", gap: 1, paddingHorizontal: 2 },
  bar: { flex: 1, borderTopLeftRadius: 2, borderTopRightRadius: 2 },
  tint: { position: "absolute", left: 0, top: 0, bottom: 18, backgroundColor: UI.accent, opacity: 0.07, borderTopLeftRadius: 8, borderBottomLeftRadius: 8 },
  ticks: { position: "absolute", left: 0, right: 0, bottom: 0, height: 14 },
  tick: { position: "absolute", top: 0, width: 34, color: "#8a929c", fontFamily: MONO, fontSize: 10 },
  dotHit: { position: "absolute", top: 16, width: 20, height: 20, alignItems: "center", justifyContent: "center" },
  dot: { width: 8, height: 8, borderRadius: 4 },
  head: { position: "absolute", top: 0, bottom: 14, width: 0 },
  headLine: { position: "absolute", top: -4, bottom: 0, left: -1, width: 2, borderRadius: 1, backgroundColor: UI.text },
  handle: { position: "absolute", top: -10, left: -10, width: 20, height: 20, borderRadius: 10, backgroundColor: UI.text, shadowColor: "#000", shadowOpacity: 0.4, shadowRadius: 8, shadowOffset: { width: 0, height: 2 }, elevation: 4 },
  tip: { position: "absolute", flexDirection: "row", gap: 8, paddingHorizontal: 8, paddingVertical: 5, borderRadius: 8, backgroundColor: "#232b35", elevation: 6 },
  tipText: { color: UI.text, fontFamily: SANS, fontSize: 11 },
});
