import { useEffect } from "react";
import { getSystemTheme } from "../lib/tauri";
import type { ThemePreference } from "../types/bridge";

export type ResolvedTheme = "light" | "dark";

const MEDIA_QUERY = "(prefers-color-scheme: light)";
/** How often a visible Settings window re-reads the Windows theme. */
const SYSTEM_THEME_POLL_MS = 30_000;

function prefersLight(): boolean {
  if (typeof window === "undefined" || !window.matchMedia) {
    return false;
  }
  return window.matchMedia(MEDIA_QUERY).matches;
}

/** True inside the Tauri shell. The browser proof's `mockIPC` sets the same global. */
function hasTauriBridge(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export function resolveTheme(
  preference: ThemePreference,
  systemLight: boolean = prefersLight(),
): ResolvedTheme {
  if (preference === "light") return "light";
  if (preference === "dark") return "dark";
  return systemLight ? "light" : "dark";
}

function apply(theme: ResolvedTheme) {
  if (typeof document === "undefined") return;
  document.documentElement.dataset.theme = theme;
  document.documentElement.style.colorScheme = theme;
}

/**
 * Applies the Windows app theme read through the shell. Every native window is
 * pinned Dark, so the webview's media query can't report the OS setting. The
 * shell is re-asked on focus, on settings changes, and every
 * SYSTEM_THEME_POLL_MS while the window is visible. No theme-change event is
 * used: the pinned theme may suppress one.
 */
function followWindowsTheme(): () => void {
  let cancelled = false;
  const sync = () => {
    getSystemTheme()
      .then((theme) => {
        if (!cancelled) apply(theme);
      })
      .catch(() => {});
  };
  sync();

  const timer = window.setInterval(() => {
    if (document.visibilityState === "visible") sync();
  }, SYSTEM_THEME_POLL_MS);
  window.addEventListener("focus", sync);
  document.addEventListener("visibilitychange", sync);
  window.addEventListener("codexbar:settings-updated", sync);

  return () => {
    cancelled = true;
    window.clearInterval(timer);
    window.removeEventListener("focus", sync);
    document.removeEventListener("visibilitychange", sync);
    window.removeEventListener("codexbar:settings-updated", sync);
  };
}

export interface UseThemeOptions {
  /**
   * Follow the Windows app theme when the preference is `auto`. Only the
   * Settings surface sets this; other windows keep the media query, which
   * reads dark on every pinned window.
   */
  followSystem?: boolean;
}

/**
 * Applies the user's theme preference at runtime.
 *
 * When `preference === "auto"`, the UI follows the OS: the Windows app-theme
 * setting when `followSystem` is set under Tauri, otherwise the
 * `prefers-color-scheme` media query. Explicit "light"/"dark" overrides detach
 * from both.
 */
export function useTheme(preference: ThemePreference, options: UseThemeOptions = {}): void {
  const followSystem = options.followSystem === true;

  useEffect(() => {
    if (preference === "auto" && followSystem && hasTauriBridge()) {
      return followWindowsTheme();
    }

    apply(resolveTheme(preference));

    if (preference !== "auto" || typeof window === "undefined" || !window.matchMedia) {
      return;
    }

    const mq = window.matchMedia(MEDIA_QUERY);
    const handle = (event: MediaQueryListEvent) => apply(event.matches ? "light" : "dark");

    if (typeof mq.addEventListener === "function") {
      mq.addEventListener("change", handle);
      return () => mq.removeEventListener("change", handle);
    }
    // Safari < 14 fallback.
    mq.addListener(handle);
    return () => mq.removeListener(handle);
  }, [preference, followSystem]);
}
