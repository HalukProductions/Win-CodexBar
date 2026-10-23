import type { LocaleKey } from "../i18n/keys";

type Translate = (key: LocaleKey) => string;

/**
 * Render a Unix-ms timestamp as a localized "updated N ago" string,
 * matching the egui `render_advanced_tab` time display.
 *
 * Returns the `NeverUpdated` locale string when `timestampMs` is `null`
 * or `undefined`, mirroring how egui surfaces provider-update timing.
 */
export function formatRelativeUpdated(
  timestampMs: number | null | undefined,
  t: Translate,
  nowMs: number = Date.now(),
): string {
  if (timestampMs == null) {
    return t("NeverUpdated");
  }
  const diffSecs = Math.max(0, Math.floor((nowMs - timestampMs) / 1000));
  if (diffSecs < 60) {
    return t("UpdatedJustNow");
  }
  const diffMins = Math.floor(diffSecs / 60);
  if (diffMins < 60) {
    return countAgo(t, diffMins, "UpdatedOneMinuteAgo", "UpdatedMinutesAgo");
  }
  const diffHours = Math.floor(diffMins / 60);
  if (diffHours < 24) {
    return countAgo(t, diffHours, "UpdatedOneHourAgo", "UpdatedHoursAgo");
  }
  const diffDays = Math.floor(diffHours / 24);
  return countAgo(t, diffDays, "UpdatedOneDayAgo", "UpdatedDaysAgo");
}

function countAgo(t: Translate, count: number, one: LocaleKey, many: LocaleKey): string {
  return t(count === 1 ? one : many).replace("{}", String(count));
}
