import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import type {
  ClaudeSwapAccount,
  ClaudeSwapAccountSource,
  ClaudeSwapAccountsState,
  ClaudeSwapHistoricalUsage,
  ClaudeSwapSpendWindow,
  Language,
} from "../../../../../types/bridge";
import type { LocaleKey } from "../../../../../i18n/keys";
import {
  claudeSwapAccountsList,
  claudeSwapAccountReauthenticate,
  claudeSwapAccountSwitch,
} from "../../../../../lib/tauri";
import {
  localClaudeReconciliationOutcome,
  useClaudeReconciliation,
} from "../../../../../hooks/useClaudeReconciliation";
import { useCurrency } from "../../../../../hooks/CurrencyProvider";
import { formatSessionEquivalentEstimate } from "../UsageDetailsSection";

interface Props {
  t: (key: LocaleKey) => string;
  language?: Language;
  /** Settings `claudeSwapEnabled`; the switch lives in Claude > Options. */
  enabled: boolean;
  /** Settings `claudeSwapExecutablePath`; the field sits under that switch. */
  executablePath: string;
  /** Settings `claudeSwapSource`; the Source picker sits under that switch. */
  source?: ClaudeSwapAccountSource;
  /** Settings `showAsUsed`; quota windows read as used instead of left. */
  showAsUsed?: boolean;
  /** Settings `hidePersonalInfo`. The backend masks the labels, so a change reloads the list. */
  hidePersonalInfo?: boolean;
}

interface SourceText {
  title: LocaleKey;
  noExecutable: LocaleKey;
  ready: LocaleKey;
  empty: LocaleKey;
  disabled: LocaleKey;
}

/** Copy that names the selected tool. */
const SOURCE_TEXT: Record<ClaudeSwapAccountSource, SourceText> = {
  "claude-swap": {
    title: "ClaudeSwapTitle",
    noExecutable: "ClaudeSwapStatusNoExecutable",
    ready: "ClaudeSwapStatusReady",
    empty: "ClaudeSwapEmpty",
    disabled: "ClaudeSwapDisabled",
  },
  tcr: {
    title: "ClaudeTcrTitle",
    noExecutable: "ClaudeTcrStatusNoExecutable",
    ready: "ClaudeTcrStatusReady",
    empty: "ClaudeTcrEmpty",
    disabled: "ClaudeTcrDisabled",
  },
};

const EMPTY_STATE: ClaudeSwapAccountsState = {
  enabled: false,
  executableConfigured: false,
  accounts: [],
  error: null,
};

function quotaPercent(
  t: (key: LocaleKey) => string,
  usedPercent: number,
  showAsUsed: boolean,
): string {
  const used = Math.max(0, Math.min(100, Math.round(usedPercent)));
  return showAsUsed
    ? `${used}% ${t("PanelUsedSuffix")}`
    : `${100 - used}% ${t("PanelLeftSuffix")}`;
}

function usageLabel(
  t: (key: LocaleKey) => string,
  key: LocaleKey,
  window: { usedPercent: number } | null,
  showAsUsed: boolean,
): string | null {
  if (!window) return null;
  return `${t(key)} ${quotaPercent(t, window.usedPercent, showAsUsed)}`;
}

function scopedLabel(
  t: (key: LocaleKey) => string,
  window: { name: string; usedPercent: number },
  showAsUsed: boolean,
): string {
  return `${window.name} ${quotaPercent(t, window.usedPercent, showAsUsed)}`;
}

function spendLabel(
  t: (key: LocaleKey) => string,
  spend: ClaudeSwapSpendWindow | null,
  locale: string,
  formatCurrency: (amount: number | null | undefined, sourceCode: string) => string,
): string | null {
  if (!spend) return null;
  const formatter = new Intl.NumberFormat(locale, {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  });
  const used = spend.currencyCode ? formatCurrency(spend.used, spend.currencyCode) : formatter.format(spend.used);
  const limit = spend.currencyCode ? formatCurrency(spend.limit, spend.currencyCode) : formatter.format(spend.limit);
  return `${t("ClaudeSwapSpend")} ${used} / ${limit} (${Math.round(spend.usedPercent)}%)`;
}

