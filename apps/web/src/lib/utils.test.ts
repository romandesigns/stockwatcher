import { expect, test } from "bun:test";
import { cn } from "./utils.ts";

test("cn joins classes and lets later Tailwind utilities win", () => {
  expect(cn("p-2", undefined, "text-sm")).toBe("p-2 text-sm");
  expect(cn("p-2", "p-4")).toBe("p-4");
});
