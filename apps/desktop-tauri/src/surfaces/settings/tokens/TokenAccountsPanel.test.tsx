import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ProviderTokenAccountsBridge, TokenAccountBridge } from "../../../types/bridge";
import { TokenAccountsPanel } from "./TokenAccountsPanel";

const tauri = vi.hoisted(() => ({
  getTokenAccounts: vi.fn(),
  addTokenAccount: vi.fn(),
  removeTokenAccount: vi.fn(),
  setActiveTokenAccount: vi.fn(),
  setTokenAccountClaudeConfigDir: vi.fn(),
  triggerProviderLogin: vi.fn(),
  updateTokenAccount: vi.fn(),
  moveTokenAccount: vi.fn(),
}));

vi.mock("../../../lib/tauri", () => tauri);
vi.mock("../../../hooks/useLocale", () => ({
  useLocale: () => ({ t: (key: string) => key, language: "english" }),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

// Synthetic fixtures only: the bridge never carries a credential.
const WORK: TokenAccountBridge = {
  id: "fixture-work",
  label: "Work",
  addedAt: "Oct 01, 2026",
  lastUsed: null,
  isActive: true,
  claudeConfigDirectory: null,
  claudeConfigDirectoryAvailable: false,
};
const PERSONAL: TokenAccountBridge = {
  ...WORK,
  id: "fixture-personal",
  label: "Personal",
  addedAt: "Oct 02, 2026",
  isActive: false,
};

function snapshot(accounts: TokenAccountBridge[]): ProviderTokenAccountsBridge {
  return {
    providerId: "claude",
    support: {
      providerId: "claude",
      displayName: "Claude",
      title: "Claude",
      subtitle: "",
      placeholder: "fixture-placeholder",
    },
    accounts,
    activeIndex: Math.max(0, accounts.findIndex((account) => account.isActive)),
  };
}

function row(label: string): HTMLElement {
  return screen.getByText(label, { selector: "strong" }).closest("li") as HTMLElement;
}

function editor(container: HTMLElement): HTMLElement | null {
  return container.querySelector<HTMLElement>(".token-accounts-edit");
}

async function openEditorFor(label: string) {
  fireEvent.click(within(row(label)).getByRole("button", { name: "TokenAccountEdit" }));
}

beforeEach(() => {
  vi.clearAllMocks();
  tauri.getTokenAccounts.mockResolvedValue(snapshot([WORK, PERSONAL]));
});

describe("TokenAccountsPanel", () => {
  it("lists accounts in order with edit and reorder controls per row", async () => {
    render(<TokenAccountsPanel providerId="claude" />);
    await screen.findByText("Work", { selector: "strong" });

    const work = within(row("Work"));
    expect(work.getByRole("button", { name: "TokenAccountMoveUp" }).hasAttribute("disabled")).toBe(
      true,
    );
    expect(
      work.getByRole("button", { name: "TokenAccountMoveDown" }).hasAttribute("disabled"),
    ).toBe(false);

    const personal = within(row("Personal"));
    expect(
      personal.getByRole("button", { name: "TokenAccountMoveUp" }).hasAttribute("disabled"),
    ).toBe(false);
    expect(
      personal.getByRole("button", { name: "TokenAccountMoveDown" }).hasAttribute("disabled"),
    ).toBe(true);
    expect(personal.getByRole("button", { name: "TokenAccountEdit" })).toBeTruthy();
  });

  it("opens the editor prefilled with the label and an empty masked credential", async () => {
    const { container } = render(<TokenAccountsPanel providerId="claude" />);
    await screen.findByText("Personal", { selector: "strong" });
    expect(editor(container)).toBeNull();

    await openEditorFor("Personal");

    const form = editor(container);
    expect(form).not.toBeNull();
    const label = within(form as HTMLElement).getByRole("textbox", {
      name: "TokenAccountLabelPlaceholder",
    }) as HTMLInputElement;
    expect(label.value).toBe("Personal");
    const credential = within(form as HTMLElement).getByLabelText(
      "TokenAccountTokenPlaceholder",
    ) as HTMLInputElement;
    expect(credential.type).toBe("password");
    expect(credential.value).toBe("");
    expect(
      within(form as HTMLElement).getByText("TokenAccountEditKeepCredentialHint"),
    ).toBeTruthy();
  });

  it("saves a label with a blank credential as null so the saved secret is kept", async () => {
    tauri.updateTokenAccount.mockResolvedValue(
      snapshot([WORK, { ...PERSONAL, label: "Renamed" }]),
    );
    const { container } = render(<TokenAccountsPanel providerId="claude" />);
    await screen.findByText("Personal", { selector: "strong" });
    await openEditorFor("Personal");
    const form = editor(container) as HTMLElement;
    fireEvent.change(within(form).getByRole("textbox", { name: "TokenAccountLabelPlaceholder" }), {
      target: { value: "  Renamed  " },
    });

    fireEvent.click(within(form).getByRole("button", { name: "TokenAccountEditSave" }));

    await waitFor(() =>
      expect(tauri.updateTokenAccount).toHaveBeenCalledWith(
        "claude",
        "fixture-personal",
        "Renamed",
        null,
      ),
    );
    await waitFor(() => expect(editor(container)).toBeNull());
    expect(screen.getByText("Renamed", { selector: "strong" })).toBeTruthy();
  });

  it("sends a typed replacement credential trimmed", async () => {
    tauri.updateTokenAccount.mockResolvedValue(snapshot([WORK, PERSONAL]));
    const { container } = render(<TokenAccountsPanel providerId="claude" />);
    await screen.findByText("Personal", { selector: "strong" });
    await openEditorFor("Personal");
    const form = editor(container) as HTMLElement;
    fireEvent.change(within(form).getByLabelText("TokenAccountTokenPlaceholder"), {
      target: { value: "  fixture-replacement  " },
    });

    fireEvent.click(within(form).getByRole("button", { name: "TokenAccountEditSave" }));

    await waitFor(() =>
      expect(tauri.updateTokenAccount).toHaveBeenCalledWith(
        "claude",
        "fixture-personal",
        "Personal",
        "fixture-replacement",
      ),
    );
  });

  it("does not allow saving a blank label", async () => {
    const { container } = render(<TokenAccountsPanel providerId="claude" />);
    await screen.findByText("Personal", { selector: "strong" });
    await openEditorFor("Personal");
    const form = editor(container) as HTMLElement;
    fireEvent.change(within(form).getByRole("textbox", { name: "TokenAccountLabelPlaceholder" }), {
      target: { value: "   " },
    });

    const save = within(form).getByRole("button", { name: "TokenAccountEditSave" });
    expect(save.hasAttribute("disabled")).toBe(true);
    expect(tauri.updateTokenAccount).not.toHaveBeenCalled();
  });

  it("cancel discards the draft without a backend call", async () => {
    const { container } = render(<TokenAccountsPanel providerId="claude" />);
    await screen.findByText("Personal", { selector: "strong" });
    await openEditorFor("Personal");
    const form = editor(container) as HTMLElement;
    fireEvent.change(within(form).getByRole("textbox", { name: "TokenAccountLabelPlaceholder" }), {
      target: { value: "Discarded" },
    });
    fireEvent.change(within(form).getByLabelText("TokenAccountTokenPlaceholder"), {
      target: { value: "fixture-discarded" },
    });

    fireEvent.click(within(form).getByRole("button", { name: "TokenAccountEditCancel" }));

    expect(editor(container)).toBeNull();
    expect(tauri.updateTokenAccount).not.toHaveBeenCalled();
    expect(screen.getByText("Personal", { selector: "strong" })).toBeTruthy();
  });

  it("keeps the editor open and shows the error when the save is rejected", async () => {
    tauri.updateTokenAccount.mockRejectedValue(new Error("Token account not found"));
    const { container } = render(<TokenAccountsPanel providerId="claude" />);
    await screen.findByText("Personal", { selector: "strong" });
    await openEditorFor("Personal");
    const form = editor(container) as HTMLElement;

    fireEvent.click(within(form).getByRole("button", { name: "TokenAccountEditSave" }));

    expect(await screen.findByText("Token account not found")).toBeTruthy();
    expect(editor(container)).not.toBeNull();
  });

  it("moves an account one position and keeps the active account", async () => {
    tauri.moveTokenAccount.mockResolvedValue(
      snapshot([PERSONAL, { ...WORK, isActive: true }]),
    );
    render(<TokenAccountsPanel providerId="claude" />);
    await screen.findByText("Personal", { selector: "strong" });

    fireEvent.click(within(row("Personal")).getByRole("button", { name: "TokenAccountMoveUp" }));

    await waitFor(() =>
      expect(tauri.moveTokenAccount).toHaveBeenCalledWith("claude", "fixture-personal", 0),
    );
    const items = Array.from(document.querySelectorAll("li.token-accounts-card strong")).map(
      (node) => node.textContent,
    );
    expect(items).toEqual(["Personal", "Work"]);
    expect(within(row("Work")).getByText("TokenAccountActive")).toBeTruthy();
    expect(within(row("Personal")).queryByText("TokenAccountActive")).toBeNull();
    expect(
      within(row("Personal"))
        .getByRole("button", { name: "TokenAccountMoveUp" })
        .hasAttribute("disabled"),
    ).toBe(true);
  });

  it("moves an account down by sending its next index", async () => {
    tauri.moveTokenAccount.mockResolvedValue(snapshot([PERSONAL, WORK]));
    render(<TokenAccountsPanel providerId="claude" />);
    await screen.findByText("Work", { selector: "strong" });

    fireEvent.click(within(row("Work")).getByRole("button", { name: "TokenAccountMoveDown" }));

    await waitFor(() =>
      expect(tauri.moveTokenAccount).toHaveBeenCalledWith("claude", "fixture-work", 1),
    );
  });
});
