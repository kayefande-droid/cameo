import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  streamChat: vi.fn(),
  cancelChat: vi.fn().mockResolvedValue(true),
  tasks: [] as Array<Record<string, unknown>>,
  selection: JSON.stringify({ provider: "openrouter", model: "test/model" }),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    onScaleChanged: vi.fn().mockResolvedValue(() => undefined),
    onFocusChanged: vi.fn().mockResolvedValue(() => undefined),
    setAlwaysOnTop: vi.fn().mockResolvedValue(undefined),
    setFocus: vi.fn().mockResolvedValue(undefined),
    show: vi.fn().mockResolvedValue(undefined),
    startDragging: vi.fn().mockResolvedValue(undefined),
  }),
}));
vi.mock("../src/services/store", () => ({
  getPref: vi.fn(async (key: string) => key === "model_selection" ? mocks.selection : null),
  setPref: vi.fn().mockResolvedValue(undefined),
  listTasks: vi.fn(async () => mocks.tasks),
  upsertTask: vi.fn().mockResolvedValue(undefined),
  addTaskEvent: vi.fn().mockResolvedValue(undefined),
  getTaskEvents: vi.fn().mockResolvedValue([]),
  deleteTask: vi.fn().mockResolvedValue(undefined),
}));
vi.mock("../src/services/ai", () => ({
  deleteKey: vi.fn().mockResolvedValue(undefined),
  hasKey: vi.fn().mockResolvedValue(false),
  listModels: vi.fn().mockResolvedValue([]),
  saveKey: vi.fn().mockResolvedValue(undefined),
  streamChat: mocks.streamChat,
  cancelChat: mocks.cancelChat,
  testConnection: vi.fn().mockResolvedValue({ ok: true, message: "ok" }),
  newRequestId: vi.fn(() => "test-request"),
}));

import App from "../src/App";

afterEach(() => cleanup());
beforeEach(() => {
  mocks.invoke.mockClear();
  mocks.streamChat.mockReset();
  mocks.cancelChat.mockClear();
  mocks.tasks = [];
  mocks.selection = JSON.stringify({ provider: "openrouter", model: "test/model" });
});

describe("floating CAMEO assistant", () => {
  it("opens a compact panel below the dock and dismisses it without closing the app", async () => {
    const { container } = render(<App />);
    expect(await screen.findByRole("button", { name: "Open CAMEO assistant panel" })).toBeTruthy();
    expect(container.querySelector(".app-shell.dock")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Open CAMEO assistant panel" }));
    expect(await screen.findByRole("textbox", { name: "Chat input" })).toBeTruthy();
    expect(container.querySelector(".cameo-assistant-panel.is-expanded")).toBeTruthy();
    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("set_window_mode", { mode: "panel" }));

    fireEvent.click(screen.getByRole("button", { name: "Dismiss CAMEO assistant panel" }));
    await waitFor(() => expect(container.querySelector(".app-shell.dock")).toBeTruthy());
    expect(screen.getByRole("button", { name: "Open CAMEO assistant panel" })).toBeTruthy();
  });

  it("sends through the selected provider and streams the actual assistant response", async () => {
    mocks.streamChat.mockImplementation(async (...args: unknown[]) => {
      const onEvent = args[4] as (event: { type: string; text?: string }) => void;
      onEvent({ type: "delta", text: "Connected CAMEO response" });
      onEvent({ type: "done" });
    });
    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: "Open CAMEO assistant panel" }));
    const input = await screen.findByRole("textbox", { name: "Chat input" });
    fireEvent.change(input, { target: { value: "Hello from the dock" } });
    fireEvent.click(screen.getByRole("button", { name: "Send message to CAMEO" }));

    expect(await screen.findByText("Connected CAMEO response")).toBeTruthy();
    expect(mocks.streamChat).toHaveBeenCalledWith(
      "openrouter",
      "test/model",
      expect.arrayContaining([
        expect.objectContaining({ role: "user", content: "Hello from the dock" }),
      ]),
      {
        max_tokens: 2048,
        // Agent tools + safe auto-approve are on by default so the dock
        // assistant acts without a click per step (destructive actions
        // still pause for approval).
        agent_tools: true,
        agent_max_rounds: 4,
        agent_auto_approve_safe: true,
        agent_auto_approve_all: false,
      },
      expect.any(Function),
      "test-request",
    );
  });

  it("shows persisted approval/task activity for the assigned agent", async () => {
    mocks.tasks = [{
      id: "task-atlas", title: "Review architecture", description: "", agent: "atlas",
      status: "awaiting_approval", currentStep: "Review proposed plan", steps: [{ label: "Inspect", status: "pending" }],
      changedFiles: [], logs: [], errors: [], createdAt: Date.now(), updatedAt: Date.now(),
    }];
    const { container } = render(<App />);
    await waitFor(() => expect(container.querySelector(".dock-agent-button[title*='ATLAS · Review proposed plan'] .state-approval")).toBeTruthy());
    fireEvent.click(screen.getByRole("button", { name: "Open CAMEO assistant panel" }));
    expect(await screen.findByText("ATLAS · Review architecture")).toBeTruthy();
    expect(screen.getByText("Review proposed plan · 0/1 steps")).toBeTruthy();
  });

  it("wires Stop to the active chat request only", async () => {
    let resolveStream: (() => void) | undefined;
    mocks.streamChat.mockImplementation(() => new Promise<void>((resolve) => { resolveStream = resolve; }));
    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: "Open CAMEO assistant panel" }));
    fireEvent.change(await screen.findByRole("textbox", { name: "Chat input" }), { target: { value: "Long request" } });
    fireEvent.click(screen.getByRole("button", { name: "Send message to CAMEO" }));
    fireEvent.click(await screen.findByRole("button", { name: "Stop running response" }));
    await waitFor(() => expect(mocks.cancelChat).toHaveBeenCalledWith("test-request"));
    resolveStream?.();
  });
});
