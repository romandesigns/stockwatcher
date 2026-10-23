import { expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import { PressureGauge } from "./PressureGauge.tsx";

test("stale pressure uses the neutral fill, last-known caption and no gauge animation", () => {
  const live = renderToStaticMarkup(<PressureGauge value={82} side="bull" halted={false} label="Risk" />);
  expect(live).toContain("var(--good)");
  expect(live).toContain("transition-[stroke-dasharray]");
  const stale = renderToStaticMarkup(<PressureGauge value={82} side="bear" halted={false} stale label="Reconnecting. Last known risk" />);
  expect(stale).toContain("last known risk");
  expect(stale).toContain('aria-label="Reconnecting. Last known risk"');
  expect(stale).toContain("var(--text-faint)");
  expect(stale).not.toContain("var(--critical)");
  expect(stale).not.toContain("transition-[stroke-dasharray]");
  expect(stale).not.toContain(">halted<");
});
