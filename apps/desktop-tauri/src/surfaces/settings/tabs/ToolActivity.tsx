import { useRef, useState } from "react";
import type { LocaleKey } from "../../../i18n/keys";
import { getCodexSessionToolActivity } from "../../../lib/tauri";
import type {
  CodexSessionToolActivity,
  CodexToolOperation,
  CodexToolOperationKind,
  CodexToolOperationOutcome,
} from "../../../types/bridge";

/** Operations shown when the list first opens, and how many each "show more" adds. */
export const TOOL_INITIAL_ROWS = 8;
export const TOOL_ROWS_STEP = 20;

export type ToolActivityFilter = "all" | "attention" | "slow";

type LoadState =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "loaded"; activity: CodexSessionToolActivity }
  | { status: "error" };

const KIND_KEYS: Record<CodexToolOperationKind, LocaleKey> = {
  command: "UsageSpendToolsKindCommand",
  mcp: "UsageSpendToolsKindMcp",
  dynamic: "UsageSpendToolsKindDynamic",
  fileChange: "UsageSpendToolsKindFileChange",
  webSearch: "UsageSpendToolsKindWebSearch",
  image: "UsageSpendToolsKindImage",
  extensionItem: "UsageSpendToolsKindExtensionItem",
};

const OUTCOME_KEYS: Record<CodexToolOperationOutcome, LocaleKey> = {
  completed: "UsageSpendToolsOutcomeCompleted",
  nonzeroExit: "UsageSpendToolsOutcomeNonzeroExit",
  toolError: "UsageSpendToolsOutcomeToolError",
  declined: "UsageSpendToolsOutcomeDeclined",
  unknown: "UsageSpendToolsOutcomeUnknown",
};

const FILTERS: { id: ToolActivityFilter; key: LocaleKey }[] = [
  { id: "all", key: "UsageSpendToolsFilterAll" },
  { id: "attention", key: "UsageSpendToolsFilterAttention" },
  { id: "slow", key: "UsageSpendToolsFilterSlow" },
];

export function filterToolOperations(
  operations: CodexToolOperation[],
  filter: ToolActivityFilter,
): CodexToolOperation[] {
  switch (filter) {
    case "attention":
      return operations.filter((operation) => operation.needsAttention);
    case "slow":
      return operations.filter((operation) => operation.isSlow);
    default:
      return operations;
  }
}

export function formatToolDuration(milliseconds: number | null, t: (key: LocaleKey) => string): string {
  if (milliseconds === null) return "—";
  if (milliseconds < 1000) {
    return t("UsageSpendToolsMilliseconds").replace("{}", String(Math.round(milliseconds)));
  }
  return `${(milliseconds / 1000).toFixed(1)} ${t("UsageSpendSeconds")}`;
}

function formatCompletedAt(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
}

interface ToolActivityProps {
  sessionId: string;
  historyDays?: number;
  t: (key: LocaleKey) => string;
}

/** Native tool operations of one Codex conversation, read only when opened (upstream #4363). */
export default function ToolActivity({ sessionId, historyDays, t }: ToolActivityProps) {
  const [open, setOpen] = useState(false);
  const [state, setState] = useState<LoadState>({ status: "idle" });
  const [filter, setFilter] = useState<ToolActivityFilter>("all");
  const [visibleCount, setVisibleCount] = useState(TOOL_INITIAL_ROWS);
  const request = useRef(0);

  const load = () => {
    const id = ++request.current;
    setState({ status: "loading" });
    getCodexSessionToolActivity(sessionId, historyDays).then(
      (activity) => {
        if (request.current === id) setState({ status: "loaded", activity });
      },
      () => {
        if (request.current === id) setState({ status: "error" });
      },
    );
  };

  const toggle = () => {
    const next = !open;
    setOpen(next);
    if (next && (state.status === "idle" || state.status === "error")) load();
  };

  return (
    <div
      className="settings-section__caption"
      style={{ paddingLeft: "calc(2.5ch + 10px)", display: "grid", gap: 4 }}
    >
      <button
        type="button"
        className="credential-btn credential-btn--secondary"
        style={{ justifySelf: "start" }}
        aria-expanded={open}
        onClick={toggle}
      >
        {t("UsageSpendToolsTitle")}
      </button>
      {open && (
        <div style={{ display: "grid", gap: 4 }} data-testid="tool-activity">
          <span>{t("UsageSpendToolsScope")}</span>
          {state.status === "loading" && <span role="status">{t("UsageSpendToolsLoading")}</span>}
          {state.status === "error" && <span role="alert">{t("UsageSpendToolsUnavailable")}</span>}
          {state.status === "loaded" && (
            <LoadedActivity
              activity={state.activity}
              filter={filter}
              visibleCount={visibleCount}
              onFilter={(next) => {
                setFilter(next);
                setVisibleCount(TOOL_INITIAL_ROWS);
              }}
              onMore={() => setVisibleCount((count) => count + TOOL_ROWS_STEP)}
              t={t}
            />
          )}
          {state.status !== "loading" && (
            <button
              type="button"
              className="credential-btn credential-btn--secondary"
              style={{ justifySelf: "start" }}
              onClick={load}
            >
              {t("UsageSpendToolsRefresh")}
            </button>
          )}
          <span>{t("UsageSpendToolsHelp")}</span>
        </div>
      )}
    </div>
  );
}

