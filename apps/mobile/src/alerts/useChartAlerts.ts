import { alertDisplay, isActiveAlert, lineLabel, newAlert } from "@stockwatcher/app-state";
import type { ChartAlert } from "@stockwatcher/chart-core";
import type { ChartAlerts } from "@stockwatcher/chart-native";
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
