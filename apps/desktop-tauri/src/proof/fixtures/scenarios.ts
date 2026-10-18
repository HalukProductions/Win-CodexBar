import type {
  ClaudeAccount,
  CodexAccount,
  CodexAccountsStateBridge,
  CodexAccountUsageSnapshot,
  ProviderCatalogEntry,
  ProviderUsageSnapshot,
  SettingsSnapshot,
} from "../../types/bridge";
import {
  PROOF_PROVIDER_CATALOG,
  baseSettings,
  isoAfter,
  isoBefore,
  providerSnapshot,
  rateWindow,
} from "./common";

export const PROOF_SCENARIO_NAMES = [
  "default",
  "two-codex-accounts",
  "errors",
  "empty",
  "openai-live-spend",
] as const;

export type ProofScenarioName = (typeof PROOF_SCENARIO_NAMES)[number];

/** Everything the mocked bridge serves for one scenario. All data is synthetic. */
export interface ProofScenario {
  name: ProofScenarioName;
  settings: SettingsSnapshot;
  catalog: ProviderCatalogEntry[];
  providers: ProviderUsageSnapshot[];
  codexAccounts: CodexAccountsStateBridge;
  claudeAccounts: ClaudeAccount[];
  /** Local-log spend; providers without an entry have no local logs. */
  localSpend: LocalSpendFixture[];
}

export interface LocalSpendFixture {
  providerId: string;
  daily: number;
  tokensPerDay: number;
  model: string;
}

function codexSpend(): LocalSpendFixture {
  return { providerId: "codex", daily: 1.84, tokensPerDay: 412_000, model: "gpt-5-codex" };
}

function claudeSpend(): LocalSpendFixture {
  return { providerId: "claude", daily: 2.37, tokensPerDay: 289_000, model: "claude-sonnet-4" };
}

const SESSION_MINUTES = 300;
const WEEK_MINUTES = 10_080;
const MONTH_MINUTES = 43_200;
const DAY_MINUTES = 1_440;

function codexSnapshot(now: Date, email: string): ProviderUsageSnapshot {
  return providerSnapshot(
    {
      providerId: "codex",
      displayName: "Codex",
      primary: rateWindow(now, 34, SESSION_MINUTES, 142),
      primaryLabel: "Session",
      updatedAt: isoBefore(now, 1),
    },
    {
      secondary: rateWindow(now, 58, WEEK_MINUTES, 3 * DAY_MINUTES + 190),
      secondaryLabel: "Weekly",
      planName: "Plus",
      accountEmail: email,
      sourceLabel: "oauth",
      pace: {
        stage: "slightly_behind",
        deltaPercent: 4,
        willLastToReset: true,
        etaSeconds: null,
        expectedUsedPercent: 54,
        actualUsedPercent: 58,
      },
    },
  );
}

function claudeSnapshot(now: Date): ProviderUsageSnapshot {
  return providerSnapshot(
    {
      providerId: "claude",
      displayName: "Claude",
      primary: rateWindow(now, 22, SESSION_MINUTES, 61),
      primaryLabel: "Session (5h)",
      updatedAt: isoBefore(now, 2),
    },
    {
      secondary: rateWindow(now, 41, WEEK_MINUTES, 5 * DAY_MINUTES + 30),
      secondaryLabel: "Weekly",
      planName: "Max",
      accountEmail: "claude.user@example.com",
      accountOrganization: "Example Studio",
      sourceLabel: "oauth",
    },
  );
}

function cursorSnapshot(now: Date, updatedMinutesAgo = 3): ProviderUsageSnapshot {
  return providerSnapshot(
    {
      providerId: "cursor",
      displayName: "Cursor",
      primary: rateWindow(now, 62, MONTH_MINUTES, 12 * DAY_MINUTES),
      primaryLabel: "Plan",
      updatedAt: isoBefore(now, updatedMinutesAgo),
    },
    {
      planName: "Pro",
      accountEmail: "cursor.user@example.com",
      sourceLabel: "web",
    },
  );
}

function geminiSnapshot(now: Date): ProviderUsageSnapshot {
  return providerSnapshot(
    {
      providerId: "gemini",
      displayName: "Gemini",
      primary: rateWindow(now, 15, DAY_MINUTES, 9 * 60),
      primaryLabel: "Daily",
      updatedAt: isoBefore(now, 4),
    },
    {
      planName: "Free",
      accountEmail: "gemini.user@example.com",
      sourceLabel: "cli",
    },
  );
}

function copilotSnapshot(now: Date): ProviderUsageSnapshot {
  return providerSnapshot(
    {
      providerId: "copilot",
      displayName: "Copilot",
      primary: rateWindow(now, 48, MONTH_MINUTES, 18 * DAY_MINUTES),
      primaryLabel: "Premium",
      updatedAt: isoBefore(now, 5),
    },
    {
      planName: "Business",
      accountEmail: "copilot.user@example.com",
      sourceLabel: "api",
    },
  );
}

