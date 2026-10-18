// End-to-end check of settings sync against the REAL release server and the REAL client engine
// (docs/architecture/settings-sync.md): two clients, one change, the other sees it; per-drawing
// patches from both at once; a server restart keeps everything. Not part of CI (CI's Bun jobs
// have no Rust build); run it by hand after changing either half:
//
//   cargo build --release -p stockwatcher-server
//   bun scripts/settings-sync-e2e.ts            # SERVER_BIN=… to point at another binary
//
// It starts the server on a free port with a temp DATA_DIR and archive (fixture day), and
// deletes both afterwards.

import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { diffDrawings, drawingsFromDoc, drawingsNs, parseIndicators, SYNC_NS, type Drawing } from "../packages/chart-core/src/index.ts";
import { createSettingsSync, createWsClient, type ConnectionStatus, type SettingsSync, type WsClient } from "../packages/shared/src/index.ts";

const TOKEN = "e2e-token-0123456789abcdef0123456789";
const BIN = process.env.SERVER_BIN ?? join(process.env.CARGO_TARGET_DIR ?? "target", "release", process.platform === "win32" ? "stockwatcher-server.exe" : "stockwatcher-server");

const freePort = () => {
  const s = Bun.listen({ hostname: "127.0.0.1", port: 0, socket: { data() {} } });
  const p = s.port;
  s.stop(true);
  return p;
};

const root = mkdtempSync(join(tmpdir(), "sw-sync-e2e-"));
const port = freePort();
const base = `http://127.0.0.1:${port}`;

function startServer() {
  return Bun.spawn([BIN], {
    env: { ...process.env, PORT: String(port), HOST: "127.0.0.1", DATA_DIR: join(root, "data"), ARCHIVE_ROOT: join(root, "archive"), ARCHIVE_MIN_FREE_MB: "0", SIM_AUTOFIXTURE: "1", STATIC_DIR: join(root, "static"), STOCKWATCHER_API_TOKEN: TOKEN, RUST_LOG: "warn" },
    stdio: ["ignore", "inherit", "inherit"],
  });
}

async function waitFor<T>(what: string, check: () => T | undefined | false | Promise<T | undefined | false>, ms = 10_000): Promise<T> {
  const until = Date.now() + ms;
  for (;;) {
    const v = await check();
    if (v) return v;
    if (Date.now() > until) throw new Error(`timed out: ${what}`);
    await Bun.sleep(25);
  }
}

const healthy = () =>
  waitFor("server health", async () => {
    try {
      return (await fetch(`${base}/api/health`)).ok;
    } catch {
      return false;
    }
  }, 30_000);

interface Client {
  name: string;
  sync: SettingsSync;
  ws: WsClient;
  status: () => ConnectionStatus;
  snapshots: () => number;
}

function client(name: string, token = TOKEN): Client {
  let n = 1;
  const sync = createSettingsSync({ initial: null, newOpId: () => `${name}-${n++}` });
  let status: ConnectionStatus = "connecting";
  let snapshots = 0;
  const ws = createWsClient({
    url: `ws://127.0.0.1:${port}/ws`,
    token,
    client: `e2e-${name}`,
    settings: sync,
    baseDelayMs: 100,
    maxDelayMs: 500,
    onStatus: (s) => (status = s),
    onMessage: (m) => {
      if (m.type === "settings.snapshot") snapshots++;
    },
  });
  return { name, sync, ws, status: () => status, snapshots: () => snapshots };
}

const line = (x: number): Drawing => ({ id: `d${x}`, kind: "hline", a: { t: x, p: x }, b: { t: x, p: x } });
const drawIn = (c: Client, sym: string, next: Drawing[]) => {
  const p = diffDrawings(c.sync.get(drawingsNs(sym)), next);
  if (p) c.sync.patch(drawingsNs(sym), p.set, p.remove);
};
const ids = (c: Client, sym: string) => drawingsFromDoc(c.sync.get(drawingsNs(sym))).map((d) => d.id).join(",");
const ok = (msg: string) => console.log(`PASS  ${msg}`);

