import { describe, expect, test } from "bun:test";
import { DEFAULT_SYMBOL, followCandidates, followServer, initialSymbol, probeServer, serverChoice, type ServerSymbols } from "./source.ts";

const data = (over: Partial<ServerSymbols> = {}): ServerSymbols => ({ mode: "replay", status: "running", source: "archive 2026-10-15 (fixture)", day: "2026-10-15", fixture: true, symbols: ["FXLO", "SMPL"], ...over });

const fakeFetch = (res: { status?: number; body?: unknown; type?: string } | Error): typeof fetch =>
  (async () => {
    if (res instanceof Error) throw res;
    return new Response(JSON.stringify(res.body ?? {}), { status: res.status ?? 200, headers: { "content-type": res.type ?? "application/json" } });
  }) as unknown as typeof fetch;

describe("chart source", () => {
  test("the server's answer, labelled, saying whether its data is fixture data", () => {
    const c = serverChoice(data());
    expect(c).toMatchObject({ fixtureData: true, mode: "replay", symbols: ["FXLO", "SMPL"], label: "Server replay · 2026-10-15", market: "replay" });
    expect(serverChoice(data({ fixture: false })).fixtureData).toBe(false);
    expect(initialSymbol(c, DEFAULT_SYMBOL)).toBe("SMPL");
    expect(initialSymbol({ ...c, symbols: ["ABC"] }, DEFAULT_SYMBOL)).toBe("ABC");
  });

  test("an empty server is still the server: never an invented stand-in", () => {
    // An empty live market (closed, or before the first trade) is real, and so is a replay with
    // nothing to play yet; the page shows that rather than made-up prices.
    expect(serverChoice(data({ symbols: [], status: "running", mode: "live" }), "live")).toMatchObject({ symbols: [], market: "live", label: "Server live · 2026-10-15" });
    expect(serverChoice(data({ symbols: [], status: "no_data" }))).toMatchObject({ symbols: [], reason: "archive 2026-10-15 (fixture)" });
  });

  test("probe maps HTTP, non-JSON and network failures to reasons", async () => {
    expect(await probeServer(fakeFetch({ body: data() }))).toEqual({ ok: true, data: data() });
    // 401 means the server wants an access token: the app shows its sign-in card (any other failure: "Server unreachable").
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

describe("followServer", () => {
  test("the source description follows a replay day switch", () => {
    const c = serverChoice(data());
    const after = followServer(c, data({ day: "2026-10-14", source: "archive 2026-10-14", fixture: false }));
    expect(after).toMatchObject({ label: "Server replay · 2026-10-14", reason: "archive 2026-10-14", fixtureData: false });
    // No answer yet (or a failed refresh): the last known description stays.
    expect(followServer(c, undefined)).toBe(c);
  });
});
