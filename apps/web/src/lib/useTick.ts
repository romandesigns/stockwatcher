import { useEffect, useState } from "react";

/** Re-renders every `ms` so ages ("now", "3m") keep moving between frames. */
export function useTick(ms: number): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), ms);
    return () => clearInterval(id);
  }, [ms]);
  return now;
}
