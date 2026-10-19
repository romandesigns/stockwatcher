// The web app's alert actions with their Undo toasts (approved design: "Release saves (with
// Undo)", "Toast → Undo, or Ctrl+Z"), and the full form's open state. The alerts themselves are
// app-state's synced namespace; this file only adds the person-facing feedback around writes.

import { editedAlert, newAlert, type AlertCondition, type AlertRule } from "@stockwatcher/app-state";
import { formatPrice } from "@stockwatcher/chart-core";
import { runToastAction, showToast } from "../lib/toastStore.ts";
import { getAppStore } from "../state/store.ts";

const app = () => getAppStore().getState();

/** Shows `text` with an Undo that runs `undo`. */
function withUndo(text: string, undo: () => void): void {
  showToast(text, { icon: "bell", action: { label: "Undo", run: undo } });
}

/** Ctrl+Z from a chart whose last change was an alert's: the toast's Undo, if it still shows. */
export function undoAlertChange(): boolean {
  return runToastAction("Undo");
}

export function createAlert(rule: AlertRule): void {
  app().saveAlert(rule);
  withUndo(`Alert set · ${rule.symbol}`, () => app().removeAlert(rule.id));
}

export function createPriceAlert(symbol: string, price: number, dir: "above" | "below"): void {
  createAlert(newAlert(symbol, { kind: "price", dir, price }, app().alertPrefs));
}

/** A line dragged or nudged: the price changes and the alert re-arms (a moved finished alert fires again). */
export function moveAlert(id: string, price: number, from: number, last: number | null = null): void {
  const cur = app().alerts.find((a) => a.id === id);
  if (!cur || cur.rule.kind !== "price") return;
  const before = cur;
  // The direction follows where the line now sits against the last price (the creation rule:
  // above it means "crosses above"), so dragging a line across the price keeps it meaningful.
  const dir = last === null ? cur.rule.dir : price >= last ? "above" : "below";
  app().updateAlert(id, { rule: { ...cur.rule, price, dir } });
  withUndo(`Alert moved to ${formatPrice(price)}`, () => {
    const now = app().alerts.find((a) => a.id === id);
    // Put the price back as an edit (the server re-arms it, as before the move).
    if (now && now.rule.kind === "price") app().updateAlert(id, { rule: { ...now.rule, price: from, dir: before.rule.kind === "price" ? before.rule.dir : now.rule.dir } });
    else app().saveAlert(editedAlert(before, {}));
  });
}

export function deleteAlert(id: string): void {
  const cur = app().alerts.find((a) => a.id === id);
  if (!cur) return;
  app().removeAlert(id);
  // Back as it was; a newer editedAt so the server starts it armed again.
  withUndo(`Alert deleted · ${cur.symbol}`, () => app().saveAlert(editedAlert(cur, {})));
}

export function updateAlertRule(id: string, rule: AlertCondition, patch: Partial<Omit<AlertRule, "id" | "createdAt" | "editedAt" | "rule">> = {}): void {
  app().updateAlert(id, { ...patch, rule });
}

// --- the full form's open state (one form for the chart's "More…", the Alerts page's New and Edit) ---

export interface AlertFormState {
  open: boolean;
  /** Editing this alert, else creating. */
  editId: string | null;
  draft: AlertRule | null;
  /** Bumped on every opening: the form remounts from its draft. */
  seq: number;
  /** The last price, for the "+7.3% from 2.33" hint. */
  last: number | null;
}

let form: AlertFormState = { open: false, editId: null, draft: null, last: null, seq: 0 };
const listeners = new Set<() => void>();

export const alertForm = {
  get: () => form,
  subscribe(cb: () => void) {
    listeners.add(cb);
    return () => listeners.delete(cb);
  },
  set(next: Partial<AlertFormState>) {
    form = { ...form, ...next };
    listeners.forEach((l) => l());
  },
};

/** Opens the full form to create an alert (`draft`, prefilled) or edit one (`editId`). */
export function openAlertForm(draft: AlertRule, opts: { editId?: string; last?: number | null } = {}): void {
  alertForm.set({ open: true, draft, editId: opts.editId ?? null, last: opts.last ?? null, seq: form.seq + 1 });
}
