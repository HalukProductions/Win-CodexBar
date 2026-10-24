import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ApiKeyInfoBridge } from "../../../types/bridge";

const mocks = vi.hoisted(() => ({
  getApiKeyProviders: vi.fn(),
  getApiKeys: vi.fn(),
  openProviderDashboard: vi.fn(),
  removeApiKey: vi.fn(),
  setApiKey: vi.fn(),
}));
const events = vi.hoisted(() => ({
  listen: vi.fn<(event: string, listener: (event: { payload: unknown }) => void) => Promise<() => void>>(),
}));

vi.mock("../../../lib/tauri", () => mocks);
vi.mock("@tauri-apps/api/event", () => events);
vi.mock("../../../hooks/useLocale", () => ({
  useLocale: () => ({ t: (key: string) => key }),
}));

import { ApiKeySection } from "./ApiKeySection";

const openrouter = {
  id: "openrouter",
  displayName: "OpenRouter",
  envVar: null,
  help: null,
  dashboardUrl: null,
};
const savedKey: ApiKeyInfoBridge = {
  providerId: "openrouter",
  provider: "OpenRouter",
  maskedKey: "sk-or-...t9Qz",
  savedAt: "2026-05-01 10:00",
  label: null,
};

function listenerFor(event: string): (event: { payload: unknown }) => void {
  const call = events.listen.mock.calls.find(([name]) => name === event);
  if (!call) throw new Error(`no listener registered for ${event}`);
  return call[1];
}

describe("ApiKeySection external changes", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    mocks.getApiKeyProviders.mockResolvedValue([openrouter]);
    mocks.getApiKeys.mockResolvedValue([]);
    events.listen.mockResolvedValue(() => {});
  });

  it("re-reads the key list when the backend announces a change", async () => {
    render(<ApiKeySection providerId="openrouter" />);
    await screen.findByText("ApiKeyNotSet");

    // A save from another window (or the CLI followed by a refresh) lands here.
    mocks.getApiKeys.mockResolvedValue([savedKey]);
    await act(async () => {
      listenerFor("api-keys-updated")({ payload: { providerId: "openrouter" } });
    });

    expect(await screen.findByText("KeySet")).toBeInTheDocument();
    expect(screen.getByText(savedKey.maskedKey)).toBeInTheDocument();
  });

  it("re-reads the key list when the window regains focus", async () => {
    render(<ApiKeySection providerId="openrouter" />);
    await screen.findByText("ApiKeyNotSet");
    expect(mocks.getApiKeys).toHaveBeenCalledTimes(1);

    // A CLI write can't emit, so focus is the fallback signal.
    mocks.getApiKeys.mockResolvedValue([savedKey]);
    await act(async () => {
      window.dispatchEvent(new Event("focus"));
    });

    expect(await screen.findByText("KeySet")).toBeInTheDocument();
  });

  it("keeps an open edit form and its draft when a reload arrives", async () => {
    render(<ApiKeySection providerId="openrouter" />);
    await screen.findByText("ApiKeyNotSet");
    fireEvent.click(screen.getByRole("button", { name: "AddKey" }));
    fireEvent.change(screen.getByPlaceholderText("PasteApiKeyHere"), {
      target: { value: "sk-or-draft" },
    });

    await act(async () => {
      listenerFor("api-keys-updated")({ payload: { providerId: "openrouter" } });
    });

    await waitFor(() => expect(mocks.getApiKeys).toHaveBeenCalledTimes(2));
    expect(screen.getByPlaceholderText("PasteApiKeyHere")).toHaveValue("sk-or-draft");
  });

  it("keeps the newest reload when overlapping reloads finish out of order", async () => {
    render(<ApiKeySection providerId="openrouter" />);
    await screen.findByText("ApiKeyNotSet");

    let resolveOlder: (keys: ApiKeyInfoBridge[]) => void = () => {};
    let resolveNewer: (keys: ApiKeyInfoBridge[]) => void = () => {};
    mocks.getApiKeys
      .mockImplementationOnce(() => new Promise((resolve) => { resolveOlder = resolve; }))
      .mockImplementationOnce(() => new Promise((resolve) => { resolveNewer = resolve; }));

    await act(async () => {
      window.dispatchEvent(new Event("focus"));
      listenerFor("api-keys-updated")({ payload: { providerId: "openrouter" } });
    });
    await act(async () => {
      resolveNewer([savedKey]);
    });
    expect(await screen.findByText("KeySet")).toBeInTheDocument();

    await act(async () => {
      resolveOlder([]);
    });
    expect(screen.getByText("KeySet")).toBeInTheDocument();
    expect(screen.queryByText("ApiKeyNotSet")).not.toBeInTheDocument();
  });

  it("stops listening and ignores focus after unmount", async () => {
    const unlisten = vi.fn();
    events.listen.mockResolvedValue(unlisten);
    const { unmount } = render(<ApiKeySection providerId="openrouter" />);
    await screen.findByText("ApiKeyNotSet");
    unmount();

    await waitFor(() => expect(unlisten).toHaveBeenCalledTimes(1));
    const callsBefore = mocks.getApiKeys.mock.calls.length;
    window.dispatchEvent(new Event("focus"));
    expect(mocks.getApiKeys.mock.calls.length).toBe(callsBefore);
  });
});
