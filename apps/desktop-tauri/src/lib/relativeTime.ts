import type { LocaleKey } from "../i18n/keys";

type Translate = (key: LocaleKey) => string;

/**
 * Render a Unix-ms timestamp as a localized "updated N ago" string,
 * matching the egui `render_advanced_tab` time display.
 *
 * Returns the `NeverUpdated` locale string when `timestampMs` is `null`
 * or `undefined`, mirroring how egui surfaces provider-update timing.
 */
export function formatRelativeUpdated(
  timestampMs: number | null | undefined,
  t: Translate,
  nowMs: number = Date.now(),
): string {
  if (timestampMs == null) {
    return t("NeverUpdated");
  }
  const diffSecs = Math.max(0, Math.floor((nowMs - timestampMs) / 1000));
  if (diffSecs < 60) {
    return t("UpdatedJustNow");
  }
  const diffMins = Math.floor(diffSecs / 60);
  if (diffMins < 60) {
    return countAgo(t, diffMins, MINUTES);
  }
  const diffHours = Math.floor(diffMins / 60);
  if (diffHours < 24) {
    return countAgo(t, diffHours, HOURS);
  }
  const diffDays = Math.floor(diffHours / 24);
  return countAgo(t, diffDays, DAYS);
}

/**
 * Per-unit keys for the CLDR plural categories the shipped locales use with
 * whole numbers. `other` is the base key, so a locale with no plural forms
 * (ja, ko, zh) and any category without its own key read the base template.
 */
interface CountKeys {
  other: LocaleKey;
  one: LocaleKey;
  few: LocaleKey;
  many: LocaleKey;
}

const MINUTES: CountKeys = {
  other: "UpdatedMinutesAgo",
  one: "UpdatedMinutesAgoOne",
  few: "UpdatedMinutesAgoFew",
  many: "UpdatedMinutesAgoMany",
};
const HOURS: CountKeys = {
  other: "UpdatedHoursAgo",
  one: "UpdatedHoursAgoOne",
  few: "UpdatedHoursAgoFew",
  many: "UpdatedHoursAgoMany",
};
const DAYS: CountKeys = {
  other: "UpdatedDaysAgo",
  one: "UpdatedDaysAgoOne",
  few: "UpdatedDaysAgoFew",
  many: "UpdatedDaysAgoMany",
};

const pluralRulesCache = new Map<string, Intl.PluralRules>();

/** Plural rules for the active bundle's `LocaleTag`, or English when the tag is missing or invalid. */
function pluralRules(t: Translate): Intl.PluralRules {
  const tag = t("LocaleTag");
  let rules = pluralRulesCache.get(tag);
  if (!rules) {
    try {
      rules = new Intl.PluralRules(tag);
    } catch {
      rules = new Intl.PluralRules("en-US");
    }
    pluralRulesCache.set(tag, rules);
  }
  return rules;
}

function countAgo(t: Translate, count: number, keys: CountKeys): string {
  const category = pluralRules(t).select(count);
  const key = category === "one" || category === "few" || category === "many" ? keys[category] : keys.other;
  const template = t(key);
  // A bundle without the category key echoes the key name back.
  const resolved = template === key ? t(keys.other) : template;
  return resolved.replace("{}", String(count));
}