let server = startServer();
let failed = false;
try {
  await healthy();
  const a = client("A");
  const b = client("B");
  await waitFor("both snapshots", () => a.snapshots() > 0 && b.snapshots() > 0);

  // A turns on two indicators: B's state follows without anything on B asking for it.
  const view = b.sync.view(SYNC_NS.indicators, parseIndicators);
  let renders = 0;
  view.subscribe(() => renders++);
  const t0 = performance.now();
  a.sync.set(SYNC_NS.indicators, [{ id: "vwap", kind: "vwap" }, { id: "ema-9", kind: "ema", period: 9 }]);
  await waitFor("B sees A's indicators", () => view.get().length === 2);
  ok(`B received A's indicators in ${(performance.now() - t0).toFixed(0)} ms (B re-rendered ${renders}×, origin ${view.origin()})`);
  await waitFor("A's echo confirmed", () => a.sync.pendingCount() === 0);
  ok("A's own echo was matched by opId (nothing pending, no second write)");

  // Two devices, two different drawings of one symbol, at the same moment: both survive.
  drawIn(a, "SMPL", [line(1)]);
  await waitFor("B sees d1", () => ids(b, "SMPL") === "d1");
  drawIn(a, "SMPL", [line(1), line(2)]);
  drawIn(b, "SMPL", [line(1), line(3)]);
  await waitFor("both see d1,d2,d3", () => ids(a, "SMPL") === "d1,d2,d3" && ids(b, "SMPL") === "d1,d2,d3");
  ok("concurrent drawings from A and B both landed: d1,d2,d3 on both");
  drawIn(b, "SMPL", [line(2), line(3)]);
  await waitFor("A sees d1 removed", () => ids(a, "SMPL") === "d2,d3");
  ok("B's delete of d1 reached A");

  // A burst (a drag) is coalesced, the final value always arrives.
  for (let i = 0; i < 30; i++) a.sync.set(SYNC_NS.layers, { halts: i % 2 === 0, step: i });
  await waitFor("B sees the burst's final value", () => (b.sync.get(SYNC_NS.layers) as { step?: number } | undefined)?.step === 29);
  const rest = await (await fetch(`${base}/api/settings`, { headers: { Authorization: `Bearer ${TOKEN}` } })).json();
  const rev = (rest.docs as { ns: string; rev: number }[]).find((d) => d.ns === SYNC_NS.layers)!.rev;
  ok(`30 rapid layer changes reached B as the final value with ${rev} server writes`);

  // A wrong token gets nothing.
  const intruder = client("X", "wrong-token-0123456789abcdef0123456789");
  await waitFor("intruder refused", () => intruder.status() === "unauthorized");
  if (intruder.snapshots() !== 0) throw new Error("an unauthorized client got a snapshot");
  if ((await fetch(`${base}/api/settings`)).status !== 401) throw new Error("GET /api/settings without a token was not 401");
  ok("a bad token gets no settings (ws unauthorized, REST 401)");

  // Restart: the values persist, and B reconnects and resyncs by itself.
  server.kill();
  await server.exited;
  const offline = b.snapshots();
  b.sync.set(SYNC_NS.chartSettings, { chartType: "line" }); // made while the server is down
  server = startServer();
  await healthy();
  await waitFor("B reconnected", () => b.snapshots() > offline, 20_000);
  await waitFor("B's offline edit confirmed", () => b.sync.pendingCount() === 0);
  const c = client("C");
  await waitFor("C snapshot", () => c.snapshots() > 0);
  const got = parseIndicators(c.sync.get(SYNC_NS.indicators)).map((s) => s.id).join(",");
  if (got !== "vwap,ema-9") throw new Error(`indicators after restart: ${got}`);
  if (ids(c, "SMPL") !== "d2,d3") throw new Error(`drawings after restart: ${ids(c, "SMPL")}`);
  if ((c.sync.get(SYNC_NS.chartSettings) as { chartType?: string })?.chartType !== "line") throw new Error("B's offline edit did not reach the server");
  ok("after a server restart: indicators, drawings and B's offline edit are all there for a new client C");
  await waitFor("A follows B's offline edit", () => (a.sync.get(SYNC_NS.chartSettings) as { chartType?: string })?.chartType === "line", 20_000);
  ok("A reconnected by itself and picked up B's offline edit");

  for (const x of [a, b, c, intruder]) x.ws.close();
} catch (e) {
  failed = true;
  console.error(`FAIL  ${e instanceof Error ? e.message : String(e)}`);
} finally {
  server.kill();
  await server.exited;
  rmSync(root, { recursive: true, force: true });
}
process.exit(failed ? 1 : 0);
