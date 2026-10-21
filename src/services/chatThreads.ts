import { AGENTS, type AgentId } from "../types/tasks";

export interface ChatTurn {
  id: string;
  role: "user" | "assistant";
  who: string;
  text: string;
  error?: boolean;
  thinking?: string;
}

export interface ChatSessionMeta {
  id: string;
  title: string;
  updatedAt: number;
}

const VERSION = "v2";
const MAX_SESSIONS_PER_AGENT = 20;
const MAX_TURNS_PER_SESSION = 60;
const MAX_TURN_CHARS = 50_000;

const indexKey = (agent: string): string => `cameo.chat.sessions.${VERSION}.${agent}`;
const threadKey = (sessionId: string): string => `cameo.chat.thread.${VERSION}.${sessionId}`;
const activeKey = (agent: string): string => `cameo.chat.active.${VERSION}.${agent}`;
// Predecessor single-thread keys (v1) — migrated once, then removed.
const legacyThreadKey = (agent: string): string => `cameo.chat.v1.${agent}`;
const legacyDraftKey = (agent: string): string => `cameo.chat.draft.v1.${agent}`;

function storage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

function readJson<T>(store: Storage, key: string): T | null {
  try {
    const raw = store.getItem(key);
    if (!raw) return null;
    return JSON.parse(raw) as T;
  } catch {
    return null;
  }
}

function writeJson(store: Storage, key: string, value: unknown): void {
  try {
    store.setItem(key, JSON.stringify(value));
  } catch {
    /* Storage full or unavailable — the live session keeps working. */
  }
}

function sanitizeTurns(value: unknown): ChatTurn[] {
  if (!Array.isArray(value)) return [];
  return (value as Partial<ChatTurn>[])
    .filter((t) => t && typeof t.id === "string" && (t.role === "user" || t.role === "assistant") && typeof t.text === "string")
    .map((t) => ({
      id: t.id as string,
      role: t.role as ChatTurn["role"],
      who: typeof t.who === "string" ? (t.who as string) : "CAMEO",
      text: (t.text as string).slice(0, MAX_TURN_CHARS),
      error: t.error === true,
      thinking: typeof t.thinking === "string" ? (t.thinking as string).slice(0, 8000) : undefined,
    }))
    .slice(-MAX_TURNS_PER_SESSION);
}

export function deriveTitle(turns: ChatTurn[]): string {
  const first = turns.find((t) => t.role === "user" && t.text.trim());
  if (!first) return "New chat";
  const line = first.text.trim().split(/\r?\n/)[0];
  return line.length > 42 ? `${line.slice(0, 42)}…` : line;
}

function newSession(agent: AgentId): { meta: ChatSessionMeta; turns: ChatTurn[] } {
  const meta: ChatSessionMeta = {
    id: `s${Date.now().toString(36)}${Math.random().toString(36).slice(2, 7)}`,
    title: "New chat",
    updatedAt: Date.now(),
  };
  void agent;
  return { meta, turns: [] };
}

/** One-time migration of the old single-thread keys into a session. */
function migrateLegacy(store: Storage, agent: AgentId): { meta: ChatSessionMeta; turns: ChatTurn[] } | null {
  const turns = sanitizeTurns(readJson(store, legacyThreadKey(agent)));
  const draft = (() => {
    try {
      return store.getItem(legacyDraftKey(agent));
    } catch {
      return null;
    }
  })();
  if (turns.length === 0 && !draft) return null;
  const { meta } = newSession(agent);
  meta.title = turns.length > 0 ? deriveTitle(turns) : "Restored chat";
  meta.updatedAt = Date.now();
  try {
    store.removeItem(legacyThreadKey(agent));
    store.removeItem(legacyDraftKey(agent));
  } catch {
    /* Best-effort cleanup. */
  }
  return { meta, turns };
}

export interface AgentThreads {
  sessions: ChatSessionMeta[];
  activeId: string;
  turns: ChatTurn[];
}

export function loadAgentThreads(agent: AgentId): AgentThreads {
  const store = storage();
  const fallback = newSession(agent);
  if (!store) return { sessions: [fallback.meta], activeId: fallback.meta.id, turns: [] };
  const rawIndex = readJson<ChatSessionMeta[]>(store, indexKey(agent));
  const sessions = Array.isArray(rawIndex)
    ? rawIndex
        .filter((s) => s && typeof s.id === "string")
        .map((s) => ({ id: s.id, title: typeof s.title === "string" && s.title ? s.title : "Chat", updatedAt: typeof s.updatedAt === "number" ? s.updatedAt : Date.now() }))
        .slice(0, MAX_SESSIONS_PER_AGENT)
    : [];
  if (sessions.length === 0) {
    const migrated = migrateLegacy(store, agent);
    const seed = migrated ?? fallback;
    if (migrated) writeJson(store, threadKey(seed.meta.id), migrated.turns);
    // Write the seed index through so the next load resolves the same
    // active session instead of minting a fresh id every time.
    writeJson(store, indexKey(agent), [seed.meta]);
    try {
      store.setItem(activeKey(agent), seed.meta.id);
    } catch {
      /* Best-effort. */
    }
    return { sessions: [seed.meta], activeId: seed.meta.id, turns: migrated?.turns ?? [] };
  }
  const storedActive = (() => {
    try {
      return store.getItem(activeKey(agent));
    } catch {
      return null;
    }
  })();
  const activeId = sessions.some((s) => s.id === storedActive) ? (storedActive as string) : sessions[0].id;
  return { sessions, activeId, turns: sanitizeTurns(readJson(store, threadKey(activeId))) };
}

/** Load one thread by id (sanitized). Centralizes the storage key so
 *  callers never hardcode it. */
export function loadThread(sessionId: string): ChatTurn[] {
  const store = storage();
  if (!store) return [];
  return sanitizeTurns(readJson(store, threadKey(sessionId)));
}

export function persistThread(sessionId: string, turns: ChatTurn[]): void {
  const store = storage();
  if (!store) return;
  writeJson(store, threadKey(sessionId), sanitizeTurns(turns));
}

export function persistIndex(agent: AgentId, sessions: ChatSessionMeta[], activeId: string): void {
  const store = storage();
  if (!store) return;
  writeJson(store, indexKey(agent), sessions.slice(0, MAX_SESSIONS_PER_AGENT));
  try {
    store.setItem(activeKey(agent), activeId);
  } catch {
    /* Best-effort. */
  }
}

export function deleteThread(sessionId: string): void {
  const store = storage();
  if (!store) return;
  try {
    store.removeItem(threadKey(sessionId));
  } catch {
    /* Best-effort. */
  }
}

export function createSessionMeta(): ChatSessionMeta {
  return { id: `s${Date.now().toString(36)}${Math.random().toString(36).slice(2, 7)}`, title: "New chat", updatedAt: Date.now() };
}

export function agentIds(): AgentId[] {
  return AGENTS.map((a) => a.id);
}

export const CHAT_THREAD_LIMITS = { MAX_SESSIONS_PER_AGENT, MAX_TURNS_PER_SESSION };
