// `bun run doctor`: expo-doctor with the one tolerated failure explained in
// ./doctor-policy.ts. Prints the full doctor output either way.

import { judgeDoctorOutput } from "./doctor-policy";

const proc = Bun.spawn(["expo-doctor"], {
  cwd: `${import.meta.dir}/..`,
  stdout: "pipe",
  stderr: "pipe",
});
const [stdout, stderr, exitCode] = await Promise.all([
  new Response(proc.stdout).text(),
  new Response(proc.stderr).text(),
  proc.exited,
]);
const output = stdout + stderr;
process.stdout.write(output);

const verdict = judgeDoctorOutput(output, exitCode);
if (verdict.ok) {
  if (exitCode !== 0) {
    console.log("\nexpo-doctor: only same-version duplicates from Bun's isolated linker (accepted, see scripts/doctor-policy.ts).");
  }
} else {
  console.error(`\nexpo-doctor failed:\n- ${verdict.reasons.join("\n- ")}`);
  process.exit(1);
}
