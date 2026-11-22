import { describe, expect, test } from "bun:test";
import { DETECTOR_SWITCHES, DETECTORS_NS, detectorPatch, parseDetectorSwitches } from "./detectors.ts";

describe("runtime detector switches", () => {
  test("on unless the entry says off, as the server reads it", () => {
    expect(DETECTORS_NS).toBe("detectors");
    expect(DETECTOR_SWITCHES.map((d) => d.id)).toEqual(["x-float-1"]);
    expect(parseDetectorSwitches(undefined)).toEqual({ "x-float-1": true });
    expect(parseDetectorSwitches("junk")).toEqual({ "x-float-1": true });
    expect(parseDetectorSwitches({ "x-float-1": { on: true } })).toEqual({ "x-float-1": true });
    expect(parseDetectorSwitches({ "x-float-1": { note: 1 } })).toEqual({ "x-float-1": true });
    expect(parseDetectorSwitches({ "x-float-1": { on: false } })).toEqual({ "x-float-1": false });
    expect(parseDetectorSwitches({ "x-float-1": false, other: false })).toEqual({ "x-float-1": false });
  });

  test("a flip is one entry of the patch", () => {
    expect(detectorPatch("x-float-1", false)).toEqual({ "x-float-1": { on: false } });
  });
});
