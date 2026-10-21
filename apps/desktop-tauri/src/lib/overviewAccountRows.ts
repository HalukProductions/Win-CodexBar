import { buildCodexAccountSurfaceLabels } from "../components/codexAccountDisplay";
import type {
  CodexAccount,
  CodexAccountUsageSnapshot,
  CodexUsageWindow,
  ProviderUsageSnapshot,
  RateWindowSnapshot,
} from "../types/bridge";

/**
 * Cached managed-Codex accounts, as the shared account store reports them.
 * Reading this never fetches; it is the same disk snapshot the Settings list uses.
 */
export interface CodexOverviewAccounts {
  accounts: readonly CodexAccount[];
  snapshots: Readonly<Record<string, CodexAccountUsageSnapshot>>;
  displayNames?: Readonly<Record<string, string>>;
  accountOrdinals: Readonly<Record<string, number>>;
}

export interface OverviewEntry {
  /** React key and DOM id suffix. A plain card keeps the provider id. */
  key: string;
  provider: ProviderUsageSnapshot;
  /** The account a row belongs to; null for an ordinary provider card. */
  accountId: string | null;
}

export interface OverviewRowOptions {
  hidePersonalInfo: boolean;
  /** Localized "Account", used for the generic privacy label. */
  accountWord: string;
  /** Localized "No usage data", shown for an account with no cached usage. */
  unavailableText: string;
}

const CODEX_PROVIDER_ID = "codex";

/**
 * Expand each Overview provider into one row per cached account. Only the
 * already-selected providers are passed in, so rows never take a provider slot.
 *
 * Codex is the only provider with per-account usage cached on Windows; the other
 * multi-account sources (claude-swap, saved token accounts) are not cached per
 * account and stay as one card. A provider with at most one cached account keeps
 * its single card. Each row is built from that account's own snapshot and never
 * borrows the active account's quota, plan or email.
 */
export function expandOverviewAccountRows(
  providers: readonly ProviderUsageSnapshot[],
  codex: CodexOverviewAccounts | null,
  options: OverviewRowOptions,
): OverviewEntry[] {
  const plain = (provider: ProviderUsageSnapshot): OverviewEntry => ({
    key: provider.providerId,
    provider,
    accountId: null,
  });
  if (!codex || codex.accounts.length <= 1) return providers.map(plain);

  const labels = buildCodexAccountSurfaceLabels(
    codex.accounts,
    codex.displayNames ?? {},
    codex.accountOrdinals,
    options.hidePersonalInfo,
    options.accountWord,
  );
  return providers.flatMap((provider): OverviewEntry[] => {
    if (provider.providerId !== CODEX_PROVIDER_ID) return [plain(provider)];
    return codex.accounts.map((account) => ({
      key: `${provider.providerId}/account/${account.id}`,
      provider: codexAccountOverviewSnapshot(
        provider,
        codex.snapshots[account.id] ?? null,
        labels[account.id] ?? "",
        options.unavailableText,
      ),
      accountId: account.id,
    }));
  });
}

/**
 * Build one account's card model. Only display identity (provider id, name,
 * source) comes from the provider card; plan, email, windows and cost come from
 * this account's own usage record.
 */
function codexAccountOverviewSnapshot(
  base: ProviderUsageSnapshot,
  usage: CodexAccountUsageSnapshot | null,
  label: string,
  unavailableText: string,
): ProviderUsageSnapshot {
  const windows = [usage?.primaryWindow, usage?.secondaryWindow]
    .map(codexRateWindow)
    .filter((window): window is RateWindowSnapshot => window !== null);
  const hasUsage = windows.length > 0;
  const primary = windows[0] ?? emptyRateWindow();
  return {
    providerId: base.providerId,
    displayName: base.displayName,
    primary,
    selectedMetric: primary,
    secondary: windows[1] ?? null,
    modelSpecific: null,
    tertiary: null,
    extraRateWindows: [],
    cost: usage?.cost ?? null,
    subscription: usage?.subscription ?? null,
    planName: usage?.plan ?? null,
    accountEmail: label.trim() || null,
    sourceLabel: base.sourceLabel,
    updatedAt: usage?.updatedAt ?? base.updatedAt,
    error: hasUsage ? null : unavailableText,
    errorState: hasUsage ? "ready" : "unknown",
    pace: null,
    accountOrganization: null,
    trayStatusLabel: null,
  };
}

function codexRateWindow(window: CodexUsageWindow | null | undefined): RateWindowSnapshot | null {
  if (!window) return null;
  const usedPercent = Math.min(100, Math.max(0, window.usedPercent));
  return {
    usedPercent,
    remainingPercent: 100 - usedPercent,
    windowMinutes: window.limitWindowSeconds > 0 ? Math.round(window.limitWindowSeconds / 60) : null,
    resetsAt: window.resetAt,
    resetDescription: null,
    isExhausted: usedPercent >= 100,
    reservePercent: null,
    reserveDescription: null,
  };
}

function emptyRateWindow(): RateWindowSnapshot {
  return {
    usedPercent: 0,
    remainingPercent: 100,
    windowMinutes: null,
    resetsAt: null,
    resetDescription: null,
    isExhausted: false,
    reservePercent: null,
    reserveDescription: null,
  };
}
