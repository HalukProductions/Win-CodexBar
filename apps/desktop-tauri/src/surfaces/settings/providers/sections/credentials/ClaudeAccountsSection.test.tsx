import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ClaudeAccount } from "../../../../../types/bridge";

const mocks = vi.hoisted(() => ({
  claudeAccountsList: vi.fn(), claudeAccountAdd: vi.fn(), claudeAccountCancelLogin: vi.fn(),
  claudeAccountSaveCurrent: vi.fn(), claudeAccountRemove: vi.fn(), claudeAccountSwitch: vi.fn(),
  claudeAccountReauthenticate: vi.fn(),
  claudeReconciliationState: vi.fn(),
  claudeSwapAccountsList: vi.fn(), claudeSwapAccountSwitch: vi.fn(),
  getSettingsSnapshot: vi.fn(), updateSettings: vi.fn(),
}));
const events = vi.hoisted(() => ({ listen: vi.fn<(event: string, listener: () => void) => Promise<() => void>>() }));
vi.mock("../../../../../lib/tauri", () => mocks);
vi.mock("@tauri-apps/api/event", () => events);
vi.mock("../../../../../hooks/useLocale", () => ({ useLocale: () => ({ t: (key: string) => key }) }));
import { ClaudeAccountsSection } from "./ClaudeAccountsSection";

const t = (key: string) => key;
const current: ClaudeAccount = { id: "one:org", email: "one@example.com", organization: "Work", plan: "max", isActive: true, isSaved: false };
const other: ClaudeAccount = { ...current, id: "two:org", email: "two@example.com", isActive: false, isSaved: true };

