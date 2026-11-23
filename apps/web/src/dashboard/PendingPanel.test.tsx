import { describe, expect, test } from "bun:test";
import { EMPTY_PENDING, type PendingCardWire, type PendingState } from "@stockwatcher/shared";
import { renderToStaticMarkup } from "react-dom/server";
import { PendingPanelView } from "./PendingPanel.tsx";

const now = Date.now();
const base = (over: Partial<PendingCardWire>): PendingCardWire => ({
  id: "X-1", sym: "XLAB", halted_at: now - 272_000, code: "T1", reason: "News pending", price: 2.76, prev_close: 1.9, chg_pct: 45.3,
  phase: "halted", released_at: null, news: [], offering: false, resume_at: null, resume_est: false, resumed_at: null, reopen_price: null, updated: now, ...over,
});
const state = (cards: PendingCardWire[]): PendingState => ({ ...EMPTY_PENDING, enabled: true, snapshot: true, connected: true, band: [0.1, 25], cards });
const html = (cards: PendingCardWire[]) => renderToStaticMarkup(<PendingPanelView state={state(cards)} onPick={() => {}} />);

describe("News pending panel", () => {
  test("a halted card: warning status, count-up timer, no accent", () => {
    const h = html([base({})]);
    expect(h).toContain("News pending");
    expect(h).toContain("XLAB");
    expect(h).toContain("2.76");
    expect(h).toContain("+45.3%");
    expect(h).toContain("text-warning");
    expect(h).toMatch(/4:3[12]/);
    expect(h).not.toContain("text-primary");
  });

  test("news found: the headline with its source, an offering flagged in the danger colour", () => {
    const h = html([base({ phase: "news", offering: true, news: [{ id: "n", source: "sec_edgar/424B5", label: "SEC 424B5", headline: "XLAB prices $5M registered direct offering", url: "https://www.sec.gov/x", at: now - 30_000, seen: now - 29_000, kind: "offering" }] })]);
    expect(h).toContain("SEC 424B5");
    expect(h).toContain("registered direct offering");
    expect(h).toContain('href="https://www.sec.gov/x"');
    expect(h).toContain('target="_blank"');
    expect(h).toContain(">Offering<");
    expect(h).toContain("text-critical");
  });

  test("resumption set: the countdown is the only accent; resumed dims", () => {
    const h = html([base({ phase: "resuming", resume_at: now + 190_500, resume_est: true })]);
    expect(h.match(/text-primary/g)?.length).toBe(1);
    expect(h).toContain("reopens in");
    expect(h).toMatch(/~3:1[01]/);
    expect(h).toContain("est.");
    const r = html([base({ phase: "resumed", resumed_at: now - 10_000, reopen_price: 3.1 })]);
    expect(r).toContain("opacity-60");
    expect(r).toContain("Reopened");
    expect(r).not.toContain("text-primary");
  });

  test("no borders on the card", () => {
    expect(html([base({})])).not.toMatch(/\bborder(-[a-z]+)?\b/);
  });
});
