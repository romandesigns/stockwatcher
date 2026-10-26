import type { ChartTheme, DisplayList } from "./displayList.ts";
import { tToIndex } from "./drawings.ts";
import type { ChartModel } from "./layout.ts";
import { priceY, slotX, type Slot } from "./scale.ts";
import { withAlpha } from "./theme.ts";
import type { Candle } from "./types.ts";

// Automatic supply and demand zones (owner, 2026-10-25: "automatically drawn for me and
// automatically adjust given the time frame I select"). The classic base-and-departure rule,
// read off the candles of the chart's own timeframe, so a 5-minute chart finds 5-minute zones:
//
//   base       1–BASE_MAX consecutive quiet candles: body ≤ half the candle's range and range
//              ≤ BASE_RANGE_ATR × ATR (indecision, not a trend bar);
//   departure  the next 1–LEG_MAX candles leave the base by ≥ LEG_ATR × ATR, the first of them a
//              decisive bar (body ≥ 60% of its range) closing outside the base;
//   zone       demand (departure up): from the base's lowest low (distal) to its highest body top
//              (proximal); supply (departure down): from the highest high to the lowest body bottom.
//
// A zone lives until a candle CLOSES beyond its distal edge (broken: no longer drawn). Price
// trading back into it after the departure marks it tested, drawn fainter. ATR is the mean true
// range of the ATR_N candles before the base, so a zone is judged by the volatility it formed in.
//
// Tested after it shipped (research/auto-zones-hold, pre-registered, 118 never-read days, Runners
// stocks): retested zones held 59–67% of the time vs 47–59% for random zones of the same kind,
// height and distance (+8.6 to +11.4 pp by timeframe, every interval above 0; the seen days agree).
// A hold is a level respecting price, not a trade: no costs, no entry rule, and random levels hold
// about half the time too. The Levels menu says so in one line. It is never an alert input.

export const AUTO_ZONE_RULES = { ATR_N: 14, BASE_MAX: 5, BASE_RANGE_ATR: 1.0, LEG_MAX: 3, LEG_ATR: 2.0, DECISIVE_BODY: 0.6, PER_SIDE: 3 } as const;

export interface AutoZone {
  kind: "supply" | "demand";
  /** Zone edges in dollars (lo < hi). */
  lo: number;
  hi: number;
  /** Start of the first base candle (epoch ms): where the band begins. */
  t: number;
  /** Price came back into the zone after the departure. */
  tested: boolean;
}

const body = (c: Candle) => Math.abs(c.c - c.o);
const range = (c: Candle) => c.h - c.l;

/**
 * Active zones for the given slots (oldest candles first), at most PER_SIDE per kind, the ones
 * nearest the last close first. Pure: the same candles give the same zones, live or replay.
 */
