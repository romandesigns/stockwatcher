import { type ClassValue, clsx } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

/**
 * A ≥44×44 px touch area on coarse pointers (phones, tablets; owner 2026-10-19), WCAG 2.5.5's size,
 * without changing how the control looks: an invisible ::after centred on the control, grown to
 * 44 px on any axis where the control is smaller (`min(0px, (100% - 44px) / 2)`: a negative inset
 * there, none where the control is already that big). The pseudo-element is part of the control
 * for hit testing, so a tap just outside the visible chip still lands on it. Mouse layouts never
 * see it. The host must be positioned (`relative`, or `absolute`/`fixed` already); an ancestor
 * that clips (overflow hidden) clips the area too, so a control inside one grows visibly instead.
 * Keep ≥4 px between neighbours that use it on a 40 px control so the areas do not overlap.
 */
export const TOUCH_HIT = "pointer-coarse:after:absolute pointer-coarse:after:inset-[min(0px,calc((100%_-_44px)/2))]";

/** TOUCH_HIT for a control in a row of touching segments: it grows only vertically, so neighbours never share a strip. */
export const TOUCH_HIT_Y = "pointer-coarse:after:absolute pointer-coarse:after:inset-x-0 pointer-coarse:after:inset-y-[min(0px,calc((100%_-_44px)/2))]";
