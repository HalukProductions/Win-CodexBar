import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ClaudeSwapOverviewAccountsBridge } from "../types/bridge";

const tauriMocks = vi.hoisted(() => ({
  getClaudeSwapOverviewAccounts: vi.fn(),
}));

const eventMocks = vi.hoisted(() => ({
  listen: vi.fn(),
}));

vi.mock("../lib/tauri", () => tauriMocks);
vi.mock("@tauri-apps/api/event", () => eventMocks);

import { useClaudeSwapOverviewAccounts } from "./useClaudeSwapOverviewAccounts";

function view(ids: string[]): ClaudeSwapOverviewAccountsBridge {
  return {
    accounts: ids.map((id, index) => ({ id, slot: index + 1, label: `${id}@example.com` })),
    usage: {},
  };
}

describe("useClaudeSwapOverviewAccounts", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    eventMocks.listen.mockImplementation(() => Promise.resolve(() => {}));
  });

  it("loads the cached pool accounts and their usage", async () => {
    tauriMocks.getClaudeSwapOverviewAccounts.mockResolvedValue(view(["claude-swap:1"]));
    const { result } = renderHook(() => useClaudeSwapOverviewAccounts());
    await waitFor(() => expect(result.current.loaded).toBe(true));
    expect(result.current.accounts.map((account) => account.id)).toEqual(["claude-swap:1"]);
    expect(tauriMocks.getClaudeSwapOverviewAccounts).toHaveBeenCalledTimes(1);
  });

  it("never reads the cache while disabled", async () => {
    const { result } = renderHook(() => useClaudeSwapOverviewAccounts({ enabled: false }));
    expect(result.current.loaded).toBe(false);
    expect(result.current.accounts).toEqual([]);
    expect(tauriMocks.getClaudeSwapOverviewAccounts).not.toHaveBeenCalled();
  });

  it("drops the loaded view when disabled, so re-enabling never shows removed accounts", async () => {
    tauriMocks.getClaudeSwapOverviewAccounts.mockResolvedValue(view(["claude-swap:1"]));
    const { result, rerender } = renderHook(
      ({ enabled }: { enabled: boolean }) => useClaudeSwapOverviewAccounts({ enabled }),
      { initialProps: { enabled: true } },
    );
    await waitFor(() => expect(result.current.loaded).toBe(true));

    rerender({ enabled: false });
    expect(result.current.loaded).toBe(false);
    expect(result.current.accounts).toEqual([]);

    // The account was removed while disabled. Re-enabling must not show it
    // before the reload finishes.
    tauriMocks.getClaudeSwapOverviewAccounts.mockResolvedValue(view(["claude-swap:2"]));
    rerender({ enabled: true });
    expect(result.current.loaded).toBe(false);
    expect(result.current.accounts).toEqual([]);

    await waitFor(() => expect(result.current.loaded).toBe(true));
    expect(result.current.accounts.map((account) => account.id)).toEqual(["claude-swap:2"]);
  });

  it("ignores a load that finishes after the hook was disabled", async () => {
    let resolveView: (value: ClaudeSwapOverviewAccountsBridge) => void = () => {};
    tauriMocks.getClaudeSwapOverviewAccounts.mockImplementation(
      () =>
        new Promise<ClaudeSwapOverviewAccountsBridge>((resolve) => {
          resolveView = resolve;
        }),
    );
    const { result, rerender } = renderHook(
      ({ enabled }: { enabled: boolean }) => useClaudeSwapOverviewAccounts({ enabled }),
      { initialProps: { enabled: true } },
    );
    await waitFor(() => expect(tauriMocks.getClaudeSwapOverviewAccounts).toHaveBeenCalled());

    rerender({ enabled: false });
    await act(async () => {
      resolveView(view(["claude-swap:1"]));
    });

    expect(result.current.loaded).toBe(false);
    expect(result.current.accounts).toEqual([]);
  });

  it("keeps the last good view when a reload fails", async () => {
    tauriMocks.getClaudeSwapOverviewAccounts.mockResolvedValueOnce(view(["claude-swap:1"]));
    const { result } = renderHook(() => useClaudeSwapOverviewAccounts());
    await waitFor(() => expect(result.current.loaded).toBe(true));

    tauriMocks.getClaudeSwapOverviewAccounts.mockRejectedValueOnce(new Error("cache read failed"));
    const usageUpdated = eventMocks.listen.mock.calls.find(
      ([name]) => name === "claude-swap-usage-updated",
    );
    expect(usageUpdated).toBeDefined();
    await act(async () => {
      // Fire the reload event the hook subscribed to.
      const handler = usageUpdated?.[1] as (() => void) | undefined;
      handler?.();
    });

    await waitFor(() => expect(tauriMocks.getClaudeSwapOverviewAccounts).toHaveBeenCalledTimes(2));
    expect(result.current.loaded).toBe(true);
    expect(result.current.accounts.map((account) => account.id)).toEqual(["claude-swap:1"]);
  });
});
