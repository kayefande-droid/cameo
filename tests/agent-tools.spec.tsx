import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  streamChat: vi.fn(),
  decideToolCall: vi.fn(async () => true),
}));

vi.mock("../src/services/ai", () => ({
  decideToolCall: mocks.decideToolCall,
  deleteKey: vi.fn(async () => true),
  hasKey: vi.fn(async () => true),
  newRequestId: vi.fn(() => "test-request"),
  saveKey: vi.fn(async () => true),
  streamChat: mocks.streamChat,
  testConnection: vi.fn(async () => ({ ok: true, message: "ok" })),
  cancelChat: vi.fn(async () => true),
}));

vi.mock("../src/services/store", () => ({
  getPref: vi.fn(async () => null),
  setPref: vi.fn(async () => undefined),
}));

import Chat, { buildChatHistory } from "../src/pages/Chat";

beforeEach(() => localStorage.clear());
afterEach(() => cleanup());

describe("agent tool mode", () => {
  it("pauses a risky tool call for approval and forwards the decision", async () => {
    mocks.streamChat.mockReset();
    mocks.decideToolCall.mockClear();
    mocks.streamChat.mockImplementation(async (_p: unknown, _m: unknown, _h: unknown, _o: unknown, onEvent: (ev: Record<string, unknown>) => void) => {
      onEvent({
        type: "tool",
        approval_id: "req-1-t0-0",
        call_id: "call_1",
        name: "exec_run",
        arguments: { command: "rm", args: ["-rf", "dist"] },
        preview: "rm -rf dist",
        requires_approval: true,
      });
    });

    render(<Chat selection={{ provider: "openrouter", model: "test/model" }} onSelectionChange={() => {}} />);
    fireEvent.click(screen.getByRole("checkbox", { name: "Agent tool mode" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Chat input" }), { target: { value: "clean the build" } });
    fireEvent.click(screen.getByRole("button", { name: "Send message to CAMEO" }));

    expect(await screen.findByText("waiting for approval")).toBeTruthy();
    expect(screen.getByText("rm -rf dist")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Approve & run" }));
    await waitFor(() => expect(mocks.decideToolCall).toHaveBeenCalledWith("req-1-t0-0", true));

    // The request carries the agent-mode opts.
    expect(mocks.streamChat).toHaveBeenCalledWith(
      "openrouter",
      "test/model",
      expect.anything(),
      expect.objectContaining({ agent_tools: true, agent_max_rounds: 4 }),
      expect.any(Function),
      "test-request",
    );
  });

  it("shows thinking and process status while streaming", async () => {
    mocks.streamChat.mockReset();
    mocks.streamChat.mockImplementation(async (_p: unknown, _m: unknown, _h: unknown, _o: unknown, onEvent: (ev: Record<string, unknown>) => void) => {
      onEvent({ type: "status", message: "Contacting openrouter…" });
      onEvent({ type: "thinking", text: "let me think" });
      onEvent({ type: "delta", text: "hello" });
      onEvent({ type: "done" });
    });

    render(<Chat selection={{ provider: "openrouter", model: "test/model" }} onSelectionChange={() => {}} />);
    fireEvent.change(screen.getByRole("textbox", { name: "Chat input" }), { target: { value: "hi" } });
    fireEvent.click(screen.getByRole("button", { name: "Send message to CAMEO" }));

    expect(await screen.findByText(/Contacting openrouter/)).toBeTruthy();
    expect(await screen.findByText("hello")).toBeTruthy();
    // Thinking is visible twice (live process + finished turn's record) and
    // never dumped into the answer text itself.
    expect(await screen.findAllByText(/Thinking process/)).toHaveLength(2);
    expect(screen.getAllByText("let me think")).toHaveLength(2);
  });

  it("renders auto-run tool results without approval buttons", async () => {
    mocks.streamChat.mockReset();
    mocks.streamChat.mockImplementation(async (_p: unknown, _m: unknown, _h: unknown, _o: unknown, onEvent: (ev: Record<string, unknown>) => void) => {
      onEvent({
        type: "tool",
        approval_id: "req-2-t0-0",
        call_id: "call_1",
        name: "net_search",
        arguments: { query: "rust" },
        preview: "rust",
        requires_approval: false,
      });
      onEvent({
        type: "tool_result",
        approval_id: "req-2-t0-0",
        call_id: "call_1",
        name: "net_search",
        ok: true,
        output: "[exa] …",
      });
      onEvent({ type: "done" });
    });

    render(<Chat selection={{ provider: "openrouter", model: "test/model" }} onSelectionChange={() => {}} />);
    fireEvent.click(screen.getByRole("checkbox", { name: "Agent tool mode" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Chat input" }), { target: { value: "search" } });
    fireEvent.click(screen.getByRole("button", { name: "Send message to CAMEO" }));

    expect(await screen.findByText("✔ done")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Approve & run" })).toBeNull();
  });

  it("passes full auto-approval and wider rounds when autopilot is on", async () => {
    const { getPref } = await import("../src/services/store");
    vi.mocked(getPref).mockImplementation(async (key: string) => (key === "autopilot" ? "1" : null));
    try {
      mocks.streamChat.mockReset();
      mocks.streamChat.mockImplementation(async (_p: unknown, _m: unknown, _h: unknown, _o: unknown, onEvent: (ev: Record<string, unknown>) => void) => {
        onEvent({ type: "delta", text: "autopilot answer" });
        onEvent({ type: "done" });
      });
      render(<Chat selection={{ provider: "openrouter", model: "test/model" }} onSelectionChange={() => {}} />);
      // Autopilot loads async from prefs; wait until it engages before sending.
      await waitFor(() => expect((screen.getByRole("checkbox", { name: "Autopilot (hands-free)" }) as HTMLInputElement).checked).toBe(true));
      fireEvent.change(screen.getByRole("textbox", { name: "Chat input" }), { target: { value: "do it all" } });
      fireEvent.click(screen.getByRole("button", { name: "Send message to CAMEO" }));
      expect(await screen.findByText("autopilot answer")).toBeTruthy();
      expect(mocks.streamChat).toHaveBeenCalledWith(
        "openrouter",
        "test/model",
        expect.anything(),
        expect.objectContaining({ agent_tools: true, agent_max_rounds: 8, agent_auto_approve_all: true }),
        expect.any(Function),
        "test-request",
      );
    } finally {
      vi.mocked(getPref).mockImplementation(async () => null);
    }
  });
});

describe("buildChatHistory", () => {
  it("caps turns and characters so long sessions stay fast", () => {
    const turns = Array.from({ length: 60 }, (_, i) => ({
      id: `t${i}`,
      role: "assistant" as const,
      who: "CAMEO",
      text: `answer ${i} `.repeat(200),
    }));
    const history = buildChatHistory("system", turns, "new question");
    // system + capped tail + current input.
    expect(history[0]).toEqual({ role: "system", content: "system" });
    expect(history[history.length - 1]).toEqual({ role: "user", content: "new question" });
    expect(history.length).toBeLessThanOrEqual(32);
    const total = history.reduce((n, m) => n + m.content.length, 0);
    expect(total).toBeLessThanOrEqual(24_000 + "system".length + "new question".length);
    // Most recent answer survives (possibly truncated, never dropped).
    expect(history[history.length - 2].content).toContain("answer 59");
  });
});
