import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { getSystemTheme } from "../lib/tauri";
import { resolveTheme, useTheme } from "./useTheme";

vi.mock("../lib/tauri", () => ({ getSystemTheme: vi.fn() }));

const mockedGetSystemTheme = vi.mocked(getSystemTheme);

/** The webview's own media query, which every pinned window reports as dark. */
function stubMediaQuery(lightPreferred: boolean) {
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: query === "(prefers-color-scheme: light)" && lightPreferred,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  }));
}

function setTauriBridge(present: boolean) {
  if (present) {
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      value: {},
      configurable: true,
      writable: true,
    });
  } else {
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  }
}

beforeEach(() => {
  mockedGetSystemTheme.mockReset();
  document.documentElement.removeAttribute("data-theme");
});

afterEach(() => {
  cleanup();
  setTauriBridge(false);
  vi.unstubAllGlobals();
});

describe("resolveTheme", () => {
  it("uses the OS light preference in auto mode", () => {
    vi.stubGlobal("matchMedia", (query: string) => ({
      matches: query === "(prefers-color-scheme: light)",
    }));

    expect(resolveTheme("auto")).toBe("light");

    vi.unstubAllGlobals();
  });

  it("falls back to dark in auto mode when light is not preferred", () => {
    vi.stubGlobal("matchMedia", () => ({ matches: false }));

    expect(resolveTheme("auto")).toBe("dark");

    vi.unstubAllGlobals();
  });

  it("takes the system signal from the caller when one is given", () => {
    expect(resolveTheme("auto", true)).toBe("light");
    expect(resolveTheme("auto", false)).toBe("dark");
    expect(resolveTheme("light", false)).toBe("light");
    expect(resolveTheme("dark", true)).toBe("dark");
  });
});

describe("useTheme", () => {
  it("applies the Windows app theme under Tauri when following the system", async () => {
    setTauriBridge(true);
    stubMediaQuery(false);
    mockedGetSystemTheme.mockResolvedValue("light");

    renderHook(() => useTheme("auto", { followSystem: true }));

    await waitFor(() => expect(document.documentElement.dataset.theme).toBe("light"));
    expect(document.documentElement.style.colorScheme).toBe("light");
    expect(mockedGetSystemTheme).toHaveBeenCalled();
  });

  it("keeps the media query for windows that do not follow the system", async () => {
    setTauriBridge(true);
    stubMediaQuery(false);
    mockedGetSystemTheme.mockResolvedValue("light");

    renderHook(() => useTheme("auto"));

    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(mockedGetSystemTheme).not.toHaveBeenCalled();
  });

  it("uses the media query when following the system outside Tauri", () => {
    setTauriBridge(false);
    stubMediaQuery(true);

    renderHook(() => useTheme("auto", { followSystem: true }));

    expect(document.documentElement.dataset.theme).toBe("light");
    expect(mockedGetSystemTheme).not.toHaveBeenCalled();
  });

  it("ignores the system theme for explicit overrides", () => {
    setTauriBridge(true);
    mockedGetSystemTheme.mockResolvedValue("light");

    renderHook(() => useTheme("dark", { followSystem: true }));

    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(mockedGetSystemTheme).not.toHaveBeenCalled();
  });

  it("re-reads the Windows theme when settings change", async () => {
    setTauriBridge(true);
    mockedGetSystemTheme.mockResolvedValueOnce("dark").mockResolvedValueOnce("light");

    renderHook(() => useTheme("auto", { followSystem: true }));
    await waitFor(() => expect(document.documentElement.dataset.theme).toBe("dark"));

    act(() => {
      window.dispatchEvent(new Event("codexbar:settings-updated"));
    });

    await waitFor(() => expect(document.documentElement.dataset.theme).toBe("light"));
    expect(mockedGetSystemTheme).toHaveBeenCalledTimes(2);
  });

  it("re-reads the Windows theme when the window regains focus", async () => {
    setTauriBridge(true);
    mockedGetSystemTheme.mockResolvedValueOnce("dark").mockResolvedValueOnce("light");

    renderHook(() => useTheme("auto", { followSystem: true }));
    await waitFor(() => expect(document.documentElement.dataset.theme).toBe("dark"));

    act(() => {
      window.dispatchEvent(new Event("focus"));
    });

    await waitFor(() => expect(document.documentElement.dataset.theme).toBe("light"));
  });
});
