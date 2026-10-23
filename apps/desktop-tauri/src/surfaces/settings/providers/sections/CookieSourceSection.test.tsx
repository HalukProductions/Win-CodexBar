import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const tauriMocks = vi.hoisted(() => ({
  setProviderCookieSource: vi.fn(),
}));

vi.mock("../../../../lib/tauri", () => tauriMocks);

import { loadStyles, ruleBlock } from "../../../../test/styles";
import { CookieSourceSection } from "./CookieSourceSection";

const OPTIONS = [
  { value: "auto", label: "Automatic" },
  { value: "manual", label: "Manual" },
];

function renderSection(
  overrides: Partial<Parameters<typeof CookieSourceSection>[0]> = {},
) {
  const onChanged = vi.fn();
  render(
    <CookieSourceSection
      providerId="ollama"
      currentValue="manual"
      options={OPTIONS}
      manualCookieMissing
      t={(key) => key}
      onChanged={onChanged}
      {...overrides}
    />,
  );
  return { onChanged };
}

describe("CookieSourceSection manual cookie hint", () => {
  beforeEach(() => {
    tauriMocks.setProviderCookieSource.mockReset();
  });

  it("explains an empty Manual configuration and switches to Automatic on request", async () => {
    tauriMocks.setProviderCookieSource.mockResolvedValue(undefined);
    const { onChanged } = renderSection();

    expect(screen.getByText("ProviderManualCookieMissing")).toBeInTheDocument();
    expect(tauriMocks.setProviderCookieSource).not.toHaveBeenCalled();

    fireEvent.click(
      screen.getByRole("button", { name: "ProviderUseAutomaticCookies" }),
    );

    await waitFor(() => expect(onChanged).toHaveBeenCalledTimes(1));
    expect(tauriMocks.setProviderCookieSource).toHaveBeenCalledWith(
      "ollama",
      "auto",
    );
  });

  it("stays quiet when the backend reports a header is configured", () => {
    renderSection({ manualCookieMissing: false });

    expect(
      screen.queryByText("ProviderManualCookieMissing"),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "ProviderUseAutomaticCookies" }),
    ).not.toBeInTheDocument();
  });

  it("hides the action when Automatic is already selected", () => {
    renderSection({ currentValue: "auto" });

    expect(
      screen.queryByRole("button", { name: "ProviderUseAutomaticCookies" }),
    ).not.toBeInTheDocument();
  });

  it("hides the action for providers without an Automatic option", () => {
    renderSection({ options: [{ value: "manual", label: "Manual" }] });

    expect(
      screen.queryByRole("button", { name: "ProviderUseAutomaticCookies" }),
    ).not.toBeInTheDocument();
  });

  it("renders the picker for a cookie provider and nothing without options", () => {
    const { unmount } = render(
      <CookieSourceSection
        providerId="claude"
        currentValue="auto"
        options={OPTIONS}
        t={(key) => key}
        onChanged={vi.fn()}
      />,
    );
    expect(screen.getByRole("radiogroup", { name: "ProviderCookieSource" })).toBeInTheDocument();
    unmount();

    const { container } = render(
      <CookieSourceSection
        providerId="vercel"
        currentValue={null}
        options={[]}
        t={(key) => key}
        onChanged={vi.fn()}
      />,
    );
    expect(container).toBeEmptyDOMElement();
  });

  it("wraps the segmented options inside the narrow detail pane", () => {
    const rule = ruleBlock(loadStyles(), ".provider-detail-segmented");
    expect(rule).toMatch(/flex-wrap:\s*wrap;/);
    expect(rule).toMatch(/max-width:\s*100%;/);
  });
});