function historicalLabel(
  t: (key: LocaleKey) => string,
  history: ClaudeSwapHistoricalUsage | null,
  locale: string,
  formatCurrency: (amount: number | null | undefined, sourceCode: string) => string,
  showAsUsed: boolean,
): string | null {
  if (!history) return null;
  const windows = [
    usageLabel(t, "ProviderSession", history.fiveHour, showAsUsed),
    usageLabel(t, "ProviderWeekly", history.sevenDay, showAsUsed),
    ...history.scoped.map((window) => scopedLabel(t, window, showAsUsed)),
    spendLabel(t, history.spend, locale, formatCurrency),
  ].filter(Boolean);
  const captured = new Intl.DateTimeFormat(locale, {
    dateStyle: "short",
    timeStyle: "short",
  }).format(new Date(history.fetchedAt));
  return `${t("ClaudeSwapHistoricalUsage")}: ${windows.join(" · ")} (${t("ClaudeSwapHistoricalCapturedAt")} ${captured})`;
}

function languageLocale(language: Language): string {
  return {
    english: "en-US",
    chinese: "zh-CN",
    chinesetraditional: "zh-TW",
    japanese: "ja-JP",
    korean: "ko-KR",
    spanish: "es-MX",
    portuguesebrazil: "pt-BR",
    russian: "ru-RU",
    turkish: "tr-TR",
    ukrainian: "uk-UA",
  }[language];
}

/**
 * External Claude subscription accounts read from the claude-swap (`cswap`)
 * executable (issue #477, port of upstream claude-swap Phase 1-2).
 *
 * This list is deliberately separate from the built-in saved Claude Code
 * accounts above: it is display + explicit activation only, is hidden while the
 * integration is disabled, and CodexBar never reads or stores cswap credentials.
 * The opt-in switch and executable path are settings rows elsewhere in the
 * pane; this list reloads whenever either changes.
 */
