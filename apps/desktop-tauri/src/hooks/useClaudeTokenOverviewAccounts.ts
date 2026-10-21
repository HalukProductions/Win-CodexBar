import { useCallback, useEffect, useRef, useState } from "react";
import type { ProviderUsageSnapshot } from "../types/bridge";
import { getProviderAccountUsage, getTokenAccounts } from "../lib/tauri";
import type { ClaudeTokenOverviewAccounts } from "../lib/overviewAccountRows";
import { useTauriEvent } from "./useTauriEvent";

type ClaudeTokenView = ClaudeTokenOverviewAccounts;

const CLAUDE_PROVIDER_ID = "claude";

const EMPTY: ClaudeTokenView = { accounts: [], usage: {} };

/**
 * Saved Claude token accounts plus each account's cached last usage, for the
 * Overview account rows. Both reads are cache-only. Reloads when a Claude
 * refresh finishes or Claude's snapshot changes. Token edits emit no event, so
 * they show up on the next refresh.
 */
export function useClaudeTokenOverviewAccounts({ enabled = true }: { enabled?: boolean } = {}) {
  const [view, setView] = useState<ClaudeTokenView | null>(null);
  const requestRef = useRef(0);

  const load = useCallback(async () => {
    const request = ++requestRef.current;
    try {
      const [tokens, usage] = await Promise.all([
        getTokenAccounts(CLAUDE_PROVIDER_ID),
        getProviderAccountUsage(CLAUDE_PROVIDER_ID),
      ]);
      if (request !== requestRef.current) return;
      setView({
        accounts: tokens.accounts.map((account) => ({ id: account.id, label: account.label })),
        usage: usage.accounts,
      });
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

  useTauriEvent<ProviderUsageSnapshot>(
    "provider-updated",
    (event) => {
      if (enabled && event.payload.providerId === CLAUDE_PROVIDER_ID) void load();
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
