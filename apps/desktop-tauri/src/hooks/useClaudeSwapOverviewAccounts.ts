import { useCallback, useEffect, useRef, useState } from "react";
import type { ClaudeSwapOverviewAccountsBridge } from "../types/bridge";
import { getClaudeSwapOverviewAccounts } from "../lib/tauri";
import { useTauriEvent } from "./useTauriEvent";

/** Event the shell emits after a claude-swap/tcr pool read changes the cache. */
const CLAUDE_SWAP_USAGE_UPDATED = "claude-swap-usage-updated";

const EMPTY: ClaudeSwapOverviewAccountsBridge = { accounts: [], usage: {} };

/**
 * claude-swap/tcr pool accounts and their last recorded usage, for the Overview
 * account rows. The read is cache-only: the shell fills the cache whenever it
 * reads the pool, so this hook never starts a fetch. Reloads when the cache
 * changes or settings change.
 */
export function useClaudeSwapOverviewAccounts({ enabled = true }: { enabled?: boolean } = {}) {
  const [view, setView] = useState<ClaudeSwapOverviewAccountsBridge | null>(null);
  const requestRef = useRef(0);

  const load = useCallback(async () => {
    const request = ++requestRef.current;
    try {
      const next = await getClaudeSwapOverviewAccounts();
      if (request !== requestRef.current) return;
      setView(next);
    } catch {
      // Keep the last good view. A failed read leaves the plain Claude card in place.
    }
  }, []);

  useEffect(() => {
    if (enabled) {
      void load();
      return;
    }
    // Disabled: drop the view so re-enabling never shows removed accounts, and
    // invalidate any load still in flight so it cannot set the view afterwards.
    requestRef.current += 1;
    setView(null);
  }, [enabled, load]);

  useTauriEvent(
    CLAUDE_SWAP_USAGE_UPDATED,
    () => {
      if (enabled) void load();
    },
    [enabled, load],
  );

  useTauriEvent(
    "settings-changed",
    () => {
      if (enabled) void load();
    },
    [enabled, load],
  );

  return { ...(view ?? EMPTY), loaded: view !== null };
}
