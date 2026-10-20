import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useState } from "react";

const mocks = vi.hoisted(() => ({
  reviewRuns: [] as any[],
  getPref: vi.fn(),
  getTaskEvents: vi.fn(),
  listAgentRuns: vi.fn(),
  createAgentRun: vi.fn(),
  startAgentRun: vi.fn(),
  finishAgentRun: vi.fn(),
  cancelAgentRun: vi.fn(),
  getTaskProjectContext: vi.fn(),
  listFileProposals: vi.fn(),
  proposeFiles: vi.fn(),
  rejectFileProposal: vi.fn(),
  applyFileProposal: vi.fn(),
  upsertTask: vi.fn(),
  addTaskEvent: vi.fn(),
  streamChat: vi.fn(),
}));

vi.mock("../src/services/store", () => mocks);
vi.mock("../src/services/ai", () => ({
  streamChat: mocks.streamChat,
  cancelChat: vi.fn().mockResolvedValue(true),
  newRequestId: vi.fn(() => `request-${Math.random()}`),
}));

import TaskExecutionWindow from "../src/components/TaskExecutionWindow";
import type { TaskRecord } from "../src/types/tasks";

const initialTask: TaskRecord = {
  id: "task-test",
  title: "Add a focused feature",
  description: "Keep changes scoped and reviewable.",
  agent: "atlas",
  status: "queued",
  currentStep: "Ready",
  steps: [],
  changedFiles: [],
  logs: [],
  errors: [],
  projectRoot: "C:/workspace",
  createdAt: 1,
  updatedAt: 1,
};

const proposal = {
  id: "proposal-1",
  task_id: initialTask.id,
  path: "src/feature.ts",
  original_contents: "export const oldValue = 1;",
  contents: "export const newValue = 2;",
  status: "pending",
  base_sha256: "base-hash",
  backup_path: null,
};

function Harness() {
  const [task, setTask] = useState(initialTask);
  return (
    <TaskExecutionWindow
      task={task}
      onRefresh={() => {
        const latest = mocks.upsertTask.mock.calls.at(-1)?.[0] as TaskRecord | undefined;
        if (latest) setTask(latest);
      }}
    />
  );
}

afterEach(() => cleanup());
beforeEach(() => {
  vi.clearAllMocks();
  mocks.reviewRuns.length = 0;
  mocks.getPref.mockResolvedValue(JSON.stringify({ provider: "openrouter", model: "vendor/model-x" }));
  mocks.getTaskEvents.mockResolvedValue([]);
  mocks.listAgentRuns.mockImplementation(async () => mocks.reviewRuns.map((run) => ({ ...run })));
  mocks.createAgentRun.mockImplementation(async (input) => {
    const run = { id: `review-${mocks.reviewRuns.length + 1}`, taskId: input.taskId, agent: input.agent, provider: input.provider, model: input.model, status: "queued", attempt: 0, result: null, error: null, createdAt: Date.now(), updatedAt: Date.now() };
    mocks.reviewRuns.push(run);
    return { ...run };
  });
  mocks.startAgentRun.mockImplementation(async (id) => {
    const run = mocks.reviewRuns.find((item) => item.id === id);
    if (!run || run.attempt >= 2 || mocks.reviewRuns.filter((item) => item.status === "running").length >= 2) return null;
    run.status = "running";
    run.attempt += 1;
    return run.attempt;
  });
  mocks.finishAgentRun.mockImplementation(async (id, _attempt, status, result, error) => {
    const run = mocks.reviewRuns.find((item) => item.id === id);
    if (run) Object.assign(run, { status, result, error, updatedAt: Date.now() });
  });
  mocks.cancelAgentRun.mockImplementation(async (id) => {
    const run = mocks.reviewRuns.find((item) => item.id === id);
    if (run && ["queued", "running"].includes(run.status)) run.status = "cancelled";
  });
  mocks.getTaskProjectContext.mockResolvedValue([{ path: "src/feature.ts", contents: "export const oldValue = 1;" }]);
  mocks.listFileProposals.mockResolvedValue([]);
  mocks.proposeFiles.mockImplementation(async () => {
    mocks.listFileProposals.mockResolvedValue([proposal]);
    return [proposal];
  });
  mocks.applyFileProposal.mockResolvedValue({ ...proposal, status: "applied", backup_path: "backup.bak" });
  mocks.upsertTask.mockResolvedValue(undefined);
  mocks.addTaskEvent.mockResolvedValue(undefined);
  mocks.streamChat.mockImplementation(async (_provider, _model, messages, _opts, onEvent) => {
    const userPrompt = messages.at(-1)?.content ?? "";
    const text = userPrompt.startsWith("Approved task:")
      ? JSON.stringify({ summary: "Update the feature value.", files: [{ path: proposal.path, contents: proposal.contents }] })        : messages[0]?.content?.includes("read-only CAMEO review worker")
          ? "Review finding."
          : "1. Inspect the relevant module\n2. Prepare a scoped change\n3. Verify the result";
    onEvent({ type: "delta", text });
  });
});

