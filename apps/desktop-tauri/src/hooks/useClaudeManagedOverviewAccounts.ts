import { useCallback, useEffect, useRef, useState } from "react";
import type { ClaudeAccount } from "../types/bridge";
import { claudeAccountsList } from "../lib/tauri";
import type { ClaudeManagedOverviewAccounts } from "../lib/overviewAccountRows";
import { useTauriEvent } from "./useTauriEvent";

const EMPTY: ClaudeManagedOverviewAccounts = { accounts: [] };

/**
 * Saved Claude Code accounts (the Claude Code accounts switcher) with each
 * account's cached usage, for the Overview account rows. The read is the same
 * cache-only list the Claude settings pane shows. Reloads when the per-account
 * refresh publishes (`claude-accounts-updated`), when a refresh finishes and
 * when settings change.
 */
export function useClaudeManagedOverviewAccounts({ enabled = true }: { enabled?: boolean } = {}) {
  const [view, setView] = useState<ClaudeManagedOverviewAccounts | null>(null);
  const requestRef = useRef(0);

  const load = useCallback(async () => {
    const request = ++requestRef.current;
    try {
      const accounts: ClaudeAccount[] = await claudeAccountsList();
      if (request !== requestRef.current) return;
      // Only saved accounts can be switched to, so only they get rows.
      setView({ accounts: accounts.filter((account) => account.isSaved) });
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
    "claude-accounts-updated",
    () => {
      if (enabled) void load();
    },
    [enabled, load],
  );

  useTauriEvent(
    "refresh-complete",
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
