import { useState } from "react";

/**
 * `value` while `keep` is true; the last value it had then while it is false (`initial` if it was
 * never kept). For content that must stay drawn while it animates away, and for a decision held
 * until its inputs settle. Stored by React's "state from the previous render" pattern, so a
 * changed value costs one immediate re-run of the caller, not an effect pass.
 */
export function useHeld<T>(value: T, keep: boolean, initial: T): T {
  const [held, setHeld] = useState<T>(keep ? value : initial);
  if (keep && held !== value) setHeld(value);
  return keep ? value : held;
}
