import { describe, expect, test } from "bun:test";
import { nudgeToolbox, parseToolboxPlacement, toolboxPlacement, toolboxPosition, DEFAULT_TOOLBOX } from "./toolbox.ts";

const box = { width: 40, height: 300 };

describe("floating toolbox placement", () => {
  test("fractions map to the room left after the palette, inside an inset", () => {
    const c = { width: 1000, height: 600 };
    expect(toolboxPosition({ x: 0, y: 0, orientation: "vertical" }, c, box)).toEqual({ left: 6, top: 6 });
    expect(toolboxPosition({ x: 1, y: 1, orientation: "vertical" }, c, box)).toEqual({ left: 1000 - 40 - 6, top: 600 - 300 - 6 });
  });

  test("dragging round-trips and clamps inside the chart", () => {
    const c = { width: 1000, height: 600 };
    const p = toolboxPlacement(500, 120, c, box, "vertical");
    const at = toolboxPosition(p, c, box);
    expect(at.left).toBeCloseTo(500, 9);
    expect(at.top).toBeCloseTo(120, 9);
    expect(toolboxPlacement(-80, 9000, c, box, "vertical")).toEqual({ x: 0, y: 1, orientation: "vertical" });
  });

  test("a parked corner stays the corner when the chart resizes", () => {
    const p = toolboxPlacement(1000, 600, { width: 1000, height: 600 }, box, "vertical");
    expect(toolboxPosition(p, { width: 400, height: 500 }, box)).toEqual({ left: 400 - 40 - 6, top: 500 - 300 - 6 });
  });

  test("nudge moves by pixels, and bad stored values fall back safely", () => {
    const c = { width: 1000, height: 600 };
    const p = nudgeToolbox({ x: 0, y: 0, orientation: "vertical" }, 32, 8, c, box);
    expect(toolboxPosition(p, c, box)).toEqual({ left: 38, top: 14 });
    expect(parseToolboxPlacement("junk", DEFAULT_TOOLBOX)).toEqual(DEFAULT_TOOLBOX);
    expect(parseToolboxPlacement({ x: 4, y: "a", orientation: "sideways" }, DEFAULT_TOOLBOX)).toEqual({ x: 1, y: 0, orientation: "vertical" });
  });
});
