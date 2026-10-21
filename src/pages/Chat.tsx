import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ModelSelection } from "../App";
import {
  decideToolCall,
  deleteKey,
  hasKey,
  newRequestId,
  saveKey,
  streamChat,
  testConnection,
} from "../services/ai";
import { PROVIDERS, type AIProviderId, type ChatMessage } from "../types/ai";
import { AGENTS, agentById, type AgentId } from "../types/tasks";
import { getPref, setPref } from "../services/store";
import {
  createSessionMeta,
  deleteThread,
  deriveTitle,
  loadAgentThreads,
  loadThread,
  persistIndex,
  persistThread,
  type ChatSessionMeta,
} from "../services/chatThreads";

interface Turn {
  id: string;
  role: "user" | "assistant";
  who: string;
  text: string;
  error?: boolean;
  thinking?: string;
}

/** Max turns and characters sent to the provider per request. The full
 *  session stays on screen, but the payload is capped so long chats don't
 *  get slower with every message. */
const HISTORY_MAX_TURNS = 30;
const HISTORY_MAX_CHARS = 24_000;

/** Saved chats live in-app (localStorage, per agent) as a session list with
 *  one thread each. The active session survives panel switches, navigation
 *  and restarts until deleted. Legacy single-thread keys migrate once. */

export function buildChatHistory(system: string, turns: Turn[], input: string): ChatMessage[] {
  const usable = turns.filter((t) => !t.error && t.text.trim());
  const tail = usable.slice(-HISTORY_MAX_TURNS);
  let budget = HISTORY_MAX_CHARS;
  const picked: ChatMessage[] = [];
  for (let i = tail.length - 1; i >= 0 && budget > 0; i--) {
    const t = tail[i];
    const content = t.text.length > budget ? `…${t.text.slice(-budget)}` : t.text;
    budget -= content.length;
    picked.unshift({ role: t.role, content });
  }
  return [{ role: "system", content: system }, ...picked, { role: "user", content: input }];
}

interface ToolCard {
  approval_id: string;
  name: string;
  preview: string;
  requires_approval: boolean;
  /** pending = waiting for your click; running = approved/auto, executing; done = result in. */
  status: "pending" | "running" | "done";
  decision?: "approved" | "denied";
  output?: string;
  ok?: boolean;
}

type StepStatus = "queued" | "running" | "done" | "failed";

interface ProcessStep {
  id: string;
  label: string;
  status: StepStatus;
  detail?: string;
  ts: number;
}

/** Turn a raw backend status line into a short todo label + status. */
function classifyPhase(message: string): { label: string; status: StepStatus } {
  const m = message.toLowerCase();
  if (m.includes("fail") || m.includes("error") || m.includes("denied") || m.includes("✖")) return { label: message, status: "failed" };
  if (m.includes("done") || m.includes("✔") || m.includes("complete") || m.includes("verif")) return { label: message, status: "done" };
  return { label: message, status: "running" };
}

const AUTONOMY_NOTE =
  " The user grants this session access to a scoped operator layer (commands confined to one project root, https fetch, device launch). " +
  "You can propose a command, URL or app to run and the user executes it in the Operator console; when a tool result is returned, treat it as ground truth.";

