import { afterEach, describe, expect, it } from "vitest";
import type { OpenAiApiDailyUsageSnapshot } from "../types/bridge";
import {
  formatUsd,
  hasSpendDays,
  PROVIDER_CHART_MODES_STORAGE_KEY,
  readChartMode,
  spendDaySummary,
  storeChartMode,
} from "./openaiSpend";

const START = 1_788_220_800;

function day(overrides: Partial<OpenAiApiDailyUsageSnapshot> = {}): OpenAiApiDailyUsageSnapshot {
  return {
    startTime: START,
    endTime: START + 86_400,
    costUsd: 12.5,
    requests: 10,
    inputTokens: 1000,
    cachedInputTokens: 0,
    outputTokens: 400,
    totalTokens: 1_400,
    lineItems: [],
    models: [],
    ...overrides,
  };
}

const t = (key: string) => (key === "OpenAiSpendHoverDay" ? "{}: {} · {} tokens" : key);

describe("spend helpers", () => {
  it("formats sub-cent spend without rounding it to zero", () => {
    expect(formatUsd(0.004)).toBe("<$0.01");
    expect(formatUsd(0)).toBe("$0.00");
    expect(formatUsd(12.5)).toBe("$12.50");
  });

  it("summarizes a UTC day as date, cost and tokens", () => {
    expect(spendDaySummary(day(), t)).toBe("2026-09-01: $12.50 · 1.4K tokens");
  });

  it("needs at least one day of history to offer Spend", () => {
    expect(hasSpendDays(null)).toBe(false);
    expect(hasSpendDays({ historyDays: 30, projectId: null, daily: [] })).toBe(false);
    expect(hasSpendDays({ historyDays: 30, projectId: null, daily: [day()] })).toBe(true);
  });
});

describe("chart mode persistence", () => {
  afterEach(() => window.localStorage.removeItem(PROVIDER_CHART_MODES_STORAGE_KEY));

  it("defaults to Live and remembers the choice per provider", () => {
    expect(readChartMode("openaiapi")).toBe("live");
    storeChartMode("openaiapi", "spend");
    expect(readChartMode("openaiapi")).toBe("spend");
    expect(readChartMode("other")).toBe("live");
    storeChartMode("other", "live");
    expect(JSON.parse(window.localStorage.getItem(PROVIDER_CHART_MODES_STORAGE_KEY) ?? "")).toEqual({
      openaiapi: "spend",
      other: "live",
    });
  });

  it("falls back to Live for unknown or malformed stored values", () => {
    window.localStorage.setItem(PROVIDER_CHART_MODES_STORAGE_KEY, JSON.stringify({ openaiapi: "weekly" }));
    expect(readChartMode("openaiapi")).toBe("live");
    window.localStorage.setItem(PROVIDER_CHART_MODES_STORAGE_KEY, "not json");
    expect(readChartMode("openaiapi")).toBe("live");
    window.localStorage.setItem(PROVIDER_CHART_MODES_STORAGE_KEY, "[]");
    expect(readChartMode("openaiapi")).toBe("live");
  });
});
