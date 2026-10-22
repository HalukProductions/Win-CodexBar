import { describe, expect, it } from "vitest";

import {
  buildSpendProviderGroups,
  compareSpendProviderGroups,
  findSpendDay,
  formatSpendDay,
  type SpendProviderGroup,
} from "./usageSpendBreakdown";
import type { SpendContract, SpendModelRow } from "../types/bridge";

const model = (name: string, costUsd: number | null, totalTokens: number): SpendModelRow => ({
  model: name,
  costUsd,
  inputTokens: totalTokens,
  outputTokens: 0,
  cacheReadTokens: 0,
  totalTokens,
  customPricing: false,
});

const contract = (overrides: Partial<SpendContract> = {}): SpendContract =>
  ({
    providerId: "codex",
    knownCostUsd: 3.5,
    historyCoverageEstablished: true,
    priceCoverage: { priced: 2, unpriced: 0, unmetered: 0, estimated: 0 },
    models: [model("gpt-5", 3, 1000), model("gpt-5-mini", 0.5, 500)],
    daily: [
      { day: "2026-09-18", costUsd: 1, totalTokens: 100 },
      { day: "2026-09-19", costUsd: null, totalTokens: null },
    ],
    ...overrides,
  }) as SpendContract;

const group = (displayName: string, costUsd: number | null, totalTokens: number | null): SpendProviderGroup => ({
  providerId: displayName.toLowerCase(),
  displayName,
  costUsd,
  totalTokens,
  costIsPartial: false,
  hasPartialModelHistory: false,
  models: [],
});

describe("usage spend provider breakdown", () => {
  it("groups the contract models under their provider with summed tokens", () => {
    const [codex] = buildSpendProviderGroups(contract(), "Codex");
    expect(codex).toMatchObject({
      providerId: "codex",
      displayName: "Codex",
      costUsd: 3.5,
      totalTokens: 1500,
      costIsPartial: false,
      hasPartialModelHistory: false,
    });
    expect(codex.models.map((row) => row.model)).toEqual(["gpt-5", "gpt-5-mini"]);
  });

  it("leaves the provider token total unknown when any model's tokens are unknown", () => {
    const [codex] = buildSpendProviderGroups(
      contract({
        models: [
          model("gpt-5", 3, 1000),
          { ...model("glm-5", null, 0), inputTokens: null, outputTokens: null, cacheReadTokens: null, totalTokens: null },
        ],
      }),
      "Codex",
    );
    expect(codex.totalTokens).toBeNull();
  });

  it("returns no groups without model-level history", () => {
    expect(buildSpendProviderGroups(contract({ models: [] }), "Codex")).toEqual([]);
  });

  it("flags unpriced models and incomplete history as partial", () => {
    const [codex] = buildSpendProviderGroups(
      contract({
        priceCoverage: { priced: 1, unpriced: 1, unmetered: 0, estimated: 0 },
        historyCoverageEstablished: false,
      }),
      "Codex",
    );
    expect(codex.costIsPartial).toBe(true);
    expect(codex.hasPartialModelHistory).toBe(true);
  });

  it("orders groups by cost, then tokens, then name with unknown values last", () => {
    const ordered = [
      group("Zed", null, null),
      group("Beta", 2, 10),
      group("Alpha", 2, 10),
      group("Gamma", 5, 1),
      group("Tokens", null, 99),
    ]
      .sort(compareSpendProviderGroups)
      .map((entry) => entry.displayName);
    expect(ordered).toEqual(["Gamma", "Alpha", "Beta", "Tokens", "Zed"]);
  });

  it("finds a selected day only while it is inside the current range", () => {
    const { daily } = contract();
    expect(findSpendDay(daily, "2026-09-18")?.costUsd).toBe(1);
    expect(findSpendDay(daily, "2026-01-01")).toBeNull();
    expect(findSpendDay(daily, null)).toBeNull();
  });

  it("formats dashboard days and falls back for invalid dates", () => {
    expect(formatSpendDay("2026-09-19")).toContain("2026");
    expect(formatSpendDay("2026-02-31")).toBe("2026-02-31");
    expect(formatSpendDay("not-a-day")).toBe("not-a-day");
  });
});
