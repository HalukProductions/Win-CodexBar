import { useEffect, useState } from "react";
import type { LocaleKey } from "../../../../../i18n/keys";
import {
  hasVercelAccessToken,
  removeVercelAccessToken,
  setVercelAccessToken,
} from "../../../../../lib/tauri";

interface Props {
  t: (key: LocaleKey) => string;
  onChanged?: () => void;
}

/**
 * Optional Vercel access token for the personal AI Gateway budget. Stored
 * through the provider's secret slot; the value is never read back to the UI.
 */
export function VercelAccessTokenCreds({ t, onChanged }: Props) {
  const [configured, setConfigured] = useState(false);
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void hasVercelAccessToken()
      .then((present) => {
        if (!cancelled) setConfigured(present);
      })
      .catch((cause: unknown) => {
        if (!cancelled) setError(cause instanceof Error ? cause.message : String(cause));
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const save = async () => {
    const next = value.trim();
    if (!next) return;
    setBusy(true);
    setError(null);
    try {
      await setVercelAccessToken(next);
      setConfigured(true);
      setValue("");
      onChanged?.();
    } catch (cause: unknown) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    setBusy(true);
    setError(null);
    try {
      await removeVercelAccessToken();
      setConfigured(false);
      setValue("");
      onChanged?.();
    } catch (cause: unknown) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="provider-detail-section">
      <h4>{t("VercelAccessTokenTitle")}</h4>
      <label className="provider-detail-field">
        <span className="provider-detail-field__label">
          {t("VercelAccessTokenLabel")}
        </span>
        <input
          className="provider-detail-field__input"
          type="password"
          value={value}
          autoComplete="off"
          spellCheck={false}
          placeholder={configured ? t("VercelAccessTokenConfigured") : ""}
          aria-label={t("VercelAccessTokenLabel")}
          onChange={(event) => setValue(event.target.value)}
        />
      </label>
      <div className="provider-detail-helper">{t("VercelAccessTokenHelp")}</div>
      <div className="provider-detail-actions">
        <button
          type="button"
          className="credential-btn credential-btn--primary"
          disabled={busy || value.trim().length === 0}
          onClick={() => void save()}
        >
          {t("Save")}
        </button>
        {configured && (
          <button
            type="button"
            className="credential-btn credential-btn--secondary"
            disabled={busy}
            onClick={() => void remove()}
          >
            {t("Remove")}
          </button>
        )}
      </div>
      {error && <div className="provider-detail-error">{error}</div>}
    </section>
  );
}
