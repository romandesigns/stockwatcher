import { describe, expect, test } from "bun:test";
import { chooseSource, initialSymbol, probeServer, followCandidates, type ServerSymbols } from "./source.ts";

const data = (over: Partial<ServerSymbols> = {}): ServerSymbols => ({ mode: "replay", status: "running", source: "archive 2026-10-15 (fixture)", day: "2026-10-15", fixture: true, symbols: ["FXLO", "SMPL"], ...over });

const fakeFetch = (res: { status?: number; body?: unknown; type?: string } | Error): typeof fetch =>
  (async () => {
    if (res instanceof Error) throw res;
    return new Response(JSON.stringify(res.body ?? {}), { status: res.status ?? 200, headers: { "content-type": res.type ?? "application/json" } });
  }) as unknown as typeof fetch;

describe("chart source", () => {
  test("server is the default when it has symbols, and says if its data is fixture data", () => {
    const c = chooseSource(undefined, { ok: true, data: data() }, "SMPL");
    expect(c).toMatchObject({ kind: "server", fixtureData: true, mode: "replay", symbols: ["FXLO", "SMPL"], label: "Server replay · 2026-10-15" });
    expect(chooseSource("server", { ok: true, data: data({ fixture: false }) }, "SMPL").fixtureData).toBe(false);
    expect(initialSymbol(c, "SMPL")).toBe("SMPL");
    expect(initialSymbol({ ...c, symbols: ["ABC"] }, "SMPL")).toBe("ABC");
  });

  test("falls back to the in-browser fixture only when the server is unusable, with the reason", () => {
    expect(chooseSource(undefined, { ok: false, reason: "server unreachable (x)" }, "SMPL")).toMatchObject({ kind: "fixture", reason: "server unreachable (x)", fixtureData: true, symbols: ["SMPL"] });
    expect(chooseSource(undefined, { ok: true, data: data({ symbols: [], status: "no_data" }) }, "SMPL")).toMatchObject({ kind: "fixture", reason: "server has no data (no_data, replay mode)" });
    expect(chooseSource(undefined, { ok: true, data: data({ symbols: [], status: "not_configured", mode: "live" }) }, "SMPL").reason).toBe("server has no data (not_configured, live mode)");
    expect(chooseSource("fixture", { ok: true, data: data() }, "SMPL")).toMatchObject({ kind: "fixture", reason: "VITE_CHART_SOURCE=fixture" });
  });

  test("probe maps HTTP, non-JSON and network failures to reasons", async () => {
    expect(await probeServer(fakeFetch({ body: data() }))).toEqual({ ok: true, data: data() });
    // 401 means the server wants an access token: the app shows its sign-in card instead of the fixture.
    expect(await probeServer(fakeFetch({ status: 401 }))).toEqual({ ok: false, reason: "the server needs an access token", unauthorized: true });
    expect(await probeServer(fakeFetch({ status: 500 }))).toEqual({ ok: false, reason: "server answered HTTP 500" });
    expect(await probeServer(fakeFetch({ type: "text/html" }))).toMatchObject({ ok: false });
    expect(await probeServer(fakeFetch(new Error("ECONNREFUSED")))).toEqual({ ok: false, reason: "server unreachable (ECONNREFUSED)" });
  });
});

describe("followCandidates", () => {
  test("keeps the current symbol while it is a candidate", () => {
    expect(followCandidates("SMPL", ["FXLO", "SMPL"])).toBe("SMPL");
  });
  test("moves to the first candidate when the current one left the price band", () => {
    expect(followCandidates("FXHI", ["FXLO", "SMPL"])).toBe("FXLO");
  });
  test("keeps the current symbol when no candidates are known yet", () => {
    expect(followCandidates("SMPL", [])).toBe("SMPL");
  });
});
