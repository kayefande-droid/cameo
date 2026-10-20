import { invoke } from "@tauri-apps/api/core";
import { redactLikelySecrets } from "./orchestration";

export type MemoryKind = "decision" | "fact" | "lesson" | "runbook" | "preference" | "note" | "unresolved-issue";
export type MemoryStatus = "active" | "superseded";

export interface MemoryEntry {
  id: string;
  kind: MemoryKind;
  title: string;
  body: string;
  scope: "project";
  status: MemoryStatus;
  createdAt: number;
  updatedAt: number;
}

const KINDS: MemoryKind[] = ["decision", "fact", "lesson", "runbook", "preference", "note", "unresolved-issue"];

const ROW_TO_MEMORY = (row: Record<string, unknown>): MemoryEntry => ({
  id: row.id as string,
  kind: row.kind as MemoryKind,
  title: row.title as string,
  body: row.body as string,
  scope: "project",
  status: row.status as MemoryStatus,
  createdAt: row.created_at as number,
  updatedAt: row.updated_at as number,
});

/** Secret-shaped values are redacted before they reach the database; never stored plaintext. */
export function sanitizeMemoryText(text: string): string {
  return redactLikelySecrets(text);
}

export async function addMemory(input: { kind: MemoryKind; title: string; body: string }): Promise<MemoryEntry> {
  if (!KINDS.includes(input.kind)) throw new Error("Unknown memory kind; choose a listed kind.");
  const row = await invoke<Record<string, unknown>>("memory_add", {
    input: { kind: input.kind, title: sanitizeMemoryText(input.title), body: sanitizeMemoryText(input.body) },
  });
  return ROW_TO_MEMORY(row);
}

export async function listMemory(): Promise<MemoryEntry[]> {
  try {
    const rows = await invoke<Record<string, unknown>[]>("memory_list");
    return rows.map(ROW_TO_MEMORY);
  } catch {
    return [];
  }
}

export async function updateMemory(id: string, patch: { title?: string; body?: string; status?: MemoryStatus }): Promise<MemoryEntry> {
  const payload: { title?: string; body?: string; status?: string } = {};
  if (patch.title !== undefined) payload.title = sanitizeMemoryText(patch.title);
  if (patch.body !== undefined) payload.body = sanitizeMemoryText(patch.body);
  if (patch.status !== undefined) payload.status = patch.status;
  const row = await invoke<Record<string, unknown>>("memory_update", { id, ...payload });
  return ROW_TO_MEMORY(row);
}

export async function deleteMemory(id: string): Promise<boolean> {
  return invoke<boolean>("memory_delete", { id });
}
