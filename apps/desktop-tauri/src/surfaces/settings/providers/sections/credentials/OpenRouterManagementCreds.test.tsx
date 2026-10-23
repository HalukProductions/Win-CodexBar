import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const tauri = vi.hoisted(() => ({
  hasOpenRouterManagementApiKey: vi.fn(),
  setOpenRouterManagementApiKey: vi.fn(),
  removeOpenRouterManagementApiKey: vi.fn(),
}));
vi.mock("../../../../../lib/tauri", () => tauri);

import { OpenRouterManagementCreds } from "./OpenRouterManagementCreds";

describe("OpenRouterManagementCreds", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    tauri.setOpenRouterManagementApiKey.mockResolvedValue(undefined);
    tauri.removeOpenRouterManagementApiKey.mockResolvedValue(undefined);
  });

  it("reports saved and removed keys so the storage summary can refresh", async () => {
    tauri.hasOpenRouterManagementApiKey.mockResolvedValue(false);
    const onChanged = vi.fn();
    render(<OpenRouterManagementCreds t={(key) => key} onChanged={onChanged} />);

    fireEvent.change(screen.getByLabelText("OpenRouterManagementKeyLabel"), {
      target: { value: "or-test-key" },
    });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Save" })));
    expect(tauri.setOpenRouterManagementApiKey).toHaveBeenCalledWith("or-test-key");
    expect(onChanged).toHaveBeenCalledTimes(1);

    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Remove" })));
    expect(tauri.removeOpenRouterManagementApiKey).toHaveBeenCalledTimes(1);
    expect(onChanged).toHaveBeenCalledTimes(2);
  });
});
