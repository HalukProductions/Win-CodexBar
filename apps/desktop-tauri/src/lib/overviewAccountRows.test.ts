import { describe, expect, it } from "vitest";
import { makeUsageSnapshot } from "../test/fixtures";
import type {
  ClaudeAccount,
  ClaudeSwapOverviewAccountsBridge,
  CodexAccount,
  CodexAccountUsageSnapshot,
} from "../types/bridge";
import {
  expandOverviewAccountRows,
  type ClaudeManagedOverviewAccounts,
  type ClaudeTokenOverviewAccounts,
  type CodexOverviewAccounts,
} from "./overviewAccountRows";

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

/** The active Claude card: its plan and email must never reach a token account row. */
const activeClaude = makeUsageSnapshot("claude", {
  displayName: "Claude",
  planName: "Max",
  accountEmail: "active@example.com",
  error: null,
});

function claudeState(overrides: Partial<ClaudeTokenOverviewAccounts> = {}): ClaudeTokenOverviewAccounts {
  return {
    accounts: [
      { id: "tok-work", label: "Team Alpha" },
      { id: "tok-personal", label: "Side Project" },
    ],
    usage: {
      "tok-work": {
        primary: { usedPercent: 22, windowMinutes: 300, resetsAt: "2026-05-24T05:00:00Z" },
        secondary: { usedPercent: 41, windowMinutes: 10_080, resetsAt: null },
        updatedAt: "2026-05-24T00:00:00Z",
      },
    },
    ...overrides,
  };
}

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

  it("expands Claude token accounts into one row each, with unique keys next to codex rows", () => {
    const entries = expandOverviewAccountRows(
      [activeCodex, activeClaude],
      codexState(),
      OPTIONS,
      claudeState(),
    );
    expect(entries.map((e) => e.key)).toEqual([
      "codex/account/acc-work",
      "codex/account/acc-home",
      "claude/account/tok-work",
      "claude/account/tok-personal",
    ]);
    expect(entries.map((e) => e.accountId)).toEqual([
      "acc-work",
      "acc-home",
      "tok-work",
      "tok-personal",
    ]);
    const keys = entries.map((e) => e.key);
    expect(new Set(keys).size).toBe(keys.length);
  });

  it("builds each Claude token row from its own cached usage, never from the active card", () => {
    const [work] = expandOverviewAccountRows([activeClaude], null, OPTIONS, claudeState());
    const row = work.provider;
    expect(row.accountEmail).toBe("Team Alpha");
    expect(row.planName).toBeNull();
    expect(row.cost).toBeNull();
    expect(row.subscription).toBeNull();
    expect(row.primary.usedPercent).toBe(22);
    expect(row.primary.remainingPercent).toBe(78);
    expect(row.primary.windowMinutes).toBe(300);
    expect(row.primary.resetsAt).toBe("2026-05-24T05:00:00Z");
    expect(row.secondary?.usedPercent).toBe(41);
    expect(row.secondary?.windowMinutes).toBe(10_080);
    expect(row.updatedAt).toBe("2026-05-24T00:00:00Z");
    expect(row.error).toBeNull();
    expect(row.errorState).toBe("ready");
    expect(JSON.stringify(row)).not.toContain("active@example.com");
    expect(JSON.stringify(row)).not.toContain("Max");
  });

  it("shows a Claude token account with no cached usage as unavailable, with no update time", () => {
    const [, personal] = expandOverviewAccountRows([activeClaude], null, OPTIONS, claudeState());
    const row = personal.provider;
    expect(row.error).toBe("No usage data");
    expect(row.errorState).toBe("unknown");
    expect(row.primary.usedPercent).toBe(0);
    expect(row.secondary).toBeNull();
    expect(row.updatedAt).toBe("");
    expect(row.planName).toBeNull();
    expect(row.accountEmail).toBe("Side Project");
  });

  it("uses generic Claude token labels under Hide personal information", () => {
    const entries = expandOverviewAccountRows([activeClaude], null, {
      ...OPTIONS,
      hidePersonalInfo: true,
    }, claudeState());
    // Ordinals follow the row order, not the sorted id ("tok-personal" sorts
    // first but is listed second), so the first row reads Account 1.
    expect(entries.map((e) => e.provider.accountEmail)).toEqual(["Account 1", "Account 2"]);
    const serialized = JSON.stringify(entries.map((e) => e.provider));
    expect(serialized).not.toContain("Team Alpha");
    expect(serialized).not.toContain("Side Project");
  });

  it("keeps the single Claude card when there is at most one token account", () => {
    const single = claudeState({ accounts: [{ id: "tok-work", label: "Team Alpha" }] });
    const entries = expandOverviewAccountRows([activeClaude], null, OPTIONS, single);
    expect(entries.map((e) => e.key)).toEqual(["claude"]);
    expect(entries[0].accountId).toBeNull();
    expect(entries[0].provider).toBe(activeClaude);
  });

  it("keeps the Claude card when token accounts are absent, and leaves other providers alone", () => {
    expect(
      expandOverviewAccountRows([activeCodex, activeClaude], null, OPTIONS, null).map((e) => e.key),
    ).toEqual(["codex", "claude"]);
    const entries = expandOverviewAccountRows(
      [activeCodex, activeClaude],
      null,
      OPTIONS,
      claudeState(),
    );
    expect(entries.filter((e) => e.provider.providerId === "codex")).toHaveLength(1);
  });
});

