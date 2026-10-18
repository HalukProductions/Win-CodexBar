import type { LocaleKey } from "../i18n/keys";
import type { OpenAiApiDailyUsageSnapshot, OpenAiApiUsageSnapshot } from "../types/bridge";
import { formatTokens } from "./openaiThroughput";

const usdFormat = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" });

/** USD amount; sub-cent spend reads as "<$0.01" rather than "$0.00". */
export function formatUsd(value: number): string {
  if (value > 0 && value < 0.01) return "<$0.01";
  return usdFormat.format(value);
}

/** The Admin API reports UTC days, so the label stays on the UTC calendar. */
export function dayLabel(day: OpenAiApiDailyUsageSnapshot): string {
  return new Date(day.startTime * 1000).toISOString().slice(0, 10);
}

/** True when the OpenAI API usage history has at least one day to chart. */
export function hasSpendDays(usage: OpenAiApiUsageSnapshot | null | undefined): boolean {
  return (usage?.daily.length ?? 0) > 0;
}

/** Hover line for one day, upstream `InlineUsageDashboardModel.HoverDetail.summary`. */
export function spendDaySummary(day: OpenAiApiDailyUsageSnapshot, t: (key: LocaleKey) => string): string {
  return t("OpenAiSpendHoverDay")
    .replace("{}", dayLabel(day))
    .replace("{}", formatUsd(day.costUsd))
    .replace("{}", formatTokens(day.totalTokens));
}

export type ProviderChartMode = "live" | "spend";

export const PROVIDER_CHART_MODES_STORAGE_KEY = "codexbar.providerMenuChartModes";

function readModes(): Record<string, unknown> {
  try {
    const raw = window.localStorage.getItem(PROVIDER_CHART_MODES_STORAGE_KEY);
    if (!raw) return {};
    const parsed: unknown = JSON.parse(raw);
    return parsed && typeof parsed === "object" && !Array.isArray(parsed)
      ? (parsed as Record<string, unknown>)
      : {};
  } catch {
    return {};
  }
}

/** The stored Live / Spend choice for a provider; anything unknown is Live. */
export function readChartMode(providerId: string): ProviderChartMode {
  return readModes()[providerId] === "spend" ? "spend" : "live";
}

export function storeChartMode(providerId: string, mode: ProviderChartMode): void {
  try {
    const modes = readModes();
    modes[providerId] = mode;
    window.localStorage.setItem(PROVIDER_CHART_MODES_STORAGE_KEY, JSON.stringify(modes));
  } catch {
    // Not persisting the mode is harmless; the card falls back to Live.
  }
}