describe("specialist task execution", () => {
  it("routes planning to the selected specialist and waits for per-file approval before writing", async () => {
    render(<Harness />);

    fireEvent.click(screen.getByRole("button", { name: "Plan with ATLAS" }));
    await screen.findByRole("button", { name: "Approve plan & prepare ATLAS proposal" });

    const planCall = mocks.streamChat.mock.calls[0];
    expect(planCall[2][0].content).toContain("ATLAS (Architect)");
    expect(planCall[2][1].content).toContain("Add a focused feature");
    expect(planCall[2][0].content).toContain("Workflow gates for this specialist");
    expect(planCall[2][0].content).toContain("plan-acceptance-criteria:");
    expect(mocks.addTaskEvent.mock.calls.some(([, kind, message]) => kind === "capabilities_applied" && typeof message === "string" && message.includes("plan-acceptance-criteria"))).toBe(true);

    fireEvent.click(screen.getByRole("button", { name: "Approve plan & prepare ATLAS proposal" }));
    await screen.findByText("File change proposals — preview only");
    expect(screen.getByText("src/feature.ts")).toBeTruthy();
    expect(mocks.applyFileProposal).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Approve this file write…" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog.textContent).toContain("src/feature.ts");
    expect(mocks.applyFileProposal).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Approve this file" }));
    await waitFor(() => expect(mocks.applyFileProposal).toHaveBeenCalledWith("proposal-1"));
  });

  it("launches all three persisted review workers and displays their results without writing proposals", async () => {
    render(<Harness />);
    fireEvent.click(await screen.findByRole("button", { name: "Run 3-agent review" }));
    await waitFor(() => expect(screen.getAllByText("Review finding.")).toHaveLength(3));
    await waitFor(() => expect(mocks.reviewRuns).toHaveLength(3));
    expect(mocks.reviewRuns.map((run) => run.status)).toEqual(["completed", "completed", "completed"]);
    expect(mocks.streamChat).toHaveBeenCalledTimes(3);
    expect(screen.getByText(/Read-only: no commands or file writes/)).toBeTruthy();
    expect(screen.getByText(/task description and project snapshot to the provider shown for that run/)).toBeTruthy();
    expect(mocks.applyFileProposal).not.toHaveBeenCalled();
    expect(mocks.proposeFiles).not.toHaveBeenCalled();
  });

  it("cancels running review workers without leaving retry controls locked", async () => {
    let unblock!: () => void;
    const blocked = new Promise<void>((resolve) => { unblock = resolve; });
    mocks.streamChat.mockImplementation(async (_provider, _model, _messages, _opts, onEvent) => {
      await blocked;
      onEvent({ type: "delta", text: "Late review result." });
    });
    render(<Harness />);
    fireEvent.click(await screen.findByRole("button", { name: "Run 3-agent review" }));
    await waitFor(() => expect(mocks.reviewRuns.some((run) => run.status === "running")).toBe(true));
    fireEvent.click(screen.getByRole("button", { name: "Cancel review" }));
    await waitFor(() => expect(mocks.reviewRuns.every((run) => run.status === "cancelled")).toBe(true));
    expect(mocks.finishAgentRun.mock.calls.every((call) => call[2] === "cancelled")).toBe(true);
    expect(mocks.applyFileProposal).not.toHaveBeenCalled();
    unblock();
  });

  it("shows the read-only parallel review entry and does not apply file proposals", async () => {
    render(<Harness />);
    const reviewButton = await screen.findByRole("button", { name: "Run 3-agent review" });
    expect(screen.getByText(/Read-only: no commands or file writes/)).toBeTruthy();
    expect(reviewButton).toBeTruthy();
    expect(mocks.applyFileProposal).not.toHaveBeenCalled();
    expect(mocks.proposeFiles).not.toHaveBeenCalled();
  });
});
