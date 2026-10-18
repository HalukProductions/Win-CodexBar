import { useEffect, useRef, useState } from "react";
import type { SettingsTabRequest } from "./settingsTabs";

/** A visited Settings pane: a tab and, on Providers, the provider shown. */
export type SettingsPane = SettingsTabRequest;

export interface SettingsHistory {
  current: SettingsPane;
  back: SettingsPane[];
  forward: SettingsPane[];
}

export function samePane(lhs: SettingsPane, rhs: SettingsPane): boolean {
  return lhs.tab === rhs.tab && lhs.providerId === rhs.providerId;
}

/**
 * Record a pane change (macOS `PreferencesSelection.pane` didSet). Reselecting
 * the current pane is not a visit; a new visit clears the forward stack.
 */
export function visitPane(history: SettingsHistory, pane: SettingsPane): SettingsHistory {
  if (samePane(history.current, pane)) return history;
  return { current: pane, back: [...history.back, history.current], forward: [] };
}

export function stepBack(history: SettingsHistory): SettingsHistory | null {
  const previous = history.back[history.back.length - 1];
  if (!previous) return null;
  return {
    current: previous,
    back: history.back.slice(0, -1),
    forward: [...history.forward, history.current],
  };
}

export function stepForward(history: SettingsHistory): SettingsHistory | null {
  const next = history.forward[history.forward.length - 1];
  if (!next) return null;
  return {
    current: next,
    back: [...history.back, history.current],
    forward: history.forward.slice(0, -1),
  };
}

/** `MouseEvent.button` values of the mouse's side (X1 / X2) buttons. */
const MOUSE_BACK = 3;
const MOUSE_FORWARD = 4;

function historyKeyDirection(event: KeyboardEvent): "back" | "forward" | null {
  if (event.key === "BrowserBack") return "back";
  if (event.key === "BrowserForward") return "forward";
  if (!event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return null;
  if (event.key === "ArrowLeft") return "back";
  if (event.key === "ArrowRight") return "forward";
  return null;
}

function historyMouseDirection(event: MouseEvent): "back" | "forward" | null {
  if (event.button === MOUSE_BACK) return "back";
  if (event.button === MOUSE_FORWARD) return "forward";
  return null;
}

/** A modal sheet owns the keyboard and pointer; the pane behind it stays put. */
function modalOpen(): boolean {
  return document.querySelector('[aria-modal="true"]') !== null;
}

export interface SettingsHistoryControls {
  canGoBack: boolean;
  canGoForward: boolean;
  goBack: () => void;
  goForward: () => void;
}

/**
 * Back/Forward through visited Settings panes. Every change of `current` counts
 * as a visit, including ones driven from outside (deep links, the shell's
 * surface target, proof mode), matching macOS where a pane the controller
 * presents joins the history. Traversal hands the target to `navigate`, and the
 * pane it lands on is not recorded again.
 *
 * Also listens on the window for Alt+Left / Alt+Right, the BrowserBack /
 * BrowserForward keys and the mouse side buttons, and suppresses WebView2's
 * own page navigation for them.
 */
export function useSettingsHistory(
  current: SettingsPane,
  navigate: (pane: SettingsPane) => void,
): SettingsHistoryControls {
  const [stored, setStored] = useState<SettingsHistory>(() => ({
    current,
    back: [],
    forward: [],
  }));

  // Adjust during render when the pane changed (no effect sync).
  let history = stored;
  if (!samePane(stored.current, current)) {
    history = visitPane(stored, current);
    setStored(history);
  }

  const traverse = (next: SettingsHistory | null) => {
    if (!next) return;
    setStored(next);
    navigate(next.current);
  };
  const goBack = () => traverse(stepBack(history));
  const goForward = () => traverse(stepForward(history));

  const latest = useRef({ goBack, goForward });
  latest.current = { goBack, goForward };

  useEffect(() => {
    const go = (direction: "back" | "forward") => {
      if (direction === "back") latest.current.goBack();
      else latest.current.goForward();
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || modalOpen()) return;
      const direction = historyKeyDirection(event);
      if (!direction) return;
      event.preventDefault();
      go(direction);
    };
    // The side buttons navigate on mouseup; mousedown and auxclick are only
    // cancelled so the webview cannot run its own history navigation.
    const suppress = (event: MouseEvent) => {
      if (historyMouseDirection(event)) event.preventDefault();
    };
    const onMouseUp = (event: MouseEvent) => {
      const direction = historyMouseDirection(event);
      if (!direction) return;
      event.preventDefault();
      if (modalOpen()) return;
      go(direction);
    };
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("mousedown", suppress);
    window.addEventListener("auxclick", suppress);
    window.addEventListener("mouseup", onMouseUp);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("mousedown", suppress);
      window.removeEventListener("auxclick", suppress);
      window.removeEventListener("mouseup", onMouseUp);
    };
  }, []);

  return {
    canGoBack: history.back.length > 0,
    canGoForward: history.forward.length > 0,
    goBack,
    goForward,
  };
}
