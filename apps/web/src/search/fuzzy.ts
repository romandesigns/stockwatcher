// Quick-open matching, VS Code style: the query's letters must appear in
// order (a subsequence), case-insensitive. Scored so the obvious match wins:
// a match at the start beats one later on, consecutive letters beat
// scattered ones, a letter starting a word beats one inside it, and a
// shorter target beats a longer one at an equal fit. Pure, so it is tested
// directly and reused by any list.

export interface FuzzyMatch {
  score: number;
  /** Indices of the matched characters in the target, for highlighting. */
  indices: number[];
}

/** The best match of `query` in `target`, or null when the letters are not all there in order. */
export function fuzzyMatch(query: string, target: string): FuzzyMatch | null {
  const q = query.toLowerCase().replace(/\s+/g, "");
  if (!q) return { score: 0, indices: [] };
  const t = target.toLowerCase();
  const indices: number[] = [];
  let score = 0;
  let from = 0;
  let prev = -2;
  for (const ch of q) {
    const i = t.indexOf(ch, from);
    if (i < 0) return null;
    indices.push(i);
    if (i === 0) score += 10;
    if (i === prev + 1) score += 6;
    const before = target[i - 1];
    if (i > 0 && (before === " " || before === "-" || before === ".")) score += 4;
    score += 1;
    prev = i;
    from = i + 1;
  }
  // Tighter spans and shorter targets rank higher at an equal fit.
  score -= (indices[indices.length - 1]! - indices[0]!) * 0.2;
  score -= target.length * 0.05;
  return { score, indices };
}

export interface Searchable {
  symbol: string;
  name: string;
}

export interface Ranked<T> {
  item: T;
  score: number;
  /** Matched indices in the symbol, and in the name (when the name matched instead). */
  symbolIdx: number[];
  nameIdx: number[];
}

/**
 * Ranks items by the better of a symbol match and a name match. A symbol
 * match is worth more than the same match on the name: traders type tickers.
 */
export function rankTickers<T extends Searchable>(items: readonly T[], query: string): Ranked<T>[] {
  const out: Ranked<T>[] = [];
  for (const item of items) {
    const s = fuzzyMatch(query, item.symbol);
    const n = fuzzyMatch(query, item.name);
    if (!s && !n) continue;
    const sScore = s ? s.score + 8 : -Infinity;
    const nScore = n ? n.score : -Infinity;
    if (sScore >= nScore) out.push({ item, score: sScore, symbolIdx: s!.indices, nameIdx: [] });
    else out.push({ item, score: nScore, symbolIdx: [], nameIdx: n!.indices });
  }
  return out.sort((a, b) => b.score - a.score || a.item.symbol.localeCompare(b.item.symbol));
}
