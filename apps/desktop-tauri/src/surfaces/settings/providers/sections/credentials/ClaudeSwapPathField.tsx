import { useEffect, useState } from "react";
import type { LocaleKey } from "../../../../../i18n/keys";
import type { ClaudeSwapAccountSource, SettingsUpdate } from "../../../../../types/bridge";
import { Field } from "../../../../../components/FormControls";

interface Props {
  /** Saved `claudeSwapExecutablePath`. */
  value: string;
  /** Saved `claudeSwapSource`; picks the tool the label and help name. */
  source: ClaudeSwapAccountSource;
  disabled: boolean;
  t: (key: LocaleKey) => string;
  onSettingsChange: (patch: SettingsUpdate) => void;
}

const SOURCE_TEXT: Record<
  ClaudeSwapAccountSource,
  { label: LocaleKey; help: LocaleKey; placeholder: LocaleKey }
> = {
  "claude-swap": {
    label: "ClaudeSwapExecutablePath",
    help: "ClaudeSwapExecutablePathHelp",
    placeholder: "ClaudeSwapExecutablePathPlaceholder",
  },
  tcr: {
    label: "ClaudeTcrExecutablePath",
    help: "ClaudeTcrExecutablePathHelp",
    placeholder: "ClaudeTcrExecutablePathPlaceholder",
  },
};

/**
 * claude-swap executable path (macOS inline field of the claude-swap toggle),
 * shown directly under "Read accounts from claude-swap" only while it is on. Saves on blur or Enter. A failed save
 * leaves the saved value unchanged, so the same draft stays retryable. The copy
 * names the selected source's executable, which runs with that source's list arguments.
 */
export function ClaudeSwapPathField({ value, source, disabled, t, onSettingsChange }: Props) {
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);
  const text = SOURCE_TEXT[source] ?? SOURCE_TEXT["claude-swap"];

  const save = () => {
    const next = draft.trim();
    if (next === value.trim()) return;
    onSettingsChange({ claudeSwapExecutablePath: next });
  };

  return (
    <Field label={t(text.label)} description={t(text.help)}>
      <input
        type="text"
        className="text-input"
        value={draft}
        disabled={disabled}
        spellCheck={false}
        aria-label={t(text.label)}
        placeholder={t(text.placeholder)}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={save}
        onKeyDown={(e) => {
          if (e.key === "Enter") save();
        }}
      />
    </Field>
  );
}
