import rawRegistry from "../data/capabilities.json";
import type { AgentId, TaskStatus } from "../types/tasks";

export type CapabilityPhase = "research" | "plan" | "implement" | "review" | "fix" | "verify" | "report" | "remember";
export type CapabilityAvailability = "active" | "advisory-only";

export interface CapabilityEntry {
  id: string;
  origin: string;
  agent: AgentId;
  phase: CapabilityPhase;
  description: string;
  tools: string[];
  permissions: string[];
  dependencies: string[];
  availability: CapabilityAvailability;
}

export interface CapabilityRegistry {
  schema: string;
  source: { upstream: string; upstreamVersion: string; upstreamCommit: string; upstreamLicense: string; note: string };
  selection: { toolBudget: string; externalExecution: string };
  capabilities: CapabilityEntry[];
}

const VALID_AGENTS: AgentId[] = ["cameo", "atlas", "forge", "pixel", "sentinel"];
const VALID_PHASES: CapabilityPhase[] = ["research", "plan", "implement", "review", "fix", "verify", "report", "remember"];

/** Validates the bundled registry once; a corrupt registry disables guidance instead of degrading silently. */
const parseRegistry = (): CapabilityRegistry | null => {
  try {
    const registry = rawRegistry as CapabilityRegistry;
    if (registry.schema !== "cameo.capability-registry.v1") return null;
    if (!registry.capabilities || !Array.isArray(registry.capabilities)) return null;
    for (const entry of registry.capabilities) {
      const ok =
        !!entry &&
        typeof entry.id === "string" && entry.id.trim().length > 0 &&
        VALID_AGENTS.includes(entry.agent) &&
        VALID_PHASES.includes(entry.phase) &&
        typeof entry.description === "string" && entry.description.trim().length > 0 &&
        Array.isArray(entry.tools) &&
        Array.isArray(entry.permissions) &&
        Array.isArray(entry.dependencies) &&
        (entry.availability === "active" || entry.availability === "advisory-only") &&
        // Every dependency must resolve to another declared capability (no dangling references).
        entry.dependencies.every((dep) => typeof dep === "string" && rawRegistry.capabilities.some((c) => c.id === dep));
      if (!ok) return null;
    }
    const ids = new Set(registry.capabilities.map((entry) => entry.id));
    if (ids.size !== registry.capabilities.length) return null;
    return registry;
  } catch {
    return null;
  }
};

const parsed = parseRegistry();

export function getCapabilityRegistry(): CapabilityRegistry {
  if (!parsed) throw new Error("Bundled capability registry failed validation; refusing degraded workflow guidance.");
  return parsed;
}

export function listCapabilities(options?: { agent?: AgentId; availability?: CapabilityAvailability }): CapabilityEntry[] {
  const all = getCapabilityRegistry().capabilities;
  return all.filter((entry) =>
    (!options?.agent || entry.agent === options.agent) &&
    (!options?.availability || entry.availability === options.availability),
  );
}

const PHASE_FOR_STATUS: Partial<Record<TaskStatus, CapabilityPhase>> = {
  queued: "plan",
  planning: "plan",
  awaiting_approval: "plan",
  running: "implement",
  verifying: "verify",
  awaiting_verification: "verify",
  partially_completed: "fix",
};

/** Selects guidance by agent + current phase; never loads unrelated capabilities into one request. */
export function selectCapabilitiesFor(agent: AgentId, phase: CapabilityPhase): CapabilityEntry[] {
  return listCapabilities({ agent }).filter((entry) => entry.phase === phase && entry.availability === "active");
}

export function capabilityPhasesForStatus(agent: AgentId, status: TaskStatus): CapabilityEntry[] {
  const phase = PHASE_FOR_STATUS[status];
  if (!phase) return [];
  const direct = selectCapabilitiesFor(agent, phase);
  const deps = new Set(direct.flatMap((entry) => entry.dependencies));
  const supporting = listCapabilities().filter((entry) => deps.has(entry.id) && entry.availability === "active");
  const seen = new Set<string>();
  return [...direct, ...supporting].filter((entry) => (seen.has(entry.id) ? false : (seen.add(entry.id), true)));
}

/** Compact workflow-gate text injected into the specialist's planning prompt. */
export function capabilityGuidanceText(agent: AgentId): string {
  const entries = listCapabilities({ agent, availability: "active" });
  if (entries.length === 0) return "";
  return "Workflow gates for this specialist (verified evidence only, never claim unrun checks passed): "
    + entries.map((entry) => `${entry.id}: ${entry.description}`).join(" ");
}