export default function Chat({
  selection,
  onSelectionChange,
  compact = false,
  active = true,
  onRequestChange,
  onAudioLevel,
  onListeningChange,
}: {
  selection: ModelSelection | null;
  onSelectionChange: (sel: ModelSelection) => void;
  compact?: boolean;
  active?: boolean;
  onRequestChange?: (requestId: string | null) => void;
  onAudioLevel?: (level: number) => void;
  onListeningChange?: (listening: boolean) => void;
}) {
  const [provider, setProvider] = useState<AIProviderId>(selection?.provider ?? "openrouter");
  const [model, setModel] = useState<string>(selection?.model ?? "");
  useEffect(() => {
    if (selection) {
      setProvider(selection.provider);
      setModel(selection.model);
    }
  }, [selection]);
  const [agent, setAgent] = useState<AgentId>("cameo");
  useEffect(() => {
    void getPref("chat_agent").then((stored) => {
      if (stored && AGENTS.some((a) => a.id === stored)) setAgent(stored as AgentId);
    });
  }, []);
  const activeAgent = agentById(agent);
  const [sessionList, setSessionList] = useState<ChatSessionMeta[]>(() => loadAgentThreads("cameo").sessions);
  const [activeSessionId, setActiveSessionId] = useState<string>(() => loadAgentThreads("cameo").activeId);
  const [turns, setTurns] = useState<Turn[]>(() => loadAgentThreads("cameo").turns);
  const [input, setInput] = useState("");
  const [autopilot, setAutopilot] = useState(false);
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
  const [confirmingDeleteId, setConfirmingDeleteId] = useState<string | null>(null);
  const agentRef = useRef(agent);
  agentRef.current = agent;
  const sessionRef = useRef(activeSessionId);
  sessionRef.current = activeSessionId;
  // Set while applying a restored thread so the save effect below skips
  // one cycle instead of persisting the outgoing thread under the new key.
  const suppressSaveRef = useRef(false);
  // Restored-thread loading must run before the save effect on agent
  // switch (effects run in declaration order).
  useEffect(() => {
    suppressSaveRef.current = true;
    const loaded = loadAgentThreads(agent);
    setSessionList(loaded.sessions);
    setActiveSessionId(loaded.activeId);
    setTurns(loaded.turns);
    setRenamingId(null);
    setConfirmingDeleteId(null);
  }, [agent]);
  useEffect(() => {
    if (suppressSaveRef.current) {
      suppressSaveRef.current = false;
      return;
    }
    const id = sessionRef.current;
    persistThread(id, turns);
    setSessionList((prev) => {
      const next = prev.map((s) =>
        s.id === id ? { ...s, title: turns.length ? deriveTitle(turns) : s.title, updatedAt: Date.now() } : s
      );
      persistIndex(agentRef.current, next, id);
      return next;
    });
  }, [turns]);
  const newSession = useCallback(() => {
    const meta = createSessionMeta();
    suppressSaveRef.current = true;
    setSessionList((prev) => {
      const next = [meta, ...prev].slice(0, 20);
      persistIndex(agentRef.current, next, meta.id);
      return next;
    });
    setActiveSessionId(meta.id);
    persistThread(meta.id, []);
    setTurns([]);
    setInput("");
    setSteps([]);
    setThinking("");
    setToolCards([]);
    setPhase(null);
    setError(null);
    setRenamingId(null);
    setConfirmingDeleteId(null);
  }, []);
  const switchSession = useCallback((id: string) => {
    const current = sessionRef.current;
    if (id === current) return;
    setTurns((currentTurns) => {
      persistThread(current, currentTurns);
      return currentTurns;
    });
    suppressSaveRef.current = true;
    setActiveSessionId(id);
    setSessionList((prev) => {
      persistIndex(agentRef.current, prev, id);
      return prev;
    });
    setTurns(loadThread(id) as Turn[]);
    setInput("");
    setSteps([]);
    setThinking("");
    setToolCards([]);
    setPhase(null);
    setError(null);
    setRenamingId(null);
    setConfirmingDeleteId(null);
  }, []);
  const renameSession = useCallback((id: string, title: string) => {
    const clean = title.trim().slice(0, 60) || "Chat";
    setSessionList((prev) => {
      const next = prev.map((s) => (s.id === id ? { ...s, title: clean, updatedAt: Date.now() } : s));
      persistIndex(agentRef.current, next, sessionRef.current);
      return next;
    });
    setRenamingId(null);
  }, []);
  const deleteSession = useCallback((id: string) => {
    deleteThread(id);
    setSessionList((prev) => {
      const remaining = prev.filter((s) => s.id !== id);
      if (remaining.length === 0) {
        const meta = createSessionMeta();
        persistIndex(agentRef.current, [meta], meta.id);
        persistThread(meta.id, []);
        suppressSaveRef.current = true;
        setActiveSessionId(meta.id);
        setTurns([]);
        return [meta];
      }
      const nextActive = id === sessionRef.current ? remaining[0].id : sessionRef.current;
      persistIndex(agentRef.current, remaining, nextActive);
      if (id === sessionRef.current) {
        suppressSaveRef.current = true;
        setActiveSessionId(nextActive);
        setTurns(loadThread(nextActive) as Turn[]);
        setInput("");
      }
      return remaining;
    });
    setConfirmingDeleteId(null);
    setRenamingId(null);
  }, []);
  const [attachments, setAttachments] = useState<{ name: string; text: string }[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [agentMode, setAgentMode] = useState(false);
  const [autoApproveSafe, setAutoApproveSafe] = useState(false);
  const [toolCards, setToolCards] = useState<ToolCard[]>([]);
  // Live process visibility: the backend narrates each phase (status),
  // reasoning slices (thinking), and tool todos. Steps persist per message
  // so you can check what was executed after it finishes.
  const [phase, setPhase] = useState<string | null>(null);
  const [steps, setSteps] = useState<ProcessStep[]>([]);
  const [thinking, setThinking] = useState("");
  const [thinkExpanded, setThinkExpanded] = useState(true);
  const [processStartedAt, setProcessStartedAt] = useState<number | null>(null);
  const [elapsedMs, setElapsedMs] = useState(0);
  const thinkingEndRef = useRef<HTMLDivElement>(null);

  // Live elapsed timer while a response streams.
  useEffect(() => {
    if (!busy || processStartedAt === null) return;
    setElapsedMs(Date.now() - processStartedAt);
    const id = window.setInterval(() => setElapsedMs(Date.now() - (processStartedAt ?? Date.now())), 500);
    return () => window.clearInterval(id);
  }, [busy, processStartedAt]);

  // Keep the live thinking stream pinned to the bottom as it grows.
  useEffect(() => {
    const el = thinkingEndRef.current;
    if (el && typeof el.scrollIntoView === "function") el.scrollIntoView({ block: "nearest" });
  }, [thinking, busy]);
  // Sticky focus: the latest section heading in the reasoning trace, so
  // you always know what the model is chewing on without reading it all.
  const thinkFocus = useMemo(() => {
    if (!thinking) return null;
    const lines = thinking.split(/\r?\n/);
    for (let i = lines.length - 1; i >= 0; i--) {
      const line = lines[i].trim();
      if (!line) continue;
      if (/^(#{1,4}\s|step\s+\d+|\d+[.)]\s|[-*]\s+\[|plan:|reasoning:|analysis:|conclusion:|next:|fix:|error:|warning:)/i.test(line)) {
        return line.replace(/^#{1,4}\s*/, "").slice(0, 90);
      }
    }
    return null;
  }, [thinking]);
  const copyThinking = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(thinkingRef.current || thinking);
    } catch {
      /* Clipboard unavailable — selection copy still works. */
    }
  }, [thinking]);

  const upsertStep = useCallback((id: string, label: string, status: StepStatus, detail?: string) => {
    setSteps((prev) => {
      const now = Date.now();
      const idx = prev.findIndex((s) => s.id === id);
      if (idx >= 0) {
        const next = [...prev];
        next[idx] = { ...next[idx], label, status, detail: detail ?? next[idx].detail, ts: now };
        return next;
      }
      // Mark the previously-running step done when a new one starts, so the
      // todo list reads as a real progression instead of many spinners.
      const settled = prev.map((s) => (s.status === "running" && status === "running" ? { ...s, status: "done" as StepStatus } : s));
      return [...settled.slice(-19), { id, label, status, detail, ts: now }];
    });
  }, []);
  const pendingTextRef = useRef("");
  const pendingThinkRef = useRef("");
  const thinkingRef = useRef("");
  const flushTimerRef = useRef<number | null>(null);
  const scrollRafRef = useRef<number | null>(null);
  const stickToBottomRef = useRef(true);
  const [showJump, setShowJump] = useState(false);
  const assistantIdRef = useRef<string | null>(null);
  useEffect(() => {
    // Autonomous by default: first-run users get agent tools + safe-read
    // auto-approval on, so agents act without asking for every step.
    // Destructive / device actions still always pause for approval.
    void getPref("chat_agent_tools").then((stored) => {
      if (stored === null || stored === "1") {
        setAgentMode(true);
        if (stored === null) void setPref("chat_agent_tools", "1");
      }
    });
    void getPref("chat_agent_auto_safe").then((stored) => {
      if (stored === null || stored === "1") {
        setAutoApproveSafe(true);
        if (stored === null) void setPref("chat_agent_auto_safe", "1");
      }
    });
    void getPref("autopilot").then((stored) => {
      if (stored === "1") {
        setAutopilot(true);
        setAgentMode(true);
        setAutoApproveSafe(true);
      }
    });
  }, []);
  const [listening, setListening] = useState(false);
  const lastAudioUpdateRef = useRef(0);
  const scrollRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  // When the panel opens (dock expand) or the page mounts, put the caret in
  // the chat box so typing works immediately — no extra click required.
  useEffect(() => {
    if (active) inputRef.current?.focus();
  }, [active]);
  const requestIdRef = useRef<string>(newRequestId());
  const stopListeningRef = useRef<(() => void) | null>(null);
  const activeRef = useRef(active);
  activeRef.current = active;

  // Batched rendering: deltas arrive per token, but re-rendering the whole
  // list per token plus a forced scroll-to-bottom each time is what made
  // chats feel sluggish. Accumulate and flush ~20x/sec, and only autoscroll
  // when the user was already at the bottom.
  // Autoscroll runs at most once per animation frame: reading scrollHeight
  // and writing scrollTop on every 50 ms flush forced a sync layout each
  // time, which made long streaming answers feel sluggish.
  const scheduleScroll = useCallback(() => {
    if (scrollRafRef.current !== null || !stickToBottomRef.current) return;
    scrollRafRef.current = window.requestAnimationFrame(() => {
      scrollRafRef.current = null;
      if (!stickToBottomRef.current) return;
      const node = scrollRef.current;
      if (node) node.scrollTop = node.scrollHeight;
    });
  }, []);

  const flushPending = useCallback(() => {
    flushTimerRef.current = null;
    const text = pendingTextRef.current;
    const think = pendingThinkRef.current;
    pendingTextRef.current = "";
    pendingThinkRef.current = "";
    const asstId = assistantIdRef.current;
    if ((!text && !think) || !asstId) return;
    if (think) {
      thinkingRef.current = (thinkingRef.current + think).slice(-8000);
      setThinking(thinkingRef.current);
    }
    if (text) {
      setTurns((ts) => ts.map((t) => (t.id === asstId ? { ...t, text: t.text + text } : t)));
    }
    scheduleScroll();
  }, [scheduleScroll]);

  const scheduleFlush = useCallback(() => {
    if (flushTimerRef.current !== null) return;
    flushTimerRef.current = window.setTimeout(() => { flushPending(); }, 50);
  }, [flushPending]);

  const scrollToBottom = useCallback(() => {
    stickToBottomRef.current = true;
    setShowJump(false);
    const node = scrollRef.current;
    if (node) node.scrollTop = node.scrollHeight;
    scheduleScroll();
  }, [scheduleScroll]);

  useEffect(() => () => {
    if (flushTimerRef.current !== null) window.clearTimeout(flushTimerRef.current);
    if (scrollRafRef.current !== null) window.cancelAnimationFrame(scrollRafRef.current);
  }, []);

  const send = useCallback(async () => {
    const promptText = input.trim();
    const text = [promptText, ...attachments.map((file) => `Attached file: ${file.name}\n\`\`\`\n${file.text}\n\`\`\``)].filter(Boolean).join("\n\n");
    if (!text || busy || !selection) {
      if (!selection) setError("Choose a provider and model in Settings first.");
      return;
    }
    setError(null);
    const userTurn: Turn = { id: `u${Date.now()}`, role: "user", who: "You", text };
    const asstId = `a${Date.now()}`;
    setTurns((t) => [
      ...t,
      userTurn,
      { id: asstId, role: "assistant", who: `${activeAgent.codename} · ${selection.model}`, text: "" },
    ]);
    setInput("");
    if (inputRef.current) inputRef.current.style.height = "auto";
    setAttachments([]);
    setToolCards([]);
    setSteps([]);
    setThinking("");
    setThinkExpanded(true);
    thinkingRef.current = "";
    setPhase("Starting…");
    setProcessStartedAt(Date.now());
    upsertStep(`phase-${Date.now()}`, "Starting…", "running");
    pendingTextRef.current = "";
    pendingThinkRef.current = "";
    setBusy(true);
    const requestId = newRequestId();
    requestIdRef.current = requestId;
    onRequestChange?.(requestId);

    assistantIdRef.current = asstId;
    const history = buildChatHistory(`${activeAgent.prompt}${AUTONOMY_NOTE}`, turns, text);

    try {
      await streamChat(
        selection.provider,
        selection.model,
        history,
        {
          max_tokens: 2048,
          agent_tools: agentMode || autopilot,
          agent_max_rounds: autopilot ? 8 : 4,
          agent_auto_approve_safe: autoApproveSafe || autopilot,
          agent_auto_approve_all: autopilot,
        },
        (ev) => {
          if (ev.type === "delta") {
            pendingTextRef.current += ev.text;
            scheduleFlush();
          } else if (ev.type === "thinking") {
            pendingThinkRef.current += ev.text;
            scheduleFlush();
          } else if (ev.type === "status") {
            setPhase(ev.message);
            const { label, status } = classifyPhase(ev.message);
            upsertStep(`phase-${label.slice(0, 48)}`, label, status);
          } else if (ev.type === "tool") {
            setToolCards((cards) => [
              ...cards.filter((c) => c.approval_id !== ev.approval_id),
              {
                approval_id: ev.approval_id,
                name: ev.name,
                preview: ev.preview,
                requires_approval: ev.requires_approval,
                status: ev.requires_approval ? "pending" : "running",
              },
            ]);
            upsertStep(ev.approval_id, `${ev.name} — ${ev.preview || "running"}`, "running");
          } else if (ev.type === "tool_approval") {
            setToolCards((cards) =>
              cards.map((c) =>
                c.approval_id === ev.approval_id
                  ? { ...c, status: ev.approved ? "running" : c.status, decision: ev.approved ? "approved" : "denied" }
                  : c
              )
            );
          } else if (ev.type === "tool_result") {
            if (flushTimerRef.current !== null) {
              window.clearTimeout(flushTimerRef.current);
              flushTimerRef.current = null;
            }
            flushPending();
            upsertStep(ev.approval_id, `${ev.name} ${ev.ok ? "✔" : "✖"}`, ev.ok ? "done" : "failed", ev.output?.slice(0, 160));
            setToolCards((cards) =>
              cards.map((c) =>
                c.approval_id === ev.approval_id
                  ? { ...c, status: "done", output: ev.output, ok: ev.ok }
                  : c
              )
            );
          } else if (ev.type === "error") {
            if (flushTimerRef.current !== null) {
              window.clearTimeout(flushTimerRef.current);
              flushTimerRef.current = null;
            }
            flushPending();
            setPhase(null);
            setTurns((ts) => ts.map((t) => (t.id === asstId ? { ...t, text: ev.message, error: true } : t)));
          } else if (ev.type === "done") {
            if (flushTimerRef.current !== null) {
              window.clearTimeout(flushTimerRef.current);
              flushTimerRef.current = null;
            }
            flushPending();
            setPhase(null);
            setTurns((ts) =>
              ts.map((t) =>
                t.id === asstId
                  ? {
                      ...t,
                      text: t.text || "(empty response — the model returned no content)",
                      thinking: thinkingRef.current || undefined,
                    }
                  : t
              )
            );
          }
        },
        requestId
      );
    } catch (e) {
      setTurns((ts) =>
        ts.map((t) => (t.id === asstId ? { ...t, text: String(e), error: true } : t))
      );
    } finally {
      setBusy(false);
      requestIdRef.current = newRequestId();
      onRequestChange?.(null);
    }
  }, [activeAgent, agentMode, attachments, autoApproveSafe, autopilot, busy, flushPending, input, onRequestChange, scheduleFlush, selection, turns]);

  /** Hands-free mode: agents run every tool (including destructive/device
   *  actions) without approval clicks, and tasks auto-advance through plan →
   *  apply → verify. The operator layer is switched on so agents can reach
   *  commands, network and device launch; every action stays audit-logged. */
  const toggleAutopilot = async (on: boolean) => {
    setAutopilot(on);
    void setPref("autopilot", on ? "1" : "0");
    if (on) {
      setAgentMode(true);
      void setPref("chat_agent_tools", "1");
      setAutoApproveSafe(true);
      void setPref("chat_agent_auto_safe", "1");
      try {
        const { operatorConfigure } = await import("../services/operator");
        await operatorConfigure({ enabled: true, net: true, device: true });
      } catch (e) {
        setError(`Autopilot could not enable operator tools: ${String(e)}`);
      }
    }
  };

  const toggleListening = async () => {
    if (listening) {
      stopListeningRef.current?.();
      stopListeningRef.current = null;
      setListening(false);
      onListeningChange?.(false);
      onAudioLevel?.(0);
      return;
    }
    try {
      const media = await navigator.mediaDevices.getUserMedia({ audio: true });
      const context = new AudioContext();
      const source = context.createMediaStreamSource(media);
      const analyser = context.createAnalyser();
      analyser.fftSize = 256;
      source.connect(analyser);
      const samples = new Uint8Array(analyser.frequencyBinCount);
      let frame = 0;
      const tick = () => {
        if (!activeRef.current) {
          frame = requestAnimationFrame(tick);
          return;
        }
        analyser.getByteTimeDomainData(samples);
        let sum = 0;
        for (const value of samples) { const normalized = (value - 128) / 128; sum += normalized * normalized; }
        const now = performance.now();
        if (now - lastAudioUpdateRef.current >= 80) {
          lastAudioUpdateRef.current = now;
          onAudioLevel?.(Math.min(1, Math.sqrt(sum / samples.length) * 4));
        }
        frame = requestAnimationFrame(tick);
      };
      tick();
      stopListeningRef.current = () => {
        cancelAnimationFrame(frame);
        media.getTracks().forEach((track) => track.stop());
        void context.close();
        onAudioLevel?.(0);
        onListeningChange?.(false);
      };
      if (!activeRef.current) {
        stopListeningRef.current();
        stopListeningRef.current = null;
        return;
      }
      setListening(true);
      onListeningChange?.(true);
    } catch (micError) {
      setError(`Microphone unavailable: ${micError instanceof DOMException ? micError.name : String(micError)}`);
    }
  };

  useEffect(() => () => { stopListeningRef.current?.(); }, []);

  const addAttachments = async (files: FileList | null) => {
    if (!files?.length) return;
    const supportedExtensions = new Set(["txt", "md", "markdown", "json", "csv", "log", "yaml", "yml", "toml", "xml", "html", "htm", "css", "scss", "js", "jsx", "ts", "tsx", "py", "rs", "go", "java", "kt", "swift", "c", "h", "cpp", "sh"]);
    const supported = Array.from(files).filter((file) => supportedExtensions.has(file.name.split(".").pop()?.toLowerCase() ?? ""));
    if (supported.length !== files.length) setError("Only text and source files can be attached. Unsupported files were ignored.");
    if (attachments.length + supported.length > 3) {
      setError("Attach up to 3 text files per message.");
      return;
    }
    try {
      const next = await Promise.all(supported.map(async (file) => {
        if (file.size > 256_000) throw new Error(`${file.name} is larger than the 256 KB attachment limit.`);
        return { name: file.name, text: await file.text() };
      }));
      setAttachments((current) => [...current, ...next].slice(0, 3));
      if (next.length) setError(null);
    } catch (attachmentError) {
      setError(String(attachmentError));
    }
  };

  useEffect(() => {
    if (active || !listening) return;
    stopListeningRef.current?.();
    stopListeningRef.current = null;
    setListening(false);
  }, [active, listening]);

  const stop = async () => {
    const requestId = requestIdRef.current;
    if (requestId) await import("../services/ai").then(({ cancelChat }) => cancelChat(requestId));
  };

  return (
    <div className={`chat-wrap${compact ? " compact-chat" : ""}`}>
      {!compact && <div className="card">
        <h3>
          <span className="section-title">Agent, provider &amp; model</span>
          <AgentPicker agent={agent} onChange={(next) => {
            setAgent(next);
            void setPref("chat_agent", next);
          }} />
          <ModelPicker
            provider={provider}
            model={model}
            setProvider={setProvider}
            setModel={setModel}
            onChange={onSelectionChange}
          />
        </h3>
        <div style={{ display: "flex", gap: 16, alignItems: "center", flexWrap: "wrap", marginTop: 8 }}>
          <label className="field" style={{ flexDirection: "row", alignItems: "center", gap: 6 }}>
            <input
              type="checkbox"
              checked={agentMode}
              onChange={(e) => {
                setAgentMode(e.target.checked);
                void setPref("chat_agent_tools", e.target.checked ? "1" : "0");
              }}
            />
            Agent tool mode
          </label>
          {agentMode && (
            <label className="field" style={{ flexDirection: "row", alignItems: "center", gap: 6 }}>
              <input
                type="checkbox"
                checked={autoApproveSafe}
                onChange={(e) => {
                  setAutoApproveSafe(e.target.checked);
                  void setPref("chat_agent_auto_safe", e.target.checked ? "1" : "0");
                }}
              />
              Auto-approve safe reads
            </label>
          )}
          {agentMode && (
            <span className="note">
              The model can run project commands, fetch public URLs and search the web itself.
              Risky actions pause for your approval; everything is audit-logged.
            </span>
          )}
          <label className="field autopilot-toggle" style={{ flexDirection: "row", alignItems: "center", gap: 6 }} title="Hands-free: auto-approve every tool call and auto-advance tasks through plan, apply and verify">
            <input
              type="checkbox"
              checked={autopilot}
              onChange={(e) => void toggleAutopilot(e.target.checked)}
            />
            Autopilot (hands-free)
          </label>
          {autopilot && (
            <span className="note autopilot-note">
              Hands-free is on: agents use every tool without asking and tasks run plan → apply → verify by themselves. Pin a project folder in Operator console so file work lands in the right place.
            </span>
          )}
        </div>
        <SessionBar
          sessions={sessionList}
          activeId={activeSessionId}
          onSwitch={switchSession}
          onNew={newSession}
          onRename={renameSession}
          onDelete={deleteSession}
          renamingId={renamingId}
          renameDraft={renameDraft}
          setRenameDraft={setRenameDraft}
          setRenamingId={setRenamingId}
          confirmingDeleteId={confirmingDeleteId}
          setConfirmingDeleteId={setConfirmingDeleteId}
        />
      </div>}

      {(busy || steps.length > 0 || thinking || toolCards.length > 0) && (
        <div className={`card process-panel${busy ? " is-live" : ""}`} aria-label="Agent process">
          <div className="process-header">
            <span className={`process-spinner${busy ? " spinning" : " settled"}`} aria-hidden>◍</span>
            <span className="section-title">Live process{phase ? ` — ${phase}` : ""}</span>
            <span className="process-meta note">
              {steps.filter((s) => s.status === "done").length}/{steps.length || "—"} steps
              {processStartedAt !== null && <> · {(elapsedMs / 1000).toFixed(1)}s</>}
            </span>
          </div>
          {steps.length > 0 && (
            <div className="process-progress" aria-hidden>
              <div
                className="process-progress-fill"
                style={{ width: `${steps.length ? Math.round((steps.filter((s) => s.status === "done").length / steps.length) * 100) : 0}%` }}
              />
            </div>
          )}
          {steps.length > 0 && (
            <ol className="todo-list">
              {steps.map((s, i) => (
                <li key={s.id} className={`todo-item is-${s.status}`}>
                  <span className="todo-num" aria-hidden>{String(i + 1).padStart(2, "0")}</span>
                  <span className="todo-icon" aria-hidden>
                    {s.status === "done" ? "✔" : s.status === "failed" ? "✖" : s.status === "running" ? "◍" : "○"}
                  </span>
                  <span className="todo-label">{s.label}</span>
                  {s.detail && <span className="todo-detail mono">{s.detail}</span>}
                </li>
              ))}
            </ol>
          )}
          {thinking && (
            <div className="thinking-block">
              <div className="thinking-head">
                <button type="button" className="thinking-toggle note" onClick={() => setThinkExpanded((v) => !v)} aria-expanded={thinkExpanded}>
                  <span className={`thinking-dot${busy ? " pulsing" : ""}`} aria-hidden />
                  Thinking process · {thinking.split(/\s+/).filter(Boolean).length} words {thinkExpanded ? "▾" : "▸"}
                </button>
                <button type="button" className="btn small thinking-copy" onClick={() => void copyThinking()} title="Copy the full reasoning trace" aria-label="Copy thinking trace">
                  Copy
                </button>
              </div>
              {thinkFocus && (
                <div className="thinking-focus" role="status">
                  <span className="thinking-focus-kicker">Now</span>
                  <span className="thinking-focus-text">{thinkFocus}</span>
                  {busy && <span className="thinking-focus-live" aria-hidden />}
                </div>
              )}
              {thinkExpanded && (
                <div className={`thinking-stream${busy ? " is-streaming" : ""}`} role="log" aria-label="Thinking process">
                  <ThinkingTrace text={thinking} live={busy} />
                  <div ref={thinkingEndRef} />
                </div>
              )}
            </div>
          )}
          {toolCards.length > 0 && (
            <div className="tool-inline" aria-label="Tool activity">
              {toolCards.map((card) => (
                <div key={card.approval_id} className={`tool-row is-${card.status}${card.ok === false ? " is-failed" : ""}`}>
                  <strong className="mono tool-name">{card.name}</strong>
                  <span className="note mono tool-preview">{card.preview || "—"}</span>
                  <span className="note tool-state" role="status">
                    {card.status === "done"
                      ? card.ok ? "✔ done" : `✖ failed${card.decision === "denied" ? " (denied)" : ""}`
                      : card.status === "running" ? "… running" : "waiting for approval"}
                  </span>
                  {card.status === "pending" && (
                    <span className="tool-actions">
                      <button className="btn small primary" onClick={() => void decideToolCall(card.approval_id, true).catch((e) => setError(String(e)))}>Approve &amp; run</button>
                      <button className="btn small danger" onClick={() => void decideToolCall(card.approval_id, false).catch((e) => setError(String(e)))}>Deny</button>
                    </span>
                  )}
                  {card.status === "done" && card.output && (
                    <details className="tool-result">
                      <summary className="note">Result</summary>
                      <pre className="mono">{card.output}</pre>
                    </details>
                  )}
                </div>
              ))}
            </div>
          )}
        </div>
      )}

      <div className="chat-scroll-wrap">
        <div
          className="chat-scroll"
          ref={scrollRef}
          role="log"
          aria-live="polite"
          aria-label="CAMEO conversation"
          onScroll={(e) => {
            const el = e.currentTarget;
            const stuck = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
            stickToBottomRef.current = stuck;
            setShowJump(!stuck && turns.length > 0);
          }}
        >
          {turns.length === 0 && (
            <div className="empty">
              <img src="/logo.svg" alt="" width={54} height={54} />
              <h2>One intelligence. Multiple agents. Complete control.</h2>
              <p className="note">
                Ask CAMEO anything. Responses stream from the selected model — nothing is simulated.
              </p>
            </div>
          )}
          {turns.map((t) => (
            <TurnRow key={t.id} turn={t} />
          ))}
        </div>
        {showJump && (
          <button type="button" className="chat-jump-latest" onClick={scrollToBottom} aria-label="Jump to latest messages">
            ↓ Latest
          </button>
        )}
      </div>

      {error && <div className="note danger">{error}</div>}

      {attachments.length > 0 && <div className="assistant-attachments" aria-label="Attached files">{attachments.map((file, index) => <span className="assistant-attachment" key={`${file.name}-${index}`}>{file.name}<button type="button" aria-label={`Remove ${file.name}`} onClick={() => setAttachments((current) => current.filter((_, i) => i !== index))}>×</button></span>)}</div>}
      {compact && (
        <div className="chat-agent-row">
          <AgentPicker agent={agent} onChange={(next) => {
            setAgent(next);
            void setPref("chat_agent", next);
          }} />
          <button
            type="button"
            className={`btn small autopilot-mini${autopilot ? " primary" : ""}`}
            onClick={() => void toggleAutopilot(!autopilot)}
            title="Hands-free: auto-approve every tool call, no manual steps"
            aria-label="Toggle autopilot hands-free mode"
            aria-pressed={autopilot}
          >
            {autopilot ? "Autopilot ●" : "Autopilot ○"}
          </button>
          {activeAgent.advisory && (
            <span className="note" role="note">Information only — not professional advice.</span>
          )}
        </div>
      )}
      {compact && (
        <SessionBar
          sessions={sessionList}
          activeId={activeSessionId}
          onSwitch={switchSession}
          onNew={newSession}
          onRename={renameSession}
          onDelete={deleteSession}
          renamingId={renamingId}
          renameDraft={renameDraft}
          setRenameDraft={setRenameDraft}
          setRenamingId={setRenamingId}
          confirmingDeleteId={confirmingDeleteId}
          setConfirmingDeleteId={setConfirmingDeleteId}
        />
      )}
      <div className="chat-input-row">
        {compact && <label className="assistant-attach-button" title="Attach text or source files">＋<input aria-label="Attach files" type="file" multiple accept=".txt,.md,.markdown,.json,.csv,.log,.yaml,.yml,.toml,.xml,.html,.htm,.css,.scss,.js,.jsx,.ts,.tsx,.py,.rs,.go,.java,.kt,.swift,.c,.h,.cpp,.sh" onChange={(event) => { void addAttachments(event.target.files); event.currentTarget.value = ""; }} /></label>}
        <textarea
          ref={inputRef}
          rows={1}
          placeholder={listening ? "Microphone level active · type or stop listening" : attachments.length ? "Message with attachments…" : "Message CAMEO…"}
          value={input}
          onChange={(e) => {
            setInput(e.target.value);
            // Autogrow up to the CSS max-height, then scroll internally.
            const el = e.target;
            el.style.height = "auto";
            el.style.height = `${Math.min(el.scrollHeight, 140)}px`;
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.ctrlKey || e.metaKey || compact && !e.shiftKey)) { e.preventDefault(); void send(); }
          }}
          aria-label="Chat input"
        />
        {compact && <button className={`assistant-mic-button${listening ? " is-listening" : ""}`} onClick={() => void toggleListening()} aria-label={listening ? "Stop microphone level indicator" : "Enable live microphone level indicator"} title="Microphone level only; speech-to-text is not available">{listening ? "◉" : "◎"}</button>}
        {compact && busy && <button className="assistant-stop-button" onClick={() => void stop()} aria-label="Stop running response">Stop</button>}
        <button className="btn primary assistant-send-button" onClick={() => void send()} disabled={busy || (!input.trim() && attachments.length === 0)} aria-label="Send message to CAMEO">
          {busy ? "…" : compact ? "↑" : "Send"}
        </button>
      </div>
      {compact && listening && <span className="assistant-mic-note" role="status">Microphone level active only; CAMEO is not transcribing speech.</span>}
      {compact && turns.length > 0 && <div className="compact-chat-history-label">Chat history is saved per agent — New chat clears it.</div>}
    </div>
  );
}

