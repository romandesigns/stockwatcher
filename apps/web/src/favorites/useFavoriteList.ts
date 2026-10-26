import { useApp } from "../state/store.ts";

// The page's store needs `window` (its settings engine reads localStorage). Panels that show a
// star are also rendered to static markup in tests, where there is none: there the list is empty.
// Fixed per environment, so the hook order never changes between renders.
const HAS_WINDOW = typeof window !== "undefined";
const EMPTY: string[] = [];

/** The synced favorites list (app-state `favorites`), in the owner's order. */
export const useFavoriteList: () => string[] = HAS_WINDOW ? () => useApp((s) => s.favorites) : () => EMPTY;
