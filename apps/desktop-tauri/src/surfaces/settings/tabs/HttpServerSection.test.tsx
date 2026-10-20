import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { HttpServerConfig, HttpServerSnapshot } from "../../../types/bridge";

const tauriMocks = vi.hoisted(() => ({
  getHttpServerState: vi.fn(),
  applyHttpServerConfig: vi.fn(),
}));

vi.mock("../../../lib/tauri", () => tauriMocks);
vi.mock("../../../hooks/useLocale", () => import("../../../test/mocks/locale"));

import HttpServerSection, { bindHost, generateToken } from "./HttpServerSection";

const defaults: HttpServerConfig = {
  enabled: false,
  host: "127.0.0.1",
  port: 8080,
  token: "",
  refreshIntervalSecs: 60,
  requestTimeoutSecs: 30,
  identity: "redacted",
  allowPlainHttp: false,
};

const stopped: HttpServerSnapshot = { config: defaults, status: { state: "stopped" } };

describe("HttpServerSection", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    tauriMocks.getHttpServerState.mockResolvedValue(stopped);
  });

  it("applies the draft with a generated token and shows the running dashboard URL", async () => {
    tauriMocks.applyHttpServerConfig.mockImplementation(async (config: HttpServerConfig) => ({
      config,
      status: { state: "running", url: "http://127.0.0.1:8080/" },
    }));
    render(<HttpServerSection />);

    expect(await screen.findByText("HttpServerStatusStopped")).toBeTruthy();
    const token = screen.getByLabelText("HttpServerTokenLabel") as HTMLInputElement;
    expect(token.type).toBe("password");
    expect(screen.queryByLabelText("HttpServerPlainHttpLabel")).toBeNull();

    fireEvent.click(screen.getByLabelText("HttpServerEnableLabel"));
    fireEvent.click(screen.getByRole("button", { name: "HttpServerGenerateToken" }));
    expect(token.value).toMatch(/^[0-9a-f]{64}$/);
    fireEvent.click(screen.getByRole("button", { name: "HttpServerShowToken" }));
    expect(token.type).toBe("text");

    fireEvent.click(screen.getByRole("button", { name: "HttpServerApply" }));
    await waitFor(() => {
      expect(tauriMocks.applyHttpServerConfig).toHaveBeenCalledWith({
        ...defaults,
        enabled: true,
        token: token.value,
      });
    });
    expect(await screen.findByText("HttpServerStatusRunning")).toBeTruthy();
    expect(screen.getByText("http://127.0.0.1:8080/")).toBeTruthy();
    expect(screen.queryByText("HttpServerWildcardNote")).toBeNull();
  });

  it("asks for plain-HTTP consent off loopback and keeps the draft after a rejected apply", async () => {
    tauriMocks.applyHttpServerConfig.mockResolvedValue({
      config: defaults,
      status: { state: "invalid", code: "plainHttpNotAccepted" },
    });
    render(<HttpServerSection />);

    const host = (await screen.findByLabelText("HttpServerBindLabel")) as HTMLInputElement;
    fireEvent.change(host, { target: { value: "0.0.0.0" } });
    expect(screen.getByLabelText("HttpServerPlainHttpLabel")).toBeTruthy();
    expect(screen.getByText("HttpServerWarning")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "HttpServerApply" }));
    expect(await screen.findByText("HttpServerErrorPlainHttp")).toBeTruthy();
    expect(host.value).toBe("0.0.0.0");
  });

  it("shows the wildcard note for a server listening on every interface", async () => {
    tauriMocks.getHttpServerState.mockResolvedValue({
      config: { ...defaults, enabled: true, host: "0.0.0.0", token: "t", allowPlainHttp: true },
      status: { state: "running", url: "http://0.0.0.0:8080/" },
    });
    render(<HttpServerSection />);
    expect(await screen.findByText("HttpServerWildcardNote")).toBeTruthy();
  });

  it("surfaces startup failures with their detail", async () => {
    tauriMocks.getHttpServerState.mockResolvedValue({
      config: defaults,
      status: { state: "startFailed", detail: "address in use" },
    });
    render(<HttpServerSection />);
    expect(await screen.findByText("HttpServerStatusStartFailed")).toBeTruthy();
    expect(screen.getByText("address in use")).toBeTruthy();
  });

  it("shows a load error instead of vanishing when the state cannot be read", async () => {
    tauriMocks.getHttpServerState.mockRejectedValue(new Error("ipc down"));
    render(<HttpServerSection />);
    expect(await screen.findByText("HttpServerLoadError")).toBeTruthy();
    expect(screen.getByText("HttpServerTitle")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "HttpServerApply" })).toBeNull();
  });

  it("shows an apply error when the apply call rejects", async () => {
    tauriMocks.applyHttpServerConfig.mockRejectedValue(new Error("ipc down"));
    render(<HttpServerSection />);
    expect(await screen.findByText("HttpServerStatusStopped")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "HttpServerApply" }));
    expect(await screen.findByText("HttpServerApplyFailed")).toBeTruthy();

    // A later successful apply clears the error.
    tauriMocks.applyHttpServerConfig.mockResolvedValue({
      config: defaults,
      status: { state: "stopped" },
    });
    fireEvent.click(screen.getByRole("button", { name: "HttpServerApply" }));
    await waitFor(() => {
      expect(screen.queryByText("HttpServerApplyFailed")).toBeNull();
    });
  });

  it("requires explicit confirmation before replacing an unreadable settings file", async () => {
    tauriMocks.getHttpServerState.mockResolvedValue({
      config: defaults,
      status: { state: "loadFailed" },
    });
    tauriMocks.applyHttpServerConfig.mockResolvedValue({
      config: { ...defaults, enabled: true, token: "t" },
      status: { state: "running", url: "http://127.0.0.1:8080/" },
    });
    render(<HttpServerSection />);

    expect(await screen.findByText("HttpServerStatusLoadFailed")).toBeTruthy();
    expect(screen.getByText("HttpServerCorruptWarning")).toBeTruthy();
    const applyButton = screen.getByRole("button", {
      name: "HttpServerApply",
    }) as HTMLButtonElement;
    expect(applyButton.disabled).toBe(true);

    fireEvent.click(screen.getByLabelText("HttpServerCorruptConfirm"));
    expect(applyButton.disabled).toBe(false);
    fireEvent.click(applyButton);
    await waitFor(() => {
      expect(tauriMocks.applyHttpServerConfig).toHaveBeenCalled();
    });
    expect(await screen.findByText("HttpServerStatusRunning")).toBeTruthy();
  });
});

describe("HttpServerSection helpers", () => {
  it("normalizes localhost like the backend", () => {
    expect(bindHost(" LocalHost ")).toBe("127.0.0.1");
    expect(bindHost("100.64.0.1")).toBe("100.64.0.1");
  });

  it("generates distinct 32-byte hex tokens", () => {
    const first = generateToken();
    expect(first).toMatch(/^[0-9a-f]{64}$/);
    expect(generateToken()).not.toBe(first);
  });
});
