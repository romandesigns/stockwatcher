import { createContext, useContext } from "react";

/** The rail's collapsed state and toggle, from the Shell; null where there is no rail (phone layout). */
export const NavRailContext = createContext<{ collapsed: boolean; toggle: () => void } | null>(null);

/** True when the page sits beside the rail (so the rail, not the page, carries Settings). */
export const useHasRail = () => useContext(NavRailContext) !== null;
