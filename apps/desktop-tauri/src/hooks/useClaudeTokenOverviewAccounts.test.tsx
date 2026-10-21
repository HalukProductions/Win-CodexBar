import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { getProviderAccountUsage, getTokenAccounts } from "../lib/tauri";

const tauriMocks = vi.hoisted(() => ({
  getTokenAccounts: vi.fn(),
  getProviderAccountUsage: vi.fn(),
}));

const eventMocks = vi.hoisted(() => ({
  listen: vi.fn(),
}));

vi.mock("../lib/tauri", () => tauriMocks);
vi.mock("@tauri-apps/api/event", () => eventMocks);

import { useClaudeTokenOverviewAccounts } from "./useClaudeTokenOverviewAccounts";

type TokenList = Awaited<ReturnType<typeof getTokenAccounts>>;
type UsageList = Awaited<ReturnType<typeof getProviderAccountUsage>>;

/** The hook reads only `accounts` from the token response. */
function tokenList(accounts: Array<{ id: string; label: string }>): TokenList {
  return { accounts } as unknown as TokenList;
}

const EMPTY_USAGE: UsageList = { providerId: "claude", accounts: {} };

describe("useClaudeTokenOverviewAccounts", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    eventMocks.listen.mockImplementation(() => Promise.resolve(() => {}));
    tauriMocks.getProviderAccountUsage.mockResolvedValue(EMPTY_USAGE);
  });

  it("drops the loaded view when disabled, so re-enabling never shows removed accounts", async () => {
    tauriMocks.getTokenAccounts.mockResolvedValue(tokenList([{ id: "a", label: "Work" }]));
    const { result, rerender } = renderHook(
      ({ enabled }: { enabled: boolean }) => useClaudeTokenOverviewAccounts({ enabled }),
      { initialProps: { enabled: true } },
    );
    await waitFor(() => expect(result.current.loaded).toBe(true));
    expect(result.current.accounts.map((account) => account.id)).toEqual(["a"]);

    rerender({ enabled: false });
    expect(result.current.loaded).toBe(false);
    expect(result.current.accounts).toEqual([]);

    // Account "a" was removed while disabled. Re-enabling must not show it
    // before the reload finishes.
    tauriMocks.getTokenAccounts.mockResolvedValue(tokenList([{ id: "b", label: "Personal" }]));
    rerender({ enabled: true });
    expect(result.current.loaded).toBe(false);
    expect(result.current.accounts).toEqual([]);

    await waitFor(() => expect(result.current.loaded).toBe(true));
    expect(result.current.accounts.map((account) => account.id)).toEqual(["b"]);
  });

  it("ignores a load that finishes after the hook was disabled", async () => {
    let resolveTokens: (value: TokenList) => void = () => {};
    tauriMocks.getTokenAccounts.mockImplementation(
      () =>
        new Promise<TokenList>((resolve) => {
          resolveTokens = resolve;
        }),
    );
    const { result, rerender } = renderHook(
      ({ enabled }: { enabled: boolean }) => useClaudeTokenOverviewAccounts({ enabled }),
      { initialProps: { enabled: true } },
    );
    await waitFor(() => expect(tauriMocks.getTokenAccounts).toHaveBeenCalled());

    rerender({ enabled: false });
    await act(async () => {
      resolveTokens(tokenList([{ id: "a", label: "Work" }]));
    });

    expect(result.current.loaded).toBe(false);
    expect(result.current.accounts).toEqual([]);
  });
});