function codexAccount(
  now: Date,
  id: string,
  email: string,
  source: CodexAccount["source"],
  nickname: string | null,
): CodexAccount {
  return {
    id,
    nickname,
    emailHint: email,
    authSubject: null,
    providerAccountId: null,
    codexHomePath: source === "ambient" ? "C:\\Users\\proof\\.codex" : `C:\\proof\\codex-homes\\${id}`,
    source,
    createdAt: isoBefore(now, 30 * DAY_MINUTES),
    updatedAt: isoBefore(now, 60),
    lastAuthenticatedAt: isoBefore(now, 2 * DAY_MINUTES),
  };
}

function codexAccountUsage(
  now: Date,
  email: string,
  plan: string,
  sessionUsed: number,
  weeklyUsed: number,
): CodexAccountUsageSnapshot {
  return {
    email,
    providerAccountId: null,
    plan,
    allowed: true,
    limitReached: false,
    primaryWindow: {
      usedPercent: sessionUsed,
      resetAt: isoAfter(now, 142),
      limitWindowSeconds: SESSION_MINUTES * 60,
    },
    secondaryWindow: {
      usedPercent: weeklyUsed,
      resetAt: isoAfter(now, 3 * DAY_MINUTES + 190),
      limitWindowSeconds: WEEK_MINUTES * 60,
    },
    credits: null,
    updatedAt: isoBefore(now, 1),
  };
}

function singleCodexAccount(now: Date): CodexAccountsStateBridge {
  const email = "dev@example.com";
  return {
    accounts: [codexAccount(now, "proof-codex-ambient", email, "ambient", null)],
    accountOrdinals: { "proof-codex-ambient": 1 },
    snapshots: { "proof-codex-ambient": codexAccountUsage(now, email, "Plus", 34, 58) },
  };
}

function claudeAccounts(now: Date): ClaudeAccount[] {
  return [
    {
      id: "proof-claude-1",
      email: "claude.user@example.com",
      organization: "Example Studio",
      plan: "Max",
      isActive: true,
      isSaved: true,
      usage: {
        fiveHour: { usedPercent: 22, resetsAt: isoAfter(now, 61) },
        sevenDay: { usedPercent: 41, resetsAt: isoAfter(now, 5 * DAY_MINUTES + 30) },
        updatedAt: isoBefore(now, 2),
      },
    },
  ];
}

function emptyCodexAccounts(): CodexAccountsStateBridge {
  return { accounts: [], accountOrdinals: {}, snapshots: {} };
}

const DEFAULT_ENABLED = ["codex", "claude", "cursor", "gemini", "copilot"];

function defaultScenario(now: Date): ProofScenario {
  return {
    name: "default",
    settings: baseSettings({ enabledProviders: DEFAULT_ENABLED }),
    catalog: PROOF_PROVIDER_CATALOG,
    providers: [
      codexSnapshot(now, "dev@example.com"),
      claudeSnapshot(now),
      cursorSnapshot(now),
      geminiSnapshot(now),
      copilotSnapshot(now),
    ],
    codexAccounts: singleCodexAccount(now),
    claudeAccounts: claudeAccounts(now),
    localSpend: [codexSpend(), claudeSpend()],
  };
}

function twoCodexAccountsScenario(now: Date): ProofScenario {
  const work = "work@example.com";
  const personal = "personal@example.com";
  return {
    name: "two-codex-accounts",
    settings: baseSettings({ enabledProviders: ["codex", "claude"] }),
    catalog: PROOF_PROVIDER_CATALOG,
    providers: [codexSnapshot(now, work), claudeSnapshot(now)],
    codexAccounts: {
      accounts: [
        codexAccount(now, "proof-codex-work", work, "ambient", "Work"),
        codexAccount(now, "proof-codex-personal", personal, "managedByApp", "Personal"),
      ],
      accountOrdinals: { "proof-codex-work": 1, "proof-codex-personal": 2 },
      snapshots: {
        "proof-codex-work": codexAccountUsage(now, work, "Plus", 34, 58),
        "proof-codex-personal": codexAccountUsage(now, personal, "Pro", 9, 17),
      },
    },
    claudeAccounts: claudeAccounts(now),
    localSpend: [codexSpend(), claudeSpend()],
  };
}

function errorsScenario(now: Date): ProofScenario {
  const claude = providerSnapshot(
    {
      providerId: "claude",
      displayName: "Claude",
      primary: rateWindow(now, 0, SESSION_MINUTES, null),
      primaryLabel: "Session (5h)",
      updatedAt: isoBefore(now, 1),
    },
    {
      error: "Your Claude session has expired. Sign in again to resume usage updates.",
      errorState: "expiredSession",
      sourceLabel: "oauth",
    },
  );
  return {
    name: "errors",
    settings: baseSettings({ enabledProviders: ["codex", "claude", "cursor"] }),
    catalog: PROOF_PROVIDER_CATALOG,
    providers: [
      codexSnapshot(now, "dev@example.com"),
      claude,
      // Last good data from three hours ago: older than the 10-minute stale cutoff.
      cursorSnapshot(now, 180),
    ],
    codexAccounts: singleCodexAccount(now),
    claudeAccounts: [],
    localSpend: [codexSpend(), claudeSpend()],
  };
}

