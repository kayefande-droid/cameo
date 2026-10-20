import { beforeEach, describe, expect, it, vi } from "vitest";
import { buildReviewMessages, mapWithConcurrency, MAX_PARALLEL_REVIEW_WORKERS, redactLikelySecrets, resolveReviewModelSelection, retryReviewRun, runParallelReview } from "../src/services/orchestration";
import { streamChat } from "../src/services/ai";
import type { AgentRun, TaskRecord } from "../src/types/tasks";

const mocks = vi.hoisted(() => ({
  runs: [] as any[],
  getPref: vi.fn(),
  activeRequests: 0,
  peakRequests: 0,
  getTaskProjectContext: vi.fn(),
  createAgentRun: vi.fn(),
  listAgentRuns: vi.fn(),
  startAgentRun: vi.fn(),
  finishAgentRun: vi.fn(),
  cancelAgentRun: vi.fn(),
  recoverExpiredAgentRuns: vi.fn(),
  streamChat: vi.fn(),
}));

vi.mock("../src/services/store", () => ({
  getPref: mocks.getPref,
  getTaskProjectContext: mocks.getTaskProjectContext,
  createAgentRun: mocks.createAgentRun,
  listAgentRuns: mocks.listAgentRuns,
  startAgentRun: mocks.startAgentRun,
  finishAgentRun: mocks.finishAgentRun,
  cancelAgentRun: mocks.cancelAgentRun,
  recoverExpiredAgentRuns: mocks.recoverExpiredAgentRuns,
}));
vi.mock("../src/services/ai", () => ({
  cancelChat: vi.fn().mockResolvedValue(true),
  newRequestId: vi.fn(() => "request-test"),
  streamChat: mocks.streamChat,
}));

const task: TaskRecord = {
  id: "task-review",
  title: "Review changes",
  description: "Check safety and correctness.",
  agent: "cameo",
  status: "queued",
  currentStep: "Review",
  steps: [],
  changedFiles: [],
  logs: [],
  errors: [],
  projectRoot: "C:/repo",
  createdAt: 1,
  updatedAt: 1,
};

const run: AgentRun = {
  id: "run-1",
  taskId: task.id,
  agent: "sentinel",
  provider: "openrouter",
  model: "test/model",
  status: "queued",
  attempt: 0,
  result: null,
  error: null,
  createdAt: 1,
  updatedAt: 1,
};

const settledReviewRun: AgentRun = { ...run, status: "failed", attempt: 1 };

