import { useCallback, useEffect, useState } from "react";
import type {
  ProviderTokenAccountsBridge,
  TokenAccountBridge,
} from "../../../types/bridge";
import { useLocale } from "../../../hooks/useLocale";
import {
  addTokenAccount,
  getTokenAccounts,
  moveTokenAccount,
  removeTokenAccount,
  setActiveTokenAccount,
  setTokenAccountClaudeConfigDir,
  triggerProviderLogin,
  updateTokenAccount,
} from "../../../lib/tauri";
import { ClaudeConfigDirField } from "./ClaudeConfigDirField";

interface Props {
  providerId: string;
  /**
   * When `true`, renders a tight collapsible variant suitable for embedding
   * inside the Providers → detail pane (Phase 6e inline surface).
   * When `false` (default), renders the full standalone-tab layout.
   */
  compact?: boolean;
}

/**
 * Shared Token Accounts surface.
 *
 * Port of the token-account blocks in
 * `rust/src/native_ui/preferences.rs::render_provider_detail_panel`.
 * Wires the existing Phase-4 IPC (`get_token_accounts`,
 * `add_token_account`, `remove_token_account`, `set_active_token_account`).
 *
 * Used by:
 *   - `ProviderDetailPane.tsx` (inline in provider detail pane)
 */
