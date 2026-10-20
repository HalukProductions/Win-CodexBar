import { describe, expect, it } from "vitest";
import { fromFtl } from "../test/localeHarness";
import type { CodexWorkspacesSessionUsage, SpendContract, UsageSpendSummary } from "../types/bridge";
import { CARD_CONVERSATION_MAX_ROWS, cardConversationsModel, shortSessionId } from "./cardConversations";

const t = fromFtl("en-US.ftl");

function session(
  id: string,
  displayTitle: string,
  latestActivity: string | null,
  overrides: Partial<CodexWorkspacesSessionUsage> = {},
): CodexWorkspacesSessionUsage {
  return {
    id,
    projectId: "proj-1",
    displayTitle,
    cwd: null,
    startedAt: null,
    latestActivity,
    totals: { inputTokens: 1200, cachedInputTokens: 800, outputTokens: 300, totalTokens: 1500 },
    costEstimate: { knownUsd: 0.5, unknownTokens: 0 },
    topModel: "gpt-5-codex",
    turnPerformance: null,
    ...overrides,
  };
}

function summary(
  conversations: CodexWorkspacesSessionUsage[],
  overrides: Partial<SpendContract> = {},
  costTrackingEnabled = true,
): UsageSpendSummary {
  return {
    rows: [],
    contract: {
      providerId: "codex",
      reportingPeriod: "rolling:30",
      conversations,
      ...overrides,
    } as SpendContract,
    reportingPeriod: "rolling:30",
    reportingDay: "2026-10-20",
    dashboardTimezone: "UTC",
    costTrackingEnabled,
  };
}

describe("cardConversationsModel", () => {
  it("lists conversations newest first with title, usage line, and cost", () => {
    const model = cardConversationsModel(
      "codex",
      summary([
        session("older-session", "Fix the parser", "2026-10-18T09:00:00Z", {
          costEstimate: { knownUsd: 4.2, unknownTokens: 0 },
        }),
        session("newer-session", "Add the tray list", "2026-10-19T09:00:00Z", {
          costEstimate: { knownUsd: 1.234, unknownTokens: 10 },
        }),
      ]),
      t,
    );
    expect(model?.title).toBe("Conversations (30d)");
    expect(model?.count).toBe(2);
    expect(model?.rows.map((row) => row.title)).toEqual(["Add the tray list", "Fix the parser"]);
    expect(model?.rows[0]).toMatchObject({
      sessionLabel: "Session newe...-session",
      usageLine: "gpt-5-codex · 1.2K input · 800 cached · 300 output",
      cost: "~$1.23",
    });
    expect(model?.rows[1].cost).toBe("$4.20");
    expect(model?.rows[0].lastActivity).toBeTruthy();
  });

  it("keeps the backend's Session label for untitled or masked rows and fills a blank title", () => {
    const id = "019f0000-aaaa-bbbb-cccc-00001e92b191";
    const model = cardConversationsModel(
      "codex",
      summary([
        session(id, "Session 019f...1e92b191", "2026-10-19T09:00:00Z"),
        session("blank-title-id-0000000", "   ", "2026-10-18T09:00:00Z"),
      ]),
      t,
    );
    expect(model?.rows[0]).toMatchObject({
      title: "Session 019f...1e92b191",
      sessionLabel: "Session 019f...1e92b191",
    });
    expect(model?.rows[1].title).toBe("Session blan...-0000000");
  });

  it("marks a conversation with no priced tokens as unknown and names a missing model", () => {
    const model = cardConversationsModel(
      "codex",
      summary([
        session("unpriced", "Unpriced", null, {
          costEstimate: { knownUsd: 0, unknownTokens: 500 },
          topModel: null,
        }),
      ]),
      t,
    );
    expect(model?.rows[0].cost).toBe("—");
    expect(model?.rows[0].usageLine.startsWith("Unknown model · ")).toBe(true);
    expect(model?.rows[0].lastActivity).toBeNull();
  });

  it("names the History window the contract covers", () => {
    const model = cardConversationsModel(
      "codex",
      summary([session("a", "A", null)], { reportingPeriod: "month-to-date" }),
      t,
    );
    expect(model?.title).toBe(`Conversations (${t("CostPeriodShortMonthToDate")})`);
  });

  it("caps the listed rows but reports the full count", () => {
    const many = Array.from({ length: CARD_CONVERSATION_MAX_ROWS + 7 }, (_, index) =>
      session(`s-${index}`, `Chat ${index}`, `2026-10-01T00:${String(index).padStart(2, "0")}:00Z`),
    );
    const model = cardConversationsModel("codex", summary(many), t);
    expect(model?.rows).toHaveLength(CARD_CONVERSATION_MAX_ROWS);
    expect(model?.count).toBe(CARD_CONVERSATION_MAX_ROWS + 7);
  });

  it("shows nothing for another provider, with tracking off, or without conversations", () => {
    const rows = [session("a", "A", null)];
    expect(cardConversationsModel("claude", summary(rows), t)).toBeNull();
    expect(cardConversationsModel("codex", summary(rows, { providerId: "claude" }), t)).toBeNull();
    expect(cardConversationsModel("codex", summary(rows, {}, false), t)).toBeNull();
    expect(cardConversationsModel("codex", summary([]), t)).toBeNull();
    expect(cardConversationsModel("codex", null, t)).toBeNull();
  });
});

describe("shortSessionId", () => {
  it("matches codex_workspaces::short_session_id", () => {
    expect(shortSessionId("  short-id  ")).toBe("short-id");
    expect(shortSessionId("abcdefghijkl")).toBe("abcdefghijkl");
    expect(shortSessionId("019f0000-aaaa-bbbb-cccc-00001e92b191")).toBe("019f...1e92b191");
  });
});
