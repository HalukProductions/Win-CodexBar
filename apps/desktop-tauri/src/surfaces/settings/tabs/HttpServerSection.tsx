import { useEffect, useState } from "react";
import { useLocale } from "../../../hooks/useLocale";
import { applyHttpServerConfig, getHttpServerState } from "../../../lib/tauri";
import {
  Field,
  NumberInput,
  Select,
  SettingsGroup,
  Toggle,
} from "../../../components/FormControls";
import type {
  HttpServerConfig,
  HttpServerSnapshot,
  HttpServerStatus,
  HttpServerValidationError,
} from "../../../types/bridge";
import type { LocaleKey } from "../../../i18n/keys";

const VALIDATION_KEYS: Record<HttpServerValidationError, LocaleKey> = {
  invalidHost: "HttpServerErrorHost",
  invalidPort: "HttpServerErrorPort",
  invalidTiming: "HttpServerErrorTiming",
  invalidIdentity: "HttpServerErrorIdentity",
  invalidToken: "HttpServerErrorToken",
  plainHttpNotAccepted: "HttpServerErrorPlainHttp",
};

export function bindHost(host: string): string {
  const trimmed = host.trim();
  return trimmed.toLowerCase() === "localhost" ? "127.0.0.1" : trimmed;
}

export function generateToken(): string {
  const bytes = new Uint8Array(32);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

function statusKey(status: HttpServerStatus): LocaleKey {
  switch (status.state) {
    case "stopped":
      return "HttpServerStatusStopped";
    case "running":
      return "HttpServerStatusRunning";
    case "invalid":
      return VALIDATION_KEYS[status.code];
    case "startFailed":
      return "HttpServerStatusStartFailed";
    case "exited":
      return "HttpServerStatusExited";
    case "loadFailed":
      return "HttpServerStatusLoadFailed";
    case "saveFailed":
      return "HttpServerStatusSaveFailed";
  }
}

function statusDetail(status: HttpServerStatus): string | undefined {
  return status.state === "startFailed" || status.state === "exited"
    ? status.detail || undefined
    : undefined;
}

export default function HttpServerSection() {
  const { t } = useLocale();
  const [snapshot, setSnapshot] = useState<HttpServerSnapshot | null>(null);
  const [draft, setDraft] = useState<HttpServerConfig | null>(null);
  const [showToken, setShowToken] = useState(false);
  const [applying, setApplying] = useState(false);
  const [applyFailed, setApplyFailed] = useState(false);
  const [loadFailed, setLoadFailed] = useState(false);
  const [overwriteConfirmed, setOverwriteConfirmed] = useState(false);

  useEffect(() => {
    let active = true;
    void getHttpServerState()
      .then((state) => {
        if (!active) return;
        setSnapshot(state);
        setDraft(state.config);
      })
      .catch(() => {
        if (active) setLoadFailed(true);
      });
    return () => {
      active = false;
    };
  }, []);

  if (loadFailed) {
    return (
      <SettingsGroup title={t("HttpServerTitle")} footer={<p>{t("HttpServerCaption")}</p>}>
        <p className="settings-group__error" role="alert">
          {t("HttpServerLoadError")}
        </p>
      </SettingsGroup>
    );
  }
  if (!snapshot || !draft) return null;

  const update = (patch: Partial<HttpServerConfig>) => setDraft({ ...draft, ...patch });
  const isLoopback = bindHost(draft.host).startsWith("127.");
  const runningUrl = snapshot.status.state === "running" ? snapshot.status.url : null;
  // A corrupt settings file is only replaced after explicit acknowledgment.
  const corruptFile = snapshot.status.state === "loadFailed";

  const apply = async () => {
    setApplying(true);
    setApplyFailed(false);
    try {
      // The draft is kept: an invalid or failed apply should not discard edits.
      setSnapshot(await applyHttpServerConfig(draft));
    } catch {
      setApplyFailed(true);
    } finally {
      setApplying(false);
    }
  };

  const copy = (value: string) => {
    void navigator.clipboard.writeText(value).catch(() => {});
  };

  return (
    <SettingsGroup title={t("HttpServerTitle")} footer={<p>{t("HttpServerCaption")}</p>}>
      <Field label={t("HttpServerEnableLabel")}>
        <Toggle
          variant="switch"
          checked={draft.enabled}
          ariaLabel={t("HttpServerEnableLabel")}
          onChange={(enabled) => update({ enabled })}
        />
      </Field>
      <Field label={t("HttpServerBindLabel")}>
        <input
          type="text"
          className="text-input"
          value={draft.host}
          aria-label={t("HttpServerBindLabel")}
          placeholder="127.0.0.1"
          spellCheck={false}
          onChange={(event) => update({ host: event.target.value })}
        />
      </Field>
      <Field label={t("HttpServerPortLabel")}>
        <span className="http-server-port">
          <NumberInput
            value={draft.port}
            min={1}
            max={65535}
            ariaLabel={t("HttpServerPortLabel")}
            onChange={(port) => update({ port })}
          />
        </span>
      </Field>
      <Field label={t("HttpServerTokenLabel")}>
        <input
          type={showToken ? "text" : "password"}
          className="text-input"
          value={draft.token}
          aria-label={t("HttpServerTokenLabel")}
          autoComplete="off"
          spellCheck={false}
          onChange={(event) => update({ token: event.target.value })}
        />
        <button
          type="button"
          className="credential-btn"
          onClick={() => setShowToken(!showToken)}
        >
          {showToken ? t("HttpServerHideToken") : t("HttpServerShowToken")}
        </button>
        <button
          type="button"
          className="credential-btn"
          onClick={() => update({ token: generateToken() })}
        >
          {t("HttpServerGenerateToken")}
        </button>
        <button
          type="button"
          className="credential-btn"
          disabled={draft.token === ""}
          onClick={() => copy(draft.token)}
        >
          {t("HttpServerCopy")}
        </button>
      </Field>
      <Field label={t("HttpServerCacheLabel")}>
        <NumberInput
          value={draft.refreshIntervalSecs}
          min={0}
          ariaLabel={t("HttpServerCacheLabel")}
          onChange={(refreshIntervalSecs) => update({ refreshIntervalSecs })}
        />
      </Field>
      <Field label={t("HttpServerTimeoutLabel")}>
        <NumberInput
          value={draft.requestTimeoutSecs}
          min={0}
          ariaLabel={t("HttpServerTimeoutLabel")}
          onChange={(requestTimeoutSecs) => update({ requestTimeoutSecs })}
        />
      </Field>
      <Field label={t("HttpServerIdentityLabel")}>
        <Select
          variant="menu"
          value={draft.identity}
          ariaLabel={t("HttpServerIdentityLabel")}
          options={[
            { value: "redacted", label: t("HttpServerIdentityRedacted") },
            { value: "full", label: t("HttpServerIdentityFull") },
          ]}
          onChange={(identity) =>
            update({ identity: identity === "full" ? "full" : "redacted" })
          }
        />
      </Field>
      {!isLoopback && (
        <Field label={t("HttpServerPlainHttpLabel")} description={t("HttpServerWarning")}>
          <Toggle
            variant="switch"
            checked={draft.allowPlainHttp}
            ariaLabel={t("HttpServerPlainHttpLabel")}
            onChange={(allowPlainHttp) => update({ allowPlainHttp })}
          />
        </Field>
      )}
      <Field label={t("HttpServerStatusLabel")} description={statusDetail(snapshot.status)}>
        <span role="status">{t(statusKey(snapshot.status))}</span>
      </Field>
      {runningUrl && (
        <Field
          label={t("HttpServerDashboardLabel")}
          description={
            bindHost(snapshot.config.host) === "0.0.0.0" ? t("HttpServerWildcardNote") : undefined
          }
        >
          <code>{runningUrl}</code>
          <button type="button" className="credential-btn" onClick={() => copy(runningUrl)}>
            {t("HttpServerCopy")}
          </button>
        </Field>
      )}
      {corruptFile && (
        <Field label={t("HttpServerCorruptConfirm")} description={t("HttpServerCorruptWarning")}>
          <Toggle
            variant="switch"
            checked={overwriteConfirmed}
            ariaLabel={t("HttpServerCorruptConfirm")}
            onChange={setOverwriteConfirmed}
          />
        </Field>
      )}
      {applyFailed && (
        <p className="settings-group__error" role="alert">
          {t("HttpServerApplyFailed")}
        </p>
      )}
      <div className="settings-group__row">
        <button
          type="button"
          className="credential-btn"
          disabled={applying || (corruptFile && !overwriteConfirmed)}
          onClick={() => void apply()}
        >
          {t("HttpServerApply")}
        </button>
      </div>
    </SettingsGroup>
  );
}
