import { useEffect, useState } from "react";

/** Default re-read period for relative labels such as "1 minute ago". */
export const NOW_TICK_MS = 15_000;

/**
 * The current time in Unix ms, re-read every `intervalMs` so relative
 * timestamps ("Updated 1 minute ago") advance while the data stays unchanged.
 * Each call owns one interval, cleared on unmount.
 */
export function useNow(intervalMs: number = NOW_TICK_MS): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), intervalMs);
    return () => window.clearInterval(id);
  }, [intervalMs]);
  return now;
}
