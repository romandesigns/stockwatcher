import { alertDisplay, alertsOnDrawing, isActiveAlert, lineLabel, newAlert, newDrawingAlert } from "@stockwatcher/app-state";
import { drawingAlertParts, type ChartAlert, type WatchedDrawing } from "@stockwatcher/chart-core";
import type { ChartAlerts, ChartDrawingAlerts } from "@stockwatcher/chart-native";
import { useEffect, useMemo, useState } from "react";

import { appActions, useApp } from "../state/store";
import { createPriceAlert, deleteAlert, moveAlert, openAlertSheet } from "./actions";

/** Snoozes and cooling gaps end by the clock: lines re-read their state this often. */
const REFRESH_MS = 30_000;

/** A chart's alert wiring (chart-native `alerts`) for one symbol; the web's useChartAlerts. */
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
      onMore: (d) => openAlertSheet(newAlert(symbol, { kind: "price", dir: d.dir, price: d.price }, appActions().alertPrefs), { last }),
      onMove: (id, price, from) => moveAlert(id, price, from, last),
      onDelete: deleteAlert,
    }),
    [lines, symbol, last],
  );
}

/**
 * A chart's drawing alerts (chart-native `drawingAlerts`) for one symbol; the web's
 * useChartDrawingAlerts: each live drawing alert as a watched drawing (muted unless armed), its
 * fire until acknowledged, and the toolbox's bell (an existing alert opens for editing).
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
        const app = appActions();
        const existing = alertsOnDrawing(app.alerts, symbol, drawing.id)[0];
        if (existing) return openAlertSheet(existing, { editId: existing.id, last });
        openAlertSheet(newDrawingAlert(symbol, drawing, drawingAlertParts(drawing)[0] ?? "line", timeframe, app.alertPrefs), { last });
      },
    }),
    [watched, symbol, last],
  );
}
