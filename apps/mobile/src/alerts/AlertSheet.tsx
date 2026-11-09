import {
  ALERT_SESSIONS,
  EXPIRY_LABELS,
  GAP_CHOICES,
  MOVE_WINDOW_LABELS,
  MOVE_WINDOWS,
  SESSION_LABELS,
  editedAlert,
  expiresAtFor,
  parseSymbol,
  type AlertCondition,
  type AlertExpiry,
  type AlertRule,
  type AlertSession,
  type MoveWindow,
} from "@stockwatcher/app-state";
import { DRAWING_PART_LABELS, MAX_TOLERANCE_PCT, MAX_TOLERANCE_TICKS, formatPrice, snapPrice, type DrawingAlertPart } from "@stockwatcher/chart-core";
import { Segmented, Sheet, UI } from "@stockwatcher/chart-native";
import { useState, useSyncExternalStore, type ReactNode } from "react";
import { Pressable, ScrollView, StyleSheet, Text, TextInput, View } from "react-native";

import { useReducedMotion } from "../motion/useReducedMotion";
import { appActions } from "../state/store";
import { fonts } from "../theme";
import { alertSheet, createAlert, type AlertSheetState } from "./actions";

type Tab = "price" | "move" | "signal" | "drawing";
type Signal = "halt" | "luld" | "hod" | "pressure";

/** A toggle chip (sessions, notify, choices that wrap): pressed = raised, never accent; 40 pt tall. */
function Chip({ on, label, onPress }: { on: boolean; label: string; onPress: () => void }) {
  return (
    <Pressable accessibilityRole="button" accessibilityState={{ selected: on }} onPress={onPress} style={({ pressed }) => [st.chip, on && st.chipOn, pressed && { opacity: 0.85 }]}>
      <Text style={[st.chipText, on && { color: UI.text }]}>{label}</Text>
    </Pressable>
  );
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <View style={{ gap: 6 }}>
      <Text style={st.label}>{label}</Text>
      {children}
    </View>
  );
}

/**
 * The full alert sheet (approved design, "Create alert on the phone"): Price / % move / Signal,
 * when it fires, sessions, expiry, notify. One sheet for the chart's "More options", the Alerts
 * tab's New alert and a row's Edit. Rendered once at the app's root; opened through actions.ts.
 */
export function AlertSheet() {
  const s = useSyncExternalStore(alertSheet.subscribe, alertSheet.get);
  const reducedMotion = useReducedMotion();
  const close = () => alertSheet.set({ open: false });
  return (
    <Sheet open={s.open} onClose={close} title={s.editId ? "Edit alert" : s.draft?.rule.kind === "drawing" ? `Alert on this drawing · ${s.draft.symbol}` : s.draft?.symbol ? `New alert · ${s.draft.symbol}` : "New alert"} reducedMotion={reducedMotion} tall>
      {s.draft && <Body key={s.seq} s={s} d={s.draft} close={close} />}
    </Sheet>
  );
}

