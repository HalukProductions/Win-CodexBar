import englishFtl from "../../../../rust/src/locale/en-US.ftl?raw";
import ukFtl from "../../../../rust/src/locale/uk-UA.ftl?raw";
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

/**
 * The shipped uk-UA strings for `keys`, parsed the same way as the English file.
 * A scenario passes the result as `localeOverrides` to show one localized surface
 * while the rest of the proof stays English. Keys missing from uk-UA are omitted.
 */
export function ukLocaleOverrides(keys: readonly string[]): Record<string, string> {
  const parsed = parseFtlEntries(ukFtl);
  const overrides: Record<string, string> = {};
  for (const key of keys) {
    const value = parsed[key];
    if (value !== undefined) overrides[key] = value;
  }
  return overrides;
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
