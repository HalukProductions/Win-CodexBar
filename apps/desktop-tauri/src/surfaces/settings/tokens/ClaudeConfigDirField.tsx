import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import type { LocaleKey } from "../../../i18n/keys";
import type { TokenAccountBridge } from "../../../types/bridge";

interface Props {
  account: TokenAccountBridge;
  disabled: boolean;
  t: (key: LocaleKey) => string;
  /** Saves the mapping; `null` clears it. */
  onChange: (directory: string | null) => Promise<void>;
}

/**
 * Optional Claude config directory for one Claude token account. While the
 * account is active, local cost estimates scan only that folder's `projects`
 * history. A cancelled or failed pick leaves the saved mapping unchanged.
 */
export function ClaudeConfigDirField({ account, disabled, t, onChange }: Props) {
  const [error, setError] = useState<string | null>(null);
  const directory = account.claudeConfigDirectory;

  const choose = async () => {
    setError(null);
    try {
      const selected = await open({ directory: true, multiple: false });
      if (typeof selected !== "string" || !selected.trim()) return;
      if (selected === directory) return;
      await onChange(selected);
    } catch (err) {
      setError(`${t("TokenAccountClaudeConfigDirPickFailed")} ${String(err)}`);
    }
  };

  return (
    <div className="token-account-claude-dir">
      <div className="token-account-claude-dir__row">
        <span className="token-account-claude-dir__label">
          {t("TokenAccountClaudeConfigDirLabel")}
        </span>
        <div className="token-account-claude-dir__actions">
          <button
            type="button"
            className="credential-btn credential-btn--secondary"
            disabled={disabled}
            onClick={() => void choose()}
          >
            {t("TokenAccountClaudeConfigDirChoose")}
          </button>
          {directory && (
            <button
              type="button"
              className="credential-btn credential-btn--secondary"
              disabled={disabled}
              aria-label={`${t("TokenAccountClaudeConfigDirClear")} ${directory}`}
              onClick={() => void onChange(null)}
            >
              {t("TokenAccountClaudeConfigDirClear")}
            </button>
          )}
        </div>
      </div>
      {directory && (
        <div className="token-account-claude-dir__row">
          <span className="token-account-claude-dir__path" title={directory}>
            {directory}
          </span>
          {!account.claudeConfigDirectoryAvailable && (
            <span className="credential-card__badge token-account-claude-dir__unavailable">
              {t("TokenAccountClaudeConfigDirUnavailable")}
            </span>
          )}
        </div>
      )}
      <p className="token-account-claude-dir__hint">{t("TokenAccountClaudeConfigDirHelp")}</p>
      {error && <p role="status">{error}</p>}
    </div>
  );
}
