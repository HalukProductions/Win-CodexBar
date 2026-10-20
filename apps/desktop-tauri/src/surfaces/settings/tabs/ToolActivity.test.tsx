import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { CodexSessionToolActivity, CodexToolOperation } from "../../../types/bridge";
import type { LocaleKey } from "../../../i18n/keys";

const tauriMocks = vi.hoisted(() => ({ getCodexSessionToolActivity: vi.fn() }));
vi.mock("../../../lib/tauri", () => tauriMocks);

import ToolActivity, { TOOL_INITIAL_ROWS, TOOL_ROWS_STEP, formatToolDuration } from "./ToolActivity";

const EN: Partial<Record<LocaleKey, string>> = {
  UsageSpendToolsMilliseconds: "{} ms",
  UsageSpendToolsExit: "Exit code {}",
  UsageSpendSeconds: "s",
};
const t = (key: LocaleKey) => EN[key] ?? key;

function operation(index: number, overrides: Partial<CodexToolOperation> = {}): CodexToolOperation {
  return {
    turnId: "turn-1",
    itemId: `item-${index}`,
    kind: "command",
    name: `shell-${index}`,
    preview: `echo ${index}`,
    completedAt: "2026-10-11T10:00:00Z",
    outcome: "completed",
    exitCode: 0,
    durationMilliseconds: 250,
    timing: "native",
    needsAttention: false,
    isSlow: false,
    ...overrides,
  };
}

function activity(
  operations: CodexToolOperation[],
  overrides: Partial<CodexSessionToolActivity> = {},
): CodexSessionToolActivity {
  return { sessionId: "s-1", operations, ignoredRecordCount: 0, isPartial: false, redacted: false, ...overrides };
}

async function open(historyDays = 30) {
  render(<ToolActivity sessionId="s-1" historyDays={historyDays} t={t} />);
  fireEvent.click(screen.getByRole("button", { name: "UsageSpendToolsTitle" }));
  return screen.findByTestId("tool-activity");
}

function rows(): HTMLElement[] {
  return screen.queryAllByTestId("tool-operation");
}

