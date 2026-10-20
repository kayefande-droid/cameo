import { useCallback, useEffect, useRef, useState } from "react";
import type { ModelSelection } from "../App";
import {
  deleteKey,
  hasKey,
  newRequestId,
  saveKey,
  streamChat,
  testConnection,
} from "../services/ai";
import { PROVIDERS, type AIProviderId, type ChatMessage } from "../types/ai";

interface Turn {
  id: string;
  role: "user" | "assistant";
  who: string;
  text: string;
  error?: boolean;
}

const CAMEO_SYSTEM_PROMPT =
  "You are CAMEO, the Cognitive Autonomous Machine Executive Operator — a desktop AI chief of staff. " +
  "Answer concisely, cite evidence and trade-offs, and never claim to have performed an action you cannot perform.";

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
  const [turns, setTurns] = useState<Turn[]>([]);
  const [input, setInput] = useState("");
  const [attachments, setAttachments] = useState<{ name: string; text: string }[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [listening, setListening] = useState(false);
  const lastAudioUpdateRef = useRef(0);
  const scrollRef = useRef<HTMLDivElement>(null);
  const requestIdRef = useRef<string>(newRequestId());
  const stopListeningRef = useRef<(() => void) | null>(null);
  const activeRef = useRef(active);
  activeRef.current = active;

  useEffect(() => {
    if (!active) return;
    const node = scrollRef.current;
    if (node && typeof node.scrollTo === "function") {
      node.scrollTo({ top: node.scrollHeight });
    } else if (node) {
      node.scrollTop = node.scrollHeight;
    }
  }, [active, turns]);

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
    setTurns((t) => [...t, userTurn, { id: asstId, role: "assistant", who: selection.model, text: "" }]);
    setInput("");
    setAttachments([]);
    setBusy(true);
    const requestId = newRequestId();
    requestIdRef.current = requestId;
    onRequestChange?.(requestId);

    const history: ChatMessage[] = [
      { role: "system", content: CAMEO_SYSTEM_PROMPT },
      ...turns.filter((t) => !t.error).map<ChatMessage>((t) => ({
        role: t.role,
        content: t.text,
      })),
      { role: "user", content: text },
    ];

    try {
      await streamChat(
        selection.provider,
        selection.model,
        history,
        { max_tokens: 2048 },
        (ev) => {
          if (ev.type === "delta") {
            setTurns((ts) =>
              ts.map((t) => (t.id === asstId ? { ...t, text: t.text + ev.text } : t))
            );
          } else if (ev.type === "error") {
            setTurns((ts) => ts.map((t) => (t.id === asstId ? { ...t, text: ev.message, error: true } : t)));
          } else if (ev.type === "done") {
            setTurns((ts) =>
              ts.map((t) =>
                t.id === asstId && !t.text
                  ? { ...t, text: "(empty response — the model returned no content)" }
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
  }, [attachments, busy, input, onRequestChange, selection, turns]);

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
          <span className="section-title">Provider &amp; model</span>
          <ModelPicker
            provider={provider}
            model={model}
            setProvider={setProvider}
            setModel={setModel}
            onChange={onSelectionChange}
          />
        </h3>
      </div>}

      <div className="chat-scroll" ref={scrollRef} role="log" aria-live="polite" aria-label="CAMEO conversation">
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
          <div key={t.id} className={`msg ${t.role}${t.error ? " error" : ""}`} style={t.error ? { border: "1px solid var(--danger)" } : undefined}>
            <div className="who">{t.who}</div>
            {t.text || "…"}
          </div>
        ))}
      </div>

      {error && <div className="note danger">{error}</div>}

      {attachments.length > 0 && <div className="assistant-attachments" aria-label="Attached files">{attachments.map((file, index) => <span className="assistant-attachment" key={`${file.name}-${index}`}>{file.name}<button type="button" aria-label={`Remove ${file.name}`} onClick={() => setAttachments((current) => current.filter((_, i) => i !== index))}>×</button></span>)}</div>}
      <div className="chat-input-row">
        {compact && <label className="assistant-attach-button" title="Attach text or source files">＋<input aria-label="Attach files" type="file" multiple accept=".txt,.md,.markdown,.json,.csv,.log,.yaml,.yml,.toml,.xml,.html,.htm,.css,.scss,.js,.jsx,.ts,.tsx,.py,.rs,.go,.java,.kt,.swift,.c,.h,.cpp,.sh" onChange={(event) => { void addAttachments(event.target.files); event.currentTarget.value = ""; }} /></label>}
        <textarea
          placeholder={listening ? "Microphone level active · type or stop listening" : attachments.length ? "Message with attachments…" : "Message CAMEO…"}
          value={input}
          onChange={(e) => setInput(e.target.value)}
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
      {compact && turns.length > 0 && <div className="compact-chat-history-label">Conversation remains available while this panel is open.</div>}
    </div>
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
      <input
        placeholder="Model ID (e.g. openai/gpt-4o-mini)"
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