interface LoadedActivityProps {
  activity: CodexSessionToolActivity;
  filter: ToolActivityFilter;
  visibleCount: number;
  onFilter: (filter: ToolActivityFilter) => void;
  onMore: () => void;
  t: (key: LocaleKey) => string;
}

function LoadedActivity({ activity, filter, visibleCount, onFilter, onMore, t }: LoadedActivityProps) {
  const { operations } = activity;
  const attention = operations.filter((operation) => operation.needsAttention).length;
  const timed = operations.filter((operation) => operation.durationMilliseconds !== null).length;
  const longest = operations.reduce<CodexToolOperation | null>((best, operation) => {
    if (operation.timing !== "native" || operation.durationMilliseconds === null) return best;
    return best === null || operation.durationMilliseconds > (best.durationMilliseconds ?? 0)
      ? operation
      : best;
  }, null);
  const filtered = filterToolOperations(operations, filter);
  const visible = filtered.slice(0, visibleCount);

  return (
    <>
      {activity.isPartial && <span role="note">{t("UsageSpendToolsPartial")}</span>}
      {activity.redacted && <span role="note">{t("UsageSpendToolsHidden")}</span>}
      {operations.length === 0 ? (
        <span>{t("UsageSpendToolsEmpty")}</span>
      ) : (
        <>
          <span>
            {t("UsageSpendToolsOperations")}: {operations.length}
            {" · "}
            {t("UsageSpendToolsAttention")}: {attention}
            {" · "}
            {t("UsageSpendToolsTimed")}: {timed} / {operations.length}
          </span>
          {longest && (
            <span>
              {t("UsageSpendToolsLongest")}: {formatToolDuration(longest.durationMilliseconds, t)}
              {" · "}
              {activity.redacted || longest.name === longest.kind ? t(KIND_KEYS[longest.kind]) : longest.name}
            </span>
          )}
          <div role="group" aria-label={t("UsageSpendToolsFilter")} style={{ display: "flex", gap: 4 }}>
            {FILTERS.map((option) => (
              <button
                key={option.id}
                type="button"
                className={
                  option.id === filter
                    ? "credential-btn credential-btn--primary"
                    : "credential-btn credential-btn--secondary"
                }
                aria-pressed={option.id === filter}
                onClick={() => onFilter(option.id)}
              >
                {t(option.key)}
              </button>
            ))}
          </div>
          {filtered.length === 0 ? (
            <span>{t("UsageSpendToolsNoMatches")}</span>
          ) : (
            <ul style={{ listStyle: "none", margin: 0, padding: 0, display: "grid", gap: 4 }}>
              {visible.map((operation) => (
                <ToolOperationRow
                  key={`${operation.turnId}|${operation.itemId}`}
                  operation={operation}
                  redacted={activity.redacted}
                  t={t}
                />
              ))}
            </ul>
          )}
          {filtered.length > visible.length && (
            <button
              type="button"
              className="credential-btn credential-btn--secondary"
              style={{ justifySelf: "start" }}
              onClick={onMore}
            >
              {t("UsageSpendToolsMore")} ({filtered.length - visible.length})
            </button>
          )}
        </>
      )}
    </>
  );
}

interface ToolOperationRowProps {
  operation: CodexToolOperation;
  redacted: boolean;
  t: (key: LocaleKey) => string;
}

function ToolOperationRow({ operation, redacted, t }: ToolOperationRowProps) {
  const kind = t(KIND_KEYS[operation.kind]);
  const outcome =
    operation.exitCode !== null && operation.outcome !== "completed"
      ? `${t(OUTCOME_KEYS[operation.outcome])} · ${t("UsageSpendToolsExit").replace("{}", String(operation.exitCode))}`
      : t(OUTCOME_KEYS[operation.outcome]);
  const duration =
    operation.timing === "recordedInterval"
      ? `${formatToolDuration(operation.durationMilliseconds, t)} (${t("UsageSpendToolsInterval")})`
      : formatToolDuration(operation.durationMilliseconds, t);

  return (
    <li
      data-testid="tool-operation"
      style={{
        display: "grid",
        gap: 2,
        paddingLeft: 6,
        borderLeft: `2px solid ${operation.needsAttention ? "var(--service-status-minor)" : "var(--border-subtle)"}`,
      }}
    >
      <span style={{ display: "flex", gap: 8, justifyContent: "space-between" }}>
        <span style={{ minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
          {redacted || operation.name === operation.kind ? kind : `${kind} · ${operation.name}`}
        </span>
        <span>{duration}</span>
      </span>
      {!redacted && operation.preview && (
        <code
          title={operation.preview}
          style={{ minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}
        >
          {operation.preview}
        </code>
      )}
      <span>
        {formatCompletedAt(operation.completedAt)}
        {" · "}
        {outcome}
      </span>
    </li>
  );
}
