import { describe, expect, test } from "bun:test";
import { DETECTOR_SWITCHES, DETECTORS_NS, detectorPatch, parseDetectorSwitches } from "./detectors.ts";

// The server's registry (crates/server/src/alerts/detectors.rs `ALL`): ids and defaults.
const DEFAULTS = { "x-gap-1": true, "x-ctx-1": true, "x-float-1": true, "x-halt-1": true, "rw-mp5-1": true, "rw2-vwapr-1": true, "rw2-level-1": true, "b-good-2": true, "b-good-1": true, run: false };

describe("runtime detector switches", () => {
  test("each takes its entry, else its default, as the server reads it", () => {
    expect(DETECTORS_NS).toBe("detectors");
    expect(DETECTOR_SWITCHES.map((d) => d.id).sort()).toEqual(Object.keys(DEFAULTS).sort());
    expect(parseDetectorSwitches(undefined)).toEqual(DEFAULTS);
    expect(parseDetectorSwitches("junk")).toEqual(DEFAULTS);
    expect(parseDetectorSwitches({ "x-float-1": { on: true } })).toEqual(DEFAULTS);
    expect(parseDetectorSwitches({ "x-float-1": { note: 1 } })).toEqual(DEFAULTS);
    expect(parseDetectorSwitches({ "x-float-1": { on: false } })).toEqual({ ...DEFAULTS, "x-float-1": false });
    expect(parseDetectorSwitches({ "x-float-1": false, other: false })).toEqual({ ...DEFAULTS, "x-float-1": false });
    // The cut kind stays off until an entry turns it on.
    expect(parseDetectorSwitches({ run: { on: true } }).run).toBe(true);
  });

  test("a flip is one entry of the patch", () => {
    expect(detectorPatch("x-float-1", false)).toEqual({ "x-float-1": { on: false } });
  });
});