describe("ClaudeAccountsSection", () => {
  it("offers targeted reauthentication only for the expired account", async () => {
    mocks.claudeAccountsList.mockResolvedValue([current, { ...other, needsAuthentication: true }]);
    mocks.claudeAccountReauthenticate.mockResolvedValue(undefined);
    render(<ClaudeAccountsSection t={t} />);
    const refresh = await screen.findByRole("button", { name: `CodexAccountsReauthenticateButton: ${other.email}` });
    expect(screen.getAllByRole("button", { name: /CodexAccountsReauthenticateButton/ })).toHaveLength(1);
    mocks.claudeAccountsList.mockResolvedValue([current, other]);
    await act(async () => fireEvent.click(refresh));
    expect(mocks.claudeAccountReauthenticate).toHaveBeenCalledWith(other.id);
    expect(screen.queryByRole("button", { name: /CodexAccountsReauthenticateButton/ })).toBeNull();
    expect(mocks.claudeAccountSwitch).not.toHaveBeenCalled();
  });
  beforeEach(() => {
    vi.resetAllMocks();
    events.listen.mockResolvedValue(() => {});
    mocks.claudeReconciliationState.mockResolvedValue(null);
    mocks.claudeAccountSwitch.mockResolvedValue({
      generation: 1, status: "succeeded", providerRefreshGeneration: 1, detail: "published",
    });
    mocks.claudeAccountsList.mockResolvedValue([current, other]);
    mocks.claudeSwapAccountsList.mockResolvedValue({
      enabled: false,
      executableConfigured: false,
      accounts: [],
      error: null,
    });
    mocks.getSettingsSnapshot.mockResolvedValue({});
  });

  it("offers to save the discovered account and switches only saved inactive accounts", async () => {
    render(<ClaudeAccountsSection t={t} />);
    await screen.findByText(current.email);
    expect(screen.getAllByText("CodexAccountsSwitchButton")).toHaveLength(1);
    await act(async () => fireEvent.click(screen.getByText("ClaudeAccountsSaveCurrent")));
    expect(mocks.claudeAccountSaveCurrent).toHaveBeenCalledOnce();
    await act(async () => fireEvent.click(screen.getByText("CodexAccountsSwitchButton")));
    expect(mocks.claudeAccountSwitch).toHaveBeenCalledWith(other.id);
    expect(screen.getByRole("status").textContent).toBe("ClaudeAccountsSwitched");
  });

  it("keeps mutations disabled across an account update during browser login and supports cancel", async () => {
    let finish!: () => void;
    mocks.claudeAccountAdd.mockImplementation(() => new Promise<void>(resolve => { finish = resolve; }));
    mocks.claudeAccountCancelLogin.mockResolvedValue(undefined);
    render(<ClaudeAccountsSection t={t} />);
    await screen.findByText(current.email);
    fireEvent.click(screen.getByText("CodexAccountsAddButton"));
    const eventCallback = events.listen.mock.calls.find(([event]) => event === "claude-accounts-updated")?.[1] as unknown as () => void;
    await act(async () => eventCallback());
    expect((screen.getByText("CodexAccountsSwitchButton") as HTMLButtonElement).disabled).toBe(true);
    await act(async () => fireEvent.click(screen.getByText("ClaudeAccountsCancelLogin")));
    expect(mocks.claudeAccountCancelLogin).toHaveBeenCalledOnce();
    await act(async () => finish());
    expect(screen.queryByText("ClaudeAccountsCancelLogin")).toBeNull();
    expect((screen.getByText("CodexAccountsAddButton") as HTMLButtonElement).disabled).toBe(false);
  });

  it("shows a switch error without claiming success and allows retry", async () => {
    mocks.claudeAccountSwitch.mockRejectedValue("Close Claude Code first.");
    render(<ClaudeAccountsSection t={t} />);
    await screen.findByText(other.email);
    await act(async () => fireEvent.click(screen.getByText("CodexAccountsSwitchButton")));
    expect(screen.getByRole("alert").textContent).toContain("Close Claude Code first.");
    expect(screen.queryByText("ClaudeAccountsSwitched")).toBeNull();
    expect((screen.getByText("CodexAccountsSwitchButton") as HTMLButtonElement).disabled).toBe(false);
  });

  it("shows initial loading errors while keeping sign-in accessible", async () => {
    mocks.claudeAccountsList.mockRejectedValue("Could not read saved accounts.");
    render(<ClaudeAccountsSection t={t} />);
    await waitFor(() => expect(screen.getByRole("alert").textContent).toContain("Could not read"));
    expect((screen.getByText("CodexAccountsAddButton") as HTMLButtonElement).disabled).toBe(false);
  });

  it("masks account emails, organizations and aria-labels under Hide Personal Info", async () => {
    mocks.claudeAccountsList.mockResolvedValue([
      { ...current, organization: "Acme Org" },
      { ...other, organization: "Acme Org", needsAuthentication: true },
    ]);
    const { container } = render(<ClaudeAccountsSection t={t} hidePersonalInfo />);
    await screen.findByText("Account 1");
    expect(screen.getByText("Account 2")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "CodexAccountsReauthenticateButton: Account 2" }),
    ).toBeInTheDocument();
    expect(container.innerHTML).not.toContain("one@example.com");
    expect(container.innerHTML).not.toContain("two@example.com");
    expect(container.innerHTML).not.toContain("Acme Org");
  });

  it("numbers hidden accounts in list order, not by sorted id", async () => {
    const live: ClaudeAccount = { ...current, id: "zz:org", email: "z@example.com" };
    const saved: ClaudeAccount = { ...other, id: "aa:org", email: "a@example.com" };
    mocks.claudeAccountsList.mockResolvedValue([live, saved]);
    const { container } = render(<ClaudeAccountsSection t={t} hidePersonalInfo />);
    await screen.findByText("Account 2");
    const labels = [...container.querySelectorAll(".credential-card__info strong")].map(
      (label) => label.textContent,
    );
    expect(labels).toEqual(["Account 1", "Account 2"]);
  });

  it("shows emails and organizations when Hide Personal Info is off", async () => {
    mocks.claudeAccountsList.mockResolvedValue([{ ...current, organization: "Acme Org" }]);
    render(<ClaudeAccountsSection t={t} />);
    await screen.findByText("one@example.com");
    expect(screen.getByText("Acme Org · max")).toBeInTheDocument();
  });

  it("passes the saved show-as-used preference to per-account usage", async () => {
    mocks.claudeAccountsList.mockResolvedValue([{
      ...current,
      usage: {
        fiveHour: { usedPercent: 30, resetsAt: null },
        sevenDay: null,
        updatedAt: "2026-10-11T00:00:00Z",
      },
    }]);
    const { rerender } = render(<ClaudeAccountsSection t={t} showAsUsed />);
    await screen.findByText("70% PanelLeftSuffix");
    expect(screen.getByText("30% PanelUsedSuffix")).toBeInTheDocument();
    rerender(<ClaudeAccountsSection t={t} showAsUsed={false} />);
    expect(screen.queryByText("30% PanelUsedSuffix")).toBeNull();
  });

  it("shows the claude-swap list only while claude-swap is on", async () => {
    const { rerender } = render(<ClaudeAccountsSection t={t} />);
    await screen.findByText(current.email);
    expect(screen.queryByText("ClaudeSwapTitle")).toBeNull();
    expect(mocks.claudeSwapAccountsList).not.toHaveBeenCalled();
    mocks.claudeSwapAccountsList.mockResolvedValue({
      enabled: true, executableConfigured: false, accounts: [], error: null,
    });
    rerender(<ClaudeAccountsSection t={t} claudeSwapEnabled claudeSwapExecutablePath="" />);
    await screen.findByText("ClaudeSwapTitle");
    expect(screen.getByText("ClaudeSwapStatusNoExecutable")).toBeTruthy();
  });
});
