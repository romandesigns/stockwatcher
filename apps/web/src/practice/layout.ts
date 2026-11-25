import type { SideColumn } from "../dashboard/DashboardPanels.tsx";

/**
 * The practice page's right column (owner, 2026-11-25): it starts narrower than the dashboard's
 * 400px so the one practice chart gets the larger share (at 1280px the 400px column left the
 * chart barely two thirds of the row). Still draggable, and kept under its own key, so widening
 * it here never moves the live dashboard's column and the dashboard's never moves this one.
 */
export const PRACTICE_SIDE: SideColumn = { defaultWidth: 300, key: "stockwatcher.practiceSideColumnWidth" };
