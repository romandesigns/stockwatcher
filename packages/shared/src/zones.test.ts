import { describe, expect, test } from "bun:test";
import { rocketRowView, rocketSectionsView, type RocketPanelWire, type RocketRowWire } from "./rocket.ts";
import { EMPTY_ZONE_VIEW, withZoneNotes, withZoneSections, parseZones, parseZoneSymbol, ZONE_BASE_RATE, zoneBandsView, zoneKey, zoneNoteView, zoneSelector, zonesFor, type ZoneSymbolWire } from "./zones.ts";

const T = 1_792_075_200_000;

const shelf = (over: Partial<ZoneSymbolWire> = {}): ZoneSymbolWire => ({
  bands: [{ lo: 2.41, hi: 2.55, formed: T - 20 * 60_000, armed: true }],
  event: null,
  ...over,
});

describe("parseZones", () => {
  test("absent, null and non-objects are empty", () => {
    expect(parseZones(undefined)).toEqual({});
    expect(parseZones(null)).toEqual({});
    expect(parseZones([1, 2])).toEqual({});
    expect(parseZones("zones")).toEqual({});
  });

  test("keeps good bands and events, drops garbage", () => {
    const z = parseZones({
      GOOD: shelf(),
      BAD: { bands: [{ lo: "x", hi: 2, formed: T }, { lo: 3, hi: 2, formed: T }, null], event: { state: "maybe", lo: 1, hi: 2, at: T } },
      NONE: { bands: [], event: null },
      EVT: { bands: "nope", event: { state: "in_shelf", lo: 1.1, hi: 1.2, entry: 1.15, at: T, resolved: null } },
      NUM: 4,
    });
    expect(Object.keys(z).sort()).toEqual(["EVT", "GOOD"]);
    expect(z.GOOD!.bands).toEqual([{ lo: 2.41, hi: 2.55, formed: T - 20 * 60_000, armed: true }]);
    expect(z.EVT!.bands).toEqual([]);
    expect(z.EVT!.event).toEqual({ state: "in_shelf", lo: 1.1, hi: 1.2, entry: 1.15, at: T, resolved: null });
  });

  test("an event missing entry or resolved still parses", () => {
    const z = parseZoneSymbol({ bands: [], event: { state: "held", lo: 1, hi: 1.2, at: T } });
    expect(z?.event).toEqual({ state: "held", lo: 1, hi: 1.2, entry: 1, at: T, resolved: null });
  });

  test("zonesFor reads one symbol and ignores inherited keys", () => {
    const panel = { zones: { ROCK: shelf() } as RocketPanelWire["zones"] };
    expect(zonesFor(panel, "ROCK")?.bands).toHaveLength(1);
    expect(zonesFor(panel, "OTHER")).toBeNull();
    expect(zonesFor(panel, "toString")).toBeNull();
    expect(zonesFor(null, "ROCK")).toBeNull();
    expect(zonesFor({}, "ROCK")).toBeNull();
    expect(zonesFor(panel, "")).toBeNull();
  });
});

describe("zone note", () => {
  test("in the shelf: the range and the muted base rate, never a success tone", () => {
    const n = zoneNoteView(shelf({ event: { state: "in_shelf", lo: 2.41, hi: 2.55, entry: 2.5, at: T, resolved: null } }));
    expect(n).toEqual({ state: "in_shelf", label: "In volume shelf $2.41–$2.55", baseRate: "about 1 in 3 hold, 1 in 2 fail", tone: "text", aria: `In volume shelf $2.41–$2.55; historically ${ZONE_BASE_RATE}` });
  });

  test("sub-dollar prices keep their precision", () => {
    const n = zoneNoteView(shelf({ event: { state: "in_shelf", lo: 0.4125, hi: 0.45, entry: 0.44, at: T, resolved: null } }));
    expect(n?.label).toBe("In volume shelf $0.4125–$0.45");
  });

  test("held and lost are stated plainly; expired and no event say nothing", () => {
    const e = { lo: 1, hi: 1.2, entry: 1.1, at: T, resolved: T + 60_000 };
    expect(zoneNoteView(shelf({ event: { ...e, state: "held" } }))).toMatchObject({ label: "Shelf held", baseRate: "", tone: "muted" });
    expect(zoneNoteView(shelf({ event: { ...e, state: "lost" } }))).toMatchObject({ label: "Shelf lost", baseRate: "", tone: "muted" });
    expect(zoneNoteView(shelf({ event: { ...e, state: "expired" } }))).toBeNull();
    expect(zoneNoteView(shelf())).toBeNull();
    expect(zoneNoteView(null)).toBeNull();
  });
});

