import type { ThemePreference } from "../types/bridge";
import { FLOATBAR_WINDOW_LABEL } from "../floatbar/api";
import { isProofScenarioName, type ProofScenarioName } from "./fixtures/scenarios";

export const PROOF_WINDOWS = ["main", "settings", "flyout", "floatbar"] as const;
export type ProofWindow = (typeof PROOF_WINDOWS)[number];

/** The Windows app theme that the `get_system_theme` fixture reports. */
export type ProofSystemTheme = "light" | "dark";

export interface ProofParams {
  window: ProofWindow;
  /** Raw settings tab request (`general`, `providers:codex`, ...); App parses it. */
  tab: string;
  fixture: ProofScenarioName;
  /** Overrides the fixture's saved theme when present. */
  theme: ThemePreference | null;
  /** The Windows app theme (`system=light|dark`); dark when absent. */
  systemTheme: ProofSystemTheme;
}

function isProofWindow(value: string): value is ProofWindow {
  return (PROOF_WINDOWS as readonly string[]).includes(value);
}

function isThemePreference(value: string): value is ThemePreference {
  return value === "dark" || value === "light" || value === "auto";
}

function isSystemTheme(value: string): value is ProofSystemTheme {
  return value === "dark" || value === "light";
}

export function parseProofParams(search: string): ProofParams {
  const params = new URLSearchParams(search);
  const windowParam = params.get("window") ?? "";
  const fixture = params.get("fixture") ?? "";
  const theme = params.get("theme") ?? "";
  const system = params.get("system") ?? "";
  return {
    window: isProofWindow(windowParam) ? windowParam : "main",
    tab: params.get("tab") || "general",
    fixture: isProofScenarioName(fixture) ? fixture : "default",
    theme: isThemePreference(theme) ? theme : null,
    systemTheme: isSystemTheme(system) ? system : "dark",
  };
}

export function windowLabel(window: ProofWindow): string {
  return window === "floatbar" ? FLOATBAR_WINDOW_LABEL : window;
}
