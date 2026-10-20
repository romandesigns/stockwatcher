// Owner, 2026-10-20: the phone app shows no scrollbars anywhere. Every scrolling view in the app
// and in chart-native must hide both indicators; this keeps a new one from bringing them back.
import { expect, test } from "bun:test";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

function files(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const p = join(dir, name);
    return statSync(p).isDirectory() ? files(p) : p.endsWith(".tsx") ? [p] : [];
  });
}

test("every ScrollView / FlatList / SectionList hides its scroll indicators", () => {
  const roots = [join(import.meta.dir), join(import.meta.dir, "../../../packages/chart-native/src")];
  const offenders: string[] = [];
  for (const f of roots.flatMap(files)) {
    const src = readFileSync(f, "utf8");
    for (const m of src.matchAll(/<(Animated\.ScrollView|Animated\.FlatList|ScrollView|FlatList|SectionList)(?=[\s>/])/g)) {
      const tag = src.slice(m.index, src.indexOf(">", m.index));
      if (!tag.includes("showsVerticalScrollIndicator={false}") || !tag.includes("showsHorizontalScrollIndicator={false}")) offenders.push(`${f}: <${m[1]}`);
    }
  }
  expect(offenders).toEqual([]);
});
