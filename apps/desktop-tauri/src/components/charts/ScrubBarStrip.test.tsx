import { cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ScrubBarStrip, type ScrubBar } from "./ScrubBarStrip";

const bars: ScrubBar[] = [
  { id: "a", value: 1 },
  { id: "b", value: 2 },
];

function renderStrip(selectedId: string | null, onSelect: (id: string | null) => void) {
  return render(
    <ScrubBarStrip
      bars={bars}
      selectedId={selectedId}
      onSelect={onSelect}
      classPrefix="openai-tpm"
      className="openai-tpm__chart"
      label="Tokens per minute"
    />,
  );
}

describe("ScrubBarStrip Escape key", () => {
  afterEach(() => {
    cleanup();
  });

  it("clears the selection without letting Escape reach window listeners", () => {
    const onSelect = vi.fn();
    const windowListener = vi.fn();
    window.addEventListener("keydown", windowListener);
    try {
      const { container } = renderStrip("a", onSelect);
      fireEvent.keyDown(container.querySelector("svg")!, { key: "Escape" });

      expect(onSelect).toHaveBeenCalledTimes(1);
      expect(onSelect).toHaveBeenCalledWith(null);
      expect(windowListener).not.toHaveBeenCalled();
    } finally {
      window.removeEventListener("keydown", windowListener);
    }
  });

  it("lets Escape reach window listeners when no bar is selected", () => {
    const onSelect = vi.fn();
    const windowListener = vi.fn();
    window.addEventListener("keydown", windowListener);
    try {
      const { container } = renderStrip(null, onSelect);
      fireEvent.keyDown(container.querySelector("svg")!, { key: "Escape" });

      expect(onSelect).not.toHaveBeenCalled();
      expect(windowListener).toHaveBeenCalledTimes(1);
    } finally {
      window.removeEventListener("keydown", windowListener);
    }
  });
});
