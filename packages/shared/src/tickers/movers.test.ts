import { describe, expect, test } from "bun:test";
import { formatChangePct, formatDollarVolume } from "./movers.ts";


describe("formatting", () => {
  test("compact dollars", () => {
    expect(formatDollarVolume(950)).toBe("$950");
    expect(formatDollarVolume(41_200)).toBe("$41.2K");
    expect(formatDollarVolume(250_000)).toBe("$250K");
    expect(formatDollarVolume(3_240_000)).toBe("$3.2M");
    expect(formatDollarVolume(12_000_000)).toBe("$12M");
    expect(formatDollarVolume(1_500_000_000)).toBe("$1.5B");
    expect(formatDollarVolume(0)).toBe("$0");
  });

  test("signed change", () => {
    expect(formatChangePct(12.44)).toBe("+12.4%");
    expect(formatChangePct(-3.06)).toBe("−3.1%");
    expect(formatChangePct(0.01)).toBe("0.0%");
  });
});
