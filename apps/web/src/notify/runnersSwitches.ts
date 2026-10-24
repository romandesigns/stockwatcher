import { ALERT_SWITCH_SECTIONS, type AlertSwitch } from "@stockwatcher/shared";

/**
 * The phone's switches, by panel, in its order and words (packages/shared
 * `ALERT_SWITCH_SECTIONS`, which apps/mobile src/settings/rocketWatch.tsx reads too), so the three
 * clients read the same.
 */
export const RUNNERS_SWITCH_SECTIONS = ALERT_SWITCH_SECTIONS;

/** Every switch, flat, in section order. */
export const RUNNERS_SWITCHES: readonly AlertSwitch[] = ALERT_SWITCH_SECTIONS.flatMap((s) => s.items);