// Memoized so each streamed flush re-renders only the message that actually
// grew, instead of the whole conversation (including big thinking blocks).
// Turn objects keep referential identity unless that turn changed.
const TurnRow = memo(function TurnRow({ turn }: { turn: Turn }) {
  return (
    <div className={`msg ${turn.role}${turn.error ? " error" : ""}`} style={turn.error ? { border: "1px solid var(--danger)" } : undefined}>
      <div className="who">{turn.who}</div>
      {turn.thinking && (
        <details className="thinking-block in-turn">
          <summary className="note thinking-toggle"><span className="thinking-dot" aria-hidden /> Thinking process · {turn.thinking.split(/\s+/).filter(Boolean).length} words</summary>
          <div className="thinking-stream">
            <ThinkingTrace text={turn.thinking} live={false} />
          </div>
        </details>
      )}
      {turn.text || "…"}
    </div>
  );
});

/** Highlighted rendering of a reasoning trace.
 *  - section headings ("Plan:", "Step 2", "## …") glow cyan
 *  - tool names (exec_run / net_fetch / …) get a violet chip
 *  - `code`, file paths and numbers get their own tones
 *  - error / fail lines get a red wash, verified / done lines green
 *  Memoized per message so streaming only re-highlights the live block. */
const ThinkingTrace = memo(function ThinkingTrace({ text, live }: { text: string; live: boolean }) {
  const lines = text.slice(-8000).split(/\r?\n/);
  return (
    <>
      {lines.map((line, i) => {
        const cls = classifyThinkLine(line);
        const isLast = i === lines.length - 1;
        return (
          <div key={i} className={`think-line ${cls}${isLast && live ? " is-active" : ""}`}>
            <InlineThink text={line} />
            {isLast && live && <span className="think-caret" aria-hidden>▍</span>}
          </div>
        );
      })}
    </>
  );
});

