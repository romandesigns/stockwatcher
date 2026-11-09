import { alertDisplay, alertsOnDrawing, isActiveAlert, lineLabel, newAlert, newDrawingAlert } from "@stockwatcher/app-state";
import { drawingAlertParts, type ChartAlert, type WatchedDrawing } from "@stockwatcher/chart-core";
import type { ChartAlerts, ChartDrawingAlerts } from "@stockwatcher/chart-web";
import { useEffect, useMemo, useState } from "react";
import { getAppStore, useApp } from "../state/store.ts";
import { createPriceAlert, deleteAlert, moveAlert, openAlertForm, undoAlertChange } from "./actions.ts";

/** Snoozes and cooling gaps end by the clock: lines re-read their state this often. */
const REFRESH_MS = 30_000;

/**
 * A chart's alert wiring (chart-web `alerts` prop) for one symbol: its price alerts still watching
 * as lines (muted while snoozed or cooling), and the actions with their Undo toasts. The chart
 * itself never touches the store or the server.
 */
export function useChartAlerts(symbol: string, last: number | null): ChartAlerts {
  const rules = useApp((s) => s.alerts);
  const states = useApp((s) => s.alertStates);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), REFRESH_MS);
    return () => clearInterval(t);
  }, []);
  const lines = useMemo<ChartAlert[]>(
    () =>
      rules
        .filter((r) => r.symbol === symbol && r.rule.kind === "price" && isActiveAlert(r, states[r.id], now))
        .map((r) => ({
          id: r.id,
          price: r.rule.kind === "price" ? r.rule.price : 0,
          dir: r.rule.kind === "price" ? r.rule.dir : "above",
          label: lineLabel(r),
          muted: alertDisplay(r, states[r.id], now).kind !== "armed",
        })),
    [rules, states, symbol, now],
  );
  return useMemo(
    () => ({
      lines,
      onCreate: (d) => createPriceAlert(symbol, d.price, d.dir),
      onMore: (d) => openAlertForm(newAlert(symbol, { kind: "price", dir: d.dir, price: d.price }, getAppStore().getState().alertPrefs), { last }),
      onMove: (id, price, from) => moveAlert(id, price, from, last),
      onDelete: deleteAlert,
      onUndo: () => void undoAlertChange(),
      defaultsText: "Once · this week · in app + phone",
    }),
    [lines, symbol, last],
  );
}

/**
 * A chart's drawing alerts (chart-web `drawingAlerts` prop) for one symbol: each drawing alert as
 * a watched drawing (muted unless armed, so the glow means "live and touching"), with its fire
 * until acknowledged, and the toolbox bell: a drawing with an alert opens it for editing, one
 * without opens a new one (a position's stop first).
 */
export function useChartDrawingAlerts(symbol: string, last: number | null): ChartDrawingAlerts {
  const rules = useApp((s) => s.alerts);
  const states = useApp((s) => s.alertStates);
  const fires = useApp((s) => s.drawingFires);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), REFRESH_MS);
    return () => clearInterval(t);
  }, []);
  const watched = useMemo<WatchedDrawing[]>(
    () =>
      rules.flatMap((r) => {
        if (r.symbol !== symbol || r.rule.kind !== "drawing") return [];
        const kind = alertDisplay(r, states[r.id], now).kind;
        if (kind === "expired" || kind === "orphaned") return [];
        const f = fires[r.id];
        return [{ alertId: r.id, drawingId: r.rule.drawingId, part: r.rule.part, tol: r.rule.tol, muted: kind !== "armed", fired: f ? { t: f.t, price: f.price, tone: f.tone } : null }];
      }),
    [rules, states, fires, symbol, now],
  );
  return useMemo(
    () => ({
      watched,
      onAlert: (drawing, timeframe) => {
        const app = getAppStore().getState();
        const existing = alertsOnDrawing(app.alerts, symbol, drawing.id)[0];
        if (existing) return openAlertForm(existing, { editId: existing.id, last });
        const part = drawingAlertParts(drawing)[0] ?? "line";
        openAlertForm(newDrawingAlert(symbol, drawing, part, timeframe, app.alertPrefs), { last });
      },
    }),
    [watched, symbol, last],
  );
}
