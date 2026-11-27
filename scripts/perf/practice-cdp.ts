// The Practice page's main-thread budget, measured in headless Edge over CDP (no Playwright).
//
// Why (the owner's recording of 2026-11-26, 17:24 ET): the Practice page stopped answering
// clicks at the replay's open, then read "Reconnecting" on every panel and never resumed. This
// script drives the page the way the owner did (play at 1×, 5×, 30×, switch tickers) and
// records what the page's main thread spends:
//
// - long tasks (PerformanceObserver `longtask`): count, total, the longest;
// - main-thread busy share (CDP Performance.getMetrics `TaskDuration` over wall time);
// - WebSocket frames per second and bytes per second, by frame type;
// - React commits per second (a minimal `__REACT_DEVTOOLS_GLOBAL_HOOK__`, which React reads in
//   production builds too);
// - reconnects (new sockets after the first) and seconds any panel read "Reconnecting";
// - the replay clock as the page shows it against the clock the server sends to a second,
//   bare socket (how far the page trails the market it shows).
//
// It FAILS (exit 1) when, in any phase at 1×–5×, a long task is over `--budget-ms` (50), the
// page reconnected, a panel said "Reconnecting" while the socket was up, or the Play button did
// not follow the server. 30× and the ticker switches are measured and reported, with a looser
// budget (`--switch-budget-ms`, 200). The `reconnect` phase drops the page's socket on purpose:
// it fails unless the page reconnects, every "Reconnecting" note is gone within a few seconds of
// it, and the replay plays on with the Play button saying so.
//
// Usage (a server running a replay day, its web app built into STATIC_DIR):
//   bun scripts/perf/practice-cdp.ts --base http://127.0.0.1:8797 [--secs 20] [--out report.json]
//     [--token <api token>] [--edge <msedge.exe>] [--port 9351] [--profile <dir>]
//     [--phases 1,5,30,switch,pause,reconnect] [--budget-ms 50] [--switch-budget-ms 200] [--width 1920 --height 1080]
//     [--no-fail]

type Json = Record<string, unknown>;

const args = new Map<string, string>();
{
  const a = process.argv.slice(2);
  for (let i = 0; i < a.length; i++) {
    const k = a[i]!;
    if (!k.startsWith("--")) continue;
    const v = a[i + 1] && !a[i + 1]!.startsWith("--") ? a[++i]! : "1";
    args.set(k.slice(2), v);
  }
}
const base = (args.get("base") ?? "http://127.0.0.1:8797").replace(/\/$/, "");
const wsBase = base.replace(/^http/, "ws");
const token = args.get("token") ?? process.env.STOCKWATCHER_API_TOKEN ?? null;
const edge = args.get("edge") ?? "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe";
const port = Number(args.get("port") ?? 9351);
const profile = args.get("profile") ?? `${process.env.TEMP ?? "."}/sw-practice-cdp-${port}`;
const secs = Number(args.get("secs") ?? 20);
const phases = (args.get("phases") ?? "1,5,30,switch,pause,reconnect").split(",").map((s) => s.trim()).filter(Boolean);
const budgetMs = Number(args.get("budget-ms") ?? 50);
const switchBudgetMs = Number(args.get("switch-budget-ms") ?? 200);
const width = Number(args.get("width") ?? 1920);
const height = Number(args.get("height") ?? 1080);
const out = args.get("out");
const noFail = args.has("no-fail");

