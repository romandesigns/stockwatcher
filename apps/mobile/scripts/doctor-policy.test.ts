import { describe, expect, test } from "bun:test";

import { judgeDoctorOutput } from "./doctor-policy";

const sameVersionDuplicates = `18/19 checks passed. 1 checks failed. Possible issues detected:
✖ Check that no duplicate dependencies are installed
Found duplicates for expo-font:
  ├─ expo-font@58.0.8 (at: node_modules/expo-font)
  │  └─ linked to: ../../node_modules/.bun/expo-font@58.0.8+aaaa/node_modules/expo-font
  └─ expo-font@58.0.8 (at: ../../node_modules/.bun/expo@58.0.14+aaaa/node_modules/expo-font)
     └─ linked to a different installation
`;

describe("judgeDoctorOutput", () => {
  test("a clean run passes", () => {
    expect(judgeDoctorOutput("19/19 checks passed.", 0).ok).toBe(true);
  });

  test("same-version duplicates from the isolated linker pass", () => {
    expect(judgeDoctorOutput(sameVersionDuplicates, 1)).toEqual({ ok: true, reasons: [] });
  });

  test("a real version conflict fails", () => {
    const output = sameVersionDuplicates.replace("expo-font@58.0.8 (at: ../../", "expo-font@57.0.1 (at: ../../");
    expect(judgeDoctorOutput(output, 1).ok).toBe(false);
  });

  test("any other failed check fails", () => {
    const output = `${sameVersionDuplicates}✖ Check that required peer dependencies are installed\n`;
    const verdict = judgeDoctorOutput(output, 1);
    expect(verdict.ok).toBe(false);
    expect(verdict.reasons).toEqual(["failed: Check that required peer dependencies are installed"]);
  });

  test("a non-zero exit with no parsed failure fails", () => {
    expect(judgeDoctorOutput("network error", 1).ok).toBe(false);
  });
});