function emptyScenario(): ProofScenario {
  return {
    name: "empty",
    settings: baseSettings({ enabledProviders: [] }),
    catalog: PROOF_PROVIDER_CATALOG,
    providers: [],
    codexAccounts: emptyCodexAccounts(),
    claudeAccounts: [],
    localSpend: [],
  };
}

const MINUTE_SECONDS = 60;
const DAY_SECONDS = 86_400;

/**
 * OpenAI API card with both a Tokens/min window and daily spend, so the card
 * offers the Live · Spend switch. The last two minutes are still reporting.
 */
function openAiApiSnapshot(now: Date): ProviderUsageSnapshot {
  const nowSeconds = Math.floor(now.getTime() / 1000);
  const firstMinute = Math.floor(nowSeconds / MINUTE_SECONDS) * MINUTE_SECONDS - 59 * MINUTE_SECONDS;
  const minutes = Array.from({ length: 60 }, (_, i) => {
    const isPending = i >= 58;
    const totalTokens = isPending ? 0 : 6_000 + ((i * 7_919) % 18_000);
    return {
      start: firstMinute + i * MINUTE_SECONDS,
      totalTokens,
      requests: isPending ? 0 : 2 + (i % 5),
      isPending,
    };
  });
  const reported = minutes.filter((minute) => !minute.isPending);
  const peak = reported.reduce((best, minute) => (minute.totalTokens > best.totalTokens ? minute : best));
  const lastFive = reported.slice(-5);
  const firstDay = Math.floor(nowSeconds / DAY_SECONDS) * DAY_SECONDS - 29 * DAY_SECONDS;
  const daily = Array.from({ length: 30 }, (_, i) => {
    const costUsd = Math.round((1.5 + ((i * 37) % 11) * 0.45) * 100) / 100;
    const totalTokens = Math.round(costUsd * 190_000);
    return {
      startTime: firstDay + i * DAY_SECONDS,
      endTime: firstDay + (i + 1) * DAY_SECONDS,
      costUsd,
      requests: 120 + i,
      inputTokens: Math.round(totalTokens * 0.7),
      cachedInputTokens: Math.round(totalTokens * 0.1),
      outputTokens: Math.round(totalTokens * 0.3),
      totalTokens,
      lineItems: [{ name: "Text tokens", costUsd }],
      models: [],
    };
  });
  return providerSnapshot(
    {
      providerId: "openaiapi",
      displayName: "OpenAI API",
      primary: rateWindow(now, 0, null, null),
      primaryLabel: "Spend",
      updatedAt: isoBefore(now, 1),
    },
    {
      planName: "Admin API",
      sourceLabel: "api",
      throughput: {
        minutes,
        frontier: reported[reported.length - 1].start,
        latestTokensPerMinute: reported[reported.length - 1].totalTokens,
        peakTokensPerMinute: peak.totalTokens,
        peakAt: peak.start,
        fiveMinuteAverageTokensPerMinute: Math.round(
          lastFive.reduce((sum, minute) => sum + minute.totalTokens, 0) / lastFive.length,
        ),
        models: [],
        windowMinutes: 60,
        fetchedAt: nowSeconds,
        projectId: null,
      },
      openAiApiUsage: { historyDays: 30, projectId: null, daily },
    },
  );
}

function openAiLiveSpendScenario(now: Date): ProofScenario {
  return {
    name: "openai-live-spend",
    settings: baseSettings({ enabledProviders: ["openaiapi"] }),
    catalog: PROOF_PROVIDER_CATALOG,
    providers: [openAiApiSnapshot(now)],
    codexAccounts: emptyCodexAccounts(),
    claudeAccounts: [],
    localSpend: [],
  };
}

const BUILDERS: Record<ProofScenarioName, (now: Date) => ProofScenario> = {
  default: defaultScenario,
  "two-codex-accounts": twoCodexAccountsScenario,
  errors: errorsScenario,
  empty: emptyScenario,
  "openai-live-spend": openAiLiveSpendScenario,
};

export function isProofScenarioName(value: string): value is ProofScenarioName {
  return (PROOF_SCENARIO_NAMES as readonly string[]).includes(value);
}

/** Fixture timestamps are relative to `now` so reset countdowns stay plausible. */
export function buildProofScenario(name: ProofScenarioName, now: Date = new Date()): ProofScenario {
  return BUILDERS[name](now);
}