// ---- the instrumentation injected before the app's first script --------------------------------
const INSTRUMENT = String.raw`(() => {
  const P = (window.__perf = { lt: [], msgs: 0, bytes: 0, commits: 0, sockets: 0, types: {}, closes: [] });
  try {
    new PerformanceObserver((l) => { for (const e of l.getEntries()) P.lt.push([Math.round(e.startTime), Math.round(e.duration)]); })
      .observe({ type: "longtask", buffered: true });
  } catch {}
  const Native = window.WebSocket;
  class Counted extends Native {
    constructor(u, p) {
      super(u, p);
      P.sockets++;
      P.ws = this;
      this.addEventListener("message", (e) => {
        const s = typeof e.data === "string" ? e.data : "";
        P.msgs++; P.bytes += s.length;
        const m = /^\{"type":"([^"]+)"/.exec(s) || /"type"\s*:\s*"([^"]+)"/.exec(s.slice(0, 200));
        const k = m ? m[1] : "?";
        const t = (P.types[k] ||= [0, 0]); t[0]++; t[1] += s.length;
      });
      this.addEventListener("close", (e) => P.closes.push([Math.round(performance.now()), e.code, e.reason]));
    }
  }
  window.WebSocket = Counted;
  // React calls inject() once per renderer and onCommitFiberRoot after every commit, in
  // production builds too. Nothing else of DevTools is emulated.
  window.__REACT_DEVTOOLS_GLOBAL_HOOK__ = {
    supportsFiber: true, isDisabled: false, renderers: new Map(),
    inject() { return 1; }, onCommitFiberRoot() { P.commits++; }, onCommitFiberUnmount() {}, onPostCommitFiberRoot() {}, checkDCE() {}, on() {}, off() {}, sub() { return () => {}; },
  };
})();`;

// What the page shows, read once a second.
const SAMPLE = String.raw`(() => {
  const P = window.__perf;
  const text = document.body ? document.body.innerText : "";
  const reconnecting = (text.match(/Reconnecting|Resynchronizing/g) || []).length;
  const play = document.querySelector('[aria-label="Pause replay"], [aria-label="Play replay"]');
  const clockEl = [...document.querySelectorAll("section[aria-label=Replay] *")].find((e) => e.childElementCount === 0 && /^\d\d:\d\d:\d\d$/.test(e.textContent || ""));
  return JSON.stringify({
    now: Math.round(performance.now()),
    lt: P.lt.length, ltSum: P.lt.reduce((a, b) => a + b[1], 0), ltMax: P.lt.reduce((a, b) => Math.max(a, b[1]), 0),
    msgs: P.msgs, bytes: P.bytes, commits: P.commits, sockets: P.sockets,
    reconnecting, playing: play ? play.getAttribute("aria-label") === "Pause replay" : null,
    clock: clockEl ? clockEl.textContent : null,
  });
})()`;

// ---- CDP plumbing -----------------------------------------------------------------------------
const proc = Bun.spawn(
  [edge, "--headless=new", `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`, "--disable-gpu", "--no-first-run", "--disable-extensions", `--window-size=${width},${height}`, "about:blank"],
  { stdout: "ignore", stderr: "ignore" },
);
let cdp: WebSocket | null = null;
for (let i = 0; i < 100 && !cdp; i++) {
  await Bun.sleep(200);
  try {
    const list = (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()) as Array<{ type: string; webSocketDebuggerUrl: string }>;
    const page = list.find((t) => t.type === "page");
    if (page) cdp = new WebSocket(page.webSocketDebuggerUrl);
  } catch {
    // Edge is still starting.
  }
}
if (!cdp) {
  proc.kill();
  throw new Error("headless Edge did not open a debugging port");
}
await new Promise((r) => cdp!.addEventListener("open", r));
let nextId = 0;
const waiting = new Map<number, (v: Json) => void>();
const exceptions: string[] = [];
cdp.addEventListener("message", (e) => {
  const m = JSON.parse(String(e.data)) as Json & { id?: number; method?: string; params?: Json };
  if (m.id && waiting.has(m.id)) {
    waiting.get(m.id)!(m);
    waiting.delete(m.id);
  } else if (m.method === "Runtime.exceptionThrown") {
    const d = (m.params as { exceptionDetails?: { text?: string; url?: string; exception?: { description?: string } } }).exceptionDetails;
    if (!String(d?.url ?? "").startsWith("chrome-extension://")) exceptions.push(d?.exception?.description ?? d?.text ?? "exception");
  }
});
const send = (method: string, params: Json = {}) =>
  new Promise<Json>((r) => {
    const i = ++nextId;
    waiting.set(i, r);
    cdp!.send(JSON.stringify({ id: i, method, params }));
  });
const evaluate = async <T>(expression: string): Promise<T> => {
  const r = (await send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true })) as { result?: { result?: { value?: unknown } } };
  return r.result?.result?.value as T;
};
const taskSeconds = async () => {
  const r = (await send("Performance.getMetrics")) as { result?: { metrics?: Array<{ name: string; value: number }> } };
  return r.result?.metrics?.find((m) => m.name === "TaskDuration")?.value ?? 0;
};

