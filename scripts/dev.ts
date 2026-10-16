// `bun dev`: the Rust server on :8790 and Vite on :5173 (which proxies
// /api and /ws to it). Either process exiting stops the other, so a
// crashed server never leaves a UI that silently shows "disconnected".
const procs = [
  Bun.spawn(["cargo", "run", "-p", "stockwatcher-server"], { stdio: ["inherit", "inherit", "inherit"] }),
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
