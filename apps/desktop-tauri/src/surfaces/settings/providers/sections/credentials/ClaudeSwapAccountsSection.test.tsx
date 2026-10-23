import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ClaudeSwapAccount, ClaudeSwapAccountsState } from "../../../../../types/bridge";

const mocks = vi.hoisted(() => ({
  claudeSwapAccountsList: vi.fn(),
  claudeSwapAccountSwitch: vi.fn(),
  claudeSwapAccountReauthenticate: vi.fn(),
  claudeReconciliationState: vi.fn(),
  getSettingsSnapshot: vi.fn(),
  getCurrencyRates: vi.fn(),
}));
const events = vi.hoisted(() => ({
  listen: vi.fn<(event: string, listener: () => void) => Promise<() => void>>(),
}));
vi.mock("../../../../../lib/tauri", () => mocks);
vi.mock("@tauri-apps/api/event", () => events);
import { ClaudeSwapAccountsSection } from "./ClaudeSwapAccountsSection";
import { CurrencyProvider } from "../../../../../hooks/CurrencyProvider";

const t = (key: string) => key;

const active: ClaudeSwapAccount = {
  id: "claude-swap:1",
  source: "claude-swap",
  slot: 1,
  label: "work@example.com",
  email: "work@example.com",
  organization: null,
  alias: null,
  isActive: true,
  action: null,
  isDisabled: false,
  status: "ok",
  error: null,
  fiveHour: { usedPercent: 12, resetsAt: null },
  sevenDay: null,
  scoped: [],
  spend: null,
  historicalUsage: null,
};

const switchable: ClaudeSwapAccount = {
  ...active,
  id: "claude-swap:2",
  slot: 2,
  label: "personal@example.com",
  email: "personal@example.com",
  isActive: false,
  action: "switch",
};

const blocked: ClaudeSwapAccount = {
  ...switchable,
  id: "claude-swap:3",
  slot: 3,
  label: "Backup",
  action: null,
  status: "token_expired",
  error: "Token expired. Switch to this account in claude-swap to refresh it.",
};

function enabledState(accounts: ClaudeSwapAccount[]): ClaudeSwapAccountsState {
  return { enabled: true, executableConfigured: true, accounts, error: null };
}

