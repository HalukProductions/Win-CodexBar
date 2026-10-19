import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { LocaleKey } from "../../../i18n/keys";
import type { TokenAccountBridge } from "../../../types/bridge";
import { ClaudeConfigDirField } from "./ClaudeConfigDirField";

const mocks = vi.hoisted(() => ({ open: vi.fn() }));

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: mocks.open }));

const t = (key: LocaleKey) => key;

function account(overrides: Partial<TokenAccountBridge> = {}): TokenAccountBridge {
  return {
    id: "fixture-account",
    label: "Work",
    addedAt: "Oct 01, 2026",
    lastUsed: null,
    isActive: true,
    claudeConfigDirectory: null,
    claudeConfigDirectoryAvailable: false,
    ...overrides,
  };
}

beforeEach(() => {
  mocks.open.mockReset();
});

describe("ClaudeConfigDirField", () => {
  it("offers only the folder picker for an unmapped account", () => {
    render(<ClaudeConfigDirField account={account()} disabled={false} t={t} onChange={vi.fn()} />);
    expect(screen.getByText("TokenAccountClaudeConfigDirLabel")).toBeTruthy();
    expect(screen.getByRole("button", { name: "TokenAccountClaudeConfigDirChoose" })).toBeTruthy();
    expect(screen.queryByText("TokenAccountClaudeConfigDirClear")).toBeNull();
    expect(screen.queryByText("TokenAccountClaudeConfigDirUnavailable")).toBeNull();
  });

  it("shows a mapped folder, flags a missing one, and clears it", () => {
    const onChange = vi.fn().mockResolvedValue(undefined);
    const directory = "C:\\fixtures\\claude-work";
    render(
      <ClaudeConfigDirField
        account={account({ claudeConfigDirectory: directory })}
        disabled={false}
        t={t}
        onChange={onChange}
      />,
    );
    expect(screen.getByText(directory)).toBeTruthy();
    expect(screen.getByText("TokenAccountClaudeConfigDirUnavailable")).toBeTruthy();
    fireEvent.click(
      screen.getByRole("button", { name: `TokenAccountClaudeConfigDirClear ${directory}` }),
    );
    expect(onChange).toHaveBeenCalledWith(null);
  });

  it("hides the unavailable badge for an existing folder", () => {
    render(
      <ClaudeConfigDirField
        account={account({
          claudeConfigDirectory: "C:\\fixtures\\claude-work",
          claudeConfigDirectoryAvailable: true,
        })}
        disabled={false}
        t={t}
        onChange={vi.fn()}
      />,
    );
    expect(screen.queryByText("TokenAccountClaudeConfigDirUnavailable")).toBeNull();
  });

  it("saves a picked folder from a single-folder picker", async () => {
    mocks.open.mockResolvedValue("D:\\claude-personal");
    const onChange = vi.fn().mockResolvedValue(undefined);
    render(<ClaudeConfigDirField account={account()} disabled={false} t={t} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: "TokenAccountClaudeConfigDirChoose" }));
    await waitFor(() => expect(onChange).toHaveBeenCalledWith("D:\\claude-personal"));
    expect(mocks.open).toHaveBeenCalledWith({ directory: true, multiple: false });
  });

  it("keeps the mapping when the picker is cancelled or fails", async () => {
    const onChange = vi.fn().mockResolvedValue(undefined);
    mocks.open.mockResolvedValueOnce(null).mockRejectedValueOnce(new Error("denied"));
    render(<ClaudeConfigDirField account={account()} disabled={false} t={t} onChange={onChange} />);
    const choose = screen.getByRole("button", { name: "TokenAccountClaudeConfigDirChoose" });
    fireEvent.click(choose);
    await waitFor(() => expect(mocks.open).toHaveBeenCalledTimes(1));
    fireEvent.click(choose);
    await waitFor(() =>
      expect(screen.getByRole("status").textContent).toContain(
        "TokenAccountClaudeConfigDirPickFailed",
      ),
    );
    expect(onChange).not.toHaveBeenCalled();
  });
});