export function ClaudeSwapAccountsSection({
  t,
  language = "english",
  enabled,
  executablePath,
  source = "claude-swap",
  showAsUsed = false,
  hidePersonalInfo = false,
}: Props) {
  const [state, setState] = useState<ClaudeSwapAccountsState>(EMPTY_STATE);
  const locale = languageLocale(language);
  const { format: formatCurrency } = useCurrency();
  const [busy, setBusy] = useState(false);
  const [operation, setOperation] = useState<{ generation: number; success: LocaleKey } | null>(null);
  const { snapshot, accept, reconciling } = useClaudeReconciliation();
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const mounted = useRef(false);
  // Bumped whenever the enabled flag, path or source changes. A list call
  // started under an older configuration must not publish its cards: a late
  // claude-swap reply would otherwise show cswap accounts under tcr.
  const generation = useRef(0);
  const shownSource = useRef(source);

  const reload = useCallback(async (owner: number) => {
    const next = await claudeSwapAccountsList();
    if (mounted.current && generation.current === owner) setState(next);
  }, []);

  useEffect(() => {
    mounted.current = true;
    const owner = ++generation.current;
    if (shownSource.current !== source) {
      shownSource.current = source;
      setState(EMPTY_STATE);
      setMessage(null);
    }
    if (!enabled) {
      setState(EMPTY_STATE);
      return () => {
        mounted.current = false;
      };
    }
    const load = () => {
      setError(null);
      void reload(owner).catch((e) => {
        if (mounted.current && generation.current === owner) setError(String(e));
      });
    };
    load();
    const unlisten = listen("claude-accounts-updated", load);
    return () => {
      mounted.current = false;
      void unlisten.then((fn) => fn()).catch(() => {});
    };
  }, [enabled, executablePath, source, hidePersonalInfo, reload]);

  useEffect(() => {
    const outcome = localClaudeReconciliationOutcome(snapshot, operation?.generation ?? null);
    if (!outcome) return;
    if (outcome.status === "failed") {
      setMessage(null);
      setError(outcome.detail);
    } else if (operation) {
      setMessage(t(operation.success));
      setError(null);
    }
  }, [operation, snapshot, t]);

  const runAccountAction = async (account: ClaudeSwapAccount) => {
    setBusy(true);
    setError(null);
    setMessage(null);
    try {
      const success = account.action === "reauthenticate"
        ? "ClaudeSwapReauthenticated"
        : "ClaudeSwapSwitched";
      let result;
      if (account.action === "reauthenticate") {
        result = await claudeSwapAccountReauthenticate(account.slot);
      } else if (account.action === "switch") {
        result = await claudeSwapAccountSwitch(account.slot);
      } else {
        throw new Error("This claude-swap account is not actionable.");
      }
      setOperation({ generation: result.generation, success });
      accept(result);
    } catch (e) {
      if (mounted.current) setError(String(e));
    } finally {
      if (mounted.current) setBusy(false);
    }
  };

  if (!enabled) return null;

  const text = SOURCE_TEXT[source] ?? SOURCE_TEXT["claude-swap"];
  const status = !state.executableConfigured ? t(text.noExecutable) : t(text.ready);

  return (
    <section className="provider-detail-section claude-swap-accounts">
      <h4>{t(text.title)}</h4>
      <p className="provider-detail-helper">{status}</p>
      {reconciling && <p className="provider-detail-helper" role="status">{t("ClaudeSwapReconciling")}</p>}
      {error && (
        <div className="provider-detail-error" role="alert">
          {error}
        </div>
      )}
      {state.error && (
        <div className="provider-detail-error" role="alert">
          {state.error}
        </div>
      )}
      {message && (
        <div className="provider-detail-note" role="status">
          {message}
        </div>
      )}
      {state.executableConfigured && state.accounts.length === 0 && !state.error && (
        <p>{t(text.empty)}</p>
      )}
      <ul className="credential-list">
        {state.accounts.map((account) => {
          const session = usageLabel(t, "ProviderSession", account.fiveHour, showAsUsed);
          const weekly = usageLabel(t, "ProviderWeekly", account.sevenDay, showAsUsed);
          const scoped = account.scoped
            .map((window) => scopedLabel(t, window, showAsUsed))
            .join(" \u00b7 ");
          const spend = spendLabel(t, account.spend, locale, formatCurrency);
          const historical = historicalLabel(
            t,
            account.historicalUsage,
            locale,
            formatCurrency,
            showAsUsed,
          );
          const forecast = formatSessionEquivalentEstimate(account.sessionEquivalentForecast);
          return (
            <li className="credential-card" key={account.id}>
              <div className="credential-card__header">
                <div className="credential-card__info">
                  <strong>{account.label}</strong>
                  <span className="credential-card__meta">
                    {[session, weekly, scoped || null, spend].filter(Boolean).join(" \u00b7 ") ||
                      (!account.error ? t("ClaudeSwapUsageUnavailable") : "")}
                  </span>
                  {forecast && <span className="credential-card__meta">{forecast}</span>}
                  {historical && <span className="credential-card__meta">{historical}</span>}
                  {account.isActive && (
                    <span className="credential-card__badge credential-card__badge--set">
                      {t("TokenAccountActive")}
                    </span>
                  )}
                  {account.isDisabled && (
                    <span className="credential-card__meta">{t(text.disabled)}</span>
                  )}
                  {account.error && (
                    <span className="credential-card__meta">{account.error}</span>
                  )}
                </div>
                <div className="credential-card__actions">
                  {account.action && (
                    <button
                      type="button"
                      className="credential-btn credential-btn--primary"
                      disabled={busy || reconciling}
                      onClick={() => void runAccountAction(account)}
                    >
                      {t(
                        account.action === "reauthenticate"
                          ? "ClaudeSwapReauthenticateButton"
                          : "ClaudeSwapSwitchButton",
                      )}
                    </button>
                  )}
                </div>
              </div>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