// ---- a bare socket: the server's own clock, and the controls -----------------------------------
interface ServerClock { now: number; playing: boolean; speed: unknown; wall: number }
let serverClock: ServerClock | null = null;
const control = new WebSocket(`${wsBase}/ws`);
await new Promise((r, j) => {
  control.addEventListener("open", r);
  control.addEventListener("error", j);
});
control.send(JSON.stringify({ type: "hello", client: "perf", source: "replay", ...(token ? { token } : {}) }));
control.addEventListener("message", (e) => {
  const s = String(e.data);
  if (s.startsWith('{"type":"ping"')) {
    const ts = JSON.parse(s).ts;
    control.send(JSON.stringify({ type: "pong", ts }));
  } else if (s.includes('"replay.clock"')) {
    const m = JSON.parse(s) as { clock: { now: number; playing: boolean; speed: unknown } };
    serverClock = { now: m.clock.now, playing: m.clock.playing, speed: m.clock.speed, wall: Date.now() };
  }
});
const replayControl = (action: string, value?: unknown) => control.send(JSON.stringify({ type: "replay.control", action, ...(value === undefined ? {} : { value }) }));
const etTime = (ms: number) => new Date(ms).toLocaleTimeString("en-US", { timeZone: "America/New_York", hour12: false });

// ---- the run ----------------------------------------------------------------------------------
await send("Page.enable");
await send("Runtime.enable");
await send("Performance.enable", { timeDomain: "threadTicks" });
await send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile: false });
await send("Page.addScriptToEvaluateOnNewDocument", { source: INSTRUMENT });
if (token) await send("Page.addScriptToEvaluateOnNewDocument", { source: `try{localStorage.setItem("sw-access-token", ${JSON.stringify(token)})}catch{}` });
await send("Page.navigate", { url: `${base}/#/chart` });

// Up when the replay bar's Play/Pause button is there and the page has a clock.
for (let i = 0; i < 120; i++) {
  await Bun.sleep(500);
  const s = JSON.parse((await evaluate<string>(SAMPLE)) ?? "{}") as { playing: boolean | null; clock: string | null };
  if (s.playing !== null && s.clock) break;
}
await Bun.sleep(3000);

interface Sample { now: number; lt: number; ltSum: number; ltMax: number; msgs: number; bytes: number; commits: number; sockets: number; reconnecting: number; playing: boolean | null; clock: string | null }
const sample = async () => JSON.parse(await evaluate<string>(SAMPLE)) as Sample;
const longTasksSince = async (t: number) => JSON.parse(await evaluate<string>(`JSON.stringify(window.__perf.lt.filter((x) => x[0] >= ${t}))`)) as Array<[number, number]>;
const typesNow = async () => JSON.parse(await evaluate<string>(`JSON.stringify(window.__perf.types)`)) as Record<string, [number, number]>;

interface PhaseResult {
  phase: string;
  seconds: number;
  longTasks: number;
  longTaskMs: number;
  longestMs: number;
  busyShare: number;
  msgsPerSec: number;
  kbPerSec: number;
  commitsPerSec: number;
  reconnects: number;
  reconnectingSecs: number;
  playMismatchSecs: number;
  serverVirtualPerWall: number | null;
  pageTrailsServerSecs: number | null;
  topTypes: Array<[string, number, number]>;
  budget: number;
  failures: string[];
}

const results: PhaseResult[] = [];
const clockSecs = (hms: string | null) => {
  if (!hms) return null;
  const [h, m, s] = hms.split(":").map(Number) as [number, number, number];
  return h * 3600 + m * 60 + s;
};

