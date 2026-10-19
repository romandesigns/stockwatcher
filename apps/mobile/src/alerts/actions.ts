// The phone's alert actions with their Undo toasts (the web's apps/web/src/alerts/actions.ts),
// and the full sheet's open state. The alerts are app-state's synced namespace; this adds only
// the feedback around writes.

import { editedAlert, newAlert, type AlertRule } from "@stockwatcher/app-state";
import { formatPrice } from "@stockwatcher/chart-core";

import { navigateToAlerts } from "../navigation/ref";
import { appActions } from "../state/store";
import { showToast } from "../ui/toast";

const withUndo = (text: string, undo: () => void) => showToast(text, { icon: "bell", action: { label: "Undo", run: undo } });

export function createAlert(rule: AlertRule): void {
  appActions().saveAlert(rule);
  withUndo(`Alert set · ${rule.symbol}`, () => appActions().removeAlert(rule.id));
}

export function createPriceAlert(symbol: string, price: number, dir: "above" | "below"): void {
  createAlert(newAlert(symbol, { kind: "price", dir, price }, appActions().alertPrefs));
}

/** A handle dragged: the price moves, the direction follows the last price, the alert re-arms. */
export function moveAlert(id: string, price: number, from: number, last: number | null): void {
  const app = appActions();
  const cur = app.alerts.find((a) => a.id === id);
  if (!cur || cur.rule.kind !== "price") return;
  const before = cur.rule;
  const dir = last === null ? before.dir : price >= last ? "above" : "below";
  app.updateAlert(id, { rule: { ...before, price, dir } });
  withUndo(`Alert moved to ${formatPrice(price)}`, () => appActions().updateAlert(id, { rule: { ...before, price: from } }));
}

export function deleteAlert(id: string): void {
  const cur = appActions().alerts.find((a) => a.id === id);
  if (!cur) return;
  appActions().removeAlert(id);
  withUndo(`Alert deleted · ${cur.symbol}`, () => appActions().saveAlert(editedAlert(cur, {})));
}

/** The in-app banner for a fire heard live (a reconnect's Activity is history, not news). */
export function showFired(symbol: string, text: string): void {
  showToast(`${symbol} · ${text}`, { icon: "bell", ms: 6000, action: { label: "View", run: navigateToAlerts } });
}

// --- the full sheet's open state ---------------------------------------------------------------

export interface AlertSheetState {
  open: boolean;
  editId: string | null;
  draft: AlertRule | null;
  last: number | null;
  seq: number;
}

let state: AlertSheetState = { open: false, editId: null, draft: null, last: null, seq: 0 };
const listeners = new Set<() => void>();

export const alertSheet = {
  get: () => state,
  subscribe(cb: () => void) {
    listeners.add(cb);
    return () => {
      listeners.delete(cb);
    };
  },
  set(next: Partial<AlertSheetState>) {
    state = { ...state, ...next };
    listeners.forEach((l) => l());
  },
};

export function openAlertSheet(draft: AlertRule, opts: { editId?: string; last?: number | null } = {}): void {
  alertSheet.set({ open: true, draft, editId: opts.editId ?? null, last: opts.last ?? null, seq: state.seq + 1 });
}
