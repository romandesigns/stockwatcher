// `bun dev`: the Rust server on :8790 and Vite on :5173 (which proxies
// /api and /ws to it). Either process exiting stops the other, so a
// crashed server never leaves a UI that silently shows "disconnected".
const procs = [
  // Dev only: replay a deterministic, clearly labelled fixture day when the
  // archive is empty, so the chart has data without any market credentials.
  Bun.spawn(["cargo", "run", "-p", "stockwatcher-server"], {
    stdio: ["inherit", "inherit", "inherit"],
    env: { ...process.env, SIM_AUTOFIXTURE: process.env.SIM_AUTOFIXTURE ?? "1" },
  }),
  Bun.spawn(["bun", "run", "--filter", "@stockwatcher/web", "dev"], { stdio: ["inherit", "inherit", "inherit"] }),
];

const stopAll = () => {
  for (const p of procs) p.kill();
};
process.on("SIGINT", stopAll);
process.on("SIGTERM", stopAll);

const code = await Promise.race(procs.map((p) => p.exited));
stopAll();
process.exit(code);
