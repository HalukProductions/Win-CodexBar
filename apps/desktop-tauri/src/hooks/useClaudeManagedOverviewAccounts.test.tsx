import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ClaudeAccount } from "../types/bridge";

const tauriMocks = vi.hoisted(() => ({
  claudeAccountsList: vi.fn(),
}));

const eventMocks = vi.hoisted(() => ({
  listen: vi.fn(),
  listeners: new Map<string, () => void>(),
}));

vi.mock("../lib/tauri", () => tauriMocks);
vi.mock("@tauri-apps/api/event", () => eventMocks);

import { useClaudeManagedOverviewAccounts } from "./useClaudeManagedOverviewAccounts";

function account(id: string, overrides: Partial<ClaudeAccount> = {}): ClaudeAccount {
  return {
    id,
    email: `${id}@example.com`,
    organization: null,
    plan: "max",
    isActive: false,
    isSaved: true,
    usage: null,
    ...overrides,
  };
}

describe("useClaudeManagedOverviewAccounts", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    eventMocks.listeners.clear();
    eventMocks.listen.mockImplementation((name: string, handler: () => void) => {
      eventMocks.listeners.set(name, handler);
      return Promise.resolve(() => {});
    });
  });

  it("keeps only saved accounts, since only they can be switched to", async () => {
    tauriMocks.claudeAccountsList.mockResolvedValue([
      account("live", { isActive: true, isSaved: false }),
      account("a"),
      account("b"),
    ]);
    const { result } = renderHook(() => useClaudeManagedOverviewAccounts());
    await waitFor(() => expect(result.current.loaded).toBe(true));
    expect(result.current.accounts.map((a) => a.id)).toEqual(["a", "b"]);
  });

  it("reloads when the per-account refresh publishes new usage", async () => {
    tauriMocks.claudeAccountsList.mockResolvedValue([account("a")]);
    const { result } = renderHook(() => useClaudeManagedOverviewAccounts());
    await waitFor(() => expect(result.current.loaded).toBe(true));

    const updated = account("a", {
      usage: {
        fiveHour: { usedPercent: 12, resetsAt: null },
        sevenDay: null,
        updatedAt: "2026-11-01T20:00:00Z",
      },
    });
    tauriMocks.claudeAccountsList.mockResolvedValue([updated, account("b")]);
    await waitFor(() => expect(eventMocks.listeners.has("claude-accounts-updated")).toBe(true));
    await act(async () => {
      eventMocks.listeners.get("claude-accounts-updated")?.();
    });
    await waitFor(() => expect(result.current.accounts).toHaveLength(2));
    expect(result.current.accounts[0].usage?.fiveHour?.usedPercent).toBe(12);
  });

  it("drops the view when disabled and never reads while disabled", async () => {
    tauriMocks.claudeAccountsList.mockResolvedValue([account("a"), account("b")]);
    const { result, rerender } = renderHook(
      ({ enabled }: { enabled: boolean }) => useClaudeManagedOverviewAccounts({ enabled }),
      { initialProps: { enabled: true } },
    );
    await waitFor(() => expect(result.current.loaded).toBe(true));

    tauriMocks.claudeAccountsList.mockClear();
    rerender({ enabled: false });
    expect(result.current.loaded).toBe(false);
    expect(result.current.accounts).toEqual([]);
    await act(async () => {
      eventMocks.listeners.get("claude-accounts-updated")?.();
    });
    expect(tauriMocks.claudeAccountsList).not.toHaveBeenCalled();
  });

  it("keeps the last good view when a reload fails", async () => {
    tauriMocks.claudeAccountsList.mockResolvedValue([account("a"), account("b")]);
    const { result } = renderHook(() => useClaudeManagedOverviewAccounts());
    await waitFor(() => expect(result.current.loaded).toBe(true));

    tauriMocks.claudeAccountsList.mockRejectedValue(new Error("unreadable store"));
    await waitFor(() => expect(eventMocks.listeners.has("refresh-complete")).toBe(true));
    await act(async () => {
      eventMocks.listeners.get("refresh-complete")?.();
    });
    expect(result.current.accounts.map((a) => a.id)).toEqual(["a", "b"]);
  });
});
