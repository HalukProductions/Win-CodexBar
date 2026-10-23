import type { LocaleKey } from "../../../../i18n/keys";
import { GeminiCliCreds } from "./credentials/GeminiCliCreds";
import { VertexAiCreds } from "./credentials/VertexAiCreds";
import { JetBrainsCreds } from "./credentials/JetBrainsCreds";
import { KiroCreds } from "./credentials/KiroCreds";
import { hasWorkspaceExtra, OpenAiExtras } from "./credentials/OpenAiExtras";
import { OpenRouterManagementCreds } from "./credentials/OpenRouterManagementCreds";
import { VercelAccessTokenCreds } from "./credentials/VercelAccessTokenCreds";

interface Props {
  providerId: string;
  t: (key: LocaleKey) => string;
  /** Called after a stored secret was saved or removed. */
  onCredentialsChanged?: () => void;
}

/**
 * Dispatch the appropriate Phase-6d credential component based on the
 * current provider. Providers without a bespoke credentials UI render
 * nothing. Claude and Codex switches live in the Options group instead.
 */
export function CredentialsDispatcher({ providerId, t, onCredentialsChanged }: Props) {
  switch (providerId) {
    case "gemini":
      return <GeminiCliCreds providerId={providerId} t={t} />;
    case "vertexai":
      return <VertexAiCreds providerId={providerId} t={t} />;
    case "jetbrains":
      return <JetBrainsCreds t={t} />;
    case "kiro":
      return <KiroCreds t={t} />;
    case "openrouter":
      return <OpenRouterManagementCreds t={t} onChanged={onCredentialsChanged} />;
    case "vercel":
      return (
        <>
          <VercelAccessTokenCreds t={t} onChanged={onCredentialsChanged} />
          <OpenAiExtras providerId={providerId} t={t} />
        </>
      );
    default:
      return hasWorkspaceExtra(providerId) ? (
        <OpenAiExtras providerId={providerId} t={t} />
      ) : null;
  }
}