function swapState(
  overrides: Partial<ClaudeSwapOverviewAccountsBridge> = {},
): ClaudeSwapOverviewAccountsBridge {
  return {
    accounts: [
      { id: "claude-swap:1", slot: 1, label: "work@example.com" },
      { id: "claude-swap:2", slot: 2, label: "personal@example.com" },
      { id: "claude-swap:3", slot: 3, label: "backup@example.com" },
    ],
    usage: {
      "claude-swap:1": {
        primary: { usedPercent: 22, windowMinutes: 300, resetsAt: "2026-05-24T05:00:00Z" },
        secondary: { usedPercent: 41, windowMinutes: 10_080, resetsAt: null },
        updatedAt: "2026-05-24T00:00:00Z",
      },
      "claude-swap:2": {
        primary: { usedPercent: 63, windowMinutes: 300, resetsAt: null },
        secondary: null,
        updatedAt: "2026-05-23T20:00:00Z",
      },
    },
    ...overrides,
  };
}

describe("expandOverviewAccountRows for claude-swap and tcr pools", () => {
  it("expands a claude-swap pool into one row per account, in pool order", () => {
    const entries = expandOverviewAccountRows(
      [activeClaude],
      null,
      OPTIONS,
      null,
      swapState(),
    );
    expect(entries.map((e) => e.key)).toEqual([
      "claude/account/claude-swap:1",
      "claude/account/claude-swap:2",
      "claude/account/claude-swap:3",
    ]);
    expect(entries.map((e) => e.accountId)).toEqual([
      "claude-swap:1",
      "claude-swap:2",
      "claude-swap:3",
    ]);
    expect(entries.map((e) => e.provider.accountEmail)).toEqual([
      "work@example.com",
      "personal@example.com",
      "backup@example.com",
    ]);
  });

  it("builds each row from its own cached usage, never from the active card", () => {
    const [work] = expandOverviewAccountRows([activeClaude], null, OPTIONS, null, swapState());
    const row = work.provider;
    expect(row.primary.usedPercent).toBe(22);
    expect(row.primary.remainingPercent).toBe(78);
    expect(row.primary.windowMinutes).toBe(300);
    expect(row.secondary?.usedPercent).toBe(41);
    expect(row.updatedAt).toBe("2026-05-24T00:00:00Z");
    expect(row.planName).toBeNull();
    expect(row.cost).toBeNull();
    expect(row.error).toBeNull();
    expect(row.errorState).toBe("ready");
    expect(JSON.stringify(row)).not.toContain("active@example.com");
    expect(JSON.stringify(row)).not.toContain("Max");
  });

  it("keeps a single-window account ready with no secondary", () => {
    const [, second] = expandOverviewAccountRows([activeClaude], null, OPTIONS, null, swapState());
    expect(second.provider.secondary).toBeNull();
    expect(second.provider.primary.usedPercent).toBe(63);
    expect(second.provider.error).toBeNull();
  });

  it("shows an account with no cached usage as unavailable, with no update time", () => {
    const entries = expandOverviewAccountRows([activeClaude], null, OPTIONS, null, swapState());
    const row = entries[2].provider;
    expect(row.error).toBe("No usage data");
    expect(row.errorState).toBe("unknown");
    expect(row.primary.usedPercent).toBe(0);
    expect(row.secondary).toBeNull();
    expect(row.updatedAt).toBe("");
    expect(row.accountEmail).toBe("backup@example.com");
  });

  it("takes precedence over Claude token accounts when both are present", () => {
    const entries = expandOverviewAccountRows(
      [activeClaude],
      null,
      OPTIONS,
      claudeState(),
      swapState(),
    );
    expect(entries.map((e) => e.key)).toEqual([
      "claude/account/claude-swap:1",
      "claude/account/claude-swap:2",
      "claude/account/claude-swap:3",
    ]);
  });

  it("keeps the single Claude card when the pool has at most one account", () => {
    const single = swapState({ accounts: [{ id: "claude-swap:1", slot: 1, label: "work@example.com" }] });
    const entries = expandOverviewAccountRows([activeClaude], null, OPTIONS, null, single);
    expect(entries.map((e) => e.key)).toEqual(["claude"]);
    expect(entries[0].accountId).toBeNull();
    expect(entries[0].provider).toBe(activeClaude);
  });

  it("uses generic Account N labels and never the real label under Hide personal information", () => {
    const entries = expandOverviewAccountRows([activeClaude], null, {
      ...OPTIONS,
      hidePersonalInfo: true,
    }, null, swapState());
    expect(entries.map((e) => e.provider.accountEmail)).toEqual([
      "Account 1",
      "Account 2",
      "Account 3",
    ]);
    const serialized = JSON.stringify(entries.map((e) => e.provider));
    expect(serialized).not.toContain("work@example.com");
    expect(serialized).not.toContain("personal@example.com");
    expect(serialized).not.toContain("backup@example.com");
  });

  it("leaves Codex rows alone when claude-swap rows expand next to them", () => {
    const entries = expandOverviewAccountRows(
      [activeCodex, activeClaude],
      codexState(),
      OPTIONS,
      null,
      swapState(),
    );
    expect(entries.map((e) => e.key)).toEqual([
      "codex/account/acc-work",
      "codex/account/acc-home",
      "claude/account/claude-swap:1",
      "claude/account/claude-swap:2",
      "claude/account/claude-swap:3",
    ]);
    const keys = entries.map((e) => e.key);
    expect(new Set(keys).size).toBe(keys.length);
  });
});

