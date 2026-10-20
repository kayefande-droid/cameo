import { beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invokeMock(...args) }));

import { addMemory, deleteMemory, listMemory, sanitizeMemoryText, updateMemory } from "../src/services/memory";

beforeEach(() => {
  invokeMock.mockReset();
});

describe("project memory service", () => {
  it("redacts credential-like values before storage", () => {
    const sanitized = sanitizeMemoryText("verified fix using api_key=super-secret-value-123 and ghp_abcdefghijklmnopqrstuvwxyz012345");
    expect(sanitized).not.toContain("super-secret-value");
    expect(sanitized).not.toContain("ghp_abcdefghijklmnopqrstuvwxyz012345");
    expect(sanitized).toContain("[REDACTED]");
  });

  it("adds memory with a redacted body through the backend command", async () => {
    invokeMock.mockResolvedValue({
      id: "mem-1", kind: "decision", title: "Use npm test", body: "verified [REDACTED]", scope: "project", status: "active", created_at: 1, updated_at: 1,
    });
    const entry = await addMemory({ kind: "decision", title: "Use npm test", body: "verified using api_key=super-secret-value-123" });
    const [command, payload] = invokeMock.mock.calls[0] as [string, { input: { kind: string; title: string; body: string } }];
    expect(command).toBe("memory_add");
    expect(payload.input.body).not.toContain("super-secret-value");
    expect(entry.id).toBe("mem-1");
    expect(entry.scope).toBe("project");
  });

  it("rejects unknown kinds without touching the backend", async () => {
    await expect(addMemory({ kind: "secret" as never, title: "t", body: "b" })).rejects.toThrow(/Unknown memory kind/);
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("lists memory rows mapped to typed entries and survives backend failure", async () => {
    invokeMock.mockResolvedValueOnce([{ id: "a", kind: "fact", title: "T", body: "B", scope: "project", status: "superseded", created_at: 5, updated_at: 6 }]);
    expect(await listMemory()).toEqual([{ id: "a", kind: "fact", title: "T", body: "B", scope: "project", status: "superseded", createdAt: 5, updatedAt: 6 }]);
    invokeMock.mockRejectedValueOnce(new Error("backend down"));
    expect(await listMemory()).toEqual([]);
  });

  it("updates and deletes explicit entries", async () => {
    invokeMock.mockResolvedValueOnce({ id: "a", kind: "fact", title: "New", body: "B", scope: "project", status: "superseded", created_at: 1, updated_at: 2 });
    await updateMemory("a", { title: "New", status: "superseded" });
    const [command, payload] = invokeMock.mock.calls[0] as [string, { id: string; title?: string; status?: string; body?: string }];
    expect(command).toBe("memory_update");
    expect(payload.id).toBe("a");
    expect(payload.title).toBe("New");
    expect(payload.body).toBeUndefined();

    invokeMock.mockResolvedValueOnce(true);
    expect(await deleteMemory("a")).toBe(true);
    expect(invokeMock).toHaveBeenLastCalledWith("memory_delete", { id: "a" });
  });
});