describe("zone bands", () => {
  test("the chart gets lo, hi and formed; none without a shelf", () => {
    expect(zoneBandsView(shelf())).toEqual([{ lo: 2.41, hi: 2.55, formed: T - 20 * 60_000 }]);
    expect(zoneBandsView(null)).toEqual([]);
  });

  test("the key changes only with what is drawn or said", () => {
    const a = shelf();
    expect(zoneKey(a)).toBe(zoneKey(shelf()));
    expect(zoneKey(a)).not.toBe(zoneKey(shelf({ event: { state: "in_shelf", lo: 2.41, hi: 2.55, entry: 2.5, at: T, resolved: null } })));
    expect(zoneKey(null)).toBe("");
  });
});

describe("zoneSelector", () => {
  test("the same object until the symbol's bands or note change", () => {
    const pick = zoneSelector();
    const a = pick({ zones: { ROCK: shelf() } }, "ROCK");
    expect(a.bands).toHaveLength(1);
    expect(pick({ zones: { ROCK: shelf(), OTHER: shelf() } }, "ROCK")).toBe(a);
    const b = pick({ zones: { ROCK: shelf({ event: { state: "in_shelf", lo: 2.41, hi: 2.55, entry: 2.5, at: T, resolved: null } }) } }, "ROCK");
    expect(b).not.toBe(a);
    expect(b.note?.state).toBe("in_shelf");
    expect(pick({}, "ROCK")).toBe(EMPTY_ZONE_VIEW);
    expect(pick(null, "ROCK")).toBe(EMPTY_ZONE_VIEW);
  });
});

describe("Runners rows carry the note", () => {
  const row: RocketRowWire = {
    id: "r1",
    s: "ROCK",
    at: T - 60_000,
    p: 2.4,
    last: 2.5,
    pc: 2,
    score: 1,
    reasons: [],
    outcome: { state: "pending", gain: null, drawdown: null, partial: false },
  };

  test("a row whose symbol is in a shelf says so, in its aria too", () => {
    const zones = { ROCK: shelf({ event: { state: "in_shelf", lo: 2.41, hi: 2.55, entry: 2.5, at: T, resolved: null } }) };
    const [v] = withZoneNotes([rocketRowView(row, T)], zones);
    expect(v!.zone?.label).toBe("In volume shelf $2.41–$2.55");
    expect(v!.aria).toContain("In volume shelf $2.41–$2.55; historically about 1 in 3 hold, 1 in 2 fail");
  });

  test("no zones (an older server) and garbage give no note", () => {
    const plain = rocketRowView(row, T);
    expect(withZoneNotes([plain], undefined)[0]).toBe(plain);
    expect(withZoneNotes([plain], { ROCK: "junk" } as never)[0]!.zone ?? null).toBeNull();
  });

  test("withZoneSections gives every row its symbol's note", () => {
    const panel: RocketPanelWire = {
      t: T,
      enabled: true,
      strategy: null,
      mode: "budget",
      budget: 10,
      threshold: null,
      today: { date: null, alerts: 1, hits: 0, hits30: 0, known: 0 },
      days: [],
      rows: [row],
      zones: { ROCK: shelf({ event: { state: "lost", lo: 2.41, hi: 2.55, entry: 2.5, at: T - 120_000, resolved: T } }) },
    };
    expect(rocketSectionsView(panel, T).runners[0]!.zone).toBeUndefined();
    expect(withZoneSections(rocketSectionsView(panel, T), panel.zones).runners[0]!.zone?.label).toBe("Shelf lost");
  });
});

describe("shelf odds below the studied range (review of 75d6418)", () => {
  test("a shelf under $0.15 quotes no historical odds; at $0.15 it does", () => {
    const at = (lo: number) => zoneNoteView({ s: "LOW", bands: [], event: { state: "in_shelf", lo, hi: lo + 0.01, entry: lo, at: 0, resolved: null } } as unknown as ZoneSymbolWire);
    expect(at(0.12)?.baseRate).toBe("");
    expect(at(0.12)?.aria).toContain("no historical odds");
    expect(at(0.15)?.baseRate).toBe(ZONE_BASE_RATE);
  });
});
