import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  getProviderWorkspaceId,
  setProviderWorkspaceId,
} from "../../../../../lib/tauri";
import { OpenAiExtras } from "./OpenAiExtras";

vi.mock("../../../../../lib/tauri", () => ({
  getProviderWorkspaceId: vi.fn(),
  setProviderWorkspaceId: vi.fn(),
}));

function renderExtras(providerId: string = "openaiapi") {
  const t = (key: string) => key;
  render(<OpenAiExtras providerId={providerId} t={t} />);
}

describe("OpenAiExtras", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getProviderWorkspaceId).mockResolvedValue(null);
    vi.mocked(setProviderWorkspaceId).mockResolvedValue(undefined);
  });

  it("renders the Project ID field for OpenAI API", async () => {
    renderExtras("openaiapi");
    expect(await screen.findByText("OpenAiAdminApiTitle")).toBeInTheDocument();
    expect(screen.getByText("OpenAiProjectIdLabel")).toBeInTheDocument();
    const input = screen.getByRole("textbox") as HTMLInputElement;
    expect(input.placeholder).toContain("OpenAiProjectIdPlaceholder");
  });

  it("renders the Project ID field for Cosmic AI", async () => {
    renderExtras("cosmic");
    expect(await screen.findByText("CosmicProjectIdTitle")).toBeInTheDocument();
    expect(screen.getByText("CosmicProjectIdLabel")).toBeInTheDocument();
    expect(screen.getByText("CosmicProjectIdHelp")).toBeInTheDocument();
    const input = screen.getByRole("textbox") as HTMLInputElement;
    expect(input.placeholder).toContain("CosmicProjectIdPlaceholder");
  });

  it("loads and saves the Cosmic Project ID", async () => {
    vi.mocked(getProviderWorkspaceId).mockResolvedValue("project_abc123");
    renderExtras("cosmic");

    const input = (await screen.findByRole("textbox")) as HTMLInputElement;
    await vi.waitFor(() => expect(input.value).toBe("project_abc123"));

    const saveBtn = screen.getByRole("button", { name: "Save" });
    expect(saveBtn).toBeDisabled(); // Already saved value

    await act(async () => {
      fireEvent.change(input, { target: { value: "project_new999" } });
    });

    expect(saveBtn).toBeEnabled();
    await act(async () => {
      fireEvent.click(saveBtn);
    });

    expect(setProviderWorkspaceId).toHaveBeenCalledWith(
      "cosmic",
      "project_new999",
    );
  });

  it("renders the Team ID field for Vercel and saves it through the workspace command", async () => {
    renderExtras("vercel");
    expect(await screen.findByText("VercelTeamTitle")).toBeInTheDocument();
    expect(screen.getByText("VercelTeamIdLabel")).toBeInTheDocument();
    const input = (await screen.findByRole("textbox")) as HTMLInputElement;
    expect(input.placeholder).toBe("team_...");

    await act(async () => {
      fireEvent.change(input, { target: { value: "team_abc123" } });
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Save" }));
    });

    expect(setProviderWorkspaceId).toHaveBeenCalledWith("vercel", "team_abc123");
  });

  it("clears a failed save error once the field is edited, even back to the saved value", async () => {
    vi.mocked(getProviderWorkspaceId).mockResolvedValue("team_saved");
    vi.mocked(setProviderWorkspaceId).mockRejectedValueOnce("Vercel Team ID is invalid");
    renderExtras("vercel");

    const input = (await screen.findByRole("textbox")) as HTMLInputElement;
    await vi.waitFor(() => expect(input.value).toBe("team_saved"));
    expect(input).not.toHaveAttribute("aria-invalid");

    await act(async () => {
      fireEvent.change(input, { target: { value: "bad id" } });
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Save" }));
    });

    const error = await screen.findByText("Vercel Team ID is invalid");
    expect(input).toHaveAttribute("aria-invalid", "true");
    expect(input).toHaveAttribute("aria-describedby", error.id);

    await act(async () => {
      fireEvent.change(input, { target: { value: "team_saved" } });
    });

    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    expect(screen.queryByText("Vercel Team ID is invalid")).not.toBeInTheDocument();
    expect(input).not.toHaveAttribute("aria-invalid");
    expect(input).not.toHaveAttribute("aria-describedby");
  });

  it("drops a save error that arrives after the field was edited", async () => {
    vi.mocked(getProviderWorkspaceId).mockResolvedValue("team_saved");
    let rejectSave: (reason: unknown) => void = () => {};
    vi.mocked(setProviderWorkspaceId).mockReturnValueOnce(
      new Promise<void>((_, reject) => {
        rejectSave = reject;
      }),
    );
    renderExtras("vercel");

    const input = (await screen.findByRole("textbox")) as HTMLInputElement;
    await vi.waitFor(() => expect(input.value).toBe("team_saved"));

    await act(async () => {
      fireEvent.change(input, { target: { value: "bad id" } });
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Save" }));
    });
    await act(async () => {
      fireEvent.change(input, { target: { value: "team_saved" } });
    });
    await act(async () => {
      rejectSave("Vercel Team ID is invalid");
    });

    expect(screen.queryByText("Vercel Team ID is invalid")).not.toBeInTheDocument();
    expect(input).not.toHaveAttribute("aria-invalid");
  });

  it("renders help text for Cosmic Project ID", async () => {
    renderExtras("cosmic");
    expect(
      await screen.findByText("CosmicProjectIdHelp"),
    ).toBeInTheDocument();
  });

  it("renders nothing for providers without workspace fields", async () => {
    renderExtras("openrouter");
    expect(screen.queryByText(/ApiTitle|ProjectIdLabel/)).not.toBeInTheDocument();
  });
});
