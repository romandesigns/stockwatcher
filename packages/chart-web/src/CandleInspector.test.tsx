import { describe, expect, test } from "bun:test";
import type { Slot } from "@stockwatcher/chart-core";
import { renderToStaticMarkup } from "react-dom/server";
import { CandleInspector } from "./CandleInspector.tsx";

// The forming candle's tag (owner, 2026-11-25): "Live" on the live dashboard, "Forming" on the
// practice page, whose candles are a replay and must never read as the live market.

const slot = (final: boolean): Slot => ({ kind: "candle", t: 1763395200000, candle: { t: 1763395200000, o: 2, h: 2.2, l: 1.9, c: 2.1, v: 12000, n: 40, final } });
const text = (html: string) => html.replace(/<[^>]+>/g, " ");

describe("CandleInspector forming tag", () => {
  test("says Live by default, the given label when asked, and nothing on a final candle", () => {
    expect(text(renderToStaticMarkup(<CandleInspector slot={slot(false)} timeframe="1m" />))).toContain("Live");
    const practice = text(renderToStaticMarkup(<CandleInspector slot={slot(false)} timeframe="1m" formingLabel="Forming" />));
    expect(practice).toContain("Forming");
    expect(practice).not.toMatch(/\bLive\b/);
    const final = text(renderToStaticMarkup(<CandleInspector slot={slot(true)} timeframe="1m" formingLabel="Forming" />));
    expect(final).not.toContain("Forming");
    expect(final).not.toMatch(/\bLive\b/);
  });
});