export function TokenAccountsPanel({ providerId, compact = false }: Props) {
  const { t } = useLocale();
  const [data, setData] = useState<ProviderTokenAccountsBridge | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [addLabel, setAddLabel] = useState("");
  const [addToken, setAddToken] = useState("");
  // One inline editor at a time. The credential draft starts empty: a blank
  // value means "keep the saved credential", and the saved secret never
  // reaches the UI.
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editLabel, setEditLabel] = useState("");
  const [editToken, setEditToken] = useState("");

  const load = useCallback(async () => {
    if (!providerId) {
      setData(null);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const next = await getTokenAccounts(providerId);
      setData(next);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
      setData(null);
    } finally {
      setBusy(false);
    }
  }, [providerId]);

  useEffect(() => {
    setAddLabel("");
    setAddToken("");
    setEditingId(null);
    setEditLabel("");
    setEditToken("");
    setError(null);
    void load();
  }, [load]);

  const startEdit = (account: TokenAccountBridge) => {
    setEditingId(account.id);
    setEditLabel(account.label);
    setEditToken("");
    setError(null);
  };

  const cancelEdit = () => {
    setEditingId(null);
    setEditLabel("");
    setEditToken("");
  };

  const handleSaveEdit = async (accountId: string) => {
    if (!providerId || !editLabel.trim()) return;
    setBusy(true);
    setError(null);
    try {
      // A blank credential sends null, which keeps the saved secret.
      const next = await updateTokenAccount(
        providerId,
        accountId,
        editLabel.trim(),
        editToken.trim() || null,
      );
      setData(next);
      cancelEdit();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const handleMove = async (accountId: string, index: number) => {
    if (!providerId) return;
    setBusy(true);
    setError(null);
    try {
      const next = await moveTokenAccount(providerId, accountId, index);
      setData(next);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const handleAdd = async () => {
    if (!providerId || !addLabel.trim() || !addToken.trim()) return;
    setBusy(true);
    setError(null);
    try {
      const next = await addTokenAccount(
        providerId,
        addLabel.trim(),
        addToken.trim(),
      );
      setData(next);
      setAddLabel("");
      setAddToken("");
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const handleRemove = async (accountId: string) => {
    if (!providerId) return;
    setBusy(true);
    setError(null);
    try {
      const next = await removeTokenAccount(providerId, accountId);
      setData(next);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const handleSetActive = async (accountId: string) => {
    if (!providerId) return;
    setBusy(true);
    setError(null);
    try {
      const next = await setActiveTokenAccount(providerId, accountId);
      setData(next);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const handleClaudeConfigDir = async (accountId: string, directory: string | null) => {
    if (!providerId) return;
    setBusy(true);
    setError(null);
    try {
      const next = await setTokenAccountClaudeConfigDir(providerId, accountId, directory);
      setData(next);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const handleProviderLogin = async () => {
    if (!providerId) return;
    setBusy(true);
    setError(null);
    try {
      await triggerProviderLogin(providerId);
      await load();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  if (!providerId) return null;

  const placeholder = data?.support.placeholder ?? t("TokenAccountTokenPlaceholder");
  const subtitle = data?.support.subtitle ?? "";

  const body = (
    <>
      {subtitle && !compact && (
        <p className="settings-section__hint">{subtitle}</p>
      )}

      {error && (
        <div className="settings-status settings-status--error">{error}</div>
      )}

      {!compact && (
        <h3 className="settings-section__title">
          {t("SectionSavedAccounts")}
        </h3>
      )}

      {data && data.accounts.length > 0 ? (
        <ul className="credential-list token-accounts-list">
          {data.accounts.map((acct, index) => (
            <li key={acct.id} className="credential-card token-accounts-card">
              <div className="credential-card__header">
                <div className="credential-card__info">
                  <strong>{acct.label}</strong>
                  <span className="credential-card__meta">
                    {acct.isActive && (
                      <span className="credential-card__badge credential-card__badge--set">
                        {t("TokenAccountActive")}
                      </span>
                    )}
                    <span className="credential-card__date">
                      {t("TokenAccountAddedPrefix")} {acct.addedAt}
                    </span>
                    {acct.lastUsed && (
                      <span className="credential-card__date">
                        · {t("TokenAccountUsedPrefix")} {acct.lastUsed}
                      </span>
                    )}
                  </span>
                </div>
                <div className="credential-card__actions">
                  <button
                    type="button"
                    className="credential-btn credential-btn--secondary"
                    disabled={busy || editingId === acct.id}
                    onClick={() => startEdit(acct)}
                  >
                    {t("TokenAccountEdit")}
                  </button>
                  <button
                    type="button"
                    className="credential-btn credential-btn--secondary"
                    aria-label={t("TokenAccountMoveUp")}
                    title={t("TokenAccountMoveUp")}
                    disabled={busy || index === 0}
                    onClick={() => void handleMove(acct.id, index - 1)}
                  >
                    ↑
                  </button>
                  <button
                    type="button"
                    className="credential-btn credential-btn--secondary"
                    aria-label={t("TokenAccountMoveDown")}
                    title={t("TokenAccountMoveDown")}
                    disabled={busy || index === data.accounts.length - 1}
                    onClick={() => void handleMove(acct.id, index + 1)}
                  >
                    ↓
                  </button>
                  {!acct.isActive && (
                    <button
                      type="button"
                      className="credential-btn credential-btn--secondary"
                      disabled={busy}
                      onClick={() => void handleSetActive(acct.id)}
                    >
                      {t("TokenAccountSetActive")}
                    </button>
                  )}
                  <button
                    type="button"
                    className="credential-btn credential-btn--danger"
                    disabled={busy}
                    onClick={() => void handleRemove(acct.id)}
                  >
                    {t("TokenAccountRemove")}
                  </button>
                </div>
              </div>
              {editingId === acct.id && (
                <form
                  className="credential-add-form token-accounts-edit"
                  onSubmit={(e) => {
                    e.preventDefault();
                    void handleSaveEdit(acct.id);
                  }}
                >
                  <input
                    className="text-input"
                    type="text"
                    aria-label={t("TokenAccountLabelPlaceholder")}
                    placeholder={t("TokenAccountLabelPlaceholder")}
                    value={editLabel}
                    onChange={(e) => setEditLabel(e.target.value)}
                    disabled={busy}
                  />
                  <input
                    className="text-input"
                    type="password"
                    autoComplete="off"
                    aria-label={t("TokenAccountTokenPlaceholder")}
                    aria-describedby={`token-edit-hint-${acct.id}`}
                    placeholder={placeholder}
                    value={editToken}
                    onChange={(e) => setEditToken(e.target.value)}
                    disabled={busy}
                  />
                  <p
                    id={`token-edit-hint-${acct.id}`}
                    className="credential-card__meta"
                  >
                    {t("TokenAccountEditKeepCredentialHint")}
                  </p>
                  <div className="credential-card__actions">
                    <button
                      type="submit"
                      className="credential-btn credential-btn--primary"
                      disabled={busy || !editLabel.trim()}
                    >
                      {t("TokenAccountEditSave")}
                    </button>
                    <button
                      type="button"
                      className="credential-btn credential-btn--secondary"
                      disabled={busy}
                      onClick={cancelEdit}
                    >
                      {t("TokenAccountEditCancel")}
                    </button>
                  </div>
                </form>
              )}
              {providerId === "claude" && (
                <ClaudeConfigDirField
                  account={acct}
                  disabled={busy}
                  t={t}
                  onChange={(directory) => handleClaudeConfigDir(acct.id, directory)}
                />
              )}
            </li>
          ))}
        </ul>
      ) : (
        <p className="credential-empty">{t("TokenAccountEmpty")}</p>
      )}

      {!compact && (
        <h3 className="settings-section__title">{t("SectionAddAccount")}</h3>
      )}
      {providerId === "copilot" && (
        <button
          type="button"
          className="credential-btn credential-btn--primary"
          disabled={busy}
          onClick={() => void handleProviderLogin()}
        >
          {t("TokenAccountGithubLoginButton")}
        </button>
      )}
      <div className="credential-add-form token-accounts-add">
        <input
          className="text-input"
          type="text"
          placeholder={t("TokenAccountLabelPlaceholder")}
          value={addLabel}
          onChange={(e) => setAddLabel(e.target.value)}
          disabled={busy}
        />
        <textarea
          className="text-input credential-textarea"
          placeholder={placeholder}
          rows={compact ? 2 : 3}
          value={addToken}
          onChange={(e) => setAddToken(e.target.value)}
          disabled={busy}
        />
        <button
          type="button"
          className="credential-btn credential-btn--primary"
          disabled={busy || !addLabel.trim() || !addToken.trim()}
          onClick={() => void handleAdd()}
        >
          {t("TokenAccountAddButton")}
        </button>
      </div>
    </>
  );

  if (compact) {
    const activeCount = data?.accounts.filter((a) => a.isActive).length ?? 0;
    const total = data?.accounts.length ?? 0;
    return (
      <details className="provider-detail-section token-accounts-inline">
        <summary className="token-accounts-inline__summary">
          <span>{t("TokenAccountInlineSummary")}</span>
          <span className="token-accounts-inline__count">
            {activeCount > 0 ? `${activeCount}/${total}` : `${total}`}
          </span>
        </summary>
        <div className="token-accounts-inline__body">{body}</div>
      </details>
    );
  }

  return (
    <section className="settings-section token-accounts-standalone">
      <h3 className="settings-section__title">{t("SectionTokenAccounts")}</h3>
      {body}
    </section>
  );
}
