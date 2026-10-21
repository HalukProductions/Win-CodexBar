import { describe, expect, it } from "vitest";
import { makeUsageSnapshot } from "../test/fixtures";
import type { CodexAccount, CodexAccountUsageSnapshot } from "../types/bridge";
import { expandOverviewAccountRows, type CodexOverviewAccounts } from "./overviewAccountRows";

const OPTIONS = {
  hidePersonalInfo: false,
  accountWord: "Account",
  unavailableText: "No usage data",
};

function account(id: string, emailHint: string): CodexAccount {
  return {
    id,
    nickname: null,
    emailHint,
    authSubject: null,
    providerAccountId: null,
    codexHomePath: `/example/${id}`,
    source: "managedByApp",
    createdAt: "2026-05-01T00:00:00Z",
    updatedAt: "2026-05-01T00:00:00Z",
    lastAuthenticatedAt: null,
  };
}

function usage(
  overrides: Partial<CodexAccountUsageSnapshot> = {},
): CodexAccountUsageSnapshot {
  return {
    email: null,
    providerAccountId: null,
    plan: "plus",
    allowed: true,
    limitReached: false,
    primaryWindow: { usedPercent: 42, resetAt: "2026-05-24T05:00:00Z", limitWindowSeconds: 18_000 },
    secondaryWindow: { usedPercent: 10, resetAt: null, limitWindowSeconds: 604_800 },
    credits: null,
    updatedAt: "2026-05-24T00:00:00Z",
    ...overrides,
  };
}

function codexState(overrides: Partial<CodexOverviewAccounts> = {}): CodexOverviewAccounts {
  return {
    accounts: [account("acc-work", "work@example.com"), account("acc-home", "home@example.com")],
    snapshots: { "acc-work": usage() },
    displayNames: { "acc-work": "work@example.com", "acc-home": "home@example.com" },
    accountOrdinals: { "acc-work": 1, "acc-home": 2 },
    ...overrides,
  };
}

/** The active-account Codex card: its plan, email, windows and cost must never reach a row. */
const activeCodex = makeUsageSnapshot("codex", {
  displayName: "Codex",
  planName: "pro",
  accountEmail: "active@example.com",
  primary: { ...makeUsageSnapshot("codex").primary, usedPercent: 80, remainingPercent: 20 },
  secondary: { ...makeUsageSnapshot("codex").primary, usedPercent: 90, remainingPercent: 10 },
  cost: {
    used: 9,
    limit: 10,
    remaining: 1,
    currencyCode: "USD",
    period: "monthly",
    resetsAt: null,
    formattedUsed: "$9",
    formattedLimit: "$10",
  },
  error: null,
});
const claude = makeUsageSnapshot("claude", { displayName: "Claude" });

describe("expandOverviewAccountRows", () => {
  it("keeps one card per provider when the codex store is missing or has one account", () => {
    const providers = [activeCodex, claude];
    expect(expandOverviewAccountRows(providers, null, OPTIONS).map((e) => e.key)).toEqual([
      "codex",
      "claude",
    ]);
    const single = codexState({ accounts: [account("acc-work", "work@example.com")] });
    const entries = expandOverviewAccountRows(providers, single, OPTIONS);
    expect(entries.map((e) => e.key)).toEqual(["codex", "claude"]);
    expect(entries.every((e) => e.accountId === null)).toBe(true);
  });

  it("expands only codex into one row per account and leaves other providers alone", () => {
    const entries = expandOverviewAccountRows([activeCodex, claude], codexState(), OPTIONS);
    expect(entries.map((e) => e.key)).toEqual([
      "codex/account/acc-work",
      "codex/account/acc-home",
      "claude",
    ]);
    expect(entries.map((e) => e.accountId)).toEqual(["acc-work", "acc-home", null]);
    expect(entries[2].provider).toBe(claude);
  });

  it("builds each row from its own account and never from the active card", () => {
    const [work] = expandOverviewAccountRows([activeCodex], codexState(), OPTIONS);
    const row = work.provider;
    expect(row.accountEmail).toBe("work@example.com");
    expect(row.planName).toBe("plus");
    expect(row.primary.usedPercent).toBe(42);
    expect(row.primary.remainingPercent).toBe(58);
    expect(row.primary.windowMinutes).toBe(300);
    expect(row.primary.resetsAt).toBe("2026-05-24T05:00:00Z");
    expect(row.secondary?.usedPercent).toBe(10);
    expect(row.secondary?.windowMinutes).toBe(10_080);
    expect(row.cost).toBeNull();
    expect(row.pace).toBeNull();
    expect(row.error).toBeNull();
    expect(row.errorState).toBe("ready");
    expect(row.updatedAt).toBe("2026-05-24T00:00:00Z");
  });

  it("shows a missing account as unavailable with no metrics, not the active quota", () => {
    const [, home] = expandOverviewAccountRows([activeCodex], codexState(), OPTIONS);
    const row = home.provider;
    expect(row.error).toBe("No usage data");
    expect(row.errorState).toBe("unknown");
    expect(row.primary.usedPercent).toBe(0);
    expect(row.secondary).toBeNull();
    expect(row.planName).toBeNull();
    expect(row.accountEmail).toBe("home@example.com");
    expect(row.cost).toBeNull();
  });

  it("uses the generic privacy label for every account when personal info is hidden", () => {
    const entries = expandOverviewAccountRows([activeCodex], codexState(), {
      ...OPTIONS,
      hidePersonalInfo: true,
    });
    expect(entries.map((e) => e.provider.accountEmail)).toEqual(["Account 1", "Account 2"]);
    const serialized = JSON.stringify(entries.map((e) => e.provider));
    expect(serialized).not.toContain("work@example.com");
    expect(serialized).not.toContain("home@example.com");
  });

  it("keeps a row's keys unique and leaves the provider slot count unchanged", () => {
    const entries = expandOverviewAccountRows([activeCodex, claude], codexState(), OPTIONS);
    const keys = entries.map((e) => e.key);
    expect(new Set(keys).size).toBe(keys.length);
    expect(new Set(entries.map((e) => e.provider.providerId))).toEqual(new Set(["codex", "claude"]));
  });
});
