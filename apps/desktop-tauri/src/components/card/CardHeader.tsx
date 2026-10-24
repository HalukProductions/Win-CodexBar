import { useCallback, useEffect, useRef, useState } from "react";
import type { LocaleKey } from "../../i18n/keys";
import { useLocale } from "../../hooks/useLocale";
import { formatRelativeUpdated } from "../../lib/relativeTime";
import { useNow } from "../../hooks/useNow";

type Translate = (key: LocaleKey) => string;

/** Upstream `CopyIconButton`: doc.on.doc, then a checkmark for 0.9 s. */
export function CopyIconButton({ text }: { text: string }) {
  const { t } = useLocale();
  const [copied, setCopied] = useState(false);
  const timer = useRef<number | null>(null);
  useEffect(
    () => () => {
      if (timer.current != null) window.clearTimeout(timer.current);
    },
    [],
  );
  const handleCopy = useCallback(() => {
    navigator.clipboard?.writeText(text).catch(() => {});
    setCopied(true);
    if (timer.current != null) window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setCopied(false), 900);
  }, [text]);
  const label = copied ? t("PanelCopied") : t("ActionCopyError");
  return (
    <button
      type="button"
      className="menu-card__copy-btn"
      onClick={handleCopy}
      aria-label={label}
      title={label}
    >
      {copied ? (
        <svg width="10" height="10" viewBox="0 0 16 16" fill="none" aria-hidden="true">
          <path d="M2.5 8.5l3.5 3.5 7.5-8" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      ) : (
        <svg width="10" height="10" viewBox="0 0 16 16" fill="none" aria-hidden="true">
          <rect x="5" y="5" width="9" height="9" rx="1.5" stroke="currentColor" strokeWidth="1.5" />
          <path d="M11 3V2.5A1.5 1.5 0 009.5 1H2.5A1.5 1.5 0 001 2.5v7A1.5 1.5 0 002.5 11H3" stroke="currentColor" strokeWidth="1.5" />
        </svg>
      )}
    </button>
  );
}

/** Login-method fallbacks that name the source, not a plan; Mac shows no plan for them. */
const SOURCE_ONLY_PLANS = new Set(["claude (oauth)", "claude (cli)"]);

/** The plan text shown on the right of the header's second line. */
export function displayPlanName(planName: string | null | undefined, t: Translate): string | null {
  const trimmed = planName?.trim();
  if (!trimmed) return null;
  const normalized = trimmed.toLowerCase();
  if (normalized === "default_claude_ai") return t("ProviderPlanClaudeAi");
  if (SOURCE_ONLY_PLANS.has(normalized)) return null;
  return trimmed;
}

/**
 * Upstream subtitle (`MenuCardView.swift:1077-1102`) without the error case:
 * "Refreshing…", else "Updated …", else "Not fetched yet".
 */
export function headerSubtitle(
  updatedAt: string | null | undefined,
  isRefreshing: boolean,
  t: Translate,
  nowMs: number = Date.now(),
): string {
  if (isRefreshing) return t("UsageSpendRefreshing");
  const raw = updatedAt?.trim();
  if (!raw) return t("ProviderNotFetchedYetTitle");
  const parsed = Date.parse(raw);
  return Number.isNaN(parsed) ? raw : formatRelativeUpdated(parsed, t, nowMs);
}

/**
 * Halves for CSS middle truncation (upstream `.truncationMode(.middle)`): the
 * head shrinks with an ellipsis, the tail keeps the end of the text visible.
 */
export function splitForMiddleTruncation(text: string): [string, string] {
  const cut = Math.ceil(text.length / 2);
  return [text.slice(0, cut), text.slice(cut)];
}

/**
 * Card header, following upstream `MenuCardView.swift:325-410`:
 * line 1 is the name (left) and the email (right, middle-truncated);
 * line 2 is the subtitle or the red error with a copy button, and the plan.
 */
export default function CardHeader({
  name,
  email,
  plan,
  error,
  updatedAt,
  isRefreshing,
}: {
  name: string;
  email: string | null;
  plan: string | null;
  error: string | null;
  updatedAt: string | null | undefined;
  isRefreshing: boolean;
}) {
  const { t } = useLocale();
  const nowMs = useNow();
  return (
    <header className="card-header">
      <div className="card-header__line">
        <span className="card-header__name">{name}</span>
        {email && (
          <span className="card-header__email" title={email}>
            <span className="card-header__email-head">
              {splitForMiddleTruncation(email)[0]}
            </span>
            <span className="card-header__email-tail">
              {splitForMiddleTruncation(email)[1]}
            </span>
          </span>
        )}
      </div>
      <div className={`card-header__line${error ? " card-header__line--error" : ""}`}>
        {error ? (
          <>
            <span className="card-header__error" title={error}>
              {error}
            </span>
            <CopyIconButton text={error} />
          </>
        ) : (
          <span className="card-header__subtitle">
            {headerSubtitle(updatedAt, isRefreshing, t, nowMs)}
          </span>
        )}
        {plan && <span className="card-header__plan">{plan}</span>}
      </div>
    </header>
  );
}