describe("ToolActivity", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("reads nothing until opened, then reads the session over the history window", async () => {
    tauriMocks.getCodexSessionToolActivity.mockResolvedValue(activity([operation(1)]));
    render(<ToolActivity sessionId="s-1" historyDays={7} t={t} />);
    expect(tauriMocks.getCodexSessionToolActivity).not.toHaveBeenCalled();

    const toggle = screen.getByRole("button", { name: "UsageSpendToolsTitle" });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    expect(screen.getByRole("status").textContent).toBe("UsageSpendToolsLoading");
    await waitFor(() => expect(rows()).toHaveLength(1));
    expect(tauriMocks.getCodexSessionToolActivity).toHaveBeenCalledWith("s-1", 7);

    // Closing and reopening keeps the loaded result.
    fireEvent.click(toggle);
    fireEvent.click(toggle);
    expect(tauriMocks.getCodexSessionToolActivity).toHaveBeenCalledTimes(1);
  });

  it("summarizes counts, timing coverage and the longest native execution", async () => {
    tauriMocks.getCodexSessionToolActivity.mockResolvedValue(
      activity([
        operation(1, { durationMilliseconds: 42_000, isSlow: true, name: "cargo-test" }),
        operation(2, { outcome: "nonzeroExit", exitCode: 2, needsAttention: true }),
        operation(3, { timing: "recordedInterval", durationMilliseconds: 90_000, isSlow: true }),
        operation(4, { durationMilliseconds: null, timing: null }),
      ]),
    );
    const panel = await open();
    await waitFor(() => expect(rows()).toHaveLength(4));

    expect(
      within(panel).getByText(
        "UsageSpendToolsOperations: 4 · UsageSpendToolsAttention: 1 · UsageSpendToolsTimed: 3 / 4",
      ),
    ).toBeTruthy();
    // A recorded interval is not a native execution, even when longer.
    expect(within(panel).getByText("UsageSpendToolsLongest: 42.0 s · cargo-test")).toBeTruthy();
    expect(within(panel).getByText("90.0 s (UsageSpendToolsInterval)")).toBeTruthy();
    expect(within(panel).getByText(/UsageSpendToolsOutcomeNonzeroExit · Exit code 2/)).toBeTruthy();
    expect(within(panel).getByText("—")).toBeTruthy();
  });

  it("filters to operations that need review or are slow", async () => {
    tauriMocks.getCodexSessionToolActivity.mockResolvedValue(
      activity([
        operation(1, { isSlow: true }),
        operation(2, { needsAttention: true, outcome: "toolError", exitCode: null }),
        operation(3),
      ]),
    );
    await open();
    await waitFor(() => expect(rows()).toHaveLength(3));
    const filters = screen.getByRole("group", { name: "UsageSpendToolsFilter" });

    fireEvent.click(within(filters).getByRole("button", { name: "UsageSpendToolsFilterAttention" }));
    expect(rows().map((row) => row.textContent)).toEqual([expect.stringContaining("shell-2")]);
    expect(
      within(filters).getByRole("button", { name: "UsageSpendToolsFilterAttention" }).getAttribute("aria-pressed"),
    ).toBe("true");

    fireEvent.click(within(filters).getByRole("button", { name: "UsageSpendToolsFilterSlow" }));
    expect(rows().map((row) => row.textContent)).toEqual([expect.stringContaining("shell-1")]);

    fireEvent.click(within(filters).getByRole("button", { name: "UsageSpendToolsFilterAll" }));
    expect(rows()).toHaveLength(3);
  });

  it("says when no operation matches the filter", async () => {
    tauriMocks.getCodexSessionToolActivity.mockResolvedValue(activity([operation(1)]));
    await open();
    await waitFor(() => expect(rows()).toHaveLength(1));
    fireEvent.click(screen.getByRole("button", { name: "UsageSpendToolsFilterSlow" }));
    expect(rows()).toHaveLength(0);
    expect(screen.getByText("UsageSpendToolsNoMatches")).toBeTruthy();
  });

  it("shows the first rows and adds more on request", async () => {
    const total = TOOL_INITIAL_ROWS + TOOL_ROWS_STEP + 3;
    tauriMocks.getCodexSessionToolActivity.mockResolvedValue(
      activity(Array.from({ length: total }, (_, index) => operation(index))),
    );
    await open();
    await waitFor(() => expect(rows()).toHaveLength(TOOL_INITIAL_ROWS));

    fireEvent.click(screen.getByRole("button", { name: `UsageSpendToolsMore (${total - TOOL_INITIAL_ROWS})` }));
    expect(rows()).toHaveLength(TOOL_INITIAL_ROWS + TOOL_ROWS_STEP);
    fireEvent.click(screen.getByRole("button", { name: "UsageSpendToolsMore (3)" }));
    expect(rows()).toHaveLength(total);
    expect(screen.queryByRole("button", { name: /UsageSpendToolsMore/ })).toBeNull();
  });

  it("hides names and previews of a redacted result and says why", async () => {
    tauriMocks.getCodexSessionToolActivity.mockResolvedValue(
      // A redacted result normally carries no previews; the row must not render one even if it did.
      activity([operation(1, { name: "command", preview: "echo 1", durationMilliseconds: 12_000 })], { redacted: true }),
    );
    const panel = await open();
    await waitFor(() => expect(rows()).toHaveLength(1));

    expect(within(panel).getByText("UsageSpendToolsHidden")).toBeTruthy();
    expect(rows()[0].textContent).not.toContain("command");
    expect(rows()[0].querySelector("code")).toBeNull();
    expect(within(panel).getByText("UsageSpendToolsLongest: 12.0 s · UsageSpendToolsKindCommand")).toBeTruthy();
  });

  it("flags partial coverage and shows the empty state", async () => {
    tauriMocks.getCodexSessionToolActivity.mockResolvedValue(activity([], { isPartial: true }));
    const panel = await open();
    expect(await within(panel).findByText("UsageSpendToolsEmpty")).toBeTruthy();
    expect(within(panel).getByText("UsageSpendToolsPartial")).toBeTruthy();
  });

  it("reports a failed read and retries on refresh", async () => {
    tauriMocks.getCodexSessionToolActivity.mockRejectedValueOnce(new Error("io"));
    tauriMocks.getCodexSessionToolActivity.mockResolvedValueOnce(activity([operation(1)]));
    await open();
    expect((await screen.findByRole("alert")).textContent).toBe("UsageSpendToolsUnavailable");

    fireEvent.click(screen.getByRole("button", { name: "UsageSpendToolsRefresh" }));
    await waitFor(() => expect(rows()).toHaveLength(1));
    expect(screen.queryByRole("alert")).toBeNull();
    expect(tauriMocks.getCodexSessionToolActivity).toHaveBeenCalledTimes(2);
  });

  it("formats sub-second durations in milliseconds and longer ones in seconds", () => {
    expect(formatToolDuration(null, t)).toBe("—");
    expect(formatToolDuration(0, t)).toBe("0 ms");
    expect(formatToolDuration(999, t)).toBe("999 ms");
    expect(formatToolDuration(1000, t)).toBe("1.0 s");
    expect(formatToolDuration(10_400, t)).toBe("10.4 s");
  });
});