async function measure(phase: string, budget: number, during?: (second: number) => Promise<void>): Promise<PhaseResult> {
  const s0 = await sample();
  const t0 = await taskSeconds();
  const types0 = await typesNow();
  const wall0 = Date.now();
  const server0 = serverClock as ServerClock | null;
  let reconnectingSecs = 0;
  let playMismatchSecs = 0;
  let trail: number | null = null;
  for (let i = 0; i < secs; i++) {
    if (during) await during(i);
    await Bun.sleep(1000);
    const s = await sample();
    if (s.reconnecting > 0) reconnectingSecs++;
    const sc = serverClock as ServerClock | null;
    if (sc && s.playing !== null && s.playing !== sc.playing && Date.now() - sc.wall < 30_000) playMismatchSecs++;
    if (sc && s.clock) {
      const page = clockSecs(s.clock);
      const server = clockSecs(etTime(sc.now));
      if (page !== null && server !== null) trail = Math.max(trail ?? 0, server - page);
    }
  }
  const s1 = await sample();
  const t1 = await taskSeconds();
  const types1 = await typesNow();
  const wall = (Date.now() - wall0) / 1000;
  const lts = await longTasksSince(s0.now);
  const server1 = serverClock as ServerClock | null;
  const topTypes = Object.entries(types1)
    .map(([k, [n, b]]) => [k, n - (types0[k]?.[0] ?? 0), Math.round((b - (types0[k]?.[1] ?? 0)) / 1024)] as [string, number, number])
    .filter((x) => x[1] > 0)
    .sort((a, b) => b[2] - a[2])
    .slice(0, 8);
  const r: PhaseResult = {
    phase,
    seconds: Math.round(wall),
    longTasks: lts.length,
    longTaskMs: lts.reduce((a, b) => a + b[1], 0),
    longestMs: lts.reduce((a, b) => Math.max(a, b[1]), 0),
    busyShare: Math.round(((t1 - t0) / wall) * 100) / 100,
    msgsPerSec: Math.round((s1.msgs - s0.msgs) / wall),
    kbPerSec: Math.round((s1.bytes - s0.bytes) / 1024 / wall),
    commitsPerSec: Math.round(((s1.commits - s0.commits) / wall) * 10) / 10,
    reconnects: s1.sockets - s0.sockets,
    reconnectingSecs,
    playMismatchSecs,
    serverVirtualPerWall: server0 && server1 ? Math.round(((server1.now - server0.now) / 1000 / wall) * 10) / 10 : null,
    pageTrailsServerSecs: trail,
    topTypes,
    budget,
    failures: [],
  };
  if (r.longestMs > budget) r.failures.push(`a ${r.longestMs} ms long task (budget ${budget} ms)`);
  if (r.reconnects > 0) r.failures.push(`${r.reconnects} reconnect(s)`);
  if (r.reconnectingSecs > 0 && r.reconnects === 0) r.failures.push(`"Reconnecting" shown ${r.reconnectingSecs} s while connected`);
  if (r.playMismatchSecs > 2) r.failures.push(`Play button disagreed with the server for ${r.playMismatchSecs} s`);
  results.push(r);
  console.log(JSON.stringify(r));
  return r;
}

const clickTicker = (i: number) =>
  evaluate<string | null>(`(() => {
    // A Top Gainers / Hot stocks row (MoversPanel): a keyboard-style click loads it at once.
    const rows = [...document.querySelectorAll('button[aria-label$="Load into the selected chart"]')];
    const b = rows[${i} % Math.max(1, rows.length)];
    if (!b) return null;
    b.click();
    return (b.getAttribute("aria-label") || "").split(" ")[0];
  })()`);
const clickPlay = () =>
  evaluate<string | null>(`(() => { const b = document.querySelector('[aria-label="Pause replay"], [aria-label="Play replay"]'); if (!b) return null; const l = b.getAttribute("aria-label"); b.click(); return l; })()`);

