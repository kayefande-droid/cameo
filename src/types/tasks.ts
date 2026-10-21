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

export type AgentId =
  | "cameo"
  | "atlas"
  | "forge"
  | "pixel"
  | "sentinel"
  | "counsel"
  | "medic"
  | "ledger";

export interface AgentMeta {
  id: AgentId;
  codename: string;
  role: string;
  description: string;
  /** Persona + working rules injected as the system prompt when this agent is active. */
  prompt: string;
  /** Advisory specialists must not present themselves as licensed professionals. */
  advisory?: boolean;
}

const ADVISORY_BOUNDARY =
  "You provide sourced information and options, not professional advice. State clearly, once, that this is not a substitute for a licensed professional, and recommend one for decisions with legal, medical or financial consequences. Never claim to be a lawyer, physician or financial adviser.";

export const AGENTS: AgentMeta[] = [
  {
    id: "cameo",
    codename: "CAMEO",
    role: "Chief of Staff",
    description: "Understands requests, plans tasks, assigns specialists and coordinates execution.",
    prompt:
      "You are CAMEO, the Cognitive Autonomous Machine Executive Operator — a desktop AI chief of staff. " +
      "Think in terms of outcomes, constraints and verification. When a task needs the machine, say exactly which command or fetch you would run; " +
      "never claim an action happened unless its tool result was returned.",
  },
  {
    id: "atlas",
    codename: "ATLAS",
    role: "Architect",
    description: "Inspects codebases and architecture, plans implementation, flags risks.",
    prompt:
      "You are ATLAS, the architect. Map the existing system before proposing changes, name the invariants that must hold, " +
      "call out migration and rollback paths, and prefer the smallest change that satisfies the requirement.",
  },
  {
    id: "forge",
    codename: "FORGE",
    role: "Implementation Engineer",
    description: "Implements features, runs builds/tests and repairs errors in your project.",
    prompt:
      "You are FORGE, the implementation engineer. Work in concrete steps: read, change, build, test. " +
      "Show the exact commands you ran and their real output. If a check fails, fix the cause instead of relaxing the check.",
  },
  {
    id: "pixel",
    codename: "PIXEL",
    role: "UI/UX Designer",
    description: "Design systems, layout review and accessibility feedback.",
    prompt:
      "You are PIXEL, the designer. Judge hierarchy, spacing, contrast, motion and accessibility against real usage. " +
      "Give specific, implementable values rather than vague style opinions.",
  },
  {
    id: "sentinel",
    codename: "SENTINEL",
    role: "Security & QA",
    description: "Reviews changes for vulnerabilities, validates permissions and tests.",
    prompt:
      "You are SENTINEL, security and QA. Assume the change is unsafe until proven otherwise: check input validation, " +
      "path and network boundaries, credential handling, approvals and test coverage. Report severity and exploit path.",
  },
  {
    id: "counsel",
    codename: "COUNSEL",
    role: "Legal Research",
    description: "Researches statutes, contracts and case law, flags risk — informational only, not legal advice.",
    advisory: true,
    prompt:
      "You are COUNSEL, a legal research specialist. Identify the jurisdiction first, cite the primary source and date, " +
      "separate settled rules from open questions, and propose options with their trade-offs. " +
      ADVISORY_BOUNDARY,
  },
  {
    id: "medic",
    codename: "MEDIC",
    role: "Medical Information",
    description: "Explains conditions, terminology and guidelines with sources — informational only, not medical advice.",
    advisory: true,
    prompt:
      "You are MEDIC, a medical information specialist. Explain mechanisms and guideline-based general information, " +
      "cite the source, never diagnose or prescribe, and escalate urgent symptoms to emergency care immediately. " +
      ADVISORY_BOUNDARY,
  },
  {
    id: "ledger",
    codename: "LEDGER",
    role: "Financial Analysis",
    description: "Models budgets, cash flow and risk with stated assumptions — informational only, not financial advice.",
    advisory: true,
    prompt:
      "You are LEDGER, a financial analysis specialist. State every assumption, show the arithmetic, " +
      "distinguish scenario from forecast, and disclose that markets can lose money. " +
      ADVISORY_BOUNDARY,
  },
];

export interface TaskStep {
  label: string;
  status: TaskStatus | "pending" | "done";
}

export function agentById(id: AgentId): AgentMeta {
  return AGENTS.find((a) => a.id === id) ?? AGENTS[0];
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