function managedAccount(id: string, email: string, overrides: Partial<ClaudeAccount> = {}): ClaudeAccount {
  return {
    id,
    email,
    organization: null,
    plan: "max",
    isActive: false,
    isSaved: true,
    usage: null,
    ...overrides,
  };
}

function managedState(
  accounts: ClaudeAccount[] = [
    managedAccount("first:org", "first@example.com", {
      isActive: true,
      usage: {
        fiveHour: { usedPercent: 99, resetsAt: "2026-11-01T22:00:00Z" },
        sevenDay: { usedPercent: 33, resetsAt: "2026-11-04T09:00:00Z" },
        updatedAt: "2026-11-01T20:00:00Z",
      },
    }),
    managedAccount("second:org", "second@example.com", {
      usage: {
        fiveHour: { usedPercent: 7, resetsAt: "2026-11-01T20:13:00Z" },
        sevenDay: { usedPercent: 65, resetsAt: null },
        updatedAt: "2026-11-01T19:55:00Z",
      },
    }),
  ],
): ClaudeManagedOverviewAccounts {
  return { accounts };
}

describe("expandOverviewAccountRows for saved Claude Code accounts", () => {
  it("expands saved Claude Code accounts into one row each, in list order", () => {
    const entries = expandOverviewAccountRows([activeClaude], null, OPTIONS, null, null, managedState());
    expect(entries.map((e) => e.key)).toEqual([
      "claude/account/first:org",
      "claude/account/second:org",
    ]);
    expect(entries.map((e) => e.accountId)).toEqual(["first:org", "second:org"]);
    expect(entries.map((e) => e.provider.accountEmail)).toEqual([
      "first@example.com",
      "second@example.com",
    ]);
  });

  it("builds each row from that account's own cached windows, never from the active card", () => {
    const [, second] = expandOverviewAccountRows(
      [activeClaude],
      null,
      OPTIONS,
      null,
      null,
      managedState(),
    );
    const row = second.provider;
    expect(row.primary.usedPercent).toBe(7);
    expect(row.primary.windowMinutes).toBe(300);
    expect(row.primary.resetsAt).toBe("2026-11-01T20:13:00Z");
    expect(row.secondary?.usedPercent).toBe(65);
    expect(row.secondary?.windowMinutes).toBe(10_080);
    expect(row.updatedAt).toBe("2026-11-01T19:55:00Z");
    expect(row.planName).toBeNull();
    expect(row.accountEmail).toBe("second@example.com");
    expect(row.error).toBeNull();
  });

  it("shows an account with no cached usage as unavailable, with no update time", () => {
    const entries = expandOverviewAccountRows(
      [activeClaude],
      null,
      OPTIONS,
      null,
      null,
      managedState([
        managedAccount("first:org", "first@example.com"),
        managedAccount("second:org", "second@example.com", { usageError: "Sign in again to check this Claude account." }),
      ]),
    );
    for (const entry of entries) {
      expect(entry.provider.error).toBe("No usage data");
      expect(entry.provider.errorState).toBe("unknown");
      expect(entry.provider.updatedAt).toBe("");
      expect(entry.provider.secondary).toBeNull();
    }
  });

  it("shows the sign-in message instead of stale windows when a saved login needs signing in again", () => {
    const [first, second] = managedState().accounts;
    const entries = expandOverviewAccountRows(
      [activeClaude],
      null,
      OPTIONS,
      null,
      null,
      managedState([
        first,
        {
          ...second,
          needsAuthentication: true,
          usageError: "Sign in again to check this Claude account.",
        },
      ]),
    );
    expect(entries[0].provider.primary.usedPercent).toBe(99);
    const row = entries[1].provider;
    expect(row.error).toBe("Sign in again to check this Claude account.");
    expect(row.errorState).toBe("unknown");
    expect(row.primary.usedPercent).toBe(0);
    expect(row.secondary).toBeNull();
    expect(row.updatedAt).toBe("");
  });

  it("uses generic Account N labels and never the email under Hide personal information", () => {
    const entries = expandOverviewAccountRows(
      [activeClaude],
      null,
      { ...OPTIONS, hidePersonalInfo: true },
      null,
      null,
      managedState(),
    );
    expect(entries.map((e) => e.provider.accountEmail)).toEqual(["Account 1", "Account 2"]);
    const serialized = JSON.stringify(entries.map((e) => e.provider));
    expect(serialized).not.toContain("first@example.com");
    expect(serialized).not.toContain("second@example.com");
  });

  it("keeps the single Claude card when there is at most one saved account", () => {
    const entries = expandOverviewAccountRows(
      [activeClaude],
      null,
      OPTIONS,
      null,
      null,
      managedState([managedAccount("first:org", "first@example.com")]),
    );
    expect(entries.map((e) => e.key)).toEqual(["claude"]);
    expect(entries[0].provider).toBe(activeClaude);
  });

  it("yields to claude-swap pools and Claude token accounts when those have two or more", () => {
    const withTokens = expandOverviewAccountRows(
      [activeClaude],
      null,
      OPTIONS,
      claudeState(),
      null,
      managedState(),
    );
    expect(withTokens.map((e) => e.key)).toEqual([
      "claude/account/tok-work",
      "claude/account/tok-personal",
    ]);
    const withSwap = expandOverviewAccountRows(
      [activeClaude],
      null,
      OPTIONS,
      null,
      swapState(),
      managedState(),
    );
    expect(withSwap.map((e) => e.key)).toEqual([
      "claude/account/claude-swap:1",
      "claude/account/claude-swap:2",
      "claude/account/claude-swap:3",
    ]);
  });

  it("leaves Codex rows alone when Claude Code account rows expand next to them", () => {
    const entries = expandOverviewAccountRows(
      [activeCodex, activeClaude],
      codexState(),
      OPTIONS,
      null,
      null,
      managedState(),
    );
    expect(entries.map((e) => e.key)).toEqual([
      "codex/account/acc-work",
      "codex/account/acc-home",
      "claude/account/first:org",
      "claude/account/second:org",
    ]);
  });
});
