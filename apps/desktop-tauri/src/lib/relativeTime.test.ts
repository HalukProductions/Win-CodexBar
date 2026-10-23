import { describe, it, expect, vi } from "vitest";
import { formatRelativeUpdated } from "./relativeTime";
import type { LocaleKey } from "../i18n/keys";

// Simple translator stub — echoes the key plus whatever replacement the
// helper substitutes into the "{}" placeholder so we can assert structure.
const t = (key: LocaleKey): string => {
  switch (key) {
    case "NeverUpdated":
      return "Never";
    case "UpdatedJustNow":
      return "just now";
    case "UpdatedMinutesAgo":
      return "{} min ago";
    case "UpdatedHoursAgo":
      return "{} hr ago";
    case "UpdatedDaysAgo":
      return "{} d ago";
    case "UpdatedMinutesAgoOne":
      return "{} minute ago";
    case "UpdatedHoursAgoOne":
      return "{} hour ago";
    case "UpdatedDaysAgoOne":
      return "{} day ago";
    default:
      return key;
  }
};

describe("formatRelativeUpdated", () => {
  const NOW = Date.parse("2024-06-01T12:00:00Z");

  it("returns 'Never' when timestamp is null or undefined", () => {
    expect(formatRelativeUpdated(null, t, NOW)).toBe("Never");
    expect(formatRelativeUpdated(undefined, t, NOW)).toBe("Never");
  });

  it("treats future timestamps as 'just now' (clamps negative diffs)", () => {
    expect(formatRelativeUpdated(NOW + 60_000, t, NOW)).toBe("just now");
  });

  it("uses 'just now' for sub-minute diffs", () => {
    expect(formatRelativeUpdated(NOW - 1_000, t, NOW)).toBe("just now");
    expect(formatRelativeUpdated(NOW - 59_000, t, NOW)).toBe("just now");
  });

  it("renders minutes for sub-hour diffs", () => {
    expect(formatRelativeUpdated(NOW - 5 * 60_000, t, NOW)).toBe("5 min ago");
    expect(formatRelativeUpdated(NOW - 59 * 60_000, t, NOW)).toBe("59 min ago");
  });

  it("renders hours for sub-day diffs", () => {
    expect(formatRelativeUpdated(NOW - 2 * 3600_000, t, NOW)).toBe("2 hr ago");
    expect(formatRelativeUpdated(NOW - 23 * 3600_000, t, NOW)).toBe(
      "23 hr ago",
    );
  });

  it("renders days beyond 24h", () => {
    expect(formatRelativeUpdated(NOW - 2 * 24 * 3600_000, t, NOW)).toBe("2 d ago");
    expect(formatRelativeUpdated(NOW - 9 * 24 * 3600_000, t, NOW)).toBe(
      "9 d ago",
    );
  });

  it("uses the singular string for exactly one minute, hour or day", () => {
    expect(formatRelativeUpdated(NOW - 60_000, t, NOW)).toBe("1 minute ago");
    expect(formatRelativeUpdated(NOW - 119_000, t, NOW)).toBe("1 minute ago");
    expect(formatRelativeUpdated(NOW - 60 * 60_000, t, NOW)).toBe("1 hour ago");
    expect(formatRelativeUpdated(NOW - 24 * 3600_000, t, NOW)).toBe("1 day ago");
    expect(formatRelativeUpdated(NOW - 47 * 3600_000, t, NOW)).toBe("1 day ago");
  });

  it("falls back to the base template when a category key is missing", () => {
    const sparse = (key: LocaleKey): string =>
      key === "UpdatedMinutesAgo" ? "{} min ago" : key;
    expect(formatRelativeUpdated(NOW - 60_000, sparse, NOW)).toBe("1 min ago");
  });

  it("defaults `nowMs` to Date.now()", () => {
    const spy = vi.spyOn(Date, "now").mockReturnValue(NOW);
    try {
      expect(formatRelativeUpdated(NOW - 120_000, t)).toBe("2 min ago");
    } finally {
      spy.mockRestore();
    }
  });
});