function Body({ s, d, close }: { s: AlertSheetState; d: AlertRule; close: () => void }) {
  const r = d.rule;
  const [symbol, setSymbol] = useState(d.symbol);
  const [tab, setTab] = useState<Tab>(r.kind === "price" ? "price" : r.kind === "move" ? "move" : r.kind === "drawing" ? "drawing" : "signal");
  const [dir, setDir] = useState<"above" | "below">(r.kind === "price" ? r.dir : "above");
  const [price, setPrice] = useState(r.kind === "price" && d.symbol ? formatPrice(r.price) : "");
  const [moveDir, setMoveDir] = useState<"up" | "down" | "either">(r.kind === "move" ? r.dir : "up");
  const [pct, setPct] = useState(r.kind === "move" ? String(r.pct) : "5");
  const [win, setWin] = useState<MoveWindow>(r.kind === "move" ? r.window : "5m");
  const [signal, setSignal] = useState<Signal>(r.kind === "halt" || r.kind === "hod" || r.kind === "luld" || r.kind === "pressure" ? r.kind : "halt");
  const [bandPct, setBandPct] = useState(r.kind === "luld" ? String(r.pct) : "2");
  const [pMin, setPMin] = useState(r.kind === "pressure" ? String(r.min) : "80");
  const [fire, setFire] = useState(d.fire);
  const [gap, setGap] = useState(d.gapMin);
  const [sessions, setSessions] = useState<AlertSession[]>(d.sessions);
  const [expiry, setExpiry] = useState<AlertExpiry>(d.expiry);
  const [notify, setNotify] = useState(d.notify);
  const [sound, setSound] = useState(d.sound);
  const [part, setPart] = useState<DrawingAlertPart>(r.kind === "drawing" ? r.part : "line");
  const [tolUnit, setTolUnit] = useState<"ticks" | "pct">(r.kind === "drawing" ? r.tol.unit : "ticks");
  const [tolValue, setTolValue] = useState(r.kind === "drawing" ? String(r.tol.value) : "1");

  const sym = parseSymbol(symbol);
  const condition = ((): AlertCondition | null => {
    if (r.kind === "drawing") {
      const n = Number(tolValue.replace("%", ""));
      const ok = tolUnit === "ticks" ? Number.isInteger(n) && n >= 0 && n <= MAX_TOLERANCE_TICKS : Number.isFinite(n) && n > 0 && n <= MAX_TOLERANCE_PCT;
      return ok ? { kind: "drawing", drawingId: r.drawingId, part, tol: { unit: tolUnit, value: n }, tf: r.tf } : null;
    }
    if (tab === "price") {
      const p = Number(price);
      return Number.isFinite(p) && p > 0 ? { kind: "price", dir, price: snapPrice(p) } : null;
    }
    if (tab === "move") {
      const n = Number(pct.replace("%", ""));
      return Number.isFinite(n) && n > 0 ? { kind: "move", dir: moveDir, pct: n, window: win } : null;
    }
    if (signal === "luld") {
      const n = Number(bandPct);
      return Number.isFinite(n) && n > 0 ? { kind: "luld", side: "either", pct: n } : null;
    }
    if (signal === "pressure") {
      const n = Number(pMin);
      return Number.isInteger(n) && n >= 1 && n <= 100 ? { kind: "pressure", side: "buy", min: n } : null;
    }
    return { kind: signal };
  })();
  const valid = !!sym && !!condition && sessions.length > 0;

  const submit = () => {
    if (!valid || !condition || !sym) return;
    const now = Date.now();
    const patch = { symbol: sym, rule: condition, fire, gapMin: gap, sessions, expiry, expiresAt: expiresAtFor(expiry, now), notify, sound };
    if (s.editId) appActions().saveAlert(editedAlert(d, patch, now));
    else createAlert({ ...d, ...patch, createdAt: now, editedAt: now });
    close();
  };

  return (
    <ScrollView showsVerticalScrollIndicator={false} showsHorizontalScrollIndicator={false} style={{ flex: 1 }} contentContainerStyle={{ gap: 14, paddingBottom: 12 }} keyboardShouldPersistTaps="handled">
      {(!d.symbol || (s.editId && tab !== "drawing")) && (
        <Field label="Ticker">
          <TextInput value={symbol} onChangeText={(t) => setSymbol(t.toUpperCase())} autoCapitalize="characters" autoCorrect={false} placeholder="XLAB" placeholderTextColor={UI.textFaint} style={st.input} accessibilityLabel="Ticker" />
        </Field>
      )}
      {tab !== "drawing" && <Seg value={tab} onChange={setTab} options={[{ value: "price", label: "Price" }, { value: "move", label: "% move" }, { value: "signal", label: "Signal" }]} />}
      {tab === "drawing" && r.kind === "drawing" && (
        <>
          {part !== "line" && (
            <Field label="Level">
              <Seg value={part} onChange={setPart} options={(["stop", "target", "entry"] as const).map((v) => ({ value: v, label: DRAWING_PART_LABELS[v] }))} />
            </Field>
          )}
          <Field label="Touching within">
            <Seg
              value={tolUnit}
              onChange={(u) => {
                setTolUnit(u);
                setTolValue(u === "ticks" ? "1" : "0.5");
              }}
              options={[{ value: "ticks", label: "Ticks" }, { value: "pct", label: "Percent" }]}
            />
            <TextInput value={tolValue} onChangeText={setTolValue} keyboardType="decimal-pad" style={st.input} accessibilityLabel={tolUnit === "ticks" ? "Ticks" : "Percent"} />
          </Field>
          <Text style={st.hint}>
            A tick is $0.0001 under $1, $0.01 above. A print that jumps clean through the {part === "line" ? "line" : part} counts too. Follows the drawing as drawn on the {r.tf} chart: move it and the alert moves with it.
          </Text>
        </>
      )}
      {tab === "price" && (
        <>
          <Seg value={dir} onChange={setDir} options={[{ value: "above", label: "Crosses above" }, { value: "below", label: "Crosses below" }]} />
          <Field label={s.last ? `Price · last ${formatPrice(s.last)}` : "Price"}>
            <TextInput value={price} onChangeText={setPrice} keyboardType="decimal-pad" style={st.input} accessibilityLabel="Price" />
          </Field>
        </>
      )}
      {tab === "move" && (
        <>
          <Seg value={moveDir} onChange={setMoveDir} options={[{ value: "up", label: "Up" }, { value: "down", label: "Down" }, { value: "either", label: "Either way" }]} />
          <Field label="By (%)">
            <TextInput value={pct} onChangeText={setPct} keyboardType="decimal-pad" style={st.input} accessibilityLabel="Percent" />
          </Field>
          <Field label="Within">
            <View style={st.wrap}>
              {MOVE_WINDOWS.map((w) => (
                <Chip key={w} on={win === w} label={MOVE_WINDOW_LABELS[w]} onPress={() => setWin(w)} />
              ))}
            </View>
          </Field>
        </>
      )}
      {tab === "signal" && (
        <>
          <View style={st.wrap}>
            <Chip on={signal === "halt"} label="Halted or resumed" onPress={() => setSignal("halt")} />
            <Chip on={signal === "luld"} label="Near limit band" onPress={() => setSignal("luld")} />
            <Chip on={signal === "hod"} label="New high of day" onPress={() => setSignal("hod")} />
            <Chip on={signal === "pressure"} label="Buy pressure" onPress={() => setSignal("pressure")} />
          </View>
          {signal === "luld" && (
            <Field label="Within (%) of the band · modelled, not the exchange's numbers">
              <TextInput value={bandPct} onChangeText={setBandPct} keyboardType="decimal-pad" style={st.input} accessibilityLabel="Within percent" />
            </Field>
          )}
          {signal === "pressure" && (
            <Field label="Halt watcher pressure reaches (0–100)">
              <TextInput value={pMin} onChangeText={setPMin} keyboardType="number-pad" style={st.input} accessibilityLabel="Pressure" />
            </Field>
          )}
        </>
      )}
      <Field label="Fires">
        <Seg value={fire} onChange={setFire} options={[{ value: "once", label: "Once" }, { value: "every", label: `Every ${gap} min max` }]} />
        {fire === "every" && (
          <View style={st.wrap}>
            {GAP_CHOICES.map((g) => (
              <Chip key={g} on={gap === g} label={`${g} min`} onPress={() => setGap(g)} />
            ))}
          </View>
        )}
      </Field>
      <Field label="Sessions">
        <View style={st.wrap}>
          {ALERT_SESSIONS.map((x) => (
            <Chip key={x} on={sessions.includes(x)} label={SESSION_LABELS[x]} onPress={() => setSessions((c) => (c.includes(x) ? c.filter((y) => y !== x) : ALERT_SESSIONS.filter((y) => y === x || c.includes(y))))} />
          ))}
        </View>
      </Field>
      <Field label="Expires">
        <Seg value={expiry} onChange={setExpiry} options={(["week", "day", "never"] as const).map((x) => ({ value: x, label: EXPIRY_LABELS[x] }))} />
      </Field>
      <Field label="Notify">
        <View style={st.wrap}>
          <Chip on={notify.app} label="In app" onPress={() => setNotify((n) => ({ ...n, app: !n.app }))} />
          <Chip on={notify.phone} label="Phone" onPress={() => setNotify((n) => ({ ...n, phone: !n.phone }))} />
          {tab === "drawing" && <Chip on={sound} label="Sound" onPress={() => setSound((v) => !v)} />}
        </View>
      </Field>
      <Pressable accessibilityRole="button" accessibilityState={{ disabled: !valid }} disabled={!valid} onPress={submit} style={({ pressed }) => [st.cta, !valid && { opacity: 0.5 }, pressed && { opacity: 0.85 }]}>
        <Text style={st.ctaText}>{s.editId ? "Save alert" : "Create alert"}</Text>
      </Pressable>
    </ScrollView>
  );
}

