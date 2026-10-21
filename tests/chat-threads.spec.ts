import { beforeEach, describe, expect, it } from "vitest";
import {
  createSessionMeta,
  deleteThread,
  deriveTitle,
  loadAgentThreads,
  persistIndex,
  persistThread,
} from "../src/services/chatThreads";

beforeEach(() => localStorage.clear());

function turn(id: string, role: "user" | "assistant", text: string) {
  return { id, role, who: role === "user" ? "You" : "CAMEO", text };
}

describe("saved chat sessions", () => {
  it("starts empty and persists threads per session", () => {
    const first = loadAgentThreads("cameo");
    expect(first.sessions).toHaveLength(1);
    expect(first.turns).toEqual([]);
    const id = first.activeId;
    persistThread(id, [turn("u1", "user", "remember this"), turn("a1", "assistant", "hi there")]);
    const second = loadAgentThreads("cameo");
    expect(second.activeId).toBe(id);
    expect(second.turns.map((t) => t.text)).toEqual(["remember this", "hi there"]);
  });

  it("derives titles from the first user message", () => {
    expect(deriveTitle([turn("u1", "user", "  "), turn("u2", "user", "Build me a dashboard for sales")])).toBe("Build me a dashboard for sales");
    expect(deriveTitle([])).toBe("New chat");
  });

  it("keeps agents isolated and supports multiple sessions", () => {
    const cameo = loadAgentThreads("cameo");
    persistThread(cameo.activeId, [turn("u1", "user", "cameo chat")]);
    const extra = createSessionMeta();
    persistThread(extra.id, [turn("u9", "user", "second thread")]);
    persistIndex("cameo", [extra, ...cameo.sessions], extra.id);
    const reloaded = loadAgentThreads("cameo");
    expect(reloaded.sessions).toHaveLength(2);
    expect(reloaded.activeId).toBe(extra.id);
    expect(reloaded.turns.map((t) => t.text)).toEqual(["second thread"]);
    // Other agents start clean.
    expect(loadAgentThreads("forge").turns).toEqual([]);
    deleteThread(extra.id);
  });

  it("migrates legacy single-thread keys into a session once", () => {
    localStorage.setItem("cameo.chat.v1.counsel", JSON.stringify([turn("u1", "user", "legacy hello")]));
    const loaded = loadAgentThreads("counsel");
    expect(loaded.turns.map((t) => t.text)).toEqual(["legacy hello"]);
    expect(loaded.sessions[0].title).toBe("legacy hello");
    expect(localStorage.getItem("cameo.chat.v1.counsel")).toBeNull();
  });
});