/** Copies of the shipped strings; `rust/src/locale/tests.rs` pins the originals. */
const BUNDLES: Record<string, Partial<Record<LocaleKey, string>>> = {
  en: {
    LocaleTag: "en-US",
    UpdatedMinutesAgo: "{} minutes ago",
    UpdatedMinutesAgoOne: "{} minute ago",
    UpdatedMinutesAgoFew: "{} minutes ago",
    UpdatedMinutesAgoMany: "{} minutes ago",
    UpdatedHoursAgo: "{} hours ago",
    UpdatedHoursAgoOne: "{} hour ago",
    UpdatedDaysAgo: "{} days ago",
    UpdatedDaysAgoOne: "{} day ago",
  },
  ru: {
    LocaleTag: "ru-RU",
    UpdatedMinutesAgo: "{} минут назад",
    UpdatedMinutesAgoOne: "{} минуту назад",
    UpdatedMinutesAgoFew: "{} минуты назад",
    UpdatedMinutesAgoMany: "{} минут назад",
    UpdatedHoursAgo: "{} часов назад",
    UpdatedHoursAgoOne: "{} час назад",
    UpdatedHoursAgoFew: "{} часа назад",
    UpdatedHoursAgoMany: "{} часов назад",
    UpdatedDaysAgo: "{} дней назад",
    UpdatedDaysAgoOne: "{} день назад",
    UpdatedDaysAgoFew: "{} дня назад",
    UpdatedDaysAgoMany: "{} дней назад",
  },
  uk: {
    LocaleTag: "uk-UA",
    UpdatedMinutesAgo: "{} хв тому",
    UpdatedMinutesAgoOne: "{} хвилину тому",
    UpdatedMinutesAgoFew: "{} хвилини тому",
    UpdatedMinutesAgoMany: "{} хвилин тому",
    UpdatedDaysAgo: "{} дн. тому",
    UpdatedDaysAgoOne: "{} день тому",
    UpdatedDaysAgoFew: "{} дні тому",
    UpdatedDaysAgoMany: "{} днів тому",
  },
  ja: {
    LocaleTag: "ja-JP",
    UpdatedMinutesAgo: "{}分前",
    UpdatedMinutesAgoOne: "{}分前",
    UpdatedMinutesAgoFew: "{}分前",
    UpdatedMinutesAgoMany: "{}分前",
    UpdatedDaysAgo: "{}日前",
    UpdatedDaysAgoOne: "{}日前",
    UpdatedDaysAgoFew: "{}日前",
    UpdatedDaysAgoMany: "{}日前",
  },
};

function bundle(name: keyof typeof BUNDLES) {
  return (key: LocaleKey): string => BUNDLES[name][key] ?? key;
}

describe("formatRelativeUpdated plural categories", () => {
  const NOW = Date.parse("2024-06-01T12:00:00Z");
  const minutes = (n: number) => NOW - n * 60_000;
  const days = (n: number) => NOW - n * 24 * 3600_000;

  it("uses one/other for English", () => {
    const t = bundle("en");
    expect(formatRelativeUpdated(minutes(1), t, NOW)).toBe("1 minute ago");
    expect(formatRelativeUpdated(minutes(2), t, NOW)).toBe("2 minutes ago");
    expect(formatRelativeUpdated(minutes(21), t, NOW)).toBe("21 minutes ago");
    expect(formatRelativeUpdated(NOW - 3600_000, t, NOW)).toBe("1 hour ago");
    expect(formatRelativeUpdated(days(1), t, NOW)).toBe("1 day ago");
    expect(formatRelativeUpdated(days(111), t, NOW)).toBe("111 days ago");
  });

  it.each([
    [1, "1 минуту назад"],
    [2, "2 минуты назад"],
    [4, "4 минуты назад"],
    [5, "5 минут назад"],
    [11, "11 минут назад"],
    [12, "12 минут назад"],
    [20, "20 минут назад"],
    [21, "21 минуту назад"],
    [22, "22 минуты назад"],
    [25, "25 минут назад"],
  ])("Russian minutes: %i -> %s", (n, expected) => {
    expect(formatRelativeUpdated(minutes(n), bundle("ru"), NOW)).toBe(expected);
  });

  it.each([
    [1, "1 день назад"],
    [3, "3 дня назад"],
    [5, "5 дней назад"],
    [21, "21 день назад"],
    [22, "22 дня назад"],
    [25, "25 дней назад"],
    [111, "111 дней назад"],
  ])("Russian days: %i -> %s", (n, expected) => {
    expect(formatRelativeUpdated(days(n), bundle("ru"), NOW)).toBe(expected);
  });

  it("uses one/few/many for Russian hours", () => {
    const t = bundle("ru");
    expect(formatRelativeUpdated(NOW - 1 * 3600_000, t, NOW)).toBe("1 час назад");
    expect(formatRelativeUpdated(NOW - 2 * 3600_000, t, NOW)).toBe("2 часа назад");
    expect(formatRelativeUpdated(NOW - 5 * 3600_000, t, NOW)).toBe("5 часов назад");
    expect(formatRelativeUpdated(NOW - 21 * 3600_000, t, NOW)).toBe("21 час назад");
  });

  it.each([
    [1, "1 хвилину тому"],
    [2, "2 хвилини тому"],
    [4, "4 хвилини тому"],
    [5, "5 хвилин тому"],
    [11, "11 хвилин тому"],
    [20, "20 хвилин тому"],
    [21, "21 хвилину тому"],
    [22, "22 хвилини тому"],
    [25, "25 хвилин тому"],
  ])("Ukrainian minutes: %i -> %s", (n, expected) => {
    expect(formatRelativeUpdated(minutes(n), bundle("uk"), NOW)).toBe(expected);
  });

  it.each([
    [1, "1 день тому"],
    [2, "2 дні тому"],
    [5, "5 днів тому"],
    [21, "21 день тому"],
    [22, "22 дні тому"],
    [25, "25 днів тому"],
    [111, "111 днів тому"],
  ])("Ukrainian days: %i -> %s", (n, expected) => {
    expect(formatRelativeUpdated(days(n), bundle("uk"), NOW)).toBe(expected);
  });

  it("uses the base form for Japanese, which has no plural categories", () => {
    const t = bundle("ja");
    expect(formatRelativeUpdated(minutes(1), t, NOW)).toBe("1分前");
    expect(formatRelativeUpdated(minutes(5), t, NOW)).toBe("5分前");
    expect(formatRelativeUpdated(days(21), t, NOW)).toBe("21日前");
  });
});
