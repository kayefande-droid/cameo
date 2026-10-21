import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ streamChat: vi.fn() }));

vi.mock("../src/services/ai", () => ({
  decideToolCall: vi.fn(async () => true),
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

import Chat from "../src/pages/Chat";

afterEach(() => cleanup());

describe("chat session persistence", () => {
  it("restores the saved thread on reopen and clears it with New chat", async () => {
    localStorage.clear();
    mocks.streamChat.mockReset();
    mocks.streamChat.mockImplementation(async (_p: unknown, _m: unknown, _h: unknown, _o: unknown, onEvent: (ev: Record<string, unknown>) => void) => {
      onEvent({ type: "delta", text: "hi there" });
      onEvent({ type: "done" });
    });

    const first = render(<Chat selection={{ provider: "openrouter", model: "test/model" }} onSelectionChange={() => {}} />);
    fireEvent.change(screen.getByRole("textbox", { name: "Chat input" }), { target: { value: "remember me" } });
    fireEvent.click(screen.getByRole("button", { name: "Send message to CAMEO" }));
    expect(await screen.findByText("hi there")).toBeTruthy();
    first.unmount();

    render(<Chat selection={{ provider: "openrouter", model: "test/model" }} onSelectionChange={() => {}} />);
    // Both sides of the saved thread survive the reopen: the message in the
    // transcript plus the auto-derived title in the saved-chats switcher.
    await waitFor(() => expect(screen.getAllByText(/remember me/)).toHaveLength(2));
    expect(await screen.findByText("hi there")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Start a new chat" }));
    // The transcript is a fresh empty session; the old thread stays saved
    // in the switcher under its auto-derived title.
    expect(document.querySelector(".msg.user")).toBeNull();
    expect(document.querySelector(".msg.assistant")).toBeNull();
    const switcher = screen.getByRole("combobox", { name: "Saved chat" });
    expect(within(switcher).getByRole("option", { name: "remember me" })).toBeTruthy();
    expect(within(switcher).getAllByRole("option")).toHaveLength(2);
  });
});