describe("ClaudeSwapAccountsSection", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    events.listen.mockResolvedValue(() => {});
    mocks.claudeReconciliationState.mockResolvedValue(null);
    mocks.getSettingsSnapshot.mockResolvedValue({});
    mocks.getCurrencyRates.mockResolvedValue({ rates: { USD: 1, TRY: 48.5 } });
    mocks.claudeSwapAccountsList.mockResolvedValue({
      enabled: false,
      executableConfigured: false,
      accounts: [],
      error: null,
    });
  });

  it("renders nothing while claude-swap is off and lists accounts once it is on", async () => {
    const { container, rerender } = render(
      <ClaudeSwapAccountsSection t={t} enabled={false} executablePath="" />,
    );
    await act(async () => {});
    expect(container.innerHTML).toBe("");
    expect(mocks.claudeSwapAccountsList).not.toHaveBeenCalled();

    mocks.claudeSwapAccountsList.mockResolvedValue(enabledState([active, switchable]));
    rerender(<ClaudeSwapAccountsSection t={t} enabled executablePath="~/bin/cswap" />);
    await screen.findByText("work@example.com");
    expect(screen.getByText("ClaudeSwapTitle")).toBeTruthy();
    expect(screen.getByText("ClaudeSwapStatusReady")).toBeTruthy();
    expect(screen.queryByRole("checkbox")).toBeNull();
    expect(screen.queryByRole("textbox")).toBeNull();
  });

  it("reloads the list when the executable path changes", async () => {
    mocks.claudeSwapAccountsList.mockResolvedValue({
      enabled: true, executableConfigured: false, accounts: [], error: null,
    });
    const { rerender } = render(
      <ClaudeSwapAccountsSection t={t} enabled executablePath="" />,
    );
    await screen.findByText("ClaudeSwapStatusNoExecutable");
    mocks.claudeSwapAccountsList.mockResolvedValue(enabledState([active]));
    rerender(<ClaudeSwapAccountsSection t={t} enabled executablePath="C:/tools/cswap.exe" />);
    await screen.findByText("work@example.com");
    expect(mocks.claudeSwapAccountsList).toHaveBeenCalledTimes(2);
  });

  it("reloads the masked labels when Hide personal information changes", async () => {
    mocks.claudeSwapAccountsList.mockResolvedValue(enabledState([active]));
    const { rerender } = render(
      <ClaudeSwapAccountsSection t={t} enabled executablePath="C:/tools/cswap.exe" />,
    );
    await screen.findByText("work@example.com");

    mocks.claudeSwapAccountsList.mockResolvedValue(
      enabledState([{ ...active, label: "Account 1" }]),
    );
    rerender(
      <ClaudeSwapAccountsSection t={t} enabled executablePath="C:/tools/cswap.exe" hidePersonalInfo />,
    );
    await screen.findByText("Account 1");
    expect(screen.queryByText("work@example.com")).toBeNull();
    expect(mocks.claudeSwapAccountsList).toHaveBeenCalledTimes(2);
  });

  it("shows each card's own session forecast and none where history is missing", async () => {
    const forecasted: ClaudeSwapAccount = {
      ...active,
      sessionEquivalentForecast: {
        estimatedWindowsToExhaustWeekly: 3.04,
        windowsUntilReset: 12,
        availableWindowsUntilReset: 10,
        sampleCount: 3,
        weeklyResetsAt: "2026-09-19T00:00:00Z",
        weeklyUsedPercent: 30,
      },
    };
    const fresh: ClaudeSwapAccount = { ...switchable, sessionEquivalentForecast: null };
    mocks.claudeSwapAccountsList.mockResolvedValue(enabledState([forecasted, fresh]));
    render(<ClaudeSwapAccountsSection t={t} enabled executablePath="C:/tools/cswap.exe" />);
    const estimate = await screen.findByText("Estimated: 3 session quotas left");
    expect(estimate.closest("li")?.textContent).toContain("work@example.com");
    const other = screen.getByText("personal@example.com").closest("li");
    expect(other?.textContent).not.toContain("Estimated:");
    expect(screen.getAllByText(/Estimated:/)).toHaveLength(1);
  });

  it("names tcr in the title, status and empty copy", async () => {
    mocks.claudeSwapAccountsList.mockResolvedValue({
      enabled: true, source: "tcr", executableConfigured: false, accounts: [], error: null,
    });
    const { rerender } = render(
      <ClaudeSwapAccountsSection t={t} enabled executablePath="" source="tcr" />,
    );
    await screen.findByText("ClaudeTcrStatusNoExecutable");
    expect(screen.getByText("ClaudeTcrTitle")).toBeTruthy();
    expect(screen.queryByText("ClaudeSwapTitle")).toBeNull();

    mocks.claudeSwapAccountsList.mockResolvedValue({
      enabled: true, source: "tcr", executableConfigured: true, accounts: [], error: null,
    });
    rerender(<ClaudeSwapAccountsSection t={t} enabled executablePath="C:/tools/tcr.exe" source="tcr" />);
    await screen.findByText("ClaudeTcrEmpty");
    expect(screen.getByText("ClaudeTcrStatusReady")).toBeTruthy();
  });

  it("renders read-only tcr cards without a switch button", async () => {
    const pooled: ClaudeSwapAccount = {
      ...switchable,
      id: "tcr:2",
      source: "tcr",
      action: null,
      isDisabled: true,
    };
    mocks.claudeSwapAccountsList.mockResolvedValue({
      ...enabledState([pooled]),
      source: "tcr",
    });
    render(<ClaudeSwapAccountsSection t={t} enabled executablePath="C:/tools/tcr.exe" source="tcr" />);
    await screen.findByText("personal@example.com");
    expect(screen.getByText("ClaudeTcrDisabled")).toBeTruthy();
    expect(screen.queryByText("ClaudeSwapSwitchButton")).toBeNull();
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("drops a late claude-swap reply after the source changes to tcr", async () => {
    let resolveSwap: (state: ClaudeSwapAccountsState) => void = () => {};
    mocks.claudeSwapAccountsList.mockImplementationOnce(
      () => new Promise<ClaudeSwapAccountsState>((resolve) => { resolveSwap = resolve; }),
    );
    const { rerender } = render(
      <ClaudeSwapAccountsSection t={t} enabled executablePath="C:/tools/cswap.exe" />,
    );
    await act(async () => {});
    const pooled: ClaudeSwapAccount = {
      ...switchable,
      id: "tcr:7",
      source: "tcr",
      slot: 7,
      label: "pool@example.com",
      action: null,
    };
    mocks.claudeSwapAccountsList.mockResolvedValue({
      ...enabledState([pooled]),
      source: "tcr",
    });
    rerender(
      <ClaudeSwapAccountsSection t={t} enabled executablePath="C:/tools/cswap.exe" source="tcr" />,
    );
    await screen.findByText("pool@example.com");

    await act(async () => resolveSwap(enabledState([active, switchable])));
    expect(screen.getByText("pool@example.com")).toBeTruthy();
    expect(screen.queryByText("work@example.com")).toBeNull();
    expect(screen.queryByText("ClaudeSwapSwitchButton")).toBeNull();
  });

  it("switches only actionable inactive accounts and reports success", async () => {
    mocks.claudeSwapAccountsList.mockResolvedValue(
      enabledState([active, switchable, blocked]),
    );
    mocks.claudeSwapAccountSwitch.mockResolvedValue({
      generation: 1, status: "succeeded", providerRefreshGeneration: 1, detail: "published",
    });
    render(<ClaudeSwapAccountsSection t={t} enabled executablePath="~/bin/cswap" />);
    await screen.findByText("work@example.com");

    expect(screen.getByText("TokenAccountActive")).toBeTruthy();
    expect(screen.queryByText("ClaudeSwapSwitchButton")).toBeTruthy();
    // Exactly one actionable card (the other inactive card is not actionable).
    expect(screen.getAllByText("ClaudeSwapSwitchButton")).toHaveLength(1);

    await act(async () =>
      fireEvent.click(screen.getByText("ClaudeSwapSwitchButton")),
    );
    expect(mocks.claudeSwapAccountSwitch).toHaveBeenCalledWith(2);
    expect(screen.getByRole("status").textContent).toBe("ClaudeSwapSwitched");
  });

  it("renders no action button when the adapter is read-only", async () => {
    // Read-only adapters project `action: null` for every row, including
    // inactive rows that would otherwise be switchable.
    mocks.claudeSwapAccountsList.mockResolvedValue(
      enabledState([
        active,
        { ...switchable, action: null },
        { ...active, id: "claude-swap:4", slot: 4, label: "foreign@example.com", status: "foreign_credential", action: null },
      ]),
    );
    render(<ClaudeSwapAccountsSection t={t} enabled executablePath="~/bin/cswap" />);
    await screen.findByText("work@example.com");

    expect(screen.getByText("personal@example.com")).toBeTruthy();
    expect(screen.getAllByText("TokenAccountActive").length).toBeGreaterThan(0);
    expect(screen.queryByText("ClaudeSwapSwitchButton")).toBeNull();
    expect(screen.queryByText("ClaudeSwapReauthenticateButton")).toBeNull();
    expect(mocks.claudeSwapAccountSwitch).not.toHaveBeenCalled();
    expect(mocks.claudeSwapAccountReauthenticate).not.toHaveBeenCalled();
  });

  it("surfaces adapter errors without offering a switch", async () => {
    mocks.claudeSwapAccountsList.mockResolvedValue({
      enabled: true,
      executableConfigured: true,
      accounts: [],
      error: "claude-swap did not respond within 30 seconds.",
    });
    render(<ClaudeSwapAccountsSection t={t} enabled executablePath="~/bin/cswap" />);
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toContain("did not respond"),
    );
    expect(screen.queryByText("ClaudeSwapSwitchButton")).toBeNull();
  });

  it("shows a switch failure without claiming success", async () => {
    mocks.claudeSwapAccountsList.mockResolvedValue(
      enabledState([active, switchable]),
    );
    mocks.claudeSwapAccountSwitch.mockRejectedValue("cswap failed.");
    render(<ClaudeSwapAccountsSection t={t} enabled executablePath="~/bin/cswap" />);
    await screen.findByText("personal@example.com");
    await act(async () =>
      fireEvent.click(screen.getByText("ClaudeSwapSwitchButton")),
    );
    expect(screen.getByRole("alert").textContent).toContain("cswap failed.");
    expect(screen.queryByText("ClaudeSwapSwitched")).toBeNull();
  });

  it("re-authenticates an active foreign credential and reports the distinct result", async () => {
    const foreign: ClaudeSwapAccount = {
      ...active,
      status: "foreign_credential",
      action: "reauthenticate",
      error: "claude-swap reports the live credential belongs to a different account.",
    };
    mocks.claudeSwapAccountsList.mockResolvedValue(enabledState([foreign]));
    mocks.claudeSwapAccountReauthenticate.mockResolvedValue({
      generation: 2, status: "succeeded", providerRefreshGeneration: 2, detail: "published",
    });
    render(<ClaudeSwapAccountsSection t={t} enabled executablePath="~/bin/cswap" />);
    await screen.findByText("work@example.com");
    await act(async () =>
      fireEvent.click(screen.getByText("ClaudeSwapReauthenticateButton")),
    );
    expect(mocks.claudeSwapAccountReauthenticate).toHaveBeenCalledWith(1);
    expect(screen.getByRole("status").textContent).toBe("ClaudeSwapReauthenticated");
    expect(mocks.claudeSwapAccountSwitch).not.toHaveBeenCalled();
  });

  it("names quota percents as left by default and as used when the setting asks", async () => {
    const account: ClaudeSwapAccount = {
      ...active,
      scoped: [{ name: "Opus", usedPercent: 30, resetsAt: null }],
      historicalUsage: {
        fiveHour: { usedPercent: 44, resetsAt: null },
        sevenDay: null,
        scoped: [],
        spend: null,
        fetchedAt: "2026-09-12T00:45:00Z",
        provenance: "source_reported_last_good",
      },
    };
    mocks.claudeSwapAccountsList.mockResolvedValue(enabledState([account]));
    const { rerender } = render(
      <ClaudeSwapAccountsSection t={t} enabled executablePath="~/bin/cswap" />,
    );
    await screen.findByText("ProviderSession 88% PanelLeftSuffix \u00b7 Opus 70% PanelLeftSuffix");
    expect(screen.getByText(/ClaudeSwapHistoricalUsage: ProviderSession 56% PanelLeftSuffix/)).toBeTruthy();
    expect(screen.queryByText(/PanelUsedSuffix/)).toBeNull();

    rerender(
      <ClaudeSwapAccountsSection t={t} enabled executablePath="~/bin/cswap" showAsUsed />,
    );
    await screen.findByText("ProviderSession 12% PanelUsedSuffix \u00b7 Opus 30% PanelUsedSuffix");
    expect(screen.getByText(/ClaudeSwapHistoricalUsage: ProviderSession 44% PanelUsedSuffix/)).toBeTruthy();
    expect(screen.queryByText(/PanelLeftSuffix/)).toBeNull();
  });

  it("renders disabled, spend, and display-only historical usage details", async () => {
    const account: ClaudeSwapAccount = {
      ...switchable,
      isDisabled: true,
      spend: {
        used: 2,
        limit: 20,
        usedPercent: 10,
        currencyCode: "USD",
        resetsAt: null,
      },
      historicalUsage: {
        fiveHour: { usedPercent: 44, resetsAt: null },
        sevenDay: null,
        scoped: [],
        spend: {
          used: 1,
          limit: 10,
          usedPercent: 10,
          currencyCode: "USD",
          resetsAt: null,
        },
        fetchedAt: "2026-09-12T00:45:00Z",
        provenance: "source_reported_last_good",
      },
    };
    mocks.getSettingsSnapshot.mockResolvedValue({ preferredCurrencyCode: "TRY" });
    mocks.claudeSwapAccountsList.mockResolvedValue(enabledState([account]));
    render(
      <CurrencyProvider>
        <ClaudeSwapAccountsSection t={t} enabled executablePath="~/bin/cswap" />
      </CurrencyProvider>,
    );
    await screen.findByText("personal@example.com");
    await screen.findByText(/ClaudeSwapSpend .*97[.,]00.*970[.,]00.*10%/);
    expect(screen.getByText(/ClaudeSwapHistoricalUsage.*48[.,]50.*485[.,]00/)).toBeTruthy();
    expect(screen.queryByText(/USD/)).toBeNull();
    expect(screen.getByText("ClaudeSwapDisabled")).toBeTruthy();
  });
});