async function reconnectPhase() {
  replayControl("speed", 1);
  replayControl("play");
  await Bun.sleep(1500);
  const before = await sample();
  await evaluate<null>(`(() => { window.__perf.ws.close(4999, "perf: dropped on purpose"); return null; })()`);
  // Reconnected, and how long any panel still said "Reconnecting" after.
  let back = -1;
  let clearedAfter = -1;
  for (let i = 0; i < 40; i++) {
    await Bun.sleep(500);
    const s = await sample();
    if (back < 0 && s.sockets > before.sockets && s.clock) back = i * 0.5;
    if (back >= 0 && s.reconnecting === 0) {
      clearedAfter = i * 0.5;
      break;
    }
  }
  await Bun.sleep(2000);
  const s = await sample();
  const sc = serverClock as ServerClock | null;
  const r: PhaseResult = {
    phase: "reconnect@1x", seconds: 0, longTasks: 0, longTaskMs: 0, longestMs: 0, busyShare: 0, msgsPerSec: 0, kbPerSec: 0, commitsPerSec: 0,
    reconnects: s.sockets - before.sockets, reconnectingSecs: clearedAfter, playMismatchSecs: 0, serverVirtualPerWall: null, pageTrailsServerSecs: null, topTypes: [], budget: 0, failures: [],
  };
  if (r.reconnects < 1) r.failures.push("the page did not reconnect");
  if (clearedAfter < 0) r.failures.push('"Reconnecting" still shown 20 s after the socket dropped');
  if (!sc?.playing) r.failures.push("the replay stopped playing over the reconnect");
  if (s.playing !== sc?.playing) r.failures.push(`after the reconnect the button says ${s.playing ? "playing" : "paused"}, the server ${sc?.playing ? "playing" : "paused"}`);
  console.log(`reconnect: back after ${back} s, "Reconnecting" gone ${clearedAfter} s after the drop; page playing=${s.playing}, server playing=${sc?.playing}`);
  results.push(r);
}

for (const phase of phases) {
  if (/^\d+$/.test(phase)) {
    replayControl("speed", Number(phase));
    replayControl("play");
    await Bun.sleep(1500);
    await measure(`${phase}x`, Number(phase) <= 5 ? budgetMs : switchBudgetMs);
  } else if (phase === "switch") {
    replayControl("speed", 1);
    replayControl("play");
    await Bun.sleep(1500);
    const picked: Array<string | null> = [];
    await measure("switch@1x", switchBudgetMs, async (i) => {
      if (i % 3 === 0) picked.push(await clickTicker(i / 3));
    });
    console.log(`switched to: ${picked.join(", ")}`);
  } else if (phase === "reconnect") {
    await reconnectPhase();
  } else if (phase === "pause") {
    // The owner's clicks: pause and play from the page's own button, at 1×, several times.
    replayControl("speed", 1);
    replayControl("play");
    await Bun.sleep(1500);
    const clicks: Array<string | null> = [];
    const r = await measure("play-pause@1x", budgetMs, async (i) => {
      if (i % 4 === 1) clicks.push(await clickPlay());
    });
    // The last click's effect has had time to land: the button and the server must agree.
    await Bun.sleep(1500);
    const s = await sample();
    const sc = serverClock as ServerClock | null;
    if (!sc || s.playing !== sc.playing) r.failures.push(`after the clicks the button says ${s.playing ? "playing" : "paused"}, the server ${sc?.playing ? "playing" : "paused"}`);
    console.log(`clicked: ${clicks.join(", ")}; page playing=${s.playing}, server playing=${sc?.playing}`);
  }
}

replayControl("speed", 1);
replayControl("play");
control.close();
cdp.close();
proc.kill();

const failed = results.filter((r) => r.failures.length > 0);
const report = { base, secs, budgetMs, switchBudgetMs, width, height, results, exceptions: exceptions.slice(0, 20) };
if (out) await Bun.write(out, JSON.stringify(report, null, 2));
console.log("");
console.log("phase          lt  lt-ms  max-ms  busy  msg/s  KB/s  commits/s  reconn  srv-x  trail");
for (const r of results) {
  console.log(
    `${r.phase.padEnd(14)}${String(r.longTasks).padStart(3)}${String(r.longTaskMs).padStart(7)}${String(r.longestMs).padStart(8)}${r.busyShare.toFixed(2).padStart(6)}${String(r.msgsPerSec).padStart(7)}${String(r.kbPerSec).padStart(6)}${String(r.commitsPerSec).padStart(11)}${String(r.reconnects).padStart(8)}${String(r.serverVirtualPerWall ?? "-").padStart(7)}${String(r.pageTrailsServerSecs ?? "-").padStart(7)}`,
  );
}
if (exceptions.length) console.log(`page exceptions: ${exceptions.length} (first: ${exceptions[0]})`);
if (failed.length) {
  console.log(`\nOVER BUDGET:\n${failed.map((r) => `  ${r.phase}: ${r.failures.join("; ")}`).join("\n")}`);
  if (!noFail) process.exit(1);
} else {
  console.log("\nwithin budget");
}
