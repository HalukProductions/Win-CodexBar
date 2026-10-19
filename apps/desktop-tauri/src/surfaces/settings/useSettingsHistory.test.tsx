import { act, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { describe, expect, it } from "vitest";
import type { SettingsTabId } from "../../types/bridge";
import {
  stepBack,
  stepForward,
  useSettingsHistory,
  visitPane,
  type SettingsHistory,
  type SettingsPane,
} from "./useSettingsHistory";

const pane = (tab: SettingsTabId, providerId: string | null = null): SettingsPane => ({
  tab,
  providerId,
});
const fresh = (): SettingsHistory => ({ current: pane("general"), back: [], forward: [] });
const tabs = (stack: SettingsPane[]) => stack.map((p) => p.providerId ?? p.tab);

describe("settings history stacks", () => {
  it("records back history and clears forward history on a new visit", () => {
    let h = visitPane(visitPane(fresh(), pane("notifications")), pane("advanced"));
    expect(tabs(h.back)).toEqual(["general", "notifications"]);

    h = stepBack(h)!;
    expect(h.current.tab).toBe("notifications");
    expect(tabs(h.forward)).toEqual(["advanced"]);

    h = visitPane(h, pane("about"));
    expect(h.forward).toEqual([]);
    expect(tabs(h.back)).toEqual(["general", "notifications"]);
  });

  it("walks the visited panes without rewriting history", () => {
    let h = visitPane(visitPane(fresh(), pane("notifications")), pane("advanced"));
    h = stepBack(stepBack(h)!)!;
    expect(h.current.tab).toBe("general");
    expect(stepBack(h)).toBeNull();

    h = stepForward(stepForward(h)!)!;
    expect(h.current.tab).toBe("advanced");
    expect(stepForward(h)).toBeNull();
    expect(tabs(h.back)).toEqual(["general", "notifications"]);
  });

  it("does not record reselecting the current pane", () => {
    const start = fresh();
    expect(visitPane(start, pane("general"))).toBe(start);

    let h = visitPane(visitPane(start, pane("notifications")), pane("advanced"));
    h = visitPane(stepBack(h)!, pane("notifications"));
    expect(tabs(h.forward)).toEqual(["advanced"]);
    expect(tabs(h.back)).toEqual(["general"]);
  });

  it("treats each provider as its own pane", () => {
    let h = visitPane(fresh(), pane("providers", "codex"));
    h = visitPane(h, pane("providers", "claude"));
    expect(tabs(h.back)).toEqual(["general", "codex"]);
    expect(stepBack(h)!.current).toEqual(pane("providers", "codex"));
  });
});

function Harness({ initial = "general" as SettingsTabId }) {
  const [current, setCurrent] = useState<SettingsPane>(pane(initial));
  const history = useSettingsHistory(current, setCurrent);
  return (
    <div>
      <span data-testid="pane">{current.tab}</span>
      {(["general", "notifications", "advanced"] as const).map((tab) => (
        <button key={tab} type="button" onClick={() => setCurrent(pane(tab))}>
          {tab}
        </button>
      ))}
      <button type="button" aria-label="Back" disabled={!history.canGoBack} onClick={history.goBack} />
      <button
        type="button"
        aria-label="Forward"
        disabled={!history.canGoForward}
        onClick={history.goForward}
      />
    </div>
  );
}

const current = () => screen.getByTestId("pane").textContent;
const back = () => screen.getByRole("button", { name: "Back" });
const forward = () => screen.getByRole("button", { name: "Forward" });
function visit(...names: string[]) {
  for (const name of names) fireEvent.click(screen.getByRole("button", { name }));
}

describe("useSettingsHistory", () => {
  it("disables both buttons until there is history", () => {
    render(<Harness />);
    expect(back()).toBeDisabled();
    expect(forward()).toBeDisabled();
    visit("notifications");
    expect(back()).toBeEnabled();
    expect(forward()).toBeDisabled();
  });

  it("goes back and forward with the buttons", () => {
    render(<Harness />);
    visit("notifications", "advanced");
    fireEvent.click(back());
    expect(current()).toBe("notifications");
    expect(forward()).toBeEnabled();
    fireEvent.click(back());
    expect(current()).toBe("general");
    expect(back()).toBeDisabled();
    fireEvent.click(forward());
    fireEvent.click(forward());
    expect(current()).toBe("advanced");
    expect(forward()).toBeDisabled();
  });

  it("goes back and forward with Alt+Left and Alt+Right", () => {
    render(<Harness />);
    visit("notifications");
    const left = new KeyboardEvent("keydown", { key: "ArrowLeft", altKey: true, cancelable: true });
    act(() => void window.dispatchEvent(left));
    expect(current()).toBe("general");
    expect(left.defaultPrevented).toBe(true);
    fireEvent.keyDown(window, { key: "ArrowRight", altKey: true });
    expect(current()).toBe("notifications");
  });

  it("ignores arrows without Alt or with other modifiers", () => {
    render(<Harness />);
    visit("notifications");
    fireEvent.keyDown(window, { key: "ArrowLeft" });
    fireEvent.keyDown(window, { key: "ArrowLeft", altKey: true, ctrlKey: true });
    fireEvent.keyDown(window, { key: "ArrowLeft", altKey: true, shiftKey: true });
    expect(current()).toBe("notifications");
  });

  it("handles the BrowserBack and BrowserForward keys", () => {
    render(<Harness />);
    visit("notifications");
    fireEvent.keyDown(window, { key: "BrowserBack" });
    expect(current()).toBe("general");
    fireEvent.keyDown(window, { key: "BrowserForward" });
    expect(current()).toBe("notifications");
  });

  it("goes back and forward with the mouse side buttons and blocks webview navigation", () => {
    render(<Harness />);
    visit("notifications");
    const down = new MouseEvent("mousedown", { button: 3, cancelable: true });
    const aux = new MouseEvent("auxclick", { button: 3, cancelable: true });
    const up = new MouseEvent("mouseup", { button: 3, cancelable: true });
    act(() => {
      window.dispatchEvent(down);
      window.dispatchEvent(up);
      window.dispatchEvent(aux);
    });
    expect(current()).toBe("general");
    expect([down, up, aux].map((e) => e.defaultPrevented)).toEqual([true, true, true]);

    fireEvent.mouseUp(window, { button: 4 });
    expect(current()).toBe("notifications");
  });

  it("leaves other mouse buttons alone", () => {
    render(<Harness />);
    visit("notifications");
    const middle = new MouseEvent("mouseup", { button: 1, cancelable: true });
    act(() => void window.dispatchEvent(middle));
    expect(middle.defaultPrevented).toBe(false);
    expect(current()).toBe("notifications");
  });

  it("stays put while a modal sheet is open", () => {
    render(<Harness />);
    visit("notifications");
    const sheet = document.createElement("div");
    sheet.setAttribute("aria-modal", "true");
    document.body.appendChild(sheet);
    try {
      fireEvent.keyDown(window, { key: "ArrowLeft", altKey: true });
      fireEvent.mouseUp(window, { button: 3 });
      expect(current()).toBe("notifications");
    } finally {
      sheet.remove();
    }
  });

  it("stops listening after unmount", () => {
    const { unmount } = render(<Harness />);
    unmount();
    const left = new KeyboardEvent("keydown", { key: "ArrowLeft", altKey: true, cancelable: true });
    window.dispatchEvent(left);
    expect(left.defaultPrevented).toBe(false);
  });
});
