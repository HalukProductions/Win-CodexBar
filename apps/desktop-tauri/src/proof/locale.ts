import englishFtl from "../../../../rust/src/locale/en-US.ftl?raw";
import { ALL_LOCALE_KEYS } from "../i18n/keys";
import type { LanguageOption, LocaleStrings } from "../types/bridge";

/** Parse the shipped English Fluent file the way the Rust bridge flattens it. */
export function parseFtlEntries(text: string): Record<string, string> {
  const entries: Record<string, string> = {};
  for (const line of text.split(/\r?\n/)) {
    const match = /^([A-Za-z][\w-]*)\s*=\s?(.*)$/.exec(line);
    if (match) entries[match[1]] = match[2].replace(/\{ "\{\}" \}/g, "{}");
  }
  return entries;
}

let cached: LocaleStrings | null = null;

/**
 * English bundle; keys missing from the file fall back to the key name, as the UI does.
 * `overrides` replaces single keys for one scenario, so a fixture can stand in a
 * longer translation to check layout (the proof has no other locales).
 */
export function englishLocaleStrings(overrides: Record<string, string> = {}): LocaleStrings {
  if (!cached) {
    const parsed = parseFtlEntries(englishFtl);
    const entries: Record<string, string> = {};
    for (const key of ALL_LOCALE_KEYS) {
      entries[key] = parsed[key] ?? key;
    }
    cached = { language: "english", entries: { ...parsed, ...entries } };
  }
  if (Object.keys(overrides).length === 0) return cached;
  return { ...cached, entries: { ...cached.entries, ...overrides } };
}

export const PROOF_LANGUAGES: LanguageOption[] = [
  { value: "english", display: "English" },
];
