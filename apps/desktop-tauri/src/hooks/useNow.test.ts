import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { NOW_TICK_MS, useNow } from "./useNow";

describe("useNow", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(Date.parse("2026-10-11T12:00:00Z"));
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("starts at the current time and re-reads it every tick", () => {
    const { result } = renderHook(() => useNow());
    expect(result.current).toBe(Date.parse("2026-10-11T12:00:00Z"));

    act(() => {
      vi.advanceTimersByTime(NOW_TICK_MS);
    });
    expect(result.current).toBe(Date.parse("2026-10-11T12:00:15Z"));
    expect(result.current).toBe(Date.now());
  });

  it("does not re-render before the first tick", () => {
    const seen: number[] = [];
    renderHook(() => {
      seen.push(useNow());
    });
    const before = seen.length;
    act(() => {
      vi.advanceTimersByTime(NOW_TICK_MS - 1);
    });
    expect(seen.length).toBe(before);
  });

  it("clears its interval on unmount", () => {
    const clearSpy = vi.spyOn(window, "clearInterval");
    const { unmount } = renderHook(() => useNow());
    unmount();
    expect(clearSpy).toHaveBeenCalled();
    clearSpy.mockRestore();
  });
});
