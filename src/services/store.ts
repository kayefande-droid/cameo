import { invoke } from "@tauri-apps/api/core";
import type { AgentId, AgentRun, AgentRunStatus, TaskEvent, TaskRecord, TaskStatus } from "../types/tasks";

// ---------- prefs ----------

export async function getPref(key: string): Promise<string | null> {
  try {
    return (await invoke<string | null>("pref_get", { key })) ?? null;
  } catch {
    return null;
  }
}

export async function setPref(key: string, value: string) {
  return invoke("pref_set", { key, value });
}

// ---------- tasks (DB persisted) ----------

const TASK_TABLE_KEYS = {
  id: "id",
  title: "title",
  description: "description",
  agent: "agent",
  status: "status",
  current_step: "currentStep",
  steps_json: "steps",
  changed_files: "changedFiles",
  logs_json: "logs",
  approval_id: "approvalId",
  project_root: "projectRoot",
  created_at: "createdAt",
  updated_at: "updatedAt",
};

const ROW_TO_TASK = (row: Record<string, unknown>): TaskRecord => {
  const parse = (v: unknown, fallback: unknown) => {
    if (typeof v !== "string" || !v) return fallback;
    try {
      return JSON.parse(v);
    } catch {
      return fallback;
    }
  };
  return {
    id: row[TASK_TABLE_KEYS.id] as string,
    title: row.title as string,
    description: row.description as string,
    agent: row.agent as AgentId,
    status: row.status as TaskStatus,
    currentStep: row[TASK_TABLE_KEYS.current_step] as string,
    steps: parse(row[TASK_TABLE_KEYS.steps_json], []),
    changedFiles: parse(row.changed_files, []),
    logs: parse(row.logs_json, []),
    errors: [],
    approvalId: row.approval_id as string | undefined,
    projectRoot: row.project_root as string | undefined,
    createdAt: row.created_at as number,
    updatedAt: row.updated_at as number,
  };
};

export async function upsertTask(task: TaskRecord) {
  return invoke("task_upsert", {
    task: {
      id: task.id,
      title: task.title,
      description: task.description,
      agent: task.agent,
      status: task.status,
      current_step: task.currentStep,
      steps_json: JSON.stringify(task.steps),
      changed_files: JSON.stringify(task.changedFiles),
      logs_json: JSON.stringify(task.logs),
      approval_id: task.approvalId ?? null,
      project_root: task.projectRoot ?? null,
      created_at: task.createdAt,
      updated_at: task.updatedAt,
    },
  });
}

export async function listTasks(): Promise<TaskRecord[]> {
  try {
    const rows = await invoke<Record<string, unknown>[]>("task_list");
    return rows.map(ROW_TO_TASK);
  } catch {
    return [];
  }
}

export async function getTaskProjectContext(taskId: string): Promise<{ path: string; contents: string }[]> {
  return invoke("task_project_context", { taskId });
}

export async function getTaskEvents(taskId: string): Promise<TaskEvent[]> {
  try {
    return await invoke<TaskEvent[]>("task_events", { taskId });
  } catch {
    return [];
  }
}

export async function addTaskEvent(taskId: string, kind: string, message: string) {
  return invoke("task_event_add", { taskId, kind, message });
}

const ROW_TO_AGENT_RUN = (row: Record<string, unknown>): AgentRun => ({
  id: row.id as string,
  taskId: row.task_id as string,
  agent: row.agent as AgentId,
  provider: row.provider as string,
  model: row.model as string,
  status: row.status as AgentRunStatus,
  attempt: row.attempt as number,
  result: row.result as string | null,
  error: row.error as string | null,
  createdAt: row.created_at as number,
  updatedAt: row.updated_at as number,
});

export async function createAgentRun(input: { taskId: string; agent: AgentId; provider: string; model: string }): Promise<AgentRun> {
  const row = await invoke<Record<string, unknown>>("agent_run_create", {
    input: { task_id: input.taskId, agent: input.agent, provider: input.provider, model: input.model },
  });
  return ROW_TO_AGENT_RUN(row);
}

export async function listAgentRuns(taskId: string): Promise<AgentRun[]> {
  return (await invoke<Record<string, unknown>[]>("agent_run_list", { taskId })).map(ROW_TO_AGENT_RUN);
}

export async function startAgentRun(runId: string): Promise<number | null> {
  return invoke<number | null>("agent_run_start", { runId });
}

export async function finishAgentRun(runId: string, attempt: number, status: AgentRunStatus, result: string | null, error: string | null) {
  return invoke("agent_run_finish", { runId, attempt, status, result, error });
}

export async function cancelAgentRun(runId: string, attempt?: number) {
  return invoke("agent_run_cancel", { runId, attempt: attempt ?? null });
}

export async function recoverExpiredAgentRuns(taskId: string): Promise<number> {
  return invoke<number>("agent_run_recover_expired", { taskId });
}

export async function deleteTask(taskId: string) {
  return invoke("task_delete", { taskId });
}

export interface FileProposal {
  id: string;
  task_id: string;
  path: string;
  original_contents: string | null;
  contents: string;
  status: string;
  base_sha256: string | null;
  backup_path: string | null;
}

export async function proposeFiles(taskId: string, files: { path: string; contents: string }[]) {
  return invoke<FileProposal[]>("task_propose_files", { taskId, files });
}

export async function listFileProposals(taskId: string) {
  return invoke<FileProposal[]>("task_file_proposals", { taskId });
}

export async function rejectFileProposal(proposalId: string) {
  return invoke("task_reject_proposal", { proposalId });
}

export async function applyFileProposal(proposalId: string) {
  return invoke<FileProposal>("task_apply_proposal", { proposalId, confirmed: true });
}
