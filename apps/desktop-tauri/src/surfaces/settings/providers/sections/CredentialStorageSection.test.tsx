import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { CredentialStorageSection } from "./CredentialStorageSection";

describe("CredentialStorageSection", () => {
  it("lists stored access tokens next to the other credential stores", () => {
    render(
      <CredentialStorageSection
        status={{
          apiKeys: "missing",
          manualCookies: "missing",
          tokenAccounts: "missing",
          accessTokens: "protected:windows-dpapi-user",
        }}
        busy={false}
        onRevoke={vi.fn()}
        t={(key) => key}
      />,
    );

    const term = screen.getByText("CredentialAccessTokens");
    expect(term.tagName).toBe("DT");
    expect(term.nextElementSibling).toHaveTextContent(
      "CredentialProtectedPrefix (windows-dpapi-user)",
    );
    expect(screen.getByText("CredentialApiKeys").nextElementSibling).toHaveTextContent(
      "CredentialStatusNotCreated",
    );
  });

  it("reports access tokens as not created when none is stored", () => {
    render(
      <CredentialStorageSection
        status={{
          apiKeys: "missing",
          manualCookies: "missing",
          tokenAccounts: "missing",
          accessTokens: "missing",
        }}
        busy={false}
        onRevoke={vi.fn()}
        t={(key) => key}
      />,
    );

    expect(screen.getByText("CredentialAccessTokens").nextElementSibling).toHaveTextContent(
      "CredentialStatusNotCreated",
    );
  });
});