export function autoZones(slots: readonly Slot[]): AutoZone[] {
  const R = AUTO_ZONE_RULES;
  const cs: Candle[] = [];
  for (const s of slots) if (s.kind === "candle") cs.push(s.candle);
  if (cs.length < R.ATR_N + 3) return [];
  // True range and its trailing mean, so atr[i] uses only candles before i.
  const tr = cs.map((c, i) => (i === 0 ? range(c) : Math.max(range(c), Math.abs(c.h - cs[i - 1]!.c), Math.abs(c.l - cs[i - 1]!.c))));
  const atrBefore = (i: number): number => {
    if (i < R.ATR_N) return Number.NaN;
    let s = 0;
    for (let k = i - R.ATR_N; k < i; k++) s += tr[k]!;
    return s / R.ATR_N;
  };
  const quiet = (c: Candle, atr: number) => range(c) > 0 && body(c) <= range(c) * 0.5 && range(c) <= R.BASE_RANGE_ATR * atr;

  const found: { zone: AutoZone; legEnd: number; distal: number }[] = [];
  let i: number = R.ATR_N;
  while (i < cs.length - 1) {
    const atr = atrBefore(i);
    if (!(atr > 0) || !quiet(cs[i]!, atr)) {
      i++;
      continue;
    }
    let j = i;
    while (j + 1 < cs.length && j - i + 1 < R.BASE_MAX && quiet(cs[j + 1]!, atr)) j++;
    const base = cs.slice(i, j + 1);
    const baseHigh = Math.max(...base.map((c) => c.h));
    const baseLow = Math.min(...base.map((c) => c.l));
    const bodyTop = Math.max(...base.map((c) => Math.max(c.o, c.c)));
    const bodyBottom = Math.min(...base.map((c) => Math.min(c.o, c.c)));
    const first = cs[j + 1];
    let made = false;
    if (first && body(first) >= R.DECISIVE_BODY * range(first)) {
      const up = first.c > baseHigh;
      const down = first.c < baseLow;
      if (up || down) {
        // The departure: the furthest the next LEG_MAX candles reach from the base.
        let reach = 0;
        let end = j + 1;
        for (let k = j + 1; k < Math.min(cs.length, j + 1 + R.LEG_MAX); k++) {
          const d = up ? cs[k]!.h - baseHigh : baseLow - cs[k]!.l;
          if (d > reach) {
            reach = d;
            end = k;
          }
        }
        if (reach >= R.LEG_ATR * atr) {
          const zone: AutoZone = up ? { kind: "demand", lo: baseLow, hi: bodyTop, t: base[0]!.t, tested: false } : { kind: "supply", lo: bodyBottom, hi: baseHigh, t: base[0]!.t, tested: false };
          if (zone.hi > zone.lo) found.push({ zone, legEnd: end, distal: up ? baseLow : baseHigh });
          made = true;
        }
      }
    }
    // A base that launched a zone is used up; otherwise slide one candle on.
    i = made ? j + 1 : i + 1;
  }

  // Walk forward from each departure: a close beyond the distal edge breaks the zone, a trade back
  // inside it marks it tested.
  const live: AutoZone[] = [];
  for (const f of found) {
    let broken = false;
    let tested = false;
    for (let k = f.legEnd + 1; k < cs.length; k++) {
      const c = cs[k]!;
      if (f.zone.kind === "demand" ? c.c < f.distal : c.c > f.distal) {
        broken = true;
        break;
      }
      if (f.zone.kind === "demand" ? c.l <= f.zone.hi : c.h >= f.zone.lo) tested = true;
    }
    if (!broken) live.push({ ...f.zone, tested });
  }

  // Overlapping zones of one kind: keep the older (the origin of the move), drop the later.
  const kept: AutoZone[] = [];
  for (const z of live) if (!kept.some((k) => k.kind === z.kind && z.lo <= k.hi && z.hi >= k.lo)) kept.push(z);

  const last = cs.at(-1)!.c;
  const nearest = (kind: AutoZone["kind"]) =>
    kept
      .filter((z) => z.kind === kind)
      .sort((a, b) => Math.min(Math.abs(a.lo - last), Math.abs(a.hi - last)) - Math.min(Math.abs(b.lo - last), Math.abs(b.hi - last)))
      .slice(0, R.PER_SIDE);
  return [...nearest("supply"), ...nearest("demand")].sort((a, b) => a.t - b.t);
}

/**
 * Auto zones as soft bands behind the candles (layer "background", like volume shelves), from the
 * base to the plot's right edge, tinted by kind: supply in the down hue, demand in the up hue,
 * fainter once tested. Quieter than a hand-drawn zone (which is the user's own mark) and labelled
 * "auto" so the two are never confused.
 */
export function layoutAutoZones(model: ChartModel, zones: readonly AutoZone[], theme: ChartTheme): DisplayList {
  const out: DisplayList = [];
  const price = model.price;
  if (!price || !zones.length || !model.slots.length) return out;
  const plot = model.geometry.plot;
  const right = plot.x + plot.w;
  const fs = model.options.fontSize;
  for (const z of zones) {
    const x0 = Math.max(plot.x, slotX(model.index, tToIndex(model.slots, z.t, model.timeframeMs)) - model.index.barSpacing / 2);
    if (x0 >= right) continue;
    const yTop = Math.max(plot.y, priceY(price, z.hi));
    const yBottom = Math.min(plot.y + plot.h, priceY(price, z.lo));
    if (yBottom < plot.y || yTop > plot.y + plot.h) continue;
    const h = Math.max(1, yBottom - yTop);
    const hue = z.kind === "supply" ? theme.down : theme.up;
    out.push({ kind: "rect", x: x0, y: yTop, w: right - x0, h, fill: withAlpha(hue, z.tested ? 0.06 : 0.11), layer: "background" });
    // Always labelled: a band that starts near the right edge (a zone from the last few candles)
    // pulls its label back inside the plot rather than drawing an unexplained tint.
    const text = `${z.kind === "supply" ? "Supply" : "Demand"} · auto${z.tested ? " · tested" : ""}`;
    const lw = Math.ceil(text.length * (fs - 1) * 0.6);
    const lx = Math.max(plot.x + 4, Math.min(x0 + 4, right - lw - 4));
    const inside = h >= fs + 4;
    const ly = inside ? yTop + 2 : yTop - 2;
    if (inside || ly - fs >= plot.y) {
      out.push({ kind: "text", x: lx, y: ly, text, fill: withAlpha(hue, 0.85), fontSize: fs - 1, fontFamily: theme.fontSans, fontWeight: 500, align: "left", baseline: inside ? "top" : "bottom", layer: "background" });
    }
  }
  return out;
}
