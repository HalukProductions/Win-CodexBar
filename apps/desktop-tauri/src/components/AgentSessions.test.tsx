import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({
  listAgentSessions: vi.fn(),
  focusAgentSession: vi.fn(),
}));

vi.mock("../lib/tauri", () => ({
  listAgentSessions: api.listAgentSessions,
  focusAgentSession: api.focusAgentSession,
}));
// t(key) returns the key, so provider labels assert against locale-key names.
vi.mock("../hooks/useLocale", () => import("../test/mocks/locale"));

import AgentSessions from "./AgentSessions";
import type { AgentSession } from "../types/bridge";

function session(overrides: Partial<AgentSession>): AgentSession {
  return {
    id: "s",
    provider: "codex",
    source: "cli",
    state: "active",
    pid: 1,
    transcriptPath: null,
    host: "DESKTOP",
    workspace: { cwd: null, projectName: "proj" },
    activity: { startedAt: null, lastActivityAt: null },
    focusTarget: { kind: "process", pid: 1 },
    ...overrides,
  };
}

describe("AgentSessions", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("renders codex and claude provider labels from locale keys", async () => {
    api.listAgentSessions.mockResolvedValue({
      status: "hosts",
      hosts: [
        {
          host: "DESKTOP",
          sessions: [
            session({ id: "1", provider: "codex" }),
            session({ id: "2", provider: "claude" }),
          ],
          error: null,
        },
      ],
    });
    render(<AgentSessions />);

    expect(await screen.findByText("ProviderNameCodex")).toBeTruthy();
    expect(await screen.findByText("ProviderNameClaude")).toBeTruthy();
  });

  it("labels pi-family sessions by dialect and prefers sessionName", async () => {
    api.listAgentSessions.mockResolvedValue({
      status: "hosts",
      hosts: [
        {
          host: "DESKTOP",
          sessions: [
            session({ id: "pid:7", provider: "pi", dialect: "pi" }),
            session({
              id: "omp-fx",
              provider: "pi",
              dialect: "omp",
              sessionName: "OMP fixture",
            }),
          ],
          error: null,
        },
      ],
    });
    render(<AgentSessions />);

    expect(await screen.findByText("AgentSessionsProviderPi")).toBeTruthy();
    expect(await screen.findByText("AgentSessionsProviderOmp")).toBeTruthy();
    // sessionName shown when present; project fallback otherwise.
    const ompName = await screen.findByText("OMP fixture");
    expect(ompName).toBeTruthy();
    expect(ompName.getAttribute("title")).toBe("OMP fixture");
    expect(ompName.className).toContain("agent-sessions__name");
    expect(
      (await screen.findAllByText("proj")).length > 0,
    ).toBeTruthy();
  });

  it("defaults bare pi sessions (no dialect field emitted) to the family label", async () => {
    api.listAgentSessions.mockResolvedValue({
      status: "hosts",
      hosts: [
        {
          host: "DESKTOP",
          sessions: [session({ id: "pid:9", provider: "pi" })],
          error: null,
        },
      ],
    });
    render(<AgentSessions />);

    expect(await screen.findByText("AgentSessionsProviderPi")).toBeTruthy();
  });

  it("groups sessions under one header per host with its session count", async () => {
    api.listAgentSessions.mockResolvedValue({
      status: "hosts",
      hosts: [
        {
          host: "DESKTOP",
          sessions: [
            session({ id: "1", host: "DESKTOP", workspace: { cwd: null, projectName: "alpha" } }),
            session({ id: "2", host: "DESKTOP", workspace: { cwd: null, projectName: "beta" } }),
          ],
          error: null,
        },
        {
          host: "LAPTOP",
          sessions: [session({ id: "3", host: "LAPTOP", workspace: { cwd: null, projectName: "gamma" } })],
          error: null,
        },
      ],
    });
    render(<AgentSessions />);

    const desktopHeader = await screen.findByText("DESKTOP — 2");
    const laptopHeader = await screen.findByText("LAPTOP — 1");
    const alpha = screen.getByText("alpha");
    const gamma = screen.getByText("gamma");
    // Each host's rows follow its own header, and the hosts stay in order.
    expect(desktopHeader.compareDocumentPosition(alpha) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(alpha.compareDocumentPosition(laptopHeader) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(laptopHeader.compareDocumentPosition(gamma) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("shows an unreachable host as its own row with the error", async () => {
    api.listAgentSessions.mockResolvedValue({
      status: "hosts",
      hosts: [
        {
          host: "DESKTOP",
          sessions: [session({ id: "1", workspace: { cwd: null, projectName: "alpha" } })],
          error: null,
        },
        { host: "BUILD-BOX", sessions: [], error: "ssh: connect timed out" },
      ],
    });
    render(<AgentSessions />);

    expect(await screen.findByText("BUILD-BOX — AgentSessionsHostUnreachable")).toBeTruthy();
    expect(screen.getByText("ssh: connect timed out").className).toContain("agent-sessions__error");
    expect(screen.queryByText("BUILD-BOX — 0")).toBeNull();
    expect(screen.getByText("DESKTOP — 1")).toBeTruthy();
  });

  it("keeps a reachable host with no sessions visible as a zero-count header", async () => {
    api.listAgentSessions.mockResolvedValue({
      status: "hosts",
      hosts: [{ host: "IDLE-BOX", sessions: [], error: null }],
    });
    render(<AgentSessions />);

    expect(await screen.findByText("IDLE-BOX — 0")).toBeTruthy();
    expect(screen.getByText("AgentSessionsEmpty")).toBeTruthy();
  });
});