function classifyThinkLine(line: string): string {
  const m = line.toLowerCase();
  if (/^(#{1,4}\s|step\s+\d+|\d+[.)]\s|[-*]\s+\[|plan:|reasoning:|analysis:|conclusion:|next:|fix:|error:|warning:)/.test(line.trim())) return "is-heading";
  if (m.includes("fail") || m.includes("error") || m.includes("denied") || m.includes("invalid") || m.includes("cannot") || m.includes("✖")) return "is-error";
  if (m.includes("verified") || m.includes("✔") || m.includes("done") || m.includes("success") || m.includes("passed")) return "is-ok";
  if (m.includes("exec_run") || m.includes("net_fetch") || m.includes("net_search") || m.includes("device_launch") || m.includes("tool") || m.includes("running")) return "is-tool";
  return "";
}

const THINK_TOKEN_SOURCE = "(`[^`]+`|\\bexec_run\\b|\\bnet_fetch\\b|\\bnet_search\\b|\\bdevice_launch\\b|\\b[A-Za-z0-9_\\-./\\\\]+\\.(?:ts|tsx|js|jsx|rs|py|json|toml|md|css|html)\\b|\\bStep\\s+\\d+\\b|\\bfail(?:ed|ure)?\\b|\\berror\\b|\\bverifi(?:ed|er|y)\\b|\\bdone\\b|\\u2714|\\u2718)";
const THINK_INLINE_RE = new RegExp(THINK_TOKEN_SOURCE, "gi");

function InlineThink({ text }: { text: string }) {
  const parts = text.split(THINK_INLINE_RE);
  if (parts.length === 1) return <>{text || " "}</>;
  return (
    <>
      {parts.map((p, i) => {
        if (i % 2 === 0) return <span key={i}>{p}</span>;
        let cls = "think-code";
        if (/^(exec_run|net_fetch|net_search|device_launch)$/i.test(p)) cls = "think-toolchip";
        else if (/fail|error|cannot|\u2718/i.test(p)) cls = "think-err";
        else if (/verifi|done|\u2714|passed|success/i.test(p)) cls = "think-ok-inline";
        else if (/\.(ts|tsx|js|jsx|rs|py|json|toml|md|css|html)$/i.test(p)) cls = "think-path";
        else if (/^step\s+\d+$/i.test(p)) cls = "think-stepnum";
        return <span key={i} className={cls}>{p}</span>;
      })}
    </>
  );
}

/** In-app saved chats: switch, rename (inline) and delete (two-step)
 *  per-agent sessions. Titles auto-derive from the first user message. */
function SessionBar({
  sessions,
  activeId,
  onSwitch,
  onNew,
  onRename,
  onDelete,
  renamingId,
  renameDraft,
  setRenameDraft,
  setRenamingId,
  confirmingDeleteId,
  setConfirmingDeleteId,
}: {
  sessions: ChatSessionMeta[];
  activeId: string;
  onSwitch: (id: string) => void;
  onNew: () => void;
  onRename: (id: string, title: string) => void;
  onDelete: (id: string) => void;
  renamingId: string | null;
  renameDraft: string;
  setRenameDraft: (v: string) => void;
  setRenamingId: (id: string | null) => void;
  confirmingDeleteId: string | null;
  setConfirmingDeleteId: (id: string | null) => void;
}) {
  const active = sessions.find((s) => s.id === activeId);
  return (
    <div className="session-bar">
      <label className="session-select-wrap">
        <span className="sr-only">Saved chat</span>
        <select
          aria-label="Saved chat"
          className="session-select"
          value={activeId}
          onChange={(e) => onSwitch(e.target.value)}
          title={active ? `${active.title} · ${sessions.length} saved` : `${sessions.length} saved chats`}
        >
          {sessions.map((s) => (
            <option key={s.id} value={s.id}>
              {s.title.length > 34 ? `${s.title.slice(0, 34)}…` : s.title}
            </option>
          ))}
        </select>
        <span className="session-count" aria-hidden>{sessions.length}</span>
      </label>
      <button type="button" className="btn small primary" onClick={onNew} title="Start a new saved chat" aria-label="Start a new chat">
        ＋ New
      </button>
      {renamingId === activeId ? (
        <span className="session-rename">
          <input
            aria-label="Chat name"
            value={renameDraft}
            autoFocus
            maxLength={60}
            onChange={(e) => setRenameDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") onRename(activeId, renameDraft);
              if (e.key === "Escape") setRenamingId(null);
            }}
          />
          <button type="button" className="btn small primary" onClick={() => onRename(activeId, renameDraft)} aria-label="Save chat name">✓</button>
          <button type="button" className="btn small" onClick={() => setRenamingId(null)} aria-label="Cancel rename">✕</button>
        </span>
      ) : (
        <button
          type="button"
          className="btn small"
          onClick={() => {
            setConfirmingDeleteId(null);
            setRenameDraft(active?.title ?? "");
            setRenamingId(activeId);
          }}
          title="Rename this chat"
          aria-label="Rename chat"
        >
          ✎
        </button>
      )}
      {confirmingDeleteId === activeId ? (
        <span className="session-delete-confirm">
          <button type="button" className="btn small danger" onClick={() => onDelete(activeId)} aria-label="Confirm delete chat">Delete ✓</button>
          <button type="button" className="btn small" onClick={() => setConfirmingDeleteId(null)} aria-label="Cancel delete">Keep</button>
        </span>
      ) : (
        <button
          type="button"
          className="btn small danger"
          onClick={() => {
            setRenamingId(null);
            setConfirmingDeleteId(activeId);
          }}
          title="Delete this chat"
          aria-label="Delete chat"
        >
          🗑
        </button>
      )}
    </div>
  );
}

function AgentPicker({ agent, onChange }: { agent: AgentId; onChange: (next: AgentId) => void }) {
  const meta = agentById(agent);
  return (
    <span style={{ display: "inline-flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
      <select
        aria-label="Active agent"
        value={agent}
        onChange={(e) => onChange(e.target.value as AgentId)}
        style={{ width: 200 }}
      >
        {AGENTS.map((a) => (
          <option key={a.id} value={a.id}>
            {a.codename} — {a.role}
          </option>
        ))}
      </select>
      <span className="note">{meta.description}</span>
    </span>
  );
}

function ModelPicker({
  provider,
  model,
  setProvider,
  setModel,
  onChange,
}: {
  provider: AIProviderId;
  model: string;
  setProvider: (p: AIProviderId) => void;
  setModel: (m: string) => void;
  onChange: (sel: ModelSelection) => void;
}) {
  const [pendingModel, setPendingModel] = useState(model);
  useEffect(() => setPendingModel(model), [model, provider]);
  const [status, setStatus] = useState<string | null>(null);
  const [keyState, setKeyState] = useState<"unknown" | "present" | "missing">("unknown");

  const meta = PROVIDERS.find((p) => p.id === provider)!;

  useEffect(() => {
    void hasKey(provider).then((has) => setKeyState(has ? "present" : "missing"));
  }, [provider]);

  const apply = () => {
    onChange({ provider, model: pendingModel.trim() });
    setStatus(`Switched to ${provider} / ${pendingModel.trim()}`);
  };

  const test = async () => {
    setStatus("Testing connection…");
    const res = await testConnection(provider);
    setStatus(res.ok ? `✔ ${res.message}` : `✖ ${res.message}`);
  };

  const save = async () => {
    const key = prompt(`Enter the API key for ${meta.label}:`) ?? "";
    if (!key.trim()) return;
    await saveKey(provider, key);
    setKeyState("present");
    setStatus("API key saved to Windows Credential Manager.");
  };

  const remove = async () => {
    await deleteKey(provider);
    setKeyState("missing");
    setStatus("API key removed from Windows Credential Manager.");
  };

  return (
    <span style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
      <select
        aria-label="Provider"
        value={provider}
        onChange={(e) => {
          setProvider(e.target.value as AIProviderId);
          setModel("");
        }}
        style={{ width: 170 }}
      >
        {PROVIDERS.map((p) => (
          <option key={p.id} value={p.id}>
            {p.label}
          </option>
        ))}
      </select>
      <input          placeholder="Model ID (e.g. deepseek-ai/deepseek-r1)"
        value={pendingModel}
        onChange={(e) => setPendingModel(e.target.value)}
        style={{ width: 260 }}
        aria-label="Model"
      />
      <button className="btn small primary" onClick={apply} disabled={!pendingModel.trim()}>Apply</button>
      <button className="btn small" onClick={test}>Test</button>
      {meta.requiresKey && (
        <>
          <button className="btn small" onClick={save} title="Store the key in Windows Credential Manager">
            {keyState === "present" ? "Re-set key" : "Set key"}
          </button>
          {keyState === "present" && (
            <button className="btn small danger" onClick={remove}>Delete key</button>
          )}
        </>
      )}
      {status && <span className="note">{status}</span>}
    </span>
  );
}
