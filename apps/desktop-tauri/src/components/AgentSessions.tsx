import { useCallback, useEffect, useState } from "react";
import { focusAgentSession, listAgentSessions } from "../lib/tauri";
import type {
  AgentSession,
  AgentSessionDiscoveryResult,
} from "../types/bridge";
import { useLocale } from "../hooks/useLocale";

export default function AgentSessions() {
  const { t } = useLocale();
  const [result, setResult] = useState<AgentSessionDiscoveryResult | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(() => {
    setError(null);
    void listAgentSessions()
      .then(setResult)
      .catch((reason: unknown) =>
        setError(reason instanceof Error ? reason.message : String(reason)),
      );
  }, []);

  useEffect(refresh, [refresh]);

  const sessionLabel = (session: AgentSession): string => {
    if (session.provider === "codex") return t("ProviderNameCodex");
    if (session.provider === "claude") return t("ProviderNameClaude");
    return session.dialect === "omp"
      ? t("AgentSessionsProviderOmp")
      : t("AgentSessionsProviderPi");
  };

  if (result?.status === "disabled") return null;
  const hosts = result?.status === "hosts" ? result.hosts : [];
  const sessions = hosts.flatMap((host) => host.sessions);

  const focus = (session: AgentSession) => {
    void focusAgentSession(session).then((focusResult) => {
      if (focusResult.status !== "focused") setError(focusResult.message);
    });
  };

  return (
    <section className="agent-sessions" aria-label={t("AgentSessionsTitle")}>
      <div className="agent-sessions__header">
        <strong>{t("AgentSessionsTitle")}</strong>
        <button type="button" onClick={refresh}>{t("ActionRefresh")}</button>
      </div>
      {!result && <p>{t("AgentSessionsLoading")}</p>}
      {result && sessions.length === 0 && (
        <p>{t("AgentSessionsEmpty")}</p>
      )}
      {hosts.map((host) => (
        <div className="agent-sessions__host" key={host.host}>
          {/* Upstream agentSessionsSection: one header per host, then its rows. */}
          <div
            className={
              host.error
                ? "agent-sessions__host-header agent-sessions__host-header--unreachable"
                : "agent-sessions__host-header"
            }
          >
            {host.error
              ? `${host.host} — ${t("AgentSessionsHostUnreachable")}`
              : `${host.host} — ${host.sessions.length}`}
          </div>
          {host.error && <p className="agent-sessions__error">{host.error}</p>}
          {host.sessions.map((session) => (
            <button
              type="button"
              className="agent-sessions__row"
              key={`${session.host}:${session.provider}:${session.id}`}
              onClick={() => focus(session)}
            >
              <span>{sessionLabel(session)}</span>
              <span
                className="agent-sessions__name"
                title={session.sessionName ?? session.workspace.projectName ?? session.host}
              >
                {session.sessionName ?? session.workspace.projectName ?? session.host}
              </span>
              <span>{session.state}</span>
            </button>
          ))}
        </div>
      ))}
      {error && <p className="agent-sessions__error">{error}</p>}
    </section>
  );
}
