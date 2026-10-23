import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const tauri = vi.hoisted(() => ({
  hasVercelAccessToken: vi.fn(),
  setVercelAccessToken: vi.fn(),
  removeVercelAccessToken: vi.fn(),
}));
vi.mock("../../../../../lib/tauri", () => tauri);

import { VercelAccessTokenCreds } from "./VercelAccessTokenCreds";

describe("VercelAccessTokenCreds", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    tauri.setVercelAccessToken.mockResolvedValue(undefined);
    tauri.removeVercelAccessToken.mockResolvedValue(undefined);
  });

  it("reports a saved token so the storage summary can refresh", async () => {
    tauri.hasVercelAccessToken.mockResolvedValue(false);
    const onChanged = vi.fn();
    render(<VercelAccessTokenCreds t={(key) => key} onChanged={onChanged} />);

    fireEvent.change(screen.getByLabelText("VercelAccessTokenLabel"), {
      target: { value: "  vercel-test-token  " },
    });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Save" })));

    expect(tauri.setVercelAccessToken).toHaveBeenCalledWith("vercel-test-token");
    expect(onChanged).toHaveBeenCalledTimes(1);
  });

  it("reports a removed token so the storage summary can refresh", async () => {
    tauri.hasVercelAccessToken.mockResolvedValue(true);
    const onChanged = vi.fn();
    render(<VercelAccessTokenCreds t={(key) => key} onChanged={onChanged} />);

    const removeButton = await screen.findByRole("button", { name: "Remove" });
    await act(async () => fireEvent.click(removeButton));

    expect(tauri.removeVercelAccessToken).toHaveBeenCalledTimes(1);
    expect(onChanged).toHaveBeenCalledTimes(1);
  });

  it("does not report a change when saving fails", async () => {
    tauri.hasVercelAccessToken.mockResolvedValue(false);
    tauri.setVercelAccessToken.mockRejectedValue(new Error("Vercel access token must not be empty"));
    const onChanged = vi.fn();
    render(<VercelAccessTokenCreds t={(key) => key} onChanged={onChanged} />);

    fireEvent.change(screen.getByLabelText("VercelAccessTokenLabel"), {
      target: { value: "x" },
    });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Save" })));

    expect(onChanged).not.toHaveBeenCalled();
    expect(screen.getByText("Vercel access token must not be empty")).toBeInTheDocument();
  });
});
