import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const STRINGS: Record<string, string> = {
  ProviderPlanClaudeAi: "Claude AI",
  UsageSpendRefreshing: "Refreshing…",
  ProviderNotFetchedYetTitle: "Not fetched yet",
  UpdatedJustNow: "Updated just now",
  UpdatedMinutesAgo: "Updated {}m ago",
  PanelCopied: "Copied",
  ActionCopyError: "Copy error",
};
vi.mock("../../hooks/useLocale", () => ({
  useLocale: () => ({ t: (key: string) => STRINGS[key] ?? key, language: "english" }),
}));

import CardHeader, {
  displayPlanName,
  headerSubtitle,
  splitForMiddleTruncation,
} from "./CardHeader";

const t = (key: string) => STRINGS[key] ?? key;

afterEach(() => {
  vi.useRealTimers();
});

describe("displayPlanName", () => {
  it("keeps a real plan name", () => {
    expect(displayPlanName("  Pro 20x ", t)).toBe("Pro 20x");
  });

  it("maps the raw Claude AI tier", () => {
    expect(displayPlanName("default_claude_ai", t)).toBe("Claude AI");
  });

  it("drops the source-only Claude login labels", () => {
    expect(displayPlanName("Claude (OAuth)", t)).toBeNull();
    expect(displayPlanName("Claude (CLI)", t)).toBeNull();
  });

  it("returns nothing for a blank plan", () => {
    expect(displayPlanName(null, t)).toBeNull();
    expect(displayPlanName("  ", t)).toBeNull();
  });
});

describe("headerSubtitle", () => {
  const now = Date.parse("2026-10-11T12:00:00Z");

  it("prefers Refreshing… while a refresh runs", () => {
    expect(headerSubtitle("2026-10-11T11:00:00Z", true, t, now)).toBe("Refreshing…");
  });

  it("formats a parsed time as a relative update", () => {
    expect(headerSubtitle("2026-10-11T11:59:30Z", false, t, now)).toBe("Updated just now");
    expect(headerSubtitle("2026-10-11T11:55:00Z", false, t, now)).toBe("Updated 5m ago");
  });

  it("says Not fetched yet without a time", () => {
    expect(headerSubtitle("", false, t, now)).toBe("Not fetched yet");
    expect(headerSubtitle(null, false, t, now)).toBe("Not fetched yet");
  });

  it("passes an unparseable label through", () => {
    expect(headerSubtitle("cached", false, t, now)).toBe("cached");
  });
});

describe("splitForMiddleTruncation", () => {
  it("splits the text into a head and a tail", () => {
    expect(splitForMiddleTruncation("parity.user@example.com")).toEqual([
      "parity.user@",
      "example.com",
    ]);
  });
});

describe("CardHeader", () => {
  it("shows the name, the email, the subtitle and the plan", () => {
    const { container } = render(
      <CardHeader
        name="Codex"
        email="parity.user@example.com"
        plan="Pro 20x"
        error={null}
        updatedAt={new Date().toISOString()}
        isRefreshing={false}
      />,
    );
    expect(screen.getByText("Codex")).toHaveClass("card-header__name");
    expect(container.querySelector(".card-header__email")).toHaveTextContent(
      "parity.user@example.com",
    );
    expect(screen.getByText("Updated just now")).toHaveClass("card-header__subtitle");
    expect(screen.getByText("Pro 20x")).toHaveClass("card-header__plan");
  });

  it("keeps the email and plan beside an error and copies the error text", async () => {
    vi.useFakeTimers();
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const { container } = render(
      <CardHeader
        name="Claude"
        email="a@b.c"
        plan="Max"
        error="Authentication required"
        updatedAt="2026-10-11T11:00:00Z"
        isRefreshing={false}
      />,
    );
    expect(screen.getByText("Authentication required")).toHaveClass("card-header__error");
    expect(container.querySelector(".card-header__email")).toHaveTextContent("a@b.c");
    expect(screen.getByText("Max")).toBeInTheDocument();
    expect(container.querySelector(".card-header__subtitle")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Copy error" }));
    expect(writeText).toHaveBeenCalledWith("Authentication required");
    expect(screen.getByRole("button", { name: "Copied" })).toBeInTheDocument();
    act(() => {
      vi.advanceTimersByTime(900);
    });
    expect(screen.getByRole("button", { name: "Copy error" })).toBeInTheDocument();
  });

  it("advances the update subtitle while the data stays unchanged", () => {
    vi.useFakeTimers();
    vi.setSystemTime(Date.parse("2026-10-11T12:00:00Z"));
    render(
      <CardHeader
        name="Codex"
        email={null}
        plan={null}
        error={null}
        updatedAt="2026-10-11T11:59:30Z"
        isRefreshing={false}
      />,
    );
    expect(screen.getByText("Updated just now")).toBeInTheDocument();

    act(() => {
      vi.advanceTimersByTime(60_000);
    });
    expect(screen.getByText("Updated 1m ago")).toBeInTheDocument();
  });

  it("omits the email and plan when there are none", () => {
    const { container } = render(
      <CardHeader name="Cursor" email={null} plan={null} error={null} updatedAt="" isRefreshing />,
    );
    expect(container.querySelector(".card-header__email")).toBeNull();
    expect(container.querySelector(".card-header__plan")).toBeNull();
    expect(screen.getByText("Refreshing…")).toBeInTheDocument();
  });
});