const st = StyleSheet.create({
  label: { color: UI.textDim, fontSize: 12 },
  hint: { color: UI.textFaint, fontSize: 12, lineHeight: 17 },
  input: { height: 44, backgroundColor: UI.row, borderRadius: 10, paddingHorizontal: 12, color: UI.text, fontSize: 15, fontVariant: ["tabular-nums"] },
  wrap: { flexDirection: "row", flexWrap: "wrap", gap: 6 },
  chip: { minHeight: 40, paddingHorizontal: 14, borderRadius: 20, backgroundColor: UI.row, alignItems: "center", justifyContent: "center" },
  chipOn: { backgroundColor: UI.raised },
  chipText: { color: UI.textDim, fontSize: 13, fontFamily: fonts.semibold },
  cta: { height: 48, borderRadius: 12, backgroundColor: UI.accent, alignItems: "center", justifyContent: "center", marginTop: 4 },
  ctaText: { color: UI.accentFg, fontSize: 15, fontFamily: fonts.semibold },
});

/** chart-native Segmented, filled, each option named by its label for screen readers. */
function Seg<T extends string>({ value, onChange, options }: { value: T; onChange: (v: T) => void; options: readonly { value: T; label: string }[] }) {
  return <Segmented fill value={value} onChange={onChange} options={options.map((o) => ({ ...o, a11y: o.label }))} />;
}