describe("CAMEO parallel review orchestration", () => {
  beforeEach(() => {
    mocks.runs.length = 0;
    mocks.activeRequests = 0;
    mocks.peakRequests = 0;
    vi.clearAllMocks();
    mocks.getPref.mockImplementation(async (key) => key === "model_selection" ? JSON.stringify({ provider: "openrouter", model: "vendor/model-x" }) : null);
    mocks.getTaskProjectContext.mockResolvedValue([{ path: "src/main.ts", contents: "export const value = 1;" }]);
    mocks.listAgentRuns.mockImplementation(async () => mocks.runs.map((item) => ({ ...item })));
    mocks.createAgentRun.mockImplementation(async (input) => {
      const created = { ...run, ...input, id: `run-${mocks.runs.length + 1}`, status: "queued", attempt: 0 };
      mocks.runs.push(created);
      return { ...created };
    });
    mocks.startAgentRun.mockImplementation(async (id) => {
      const current = mocks.runs.find((item) => item.id === id);
      if (!current || current.attempt >= 2 || mocks.runs.filter((item) => item.status === "running").length >= 2) return null;
      current.status = "running";
      current.attempt += 1;
      return current.attempt;
    });
    mocks.finishAgentRun.mockImplementation(async (id, attempt, status, result, error) => {
      const current = mocks.runs.find((item) => item.id === id);
      if (!current || current.status !== "running" || current.attempt !== attempt) throw new Error("agent run is not active");
      Object.assign(current, { status, result, error });
    });
    mocks.cancelAgentRun.mockImplementation(async (id) => {
      const current = mocks.runs.find((item) => item.id === id);
      if (current && ["queued", "running"].includes(current.status)) current.status = "cancelled";
    });
    mocks.recoverExpiredAgentRuns.mockResolvedValue(0);
    mocks.streamChat.mockImplementation(async (_provider, _model, _messages, _opts, onEvent) => {
      mocks.activeRequests += 1;
      mocks.peakRequests = Math.max(mocks.peakRequests, mocks.activeRequests);
      await new Promise((resolve) => setTimeout(resolve, 5));
      onEvent({ type: "delta", text: "Review finding." });
      mocks.activeRequests -= 1;
    });
  });

  it("runs and persists three specialist reviews through the selected CAMEO provider within the global cap", async () => {
    await runParallelReview(task, new AbortController().signal);
    expect(mocks.runs.map((item) => item.status)).toEqual(["completed", "completed", "completed"]);
    expect(mocks.streamChat).toHaveBeenCalledTimes(3);
    expect(mocks.streamChat.mock.calls.every(([provider, model]) => provider === "openrouter" && model === "vendor/model-x")).toBe(true);
    expect(mocks.peakRequests).toBe(MAX_PARALLEL_REVIEW_WORKERS);
  });

  it("uses a configured review model independently from the primary model", async () => {
    mocks.getPref.mockImplementation(async (key) => key === "model_selection"
      ? JSON.stringify({ provider: "openrouter", model: "primary/model" })
      : key === "review_model_selection"
        ? JSON.stringify({ provider: "ollama", model: "local-review" })
        : null);
    await expect(resolveReviewModelSelection()).resolves.toEqual({ provider: "ollama", model: "local-review" });
    await runParallelReview(task, new AbortController().signal);
    expect(mocks.runs.map(({ provider, model }) => [provider, model])).toEqual(Array(3).fill(["ollama", "local-review"]));
    expect(mocks.streamChat.mock.calls.every(([provider, model]) => provider === "ollama" && model === "local-review")).toBe(true);
  });

  it("falls back to the primary model when review routing is unset or explicitly reset", async () => {
    mocks.getPref.mockImplementation(async (key) => key === "model_selection"
      ? JSON.stringify({ provider: "nvidia", model: "primary/model" })
      : key === "review_model_selection" ? "primary" : null);
    await expect(resolveReviewModelSelection()).resolves.toEqual({ provider: "nvidia", model: "primary/model" });
  });

  it("rejects an invalid saved review provider before making requests", async () => {
    mocks.getPref.mockImplementation(async (key) => key === "review_model_selection"
      ? JSON.stringify({ provider: "invalid", model: "bad" }) : null);
    await expect(resolveReviewModelSelection()).rejects.toThrow("incomplete");
    expect(mocks.streamChat).not.toHaveBeenCalled();
  });

  it("persists cancellation and ignores late model output instead of completing a cancelled run", async () => {
    let signalStarted!: () => void;
    const started = new Promise<void>((resolve) => { signalStarted = resolve; });
    mocks.streamChat.mockImplementation(async (_provider, _model, _messages, _opts, onEvent) => {
      mocks.activeRequests += 1;
      signalStarted();
      await new Promise((resolve) => setTimeout(resolve, 25));
      onEvent({ type: "delta", text: "Late response must not complete the run." });
      mocks.activeRequests -= 1;
    });
    const controller = new AbortController();
    const work = runParallelReview(task, controller.signal);
    await started;
    controller.abort();
    await work;
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(mocks.runs.map((item) => item.status)).toEqual(["cancelled", "cancelled", "cancelled"]);
    expect(mocks.runs.every((item) => item.result === null)).toBe(true);
  });

  it("retries a saved worker through its recorded provider/model, not today's settings", async () => {
    mocks.getPref.mockImplementation(async (key) => key === "review_model_selection"
      ? JSON.stringify({ provider: "nvidia", model: "new-review-model" })
      : null);
    mocks.runs.push({ ...settledReviewRun });
    await retryReviewRun(settledReviewRun, task, new AbortController().signal);
    expect(mocks.streamChat).toHaveBeenCalledTimes(1);
    expect(mocks.streamChat.mock.calls[0][0]).toBe("openrouter");
    expect(mocks.streamChat.mock.calls[0][1]).toBe("test/model");
  });

  it("does not start another request after the persisted attempt limit", async () => {
    await expect(retryReviewRun({ ...run, status: "failed", attempt: 2 }, task, new AbortController().signal))
      .rejects.toThrow("two-attempt limit");
    expect(streamChat).not.toHaveBeenCalled();
  });

  it("enforces a fixed maximum of two concurrent workers", async () => {
    let active = 0;
    let peak = 0;
    const result = await mapWithConcurrency([1, 2, 3, 4, 5], MAX_PARALLEL_REVIEW_WORKERS, async (value) => {
      active += 1;
      peak = Math.max(peak, active);
      await new Promise((resolve) => setTimeout(resolve, 5));
      active -= 1;
      return value * 2;
    });
    expect(result).toEqual([2, 4, 6, 8, 10]);
    expect(peak).toBe(2);
  });

  it("rejects an invalid concurrency limit", async () => {
    await expect(mapWithConcurrency([1], 0, async (n) => n)).rejects.toThrow("positive integer");
  });

  it("redacts credential-like values from review content", () => {
    const redacted = redactLikelySecrets("api_key=super-secret-value-123456 and token ghp_abcdefghijklmnopqrstuvwxyz012345");
    expect(redacted).not.toContain("super-secret-value");
    expect(redacted).not.toContain("ghp_abcdefghijklmnopqrstuvwxyz012345");
    expect(redacted).toContain("[REDACTED]");
  });

  it("builds a read-only, role-specific prompt with untrusted-source boundaries", () => {
    const [system, user] = buildReviewMessages(run, task, "--- src/a.ts ---\nconst x = 1;");
    expect(system.content).toContain("SENTINEL");
    expect(system.content).toContain("read-only");
    expect(system.content).toContain("untrusted data");
    expect(user.content).toContain("Review only; do not propose file writes or commands.");
    expect(user.content).toContain("src/a.ts");
    expect(user.content).not.toContain("secret-value");
  });
});
