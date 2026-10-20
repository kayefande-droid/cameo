export type TaskStatus =
  | "queued"
  | "planning"
  | "awaiting_approval"
  | "awaiting_verification"
  | "running"
  | "verifying"
  | "completed"
  | "partially_completed"
  | "failed"
  | "cancelled";

export type AgentId = "cameo" | "atlas" | "forge" | "pixel" | "sentinel";

export interface AgentMeta {
  id: AgentId;
  codename: string;
  role: string;
  description: string;
}

export const AGENTS: AgentMeta[] = [
  {
    id: "cameo",
    codename: "CAMEO",
    role: "Chief of Staff",
    description: "Understands requests, plans tasks, assigns specialists and coordinates execution.",
  },
  {
    id: "atlas",
    codename: "ATLAS",
    role: "Architect",
    description: "Inspects codebases and architecture, plans implementation, flags risks.",
  },
  {
    id: "forge",
    codename: "FORGE",
    role: "Implementation Engineer",
    description: "Implements features, runs builds/tests and repairs errors in your project.",
  },
  {
    id: "pixel",
    codename: "PIXEL",
    role: "UI/UX Designer",
    description: "Design systems, layout review and accessibility feedback.",
  },
  {
    id: "sentinel",
    codename: "SENTINEL",
    role: "Security & QA",
    description: "Reviews changes for vulnerabilities, validates permissions and tests.",
  },
];

export interface TaskStep {
  label: string;
  status: TaskStatus | "pending" | "done";
}

export interface TaskRecord {
  id: string;
  title: string;
  description: string;
  agent: AgentId;
  status: TaskStatus;
  currentStep: string;
  steps: TaskStep[];
  changedFiles: string[];
  logs: { ts: number; message: string }[];
  errors: string[];
  approvalId?: string;
  projectRoot?: string;
  createdAt: number;
  updatedAt: number;
}

export interface TaskEvent {
  id?: number;
  task_id: string;
  ts: number;
  kind: string;
  message: string;
}

export type AgentRunStatus = "queued" | "running" | "completed" | "failed" | "cancelled";

export interface AgentRun {
  id: string;
  taskId: string;
  agent: AgentId;
  provider: string;
  model: string;
  status: AgentRunStatus;
  attempt: number;
  result: string | null;
  error: string | null;
  createdAt: number;
  updatedAt: number;
}

export const TRANSITIONS: Record<TaskStatus, TaskStatus[]> = {
  queued: ["planning", "awaiting_approval", "running", "cancelled"],
  planning: ["awaiting_approval", "running", "failed", "cancelled"],
  awaiting_approval: ["running", "awaiting_verification", "cancelled", "failed"],
  awaiting_verification: ["completed", "failed", "running", "cancelled"],
  running: ["verifying", "completed", "failed", "cancelled", "planning"],
  verifying: ["completed", "partially_completed", "awaiting_verification", "failed", "running"],
  completed: [],
  partially_completed: ["awaiting_verification", "cancelled"],
  failed: ["queued"],
  cancelled: ["queued"],
};

export function canTransition(from: TaskStatus, to: TaskStatus): boolean {
  return TRANSITIONS[from]?.includes(to) ?? false;
}

export function statusLabel(s: TaskStatus): string {
  return s.replace(/_/g, " ");
}
