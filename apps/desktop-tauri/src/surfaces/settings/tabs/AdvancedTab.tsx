import { useCallback, useEffect, useState } from "react";
import { useLocale } from "../../../hooks/useLocale";
import { getSafeDiagnostics } from "../../../lib/tauri";
import { Field, SettingsGroup, Toggle } from "../../../components/FormControls";
import type { TabProps } from "../settingsTabs";

function formatCodexSessionsDirs(paths: string[]): string {
  return paths.join("; ");
}

function parseCodexSessionsDirs(value: string): string[] {
  return value
    .split(/[;\n]/)
    .map((path) => path.trim())
    .filter(Boolean);
}

export default function AdvancedTab({ settings, set, saving }: TabProps) {
  const { t } = useLocale();
  const [diagnosticsStatus, setDiagnosticsStatus] = useState<string | null>(
    null,
  );
  const [codexDirsDraft, setCodexDirsDraft] = useState(() =>
    formatCodexSessionsDirs(settings.codexCustomSessionsDirs),
  );
  const [proxyUrlDraft, setProxyUrlDraft] = useState(() =>
    settings.httpProxyUrl ?? "",
  );
  const [proxyUsernameDraft, setProxyUsernameDraft] = useState(() =>
    settings.httpProxyUsername ?? "",
  );
  const [proxyPasswordDraft, setProxyPasswordDraft] = useState(() =>
    settings.httpProxyPassword ?? "",
  );

  const copyDiagnostics = useCallback(async () => {
    try {
      const text = await getSafeDiagnostics();
      await navigator.clipboard.writeText(text);
      setDiagnosticsStatus(t("DiagnosticsCopied"));
    } catch (error) {
      setDiagnosticsStatus(`${t("DiagnosticsCopyFailed")} ${String(error)}`);
    }
  }, [t]);

  const commitCodexDirs = useCallback(() => {
    set({ codexCustomSessionsDirs: parseCodexSessionsDirs(codexDirsDraft) });
  }, [codexDirsDraft, set]);

  useEffect(() => {
    if (!saving) {
      setCodexDirsDraft(formatCodexSessionsDirs(settings.codexCustomSessionsDirs));
    }
  }, [saving, settings.codexCustomSessionsDirs]);

  useEffect(() => {
    if (!saving) setProxyUrlDraft(settings.httpProxyUrl ?? "");
  }, [saving, settings.httpProxyUrl]);

  useEffect(() => {
    if (!saving) setProxyUsernameDraft(settings.httpProxyUsername ?? "");
  }, [saving, settings.httpProxyUsername]);

  useEffect(() => {
    if (!saving) setProxyPasswordDraft(settings.httpProxyPassword ?? "");
  }, [saving, settings.httpProxyPassword]);

  const commitProxyUrl = useCallback(() => {
    const next = proxyUrlDraft.trim();
    if (next !== (settings.httpProxyUrl ?? "")) set({ httpProxyUrl: next });
  }, [proxyUrlDraft, set, settings.httpProxyUrl]);

  const commitProxyUsername = useCallback(() => {
    const next = proxyUsernameDraft.trim();
    if (next !== (settings.httpProxyUsername ?? "")) set({ httpProxyUsername: next });
  }, [proxyUsernameDraft, set, settings.httpProxyUsername]);

  const commitProxyPassword = useCallback(() => {
    if (proxyPasswordDraft !== (settings.httpProxyPassword ?? "")) {
      set({ httpProxyPassword: proxyPasswordDraft });
    }
  }, [proxyPasswordDraft, set, settings.httpProxyPassword]);

  return (
    <>
      <SettingsGroup
        title={t("PrivacyTitle")}
        footer={
          settings.disableKeychainAccess ? <p>{t("CredentialAccessFooter")}</p> : undefined
        }
      >
        <Field label={t("HidePersonalInfo")} description={t("HidePersonalInfoHelper")}>
          <Toggle
            variant="switch"
            checked={settings.hidePersonalInfo}
            ariaLabel={t("HidePersonalInfo")}
            disabled={saving}
            onChange={(v) => set({ hidePersonalInfo: v })}
          />
        </Field>
        <Field
          label={t("DisableAllKeychainLabel")}
          description={t("DisableAllKeychainHelper")}
        >
          <Toggle
            variant="switch"
            checked={settings.disableKeychainAccess}
            ariaLabel={t("DisableAllKeychainLabel")}
            disabled={saving}
            onChange={(v) => set({ disableKeychainAccess: v })}
          />
        </Field>
      </SettingsGroup>

      <SettingsGroup
        title={t("CodexLocalLogsTitle")}
        footer={<p>{t("CodexLocalLogsCaption")}</p>}
      >
        <Field label={t("CodexLogPathsLabel")} description={t("CodexLogPathsHelper")}>
          <input
            type="text"
            className="text-input"
            value={codexDirsDraft}
            aria-label={t("CodexLogPathsLabel")}
            placeholder={String.raw`\\wsl.localhost\<distro>\home\<user>\.codex`}
            disabled={saving}
            onChange={(event) => setCodexDirsDraft(event.target.value)}
            onBlur={commitCodexDirs}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.currentTarget.blur();
              }
            }}
          />
        </Field>
      </SettingsGroup>

      <SettingsGroup title={t("SectionLocalIntegrations")}>
        <Field label={t("PowerToysPipeLabel")} description={t("PowerToysPipeHelper")}>
          <Toggle
            variant="switch"
            checked={settings.powertoysStatusPipeEnabled}
            ariaLabel={t("PowerToysPipeLabel")}
            disabled={saving}
            onChange={(v) => set({ powertoysStatusPipeEnabled: v })}
          />
        </Field>
        <Field
          label={t("ShareUsageWithCliLabel")}
          description={t("ShareUsageWithCliHelper")}
        >
          <Toggle
            variant="switch"
            checked={settings.shareUsageWithCli}
            ariaLabel={t("ShareUsageWithCliLabel")}
            disabled={saving}
            onChange={(v) => set({ shareUsageWithCli: v })}
          />
        </Field>
      </SettingsGroup>

      <SettingsGroup
        title={t("NetworkProxyTitle")}
        footer={<p>{t("NetworkProxyCaption")}</p>}
      >
        <Field
          label={t("NetworkProxyEnableLabel")}
          description={t("NetworkProxyEnableHelper")}
        >
          <Toggle
            variant="switch"
            checked={settings.httpProxyEnabled ?? false}
            ariaLabel={t("NetworkProxyEnableLabel")}
            disabled={saving}
            onChange={(v) => set({ httpProxyEnabled: v })}
          />
        </Field>
        <Field label={t("NetworkProxyUrlLabel")} description={t("NetworkProxyUrlHelper")}>
          <input
            type="text"
            className="text-input"
            value={proxyUrlDraft}
            placeholder="http://127.0.0.1:7890"
            aria-label={t("NetworkProxyUrlLabel")}
            disabled={saving || !settings.httpProxyEnabled}
            onChange={(event) => setProxyUrlDraft(event.target.value)}
            onBlur={commitProxyUrl}
            onKeyDown={(event) => {
              if (event.key === "Enter") event.currentTarget.blur();
            }}
          />
        </Field>
        <Field label={t("NetworkProxyUserLabel")}>
          <input
            type="text"
            className="text-input"
            value={proxyUsernameDraft}
            aria-label={t("NetworkProxyUserLabel")}
            autoComplete="off"
            disabled={saving || !settings.httpProxyEnabled}
            onChange={(event) => setProxyUsernameDraft(event.target.value)}
            onBlur={commitProxyUsername}
            onKeyDown={(event) => {
              if (event.key === "Enter") event.currentTarget.blur();
            }}
          />
        </Field>
        <Field
          label={t("NetworkProxyPasswordLabel")}
          description={t("NetworkProxyPasswordHelper")}
        >
          <input
            type="password"
            className="text-input"
            value={proxyPasswordDraft}
            aria-label={t("NetworkProxyPasswordLabel")}
            autoComplete="new-password"
            disabled={saving || !settings.httpProxyEnabled}
            onChange={(event) => setProxyPasswordDraft(event.target.value)}
            onBlur={commitProxyPassword}
            onKeyDown={(event) => {
              if (event.key === "Enter") event.currentTarget.blur();
            }}
          />
        </Field>
      </SettingsGroup>

      <SettingsGroup
        title={t("DiagnosticsSectionHeading")}
        footer={diagnosticsStatus ? <p role="status">{diagnosticsStatus}</p> : undefined}
      >
        <div className="settings-group__row">
          <button
            type="button"
            className="credential-btn"
            onClick={() => void copyDiagnostics()}
          >
            {t("DiagnosticsCopyButton")}
          </button>
        </div>
      </SettingsGroup>
    </>
  );
}
