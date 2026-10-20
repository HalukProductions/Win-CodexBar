import type { CostSnapshotBridge, SubscriptionMetadataSnapshot } from "./usage";

// ── Codex multi-account (ADR 0003) ───────────────────────────────────

export type CodexAccountSource = "ambient" | "managedByApp";

export interface CodexAccount {
  id: string;
  nickname: string | null;
  emailHint: string | null;
  authSubject: string | null;
  providerAccountId: string | null;
  codexHomePath: string;
  source: CodexAccountSource;
  createdAt: string;
  updatedAt: string;
  lastAuthenticatedAt: string | null;
}

export interface CodexUsageWindow {
  usedPercent: number;
  resetAt: string | null;
  limitWindowSeconds: number;
}

export interface CodexCreditsBalance {
  hasCredits: boolean;
  unlimited: boolean;
  balance: number | null;
}

export interface CodexAccountUsageSnapshot {
  email: string | null;
  providerAccountId: string | null;
  plan: string | null;
  allowed: boolean | null;
  limitReached: boolean | null;
  primaryWindow: CodexUsageWindow | null;
  secondaryWindow: CodexUsageWindow | null;
  credits: CodexCreditsBalance | null;
  /** Persisted account-scoped extra-usage cost, when available. */
  cost?: CostSnapshotBridge | null;
  subscription?: SubscriptionMetadataSnapshot | null;
  updatedAt: string;
}

export interface CodexSwitchResult {
  switchId: string;
  materializedAccount: CodexAccount | null;
  backupPath: string | null;
  ambientAccount: CodexAccount | null;
  desktopSessionBackupPath: string | null;
  desktopSessionRestorePath: string | null;
  desktopSessionRestoreExists: boolean;
}

export interface CodexAccountsStateBridge {
  accounts: CodexAccount[];
  /** Canonical privacy-safe account labels, keyed by stable account id. */
  displayNames?: Record<string, string>;
  /** Canonical opaque account ordinals, keyed by stable account id. */
  accountOrdinals: Record<string, number>;
  snapshots: Record<string, CodexAccountUsageSnapshot>;
  needsAuthentication?: Record<string, boolean>;
}
export interface ClaudeAccount {
  id: string;
  email: string;
  organization: string | null;
  plan: string | null;
  isActive: boolean;
  isSaved: boolean;
  usage?: {
    fiveHour: { usedPercent: number; resetsAt: string | null } | null;
    sevenDay: { usedPercent: number; resetsAt: string | null } | null;
    updatedAt: string;
  } | null;
  usageError?: string | null;
  needsAuthentication?: boolean;
}

export interface ClaudeReconciliationSnapshot {
  generation: number;
  status: "pending" | "succeeded" | "failed";
  providerRefreshGeneration: number | null;
  detail: string;
}

export interface GrokAccount {
  id: string;
  email: string;
  organization: string | null;
  plan: string | null;
  isActive: boolean;
  isSaved: boolean;
}

export interface GrokAccountUsage {
  status?: "loading" | "ready" | "signInRequired" | "unavailable" | "failed";
  usageAvailable: boolean;
  usedPercent: number | null;
  plan: string | null;
  windowMinutes: number | null;
  resetsAt: string | null;
}

/** One source-issued usage window from the external claude-swap adapter. */
export interface ClaudeSwapUsageWindow {
  usedPercent: number;
  /** RFC 3339 timestamp, or null when cswap reported no reset. */
  resetsAt: string | null;
}

export interface ClaudeSwapScopedWindow extends ClaudeSwapUsageWindow {
  /** Display-only provider/model label (e.g. "Fable only"). */
  name: string;
}

export interface ClaudeSwapSpendWindow {
  used: number;
  limit: number;
  usedPercent: number;
  currencyCode: string | null;
  resetsAt: string | null;
}

/** Source-reported historical usage. It never drives current provider state. */
export interface ClaudeSwapHistoricalUsage {
  fiveHour: ClaudeSwapUsageWindow | null;
  sevenDay: ClaudeSwapUsageWindow | null;
  scoped: ClaudeSwapScopedWindow[];
  spend: ClaudeSwapSpendWindow | null;
  fetchedAt: string;
  provenance: "source_reported_last_good";
}

export type ClaudeSwapAccountAction = "switch" | "reauthenticate";

/**
 * The tool CodexBar runs to list Claude accounts. Both print claude-swap's
 * list schema v1; tcr pools accounts and never switches them.
 */
export type ClaudeSwapAccountSource = "claude-swap" | "tcr";

/**
 * One external claude-swap account. Identity is the source-issued numeric slot
 * (`<source>:<slot>`); CodexBar never reads or stores its credentials.
 */
export interface ClaudeSwapAccount {
  id: string;
  source: ClaudeSwapAccountSource;
  slot: number;
  /** Privacy-aware display label (alias, email, or `Account N`). */
  label: string;
  email: string | null;
  organization: string | null;
  alias: string | null;
  isActive: boolean;
  action: ClaudeSwapAccountAction | null;
  isDisabled: boolean;
  /** Raw cswap usageStatus label (e.g. "ok", "token_expired"). */
  status: string;
  error: string | null;
  fiveHour: ClaudeSwapUsageWindow | null;
  sevenDay: ClaudeSwapUsageWindow | null;
  scoped: ClaudeSwapScopedWindow[];
  spend: ClaudeSwapSpendWindow | null;
  historicalUsage: ClaudeSwapHistoricalUsage | null;
}

/** External claude-swap adapter state for the Claude accounts settings section. */
export interface ClaudeSwapAccountsState {
  enabled: boolean;
  /** Absent on older shells, which only read claude-swap. */
  source?: ClaudeSwapAccountSource;
  executableConfigured: boolean;
  accounts: ClaudeSwapAccount[];
  error: string | null;
}

// ── Hooks (Settings > Hooks; Rust `commands/hooks.rs`) ────────────────

/** The seven hook events, in the macOS picker order (`HookEventType.allCases`). */
export const HOOK_EVENT_TYPES = [
  "quota_low",
  "quota_reached",
  "quota_reset",
  "usage_updated",
  "provider_unavailable",
  "provider_recovered",
  "refresh_failed",
] as const;

export type HookEventType = (typeof HOOK_EVENT_TYPES)[number];

/** One rule in `hooks.json` (`HookRulePayload`). */
export interface HookRulePayload {
  enabled: boolean;
  /** Event names stay strings so the backend can reject an unknown one. */
  event: string | null;
  /** Extra events from a hand-edited file; the pane carries them through. */
  events: string[];
  /** Provider id, or null for any provider. */
  provider: string | null;
  /** For `quota_low`: usage fraction 0..1 (edited as 1..100 %). */
  threshold: number | null;
  executable: string;
  arguments: string[];
  timeoutSecs: number;
}

/** What the Hooks pane shows (`HooksConfigSnapshot`). */
export interface HooksConfigSnapshot {
  /** On only when `settings.hooks_enabled` and `hooks.json` `enabled` are both on. */
  enabled: boolean;
  settingsEnabled: boolean;
  fileEnabled: boolean;
  rules: HookRulePayload[];
  path: string;
  /** Set when `hooks.json` exists but can't be read; saving is refused. */
  error: string | null;
  maxRules: number;
  maxArguments: number;
}

/** A save from the Hooks pane (`HooksConfigSave`). `enabled` is sent only by the Enable switch. */
export interface HooksConfigSave {
  enabled?: boolean | null;
  rules: HookRulePayload[];
}
