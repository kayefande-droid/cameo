import { beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
  Channel: class {
    onmessage: ((value: unknown) => void) | null = null;
  },
}));

import { streamChat } from "../src/services/ai";

beforeEach(() => invokeMock.mockReset().mockResolvedValue(undefined));

describe("streamChat request model", () => {
  it("passes the selected model under the Tauri chat command model argument", async () => {
    await streamChat("openrouter", "vendor/model-x", [{ role: "user", content: "hello" }], {}, () => undefined);
    expect(invokeMock).toHaveBeenCalledOnce();
    const [command, payload] = invokeMock.mock.calls[0] as [string, Record<string, unknown>];
    expect(command).toBe("chat");
    expect(payload.model).toBe("vendor/model-x");
    expect(payload.provider).toBe("openrouter");
  });

  it("passes a request timeout in milliseconds to the native command", async () => {
    await streamChat("openrouter", "vendor/model-x", [{ role: "user", content: "hello" }], {}, () => undefined, "review-request", 120_000);
    const payload = invokeMock.mock.calls[0][1] as { opts: { timeout_ms?: number } };
    expect(payload.opts.timeout_ms).toBe(120_000);
  });

  it("rejects an empty model before invoking native networking", async () => {
    await expect(
      streamChat("openrouter", "  ", [{ role: "user", content: "hello" }], {}, () => undefined)
    ).rejects.toThrow(/choose a model/i);
    expect(invokeMock).not.toHaveBeenCalled();
  });
});
