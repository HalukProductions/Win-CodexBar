import type { LocaleKey } from "../i18n/keys";
import type { CodexWorkspacesSessionUsage, UsageSpendSummary } from "../types/bridge";
import { costPeriodShortLabel } from "./costPeriod";
import { formatCompactTokens } from "./usageSpendDashboard";

type Translate = (key: LocaleKey) => string;

/** Most conversations the card lists; the rest stay in Settings > Usage & Spend. */
export const CARD_CONVERSATION_MAX_ROWS = 50;

/**
 * One row of the card's Conversations list (upstream `CostHistoryChartMenuView.sessionRow`).
 * `title` is the backend `displayTitle`: the saved thread name, `Session <id>` when
 * there is none, and always `Session <id>` under Hide personal information.
 */
export interface CardConversationRow {
  id: string;
  title: string;
  /** Short session id label, shown as the row tooltip. */
  sessionLabel: string;
  usageLine: string;
  lastActivity: string | null;
  cost: string;
}

export interface CardConversations {
  title: string;
  /** Every conversation in the window, not only the listed rows. */
  count: number;
  rows: CardConversationRow[];
}

function fill(template: string, value: string): string {
  return template.replace("{}", value);
}

/** Mirrors `codex_workspaces::short_session_id`: ids over 12 characters keep the first 4 and last 8. */
export function shortSessionId(value: string): string {
  const chars = Array.from(value.trim());
  if (chars.length <= 12) return chars.join("");
  return `${chars.slice(0, 4).join("")}...${chars.slice(-8).join("")}`;
}

function activityMs(session: CodexWorkspacesSessionUsage): number {
  const raw = session.latestActivity ?? session.startedAt;
  const ms = raw ? Date.parse(raw) : Number.NaN;
  return Number.isNaN(ms) ? Number.NEGATIVE_INFINITY : ms;
}

/** Upstream lists conversations by last activity, newest first, then by session id descending. */
function byRecentActivity(a: CodexWorkspacesSessionUsage, b: CodexWorkspacesSessionUsage): number {
  const delta = activityMs(b) - activityMs(a);
  if (delta !== 0 && !Number.isNaN(delta)) return delta > 0 ? 1 : -1;
  return a.id < b.id ? 1 : a.id > b.id ? -1 : 0;
}

/** Same notation as Usage & Spend: `~` marks a partial estimate, `—` a conversation with no priced tokens. */
function costLabel(session: CodexWorkspacesSessionUsage): string {
  const { knownUsd, unknownTokens } = session.costEstimate;
  if (unknownTokens > 0 && knownUsd <= 0) return "—";
  return `${unknownTokens > 0 ? "~" : ""}$${knownUsd.toFixed(2)}`;
}

function usageLine(session: CodexWorkspacesSessionUsage, t: Translate): string {
  const model = session.topModel?.trim() || t("UsageSpendUnknownModel");
  const { inputTokens, cachedInputTokens, outputTokens } = session.totals;
  return [
    model,
    fill(t("CardConversationsInput"), formatCompactTokens(inputTokens)),
    fill(t("CardConversationsCached"), formatCompactTokens(cachedInputTokens)),
    fill(t("CardConversationsOutput"), formatCompactTokens(outputTokens)),
  ].join(" · ");
}

function activityLabel(session: CodexWorkspacesSessionUsage, locale: string | undefined): string | null {
  const ms = activityMs(session);
  if (!Number.isFinite(ms)) return null;
  return new Intl.DateTimeFormat(locale, {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  }).format(new Date(ms));
}

function intlLocale(t: Translate): string | undefined {
  const locale = t("IntlLocale");
  return /^[a-z]{2}-[A-Z]{2}$/.test(locale) ? locale : undefined;
}

/**
 * The card's Conversations section for `providerId`, built from the same
 * Usage & Spend contract the Settings sessions view reads. Null when the
 * contract belongs to another provider, cost tracking is off, or the window
 * has no conversations.
 */
export function cardConversationsModel(
  providerId: string,
  summary: UsageSpendSummary | null,
  t: Translate,
): CardConversations | null {
  if (!summary || summary.costTrackingEnabled === false) return null;
  const contract = summary.contract;
  if (!contract || contract.providerId !== providerId) return null;
  const conversations = contract.conversations ?? [];
  if (conversations.length === 0) return null;
  const locale = intlLocale(t);
  const rows = [...conversations]
    .sort(byRecentActivity)
    .slice(0, CARD_CONVERSATION_MAX_ROWS)
    .map((session) => {
      const sessionLabel = fill(t("CardConversationsSessionId"), shortSessionId(session.id));
      return {
        id: session.id,
        title: session.displayTitle.trim() || sessionLabel,
        sessionLabel,
        usageLine: usageLine(session, t),
        lastActivity: activityLabel(session, locale),
        cost: costLabel(session),
      };
    });
  const period = contract.reportingPeriod || summary.reportingPeriod;
  return {
    title: fill(t("CardConversationsTitle"), costPeriodShortLabel(period, t)),
    count: conversations.length,
    rows,
  };
}
